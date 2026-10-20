//! sway's IPC (the i3 IPC), as a [`super::CompositorEvent`] source.
//!
//! sway listens on one socket, `$SWAYSOCK`. Every message, both ways, is a
//! header and a JSON payload: the six bytes `i3-ipc`, the payload's length and
//! the message type, both 32-bit numbers in the machine's byte order. A
//! connection that subscribes (`SUBSCRIBE`) receives events from then on;
//! their type has the high bit set. Requests go over connections of their own
//! (`GET_OUTPUTS`, `GET_TREE`), and sway keeps such a connection open after its
//! answer, so an answer is complete when its length says so.
//!
//! As for Hyprland, an event only schedules a resync (`super::ipc`), which
//! computes every output's [`OutputView`] from the outputs and the tree
//! ([`snapshot`], pure). sway marks every window it shows as `visible` (not
//! one on a hidden workspace, behind a fullscreen window, in a hidden tab or in
//! the scratchpad), so that is what counts.
//!
//! Facts relied on, checked with sway 1.12 (the recordings in the tests):
//! output and window rectangles are in logical layout coordinates; a disabled
//! output has `active: false`; workspaces always have `fullscreen_mode: 1`, so
//! only a container's mode means a fullscreen window; `fullscreen_mode` 2 is
//! fullscreen over every output; only windows (views) have `visible`.

use serde::Deserialize;

use super::coverage::{self, Rect};
use super::ipc::{self, Config, Protocol, Sockets, MAX_REPLY};
use super::{CompositorEvents, OutputView, Snapshot};

/// Follows the sway instance listening on `socket` from the event loop of
/// `handle`, reporting to its [`CompositorEvents`].
pub fn insert<'l, D>(
    handle: &calloop::LoopHandle<'l, D>,
    socket: std::path::PathBuf,
    config: Config,
) where
    D: CompositorEvents + 'l,
{
    let sockets = Sockets {
        events: socket.clone(),
        requests: socket,
    };
    ipc::insert(handle, Sway, sockets, config);
}

/// The magic string every message starts with.
const MAGIC: &[u8; 6] = b"i3-ipc";

/// Length of a message header: the magic, the payload length, the type.
const HEADER: usize = 14;

pub const SUBSCRIBE: u32 = 2;
pub const GET_OUTPUTS: u32 = 3;
pub const GET_TREE: u32 = 4;

/// The bit that marks an event.
const EVENT: u32 = 0x8000_0000;
pub const EVENT_WORKSPACE: u32 = EVENT;
pub const EVENT_OUTPUT: u32 = EVENT | 1;
pub const EVENT_WINDOW: u32 = EVENT | 3;
pub const EVENT_SHUTDOWN: u32 = EVENT | 6;

/// The events subscribed to.
const SUBSCRIPTION: &[u8] = br#"["window","workspace","output","shutdown"]"#;

/// A message of type `kind` with `payload`.
pub fn encode(kind: u32, payload: &[u8]) -> Vec<u8> {
    let length = u32::try_from(payload.len()).unwrap_or(u32::MAX);
    let mut message = Vec::with_capacity(HEADER + payload.len());
    message.extend_from_slice(MAGIC);
    message.extend_from_slice(&length.to_ne_bytes());
    message.extend_from_slice(&kind.to_ne_bytes());
    message.extend_from_slice(payload);
    message
}

/// One message, as [`decode`] finds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message<'a> {
    pub kind: u32,
    pub payload: &'a [u8],
    /// How many bytes it takes, header included.
    pub length: usize,
}

/// The first message in `bytes`. `Ok(None)` while it is not complete; an error
/// for bytes that are not a message, or one larger than [`MAX_REPLY`].
pub fn decode(bytes: &[u8]) -> Result<Option<Message<'_>>, String> {
    let Some(header) = bytes.get(..HEADER) else {
        // A prefix of the magic may still become a message.
        let prefix = bytes.len().min(MAGIC.len());
        return if bytes[..prefix] == MAGIC[..prefix] {
            Ok(None)
        } else {
            Err("sway sent something that is not an IPC message".into())
        };
    };
    if &header[..6] != MAGIC {
        return Err("sway sent something that is not an IPC message".into());
    }
    let number = |at: usize| {
        let mut word = [0u8; 4];
        word.copy_from_slice(&header[at..at + 4]);
        u32::from_ne_bytes(word)
    };
    let (length, kind) = (number(6), number(10));
    let length = usize::try_from(length).unwrap_or(usize::MAX);
    if length > MAX_REPLY {
        return Err(format!("sway sent a message of {length} bytes"));
    }
    Ok(bytes.get(HEADER..HEADER + length).map(|payload| Message {
        kind,
        payload,
        length: HEADER + length,
    }))
}

#[derive(Deserialize)]
struct Change {
    change: String,
}

#[derive(Deserialize)]
struct Success {
    success: bool,
}

/// Whether a message on the event connection may change what an output
/// shows. An error for one that ends the connection: sway shuts down, or it
/// refused the subscription.
pub fn changes_state(kind: u32, payload: &[u8]) -> Result<bool, String> {
    let change = || serde_json::from_slice::<Change>(payload).map(|c| c.change);
    match kind {
        EVENT_SHUTDOWN => Err("sway is shutting down".into()),
        EVENT_OUTPUT => Ok(true),
        // `focus` too: a workspace shown on another output.
        EVENT_WORKSPACE => Ok(change().map_or(true, |change| change != "urgent")),
        // `focus` too: the window shown in a tabbed or stacked container.
        EVENT_WINDOW => Ok(change().map_or(true, |change| {
            !matches!(change.as_str(), "title" | "mark" | "urgent")
        })),
        SUBSCRIBE => match serde_json::from_slice::<Success>(payload) {
            Ok(Success { success: true }) => Ok(false),
            _ => Err("sway refused the subscription to its events".into()),
        },
        // Other events (modes, bindings, ticks, bars, inputs), and answers.
        _ => Ok(false),
    }
}

/// sway's wire format.
#[derive(Debug, Clone, Copy)]
pub struct Sway;

impl Protocol for Sway {
    fn name(&self) -> &'static str {
        "sway"
    }

    fn subscription(&self) -> Vec<u8> {
        encode(SUBSCRIBE, SUBSCRIPTION)
    }

    fn take_events(&self, pending: &mut Vec<u8>) -> Result<bool, String> {
        let mut changed = false;
        while let Some(Message {
            kind,
            payload,
            length,
        }) = decode(pending)?
        {
            let change = changes_state(kind, payload)?;
            if change {
                tracing::trace!("sway event {kind:#x}");
            }
            changed |= change;
            pending.drain(..length);
        }
        Ok(changed)
    }

    fn requests(&self) -> Vec<(&'static str, Vec<u8>)> {
        vec![
            ("get_outputs", encode(GET_OUTPUTS, b"")),
            ("get_tree", encode(GET_TREE, b"")),
        ]
    }

    fn reply_complete(&self, reply: &[u8]) -> bool {
        // A reply that is not a message is complete as well: reading on would
        // not make it one, and `snapshot` says what is wrong with it.
        !matches!(decode(reply), Ok(None))
    }

    fn snapshot(&self, replies: &[Vec<u8>]) -> Result<Snapshot, String> {
        let [outputs, tree] = replies else {
            return Err(format!(
                "expected 2 answers from sway, got {}",
                replies.len()
            ));
        };
        let payload = |reply: &[u8], expected: u32, name: &str| -> Result<Vec<u8>, String> {
            match decode(reply)? {
                Some(Message { kind, payload, .. }) if kind == expected => Ok(payload.to_vec()),
                Some(Message { kind, .. }) => Err(format!(
                    "sway answered {name} with a message of type {kind}"
                )),
                None => Err(format!("sway's answer to {name} is cut short")),
            }
        };
        parse_replies(
            &payload(outputs, GET_OUTPUTS, "get_outputs")?,
            &payload(tree, GET_TREE, "get_tree")?,
        )
    }
}

/// A rectangle as sway writes it, in logical layout coordinates.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct SwayRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl From<SwayRect> for Rect {
    fn from(r: SwayRect) -> Self {
        Rect::new(r.x, r.y, r.width, r.height)
    }
}

/// An output, as `get_outputs` describes it (fields veil does not use are
/// ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct Output {
    pub name: String,
    /// False for a disabled output.
    #[serde(default)]
    pub active: bool,
    /// `null` for a disabled output.
    #[serde(default)]
    pub current_workspace: Option<String>,
    pub rect: SwayRect,
}

/// A node of `get_tree`: the root, an output, a workspace, a container or a
/// window.
#[derive(Debug, Clone, Deserialize)]
pub struct Node {
    #[serde(rename = "type", default)]
    pub kind: String,
    /// For a window: without its title bar.
    pub rect: Option<SwayRect>,
    /// A window's title bar, relative to its parent (the workspace for a
    /// floating window and for one directly on a workspace). Empty without one.
    pub deco_rect: Option<SwayRect>,
    /// `splith`, `splitv`, `tabbed`, `stacked`, `output`, `none` (a window).
    #[serde(default)]
    pub layout: String,
    /// 0 none, 1 fullscreen on its output, 2 fullscreen over every output.
    /// Workspaces always have 1.
    #[serde(default)]
    pub fullscreen_mode: i64,
    /// Only windows have it: whether sway shows the window.
    #[serde(default)]
    pub visible: Option<bool>,
    #[serde(default)]
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub floating_nodes: Vec<Node>,
}

/// A window that sway shows: where, and whether it is fullscreen (itself, or
/// as part of a container that is).
struct Shown {
    rect: Rect,
    fullscreen: bool,
}

/// Collects what sway shows under `node`, whose parent starts at `parent`
/// (absolute coordinates): every window it shows, with its title bar, and
/// every tabbed or stacked container that shows one of its windows, whole,
/// because the title bars of the hidden tabs are shown too. Returns whether
/// anything under `node` is shown.
fn shown_windows(
    node: &Node,
    parent: (f64, f64),
    fullscreen_above: bool,
    shown: &mut Vec<Shown>,
) -> bool {
    let container = matches!(node.kind.as_str(), "con" | "floating_con");
    let fullscreen = fullscreen_above || (container && node.fullscreen_mode != 0);
    let mut any = false;
    if let (Some(true), Some(rect)) = (node.visible, node.rect) {
        any = true;
        shown.push(Shown {
            rect: rect.into(),
            fullscreen,
        });
        if let Some(deco) = node.deco_rect.filter(|d| d.width > 0.0 && d.height > 0.0) {
            shown.push(Shown {
                rect: Rect::new(
                    parent.0 + deco.x,
                    parent.1 + deco.y,
                    deco.width,
                    deco.height,
                ),
                fullscreen,
            });
        }
    }
    let origin = node.rect.map_or(parent, |rect| (rect.x, rect.y));
    for child in node.nodes.iter().chain(&node.floating_nodes) {
        any |= shown_windows(child, origin, fullscreen, shown);
    }
    let tabs = container && matches!(node.layout.as_str(), "tabbed" | "stacked");
    if let (true, true, Some(rect)) = (tabs, any, node.rect) {
        shown.push(Shown {
            rect: rect.into(),
            fullscreen,
        });
    }
    any
}

/// Whether `a` and `b` share an area (not just an edge).
fn overlap(a: &Rect, b: &Rect) -> bool {
    a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height
}

/// What every active output shows, from the answers to `get_outputs` and
/// `get_tree`. A window counts on every output it overlaps: the tree says on
/// which workspace it is, but a window fullscreen over every output, or a
/// floating one that sticks out, is shown on others too.
pub fn snapshot(outputs: &[Output], tree: &Node) -> Snapshot {
    let mut shown = Vec::new();
    shown_windows(tree, (0.0, 0.0), false, &mut shown);
    outputs
        .iter()
        .filter(|output| output.active)
        .map(|output| {
            let area: Rect = output.rect.into();
            let on_it: Vec<&Shown> = shown.iter().filter(|w| overlap(&w.rect, &area)).collect();
            let rects: Vec<Rect> = on_it.iter().map(|w| w.rect).collect();
            let view = OutputView {
                workspace: output.current_workspace.clone().unwrap_or_default(),
                fullscreen: on_it.iter().any(|w| w.fullscreen),
                covered: coverage::covers(area, &rects),
            };
            (output.name.clone(), view)
        })
        .collect()
}

/// [`snapshot`] from the payloads of the answers.
pub fn parse_replies(outputs: &[u8], tree: &[u8]) -> Result<Snapshot, String> {
    let outputs: Vec<Output> = serde_json::from_slice(outputs)
        .map_err(|e| format!("cannot read sway's answer to get_outputs: {e}"))?;
    let tree: Node = serde_json::from_slice(tree)
        .map_err(|e| format!("cannot read sway's answer to get_tree: {e}"))?;
    Ok(snapshot(&outputs, &tree))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_its_header_and_its_payload() {
        let message = encode(GET_TREE, b"{}");
        assert_eq!(&message[..6], b"i3-ipc");
        assert_eq!(message.len(), HEADER + 2);
        assert_eq!(
            decode(&message).expect("a message"),
            Some(Message {
                kind: GET_TREE,
                payload: b"{}",
                length: HEADER + 2
            })
        );
        // Every prefix is incomplete, not wrong.
        for cut in 0..message.len() {
            assert_eq!(decode(&message[..cut]).expect("a prefix"), None, "{cut}");
        }
        // Two in a row: the first one.
        let mut two = encode(EVENT_WINDOW, b"[1]");
        two.extend(encode(EVENT_OUTPUT, b"{}"));
        assert_eq!(
            decode(&two).expect("two messages"),
            Some(Message {
                kind: EVENT_WINDOW,
                payload: b"[1]",
                length: HEADER + 3
            })
        );
    }

    #[test]
    fn what_is_not_a_message_is_an_error() {
        assert!(decode(b"HTTP/1.1 200 OK\r\n").is_err());
        assert!(decode(b"i3").is_ok());
        assert!(decode(b"i4").is_err());
        let mut huge = encode(GET_TREE, b"");
        let length = u32::try_from(MAX_REPLY + 1).expect("fits").to_ne_bytes();
        huge[6..10].copy_from_slice(&length);
        assert!(decode(&huge).is_err());
    }

    #[test]
    fn which_events_lead_to_a_resync() {
        let window = |change: &str| format!(r#"{{"change":"{change}","container":{{"id":7}}}}"#);
        for (change, expected) in [
            ("new", true),
            ("close", true),
            ("focus", true),
            ("fullscreen_mode", true),
            ("move", true),
            ("floating", true),
            ("title", false),
            ("mark", false),
            ("urgent", false),
        ] {
            assert_eq!(
                changes_state(EVENT_WINDOW, window(change).as_bytes()),
                Ok(expected),
                "window {change}"
            );
        }
        for (change, expected) in [
            ("init", true),
            ("empty", true),
            ("focus", true),
            ("move", true),
            ("rename", true),
            ("reload", true),
            ("urgent", false),
        ] {
            let payload = format!(r#"{{"change":"{change}","current":{{"name":"2"}},"old":null}}"#);
            assert_eq!(
                changes_state(EVENT_WORKSPACE, payload.as_bytes()),
                Ok(expected),
                "workspace {change}"
            );
        }
        assert_eq!(
            changes_state(EVENT_OUTPUT, br#"{"change":"unspecified"}"#),
            Ok(true)
        );
        // A payload that cannot be read: better ask once too often.
        assert_eq!(changes_state(EVENT_WINDOW, b"nonsense"), Ok(true));
        // Modes, bindings and the like change nothing.
        assert_eq!(
            changes_state(EVENT | 2, br#"{"change":"resize"}"#),
            Ok(false)
        );
        assert_eq!(changes_state(SUBSCRIBE, br#"{"success":true}"#), Ok(false));
        assert!(changes_state(SUBSCRIBE, br#"{"success":false}"#).is_err());
        assert!(changes_state(EVENT_SHUTDOWN, br#"{"change":"exit"}"#).is_err());
    }

    #[test]
    fn events_are_taken_out_of_the_stream_one_message_at_a_time() {
        let mut pending = encode(SUBSCRIBE, br#"{"success":true}"#);
        pending.extend(encode(EVENT_WINDOW, br#"{"change":"title"}"#));
        assert_eq!(Sway.take_events(&mut pending), Ok(false));
        assert!(pending.is_empty());

        let focus = encode(EVENT_WORKSPACE, br#"{"change":"focus"}"#);
        let (first, rest) = focus.split_at(9);
        pending.extend_from_slice(first);
        assert_eq!(Sway.take_events(&mut pending), Ok(false));
        assert_eq!(pending.len(), 9, "an incomplete message stays");
        pending.extend_from_slice(rest);
        assert_eq!(Sway.take_events(&mut pending), Ok(true));
        assert!(pending.is_empty());

        pending.extend_from_slice(b"garbage that is no message");
        assert!(Sway.take_events(&mut pending).is_err());
    }

    #[test]
    fn an_answer_is_complete_when_its_length_says_so() {
        let answer = encode(GET_OUTPUTS, b"[]");
        assert!(!Sway.reply_complete(&answer[..HEADER]));
        assert!(Sway.reply_complete(&answer));
        assert!(Sway.reply_complete(b"not a message at all"));
    }

    /// Two outputs as sway 1.12 reported them in a headless session, trimmed
    /// (plus a few fields veil does not read). HEADLESS-2 is 1280x720 at
    /// x=1920, HEADLESS-1 1920x1080 at x=3200.
    const OUTPUTS: &str = r#"[
      {"id":4,"type":"output","name":"HEADLESS-2","active":true,"dpms":true,"power":true,
       "scale":1.0,"transform":"normal","current_workspace":"1","focused":true,
       "rect":{"x":1920,"y":0,"width":1280,"height":720}},
      {"id":3,"type":"output","name":"HEADLESS-1","active":true,"dpms":true,"power":true,
       "scale":1.0,"transform":"normal","current_workspace":"2","focused":false,
       "rect":{"x":3200,"y":0,"width":1920,"height":1080}}
    ]"#;

    /// The tree around the windows `windows` on workspace 1 of HEADLESS-2, and
    /// `scratch` in the scratchpad, in the shape sway 1.12 uses.
    fn tree(windows: &str, floating: &str, scratch: &str) -> String {
        format!(
            r#"{{"id":1,"type":"root","name":"root","fullscreen_mode":0,
               "rect":{{"x":1920,"y":0,"width":3200,"height":1080}},"nodes":[
              {{"id":2147483647,"type":"output","name":"__i3","fullscreen_mode":0,
                "rect":{{"x":1920,"y":0,"width":3200,"height":1080}},"nodes":[
                {{"type":"workspace","name":"__i3_scratch","fullscreen_mode":1,
                  "rect":{{"x":1920,"y":0,"width":3200,"height":1080}},
                  "nodes":[],"floating_nodes":[{scratch}]}}]}},
              {{"id":4,"type":"output","name":"HEADLESS-2","fullscreen_mode":0,
                "rect":{{"x":1920,"y":0,"width":1280,"height":720}},"nodes":[
                {{"id":5,"type":"workspace","name":"1","fullscreen_mode":1,"layout":"splith",
                  "rect":{{"x":1920,"y":0,"width":1280,"height":720}},
                  "nodes":[{windows}],"floating_nodes":[{floating}]}}]}},
              {{"id":3,"type":"output","name":"HEADLESS-1","fullscreen_mode":0,
                "rect":{{"x":3200,"y":0,"width":1920,"height":1080}},"nodes":[
                {{"id":6,"type":"workspace","name":"2","fullscreen_mode":1,
                  "rect":{{"x":3200,"y":0,"width":1920,"height":1080}},
                  "nodes":[],"floating_nodes":[]}}]}}]}}"#
        )
    }

    fn window(kind: &str, app: &str, visible: bool, fullscreen: i64, rect: [i64; 4]) -> String {
        let [x, y, width, height] = rect;
        format!(
            r#"{{"id":{x},"type":"{kind}","name":"foot","app_id":"{app}","pid":42,
               "visible":{visible},"fullscreen_mode":{fullscreen},"focused":false,"shell":"xdg_shell",
               "rect":{{"x":{x},"y":{y},"width":{width},"height":{height}}},
               "nodes":[],"floating_nodes":[]}}"#
        )
    }

    fn snap(tree: &str) -> Snapshot {
        parse_replies(OUTPUTS.as_bytes(), tree.as_bytes()).expect("valid replies")
    }

    fn view(workspace: &str, fullscreen: bool, covered: bool) -> OutputView {
        OutputView {
            workspace: workspace.into(),
            fullscreen,
            covered,
        }
    }

    #[test]
    fn empty_workspaces_are_neither_fullscreen_nor_covered() {
        let snapshot = snap(&tree("", "", ""));
        assert_eq!(snapshot["HEADLESS-2"], view("1", false, false));
        assert_eq!(snapshot["HEADLESS-1"], view("2", false, false));
        // The scratchpad's output is no output.
        assert_eq!(snapshot.len(), 2);
    }

    #[test]
    fn two_tiled_windows_without_gaps_cover_their_output() {
        let windows = [
            window("con", "tiled1", true, 0, [1920, 0, 640, 720]),
            window("con", "tiled2", true, 0, [2560, 0, 640, 720]),
        ]
        .join(",");
        let snapshot = snap(&tree(&windows, "", ""));
        assert_eq!(snapshot["HEADLESS-2"], view("1", false, true));
        assert_eq!(snapshot["HEADLESS-1"], view("2", false, false));
    }

    #[test]
    fn a_fullscreen_window_is_fullscreen_on_its_output_only() {
        // As recorded: the other window is hidden behind it.
        let windows = [
            window("con", "tiled1", false, 0, [1920, 0, 640, 720]),
            window("con", "tiled2", true, 1, [1920, 0, 1280, 720]),
        ]
        .join(",");
        let snapshot = snap(&tree(&windows, "", ""));
        assert_eq!(snapshot["HEADLESS-2"], view("1", true, true));
        assert_eq!(snapshot["HEADLESS-1"], view("2", false, false));
    }

    #[test]
    fn a_window_fullscreen_over_every_output_is_fullscreen_on_each() {
        let windows = window("con", "game", true, 2, [1920, 0, 3200, 1080]);
        let snapshot = snap(&tree(&windows, "", ""));
        assert_eq!(snapshot["HEADLESS-2"], view("1", true, true));
        assert_eq!(snapshot["HEADLESS-1"], view("2", true, true));
    }

    #[test]
    fn a_fullscreen_container_makes_its_windows_fullscreen() {
        let inner = [
            window("con", "a", true, 0, [1920, 0, 640, 720]),
            window("con", "b", true, 0, [2560, 0, 640, 720]),
        ]
        .join(",");
        let container = format!(
            r#"{{"type":"con","name":null,"fullscreen_mode":1,"layout":"splith",
               "rect":{{"x":1920,"y":0,"width":1280,"height":720}},
               "nodes":[{inner}],"floating_nodes":[]}}"#
        );
        let snapshot = snap(&tree(&container, "", ""));
        assert_eq!(snapshot["HEADLESS-2"], view("1", true, true));
    }

    #[test]
    fn hidden_floating_and_scratchpad_windows_count_as_sway_shows_them() {
        let floating = window("floating_con", "tiled1", true, 0, [2212, 113, 696, 494]);
        let scratch = window("floating_con", "tiled2", false, 0, [2240, 90, 636, 533]);
        let snapshot = snap(&tree("", &floating, &scratch));
        assert_eq!(snapshot["HEADLESS-2"], view("1", false, false));

        // A window on a workspace that is not shown, at full size: hidden.
        let hidden = window("con", "other", false, 0, [1920, 0, 1280, 720]);
        let snapshot = snap(&tree(&hidden, "", ""));
        assert_eq!(snapshot["HEADLESS-2"], view("1", false, false));
    }

    /// A window on HEADLESS-1 (1920x1080 at the origin), with sway's default
    /// title bar of 25 pixels, as sway 1.12 reports it: `rect` without the
    /// title bar, `deco_rect` relative to the parent.
    fn titled(kind: &str, visible: bool, rect: [i64; 4], deco: [i64; 4]) -> String {
        let [x, y, width, height] = rect;
        let [dx, dy, dw, dh] = deco;
        format!(
            r#"{{"type":"{kind}","layout":"none","app_id":"foot","visible":{visible},
               "fullscreen_mode":0,"border":"normal",
               "rect":{{"x":{x},"y":{y},"width":{width},"height":{height}}},
               "deco_rect":{{"x":{dx},"y":{dy},"width":{dw},"height":{dh}}},
               "nodes":[],"floating_nodes":[]}}"#
        )
    }

    /// A container of `layout` over all of HEADLESS-1, holding `children`.
    fn split(layout: &str, children: &[String]) -> String {
        format!(
            r#"{{"type":"con","layout":"{layout}","fullscreen_mode":0,
               "rect":{{"x":0,"y":0,"width":1920,"height":1080}},
               "deco_rect":{{"x":0,"y":0,"width":0,"height":0}},
               "nodes":[{}],"floating_nodes":[]}}"#,
            children.join(",")
        )
    }

    /// HEADLESS-1 alone, showing workspace 1 with `tiled` and `floating`.
    fn one_output(tiled: &str, floating: &str) -> OutputView {
        let outputs = r#"[{"name":"HEADLESS-1","active":true,"current_workspace":"1",
                          "rect":{"x":0,"y":0,"width":1920,"height":1080}}]"#;
        let tree = format!(
            r#"{{"type":"root","rect":{{"x":0,"y":0,"width":1920,"height":1080}},"nodes":[
               {{"type":"output","name":"HEADLESS-1","rect":{{"x":0,"y":0,"width":1920,"height":1080}},"nodes":[
                 {{"type":"workspace","name":"1","layout":"splith","fullscreen_mode":1,
                   "rect":{{"x":0,"y":0,"width":1920,"height":1080}},
                   "nodes":[{tiled}],"floating_nodes":[{floating}]}}]}}]}}"#
        );
        let mut snapshot = parse_replies(outputs.as_bytes(), tree.as_bytes()).expect("valid");
        snapshot.remove("HEADLESS-1").expect("the output")
    }

    #[test]
    fn title_bars_cover_the_wallpaper_too() {
        // One window: the title bar fills the strip above it.
        let one = titled("con", true, [0, 25, 1920, 1055], [0, 0, 1920, 25]);
        assert_eq!(one_output(&one, ""), view("1", false, true));
        // Without the title bar it would not cover.
        let bare = titled("con", true, [0, 25, 1920, 1055], [0, 0, 0, 0]);
        assert_eq!(one_output(&bare, ""), view("1", false, false));

        // Split vertically: each title bar relative to the split container.
        let splitv = split(
            "splitv",
            &[
                titled("con", true, [0, 25, 1920, 515], [0, 0, 1920, 25]),
                titled("con", true, [0, 565, 1920, 515], [0, 540, 1920, 25]),
            ],
        );
        assert_eq!(one_output(&splitv, ""), view("1", false, true));
    }

    #[test]
    fn tabbed_and_stacked_containers_cover_with_all_their_title_bars() {
        // As recorded: only the shown tab is visible, and its own title is
        // half of the tab bar.
        let tabbed = split(
            "tabbed",
            &[
                titled("con", false, [0, 25, 1920, 1055], [0, 0, 960, 25]),
                titled("con", true, [0, 25, 1920, 1055], [960, 0, 960, 25]),
            ],
        );
        assert_eq!(one_output(&tabbed, ""), view("1", false, true));
        let stacked = split(
            "stacked",
            &[
                titled("con", false, [0, 50, 1920, 1030], [0, 0, 1920, 25]),
                titled("con", true, [0, 50, 1920, 1030], [0, 25, 1920, 25]),
            ],
        );
        assert_eq!(one_output(&stacked, ""), view("1", false, true));
        // A tabbed container whose windows are all hidden shows nothing.
        let hidden = split(
            "tabbed",
            &[titled("con", false, [0, 25, 1920, 1055], [0, 0, 1920, 25])],
        );
        assert_eq!(one_output(&hidden, ""), view("1", false, false));
    }

    #[test]
    fn a_floating_window_with_its_title_bar_covers_only_itself() {
        let floating = titled(
            "floating_con",
            true,
            [610, 293, 700, 496],
            [610, 268, 700, 25],
        );
        assert_eq!(one_output("", &floating), view("1", false, false));
        // Over a tiled window that covers the output: covered.
        let tiled = titled("con", true, [0, 25, 1920, 1055], [0, 0, 1920, 25]);
        assert_eq!(one_output(&tiled, &floating), view("1", false, true));
    }

    #[test]
    fn a_disabled_output_is_left_out() {
        let outputs = r#"[
          {"name":"HEADLESS-1","active":true,"current_workspace":"2",
           "rect":{"x":0,"y":0,"width":1920,"height":1080},"scale":1.0},
          {"name":"HEADLESS-2","active":false,"power":false,"current_workspace":null,
           "rect":{"x":0,"y":0,"width":0,"height":0},"scale":null}
        ]"#;
        let tree = r#"{"type":"root","rect":{"x":0,"y":0,"width":1920,"height":1080},"nodes":[]}"#;
        let snapshot = parse_replies(outputs.as_bytes(), tree.as_bytes()).expect("valid");
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot["HEADLESS-1"], view("2", false, false));
    }

    #[test]
    fn answers_that_are_not_the_expected_json_are_errors() {
        for (outputs, tree) in [
            ("not json", "{}"),
            ("{}", "{}"),
            (r#"[{"name":"A"}]"#, "{}"),
            ("[]", "[]"),
        ] {
            assert!(
                parse_replies(outputs.as_bytes(), tree.as_bytes()).is_err(),
                "{outputs} / {tree}"
            );
        }
        // Framed answers of the wrong type, or cut short.
        let outputs = encode(GET_OUTPUTS, b"[]");
        let tree = encode(GET_TREE, b"{}");
        assert!(Sway.snapshot(&[outputs.clone(), tree.clone()]).is_ok());
        assert!(Sway.snapshot(&[tree.clone(), outputs.clone()]).is_err());
        assert!(Sway
            .snapshot(&[outputs[..HEADER].to_vec(), tree.clone()])
            .is_err());
        assert!(Sway.snapshot(&[outputs]).is_err());
    }
}

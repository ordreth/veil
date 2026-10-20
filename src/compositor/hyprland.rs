//! Hyprland's IPC, as a [`super::CompositorEvent`] source.
//!
//! Hyprland has two sockets in `$XDG_RUNTIME_DIR/hypr/<signature>/`:
//!
//! * `.socket2.sock` streams events, one line each, `NAME>>DATA`
//!   (`workspace>>3`, `fullscreen>>1`, `openwindow>>...`).
//! * `.socket.sock` answers one request per connection (`j/monitors`,
//!   `j/clients`: JSON) and closes it.
//!
//! The events alone do not say which output a change is on (`fullscreen>>1`
//! names no window), and floating windows move without any event. So an event
//! that may change what an output shows schedules a resync (`super::ipc`),
//! which asks for the monitors and the clients and computes every output's
//! [`OutputView`] from that ([`snapshot`], pure). A workspace switch sends
//! five lines, which are taken together.

use std::path::Path;

use serde::Deserialize;

use super::coverage::{self, Rect};
use super::ipc::{self, Protocol};
use super::{CompositorEvents, OutputView, Snapshot};

pub use super::ipc::{Config, Sockets};

/// The sockets of the instance whose directory is `dir`.
pub fn sockets(dir: &Path) -> Sockets {
    Sockets {
        events: dir.join(".socket2.sock"),
        requests: dir.join(".socket.sock"),
    }
}

/// Follows the Hyprland instance with these `sockets` from the event loop of
/// `handle`, reporting to its [`CompositorEvents`].
pub fn insert<'l, D>(handle: &calloop::LoopHandle<'l, D>, sockets: Sockets, config: Config)
where
    D: CompositorEvents + 'l,
{
    ipc::insert(handle, Hyprland, sockets, config);
}

/// Hyprland's wire format.
#[derive(Debug, Clone, Copy)]
pub struct Hyprland;

impl Protocol for Hyprland {
    fn name(&self) -> &'static str {
        "Hyprland"
    }

    fn take_events(&self, pending: &mut Vec<u8>) -> Result<bool, String> {
        let mut changed = false;
        while let Some(end) = pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line[..end]);
            if let Some((name, _)) = parse_event(&line) {
                tracing::trace!("Hyprland event {name}");
                changed |= changes_state(name);
            }
        }
        if pending.len() > MAX_EVENT_LINE {
            pending.clear();
            changed = true;
        }
        Ok(changed)
    }

    fn requests(&self) -> Vec<(&'static str, Vec<u8>)> {
        REQUESTS
            .into_iter()
            .map(|request| (request, request.as_bytes().to_vec()))
            .collect()
    }

    fn snapshot(&self, replies: &[Vec<u8>]) -> Result<Snapshot, String> {
        match replies {
            [monitors, clients] => parse_replies(monitors, clients),
            _ => Err(format!(
                "expected 2 answers from Hyprland, got {}",
                replies.len()
            )),
        }
    }
}

/// Longest event line that is kept. Longer ones (a window title can be
/// anything) are dropped, and a resync is run in case they mattered.
const MAX_EVENT_LINE: usize = 64 * 1024;

/// What is asked for in a resync, in this order. Hyprland answers each on a
/// connection of its own, and closes it.
const REQUESTS: [&str; 2] = ["j/monitors", "j/clients"];

/// Splits an event line into its name and its data. `None` for a line that is
/// not an event.
pub fn parse_event(line: &str) -> Option<(&str, &str)> {
    let (name, data) = line.split_once(">>")?;
    let plain = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    plain.then_some((name, data))
}

/// Whether an event may change what an output shows: its workspace, a
/// fullscreen window, or the windows on it. Focus, titles, layouts and the
/// like do not.
pub fn changes_state(name: &str) -> bool {
    matches!(
        name,
        "workspace"
            | "workspacev2"
            | "focusedmon"
            | "focusedmonv2"
            | "fullscreen"
            | "monitoradded"
            | "monitoraddedv2"
            | "monitorremoved"
            | "monitorremovedv2"
            | "createworkspace"
            | "createworkspacev2"
            | "destroyworkspace"
            | "destroyworkspacev2"
            | "moveworkspace"
            | "moveworkspacev2"
            | "renameworkspace"
            | "activespecial"
            | "activespecialv2"
            | "openwindow"
            | "closewindow"
            | "movewindow"
            | "movewindowv2"
            | "changefloatingmode"
            | "minimized"
            | "togglegroup"
            | "moveintogroup"
            | "moveoutofgroup"
            | "pin"
            | "configreloaded"
    )
}

/// A monitor, as `j/monitors` describes it (fields veil does not use are
/// ignored).
#[derive(Debug, Clone, Deserialize)]
pub struct Monitor {
    pub name: String,
    /// Position in the layout, in logical pixels.
    pub x: f64,
    pub y: f64,
    /// Size of the current mode, in physical pixels and before the transform.
    pub width: f64,
    pub height: f64,
    pub scale: f64,
    #[serde(default)]
    pub transform: u8,
    #[serde(default)]
    pub disabled: bool,
    #[serde(rename = "activeWorkspace")]
    pub active_workspace: WorkspaceRef,
    /// A special workspace shown over the active one; id `0` when there is
    /// none.
    #[serde(rename = "specialWorkspace", default)]
    pub special_workspace: Option<WorkspaceRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceRef {
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

/// A window, as `j/clients` describes it.
#[derive(Debug, Clone, Deserialize)]
pub struct Client {
    #[serde(default = "yes")]
    pub mapped: bool,
    /// A window in a group that is not the group's current one.
    #[serde(default)]
    pub hidden: bool,
    /// Position and size in the layout, in logical pixels.
    pub at: [f64; 2],
    pub size: [f64; 2],
    pub workspace: WorkspaceRef,
    /// Since Hyprland 0.42 a bit mask (1 maximised, 2 fullscreen); before, a
    /// boolean with the kind in `fullscreenMode` (0 fullscreen, 1 maximised).
    #[serde(default)]
    pub fullscreen: serde_json::Value,
    #[serde(rename = "fullscreenMode", default)]
    pub fullscreen_mode: Option<i64>,
}

fn yes() -> bool {
    true
}

impl Monitor {
    /// The area of the monitor in the layout, in logical pixels.
    fn area(&self) -> Rect {
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (width, height) = (self.width / scale, self.height / scale);
        // Transforms 1, 3, 5 and 7 turn the output by 90 or 270 degrees.
        let (width, height) = if self.transform % 2 == 1 {
            (height, width)
        } else {
            (width, height)
        };
        Rect::new(self.x, self.y, width, height)
    }

    /// The workspaces it shows: the active one, and a special one over it.
    fn visible_workspaces(&self) -> Vec<i64> {
        let mut ids = vec![self.active_workspace.id];
        ids.extend(
            self.special_workspace
                .as_ref()
                .map(|special| special.id)
                .filter(|id| *id != 0),
        );
        ids
    }
}

impl Client {
    /// Whether the window is really fullscreen (not maximised).
    fn is_fullscreen(&self) -> bool {
        match &self.fullscreen {
            serde_json::Value::Number(mode) => mode.as_i64().is_some_and(|mode| mode & 2 != 0),
            serde_json::Value::Bool(on) => *on && self.fullscreen_mode != Some(1),
            _ => false,
        }
    }

    fn rect(&self) -> Rect {
        Rect::new(self.at[0], self.at[1], self.size[0], self.size[1])
    }
}

/// What every enabled monitor shows, from the answers to `j/monitors` and
/// `j/clients`. A window counts on a monitor when it is mapped, not hidden,
/// and on one of the monitor's visible workspaces.
pub fn snapshot(monitors: &[Monitor], clients: &[Client]) -> Snapshot {
    monitors
        .iter()
        .filter(|monitor| !monitor.disabled)
        .map(|monitor| {
            let visible = monitor.visible_workspaces();
            let windows: Vec<&Client> = clients
                .iter()
                .filter(|c| c.mapped && !c.hidden && visible.contains(&c.workspace.id))
                .collect();
            let rects: Vec<Rect> = windows.iter().map(|c| c.rect()).collect();
            let view = OutputView {
                workspace: monitor.active_workspace.name.clone(),
                fullscreen: windows.iter().any(|c| c.is_fullscreen()),
                covered: coverage::covers(monitor.area(), &rects),
            };
            (monitor.name.clone(), view)
        })
        .collect()
}

/// [`snapshot`] from the raw answers, in the order of [`REQUESTS`].
pub fn parse_replies(monitors: &[u8], clients: &[u8]) -> Result<Snapshot, String> {
    let monitors: Vec<Monitor> = serde_json::from_slice(monitors)
        .map_err(|e| format!("cannot read the answer to j/monitors: {e}"))?;
    let clients: Vec<Client> = serde_json::from_slice(clients)
        .map_err(|e| format!("cannot read the answer to j/clients: {e}"))?;
    Ok(snapshot(&monitors, &clients))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_lines_are_split_into_name_and_data() {
        assert_eq!(parse_event("workspace>>3"), Some(("workspace", "3")));
        assert_eq!(
            parse_event("workspacev2>>3,3"),
            Some(("workspacev2", "3,3"))
        );
        assert_eq!(parse_event("fullscreen>>1"), Some(("fullscreen", "1")));
        assert_eq!(
            parse_event("configreloaded>>"),
            Some(("configreloaded", ""))
        );
        // A title may contain anything, also the separator.
        assert_eq!(
            parse_event("activewindow>>kitty,a >> b"),
            Some(("activewindow", "kitty,a >> b"))
        );
        for line in ["", "workspace", ">>3", "Workspace>>3", "work space>>3"] {
            assert_eq!(parse_event(line), None, "{line:?}");
        }
    }

    #[test]
    fn only_events_that_change_an_output_lead_to_a_resync() {
        // Lines in the format Hyprland 0.56 sends when workspaces are switched,
        // fullscreen is toggled and a window is opened, moved and closed.
        let recorded = "\
workspace>>2
workspacev2>>2,2
focusedmon>>DP-3,2
focusedmonv2>>DP-3,2
activewindow>>kitty,~
activewindowv2>>55d1a2b3c4d0
fullscreen>>1
fullscreen>>0
openwindow>>55d1a2b3c4d0,2,kitty,~
movewindowv2>>55d1a2b3c4d0,3,3
closewindow>>55d1a2b3c4d0
windowtitlev2>>55d1a2b3c4d0,vim
activelayout>>keyboard,English (US)
monitoraddedv2>>2,HDMI-A-2,Some Monitor
monitorremovedv2>>2,HDMI-A-2,Some Monitor
activespecialv2>>-98,special:magic,DP-3
submap>>resize
configreloaded>>
";
        let changes: Vec<(&str, bool)> = recorded
            .lines()
            .filter_map(parse_event)
            .map(|(name, _)| (name, changes_state(name)))
            .collect();
        assert_eq!(changes.len(), 18);
        for (name, expected) in [
            ("workspace", true),
            ("workspacev2", true),
            ("focusedmon", true),
            ("activewindow", false),
            ("activewindowv2", false),
            ("fullscreen", true),
            ("openwindow", true),
            ("movewindowv2", true),
            ("closewindow", true),
            ("windowtitlev2", false),
            ("activelayout", false),
            ("monitoraddedv2", true),
            ("monitorremovedv2", true),
            ("activespecialv2", true),
            ("submap", false),
            ("configreloaded", true),
        ] {
            assert!(
                changes.contains(&(name, expected)),
                "{name}: expected {expected}"
            );
        }
    }

    /// Two 1920x1080 monitors side by side with a 24 px bar, as Hyprland
    /// 0.56 describes them (trimmed to the fields that matter, plus a few that
    /// do not).
    const MONITORS: &str = r#"[
        {"id":0,"name":"HDMI-A-1","description":"x","width":1920,"height":1080,"refreshRate":60.0,
         "x":0,"y":0,"activeWorkspace":{"id":4,"name":"4"},"specialWorkspace":{"id":0,"name":""},
         "reserved":[0,24,0,0],"scale":1.00,"transform":0,"focused":true,"dpmsStatus":true,"disabled":false},
        {"id":1,"name":"DP-3","description":"y","width":1920,"height":1080,"refreshRate":144.0,
         "x":1920,"y":0,"activeWorkspace":{"id":3,"name":"3"},"specialWorkspace":{"id":0,"name":""},
         "reserved":[0,24,0,0],"scale":1.00,"transform":0,"focused":false,"dpmsStatus":true,"disabled":false}
    ]"#;

    fn client(workspace: i64, at: [f64; 2], size: [f64; 2], fullscreen: i64) -> String {
        format!(
            r#"{{"address":"0x1","mapped":true,"hidden":false,"at":[{},{}],"size":[{},{}],
                "workspace":{{"id":{workspace},"name":"{workspace}"}},"floating":false,"monitor":0,
                "class":"kitty","title":"~","fullscreen":{fullscreen},"fullscreenClient":{fullscreen}}}"#,
            at[0], at[1], size[0], size[1]
        )
    }

    fn clients(list: &[String]) -> String {
        format!("[{}]", list.join(","))
    }

    #[test]
    fn a_tiled_window_with_gaps_neither_covers_nor_is_fullscreen() {
        let reply = clients(&[
            client(4, [10.0, 34.0], [1900.0, 1036.0], 0),
            // On a workspace that is not shown.
            client(1, [0.0, 0.0], [1920.0, 1080.0], 2),
        ]);
        let snapshot = parse_replies(MONITORS.as_bytes(), reply.as_bytes()).expect("valid answers");
        let hdmi = &snapshot["HDMI-A-1"];
        assert_eq!(hdmi.workspace, "4");
        assert!(!hdmi.fullscreen);
        assert!(!hdmi.covered);
        // An empty workspace.
        let dp = &snapshot["DP-3"];
        assert_eq!(dp.workspace, "3");
        assert!(!dp.fullscreen && !dp.covered);
    }

    #[test]
    fn a_fullscreen_window_covers_its_monitor_only() {
        let reply = clients(&[client(3, [1920.0, 0.0], [1920.0, 1080.0], 2)]);
        let snapshot = parse_replies(MONITORS.as_bytes(), reply.as_bytes()).expect("valid answers");
        assert!(snapshot["DP-3"].fullscreen);
        assert!(snapshot["DP-3"].covered);
        assert!(!snapshot["HDMI-A-1"].fullscreen);
        assert!(!snapshot["HDMI-A-1"].covered);
    }

    #[test]
    fn a_maximised_window_is_not_fullscreen_and_leaves_the_bar() {
        let reply = clients(&[client(4, [0.0, 24.0], [1920.0, 1056.0], 1)]);
        let snapshot = parse_replies(MONITORS.as_bytes(), reply.as_bytes()).expect("valid answers");
        assert!(!snapshot["HDMI-A-1"].fullscreen);
        assert!(!snapshot["HDMI-A-1"].covered);
    }

    #[test]
    fn hidden_and_unmapped_windows_count_for_nothing() {
        let hidden = client(4, [0.0, 0.0], [1920.0, 1080.0], 2)
            .replace(r#""hidden":false"#, r#""hidden":true"#);
        let unmapped = client(4, [0.0, 0.0], [1920.0, 1080.0], 2)
            .replace(r#""mapped":true"#, r#""mapped":false"#);
        let snapshot = parse_replies(MONITORS.as_bytes(), clients(&[hidden, unmapped]).as_bytes())
            .expect("valid answers");
        assert!(!snapshot["HDMI-A-1"].fullscreen);
        assert!(!snapshot["HDMI-A-1"].covered);
    }

    #[test]
    fn a_special_workspace_shows_over_the_active_one() {
        let monitors = MONITORS.replacen(
            r#""specialWorkspace":{"id":0,"name":""}"#,
            r#""specialWorkspace":{"id":-98,"name":"special:magic"}"#,
            1,
        );
        let reply = clients(&[client(-98, [0.0, 0.0], [1920.0, 1080.0], 2)]);
        let snapshot = parse_replies(monitors.as_bytes(), reply.as_bytes()).expect("valid answers");
        assert_eq!(snapshot["HDMI-A-1"].workspace, "4");
        assert!(snapshot["HDMI-A-1"].fullscreen);
    }

    #[test]
    fn scaled_rotated_and_disabled_monitors() {
        let monitors = r#"[
            {"name":"eDP-1","x":0,"y":0,"width":2560,"height":1600,"scale":1.6,"transform":1,
             "activeWorkspace":{"id":1,"name":"1"}},
            {"name":"DP-1","x":1000,"y":0,"width":1920,"height":1080,"scale":1,
             "activeWorkspace":{"id":2,"name":"2"},"disabled":true}
        ]"#;
        // Rotated: 1000 x 1600 logical pixels.
        let reply = clients(&[client(1, [0.0, 0.0], [1000.0, 1600.0], 0)]);
        let snapshot = parse_replies(monitors.as_bytes(), reply.as_bytes()).expect("valid answers");
        assert!(snapshot["eDP-1"].covered);
        assert!(!snapshot.contains_key("DP-1"));
    }

    #[test]
    fn fullscreen_of_hyprland_before_0_42_is_read_too() {
        let old = |mode: i64| {
            client(4, [0.0, 0.0], [1920.0, 1080.0], 0).replace(
                r#""fullscreen":0,"fullscreenClient":0"#,
                &format!(r#""fullscreen":true,"fullscreenMode":{mode}"#),
            )
        };
        let full = parse_replies(MONITORS.as_bytes(), clients(&[old(0)]).as_bytes())
            .expect("valid answers");
        assert!(full["HDMI-A-1"].fullscreen);
        let maximised = parse_replies(MONITORS.as_bytes(), clients(&[old(1)]).as_bytes())
            .expect("valid answers");
        assert!(!maximised["HDMI-A-1"].fullscreen);
    }

    #[test]
    fn answers_that_are_not_what_was_asked_for_are_errors() {
        for (monitors, clients) in [
            ("ok", "[]"),
            ("[]", "unknown request"),
            (r#"{"name":"DP-3"}"#, "[]"),
            (r#"[{"name":"DP-3"}]"#, "[]"),
        ] {
            assert!(
                parse_replies(monitors.as_bytes(), clients.as_bytes()).is_err(),
                "{monitors} / {clients}"
            );
        }
        let none = parse_replies(b"[]", b"[]").expect("no monitors is valid");
        assert!(none.is_empty());
    }

    #[test]
    fn sockets_are_in_the_instance_directory() {
        let sockets = sockets(Path::new("/run/user/1000/hypr/sig"));
        assert_eq!(
            sockets.events,
            Path::new("/run/user/1000/hypr/sig/.socket2.sock")
        );
        assert_eq!(
            sockets.requests,
            Path::new("/run/user/1000/hypr/sig/.socket.sock")
        );
    }
}

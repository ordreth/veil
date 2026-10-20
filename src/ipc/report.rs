//! What the daemon reports: the `result` of `status`, `outputs` and `toggle`
//! (see `protocol::Response`), and how `veil` prints it.
//!
//! The types are the JSON that `veil status --json` and `veil outputs --json`
//! print, so their field names are part of the protocol. New fields may be
//! added (a client ignores the ones it does not know); renaming or removing one
//! is a protocol change. Every field is always present, `null` when there is
//! nothing to say, so a script does not have to tell a missing key from an
//! empty one. A state or kind a client does not know reads as `unknown`.
//!
//! Everything here is pure: the daemon fills the types (`wayland/report.rs`),
//! the client prints them ([`status_text`], [`outputs_text`]).

use serde::{Deserialize, Serialize};

use crate::policy::PauseOn;
use crate::scaling::{Color, ScaleMode};

/// The answer to `status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Status {
    /// The daemon's version (`veil --version`).
    pub version: String,
    pub pid: u32,
    /// How long the daemon has run, in seconds.
    pub uptime_secs: u64,
    /// What hides a video enough to pause it (`--pause-on`).
    pub pause_on: PauseOn,
    /// What an output that is connected later shows, if it has no wallpaper
    /// of its own; `null`: the daemon's background colour.
    pub default: Option<WallpaperInfo>,
    /// Every output the daemon draws on, also those that are not connected.
    pub outputs: Vec<OutputStatus>,
}

/// A wallpaper: a file, or no file and only the background colour.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WallpaperInfo {
    /// Absolute path of the file; `null` for the background colour alone
    /// (`veil clear`).
    pub input: Option<String>,
    pub mode: ScaleMode,
    pub bg_color: Color,
}

/// One output the daemon draws on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputStatus {
    /// The output's name; `null` for an output the compositor gave none.
    pub name: Option<String>,
    /// Whether the output is there. One that is not keeps its wallpaper for
    /// when it is back.
    pub connected: bool,
    /// What the output shows (or will show once it is connected).
    pub wallpaper: WallpaperInfo,
    pub kind: Kind,
    pub state: State,
    /// Paused by `veil pause` (until `veil resume`).
    pub paused_by_user: bool,
    /// The compositor shows something over it that pauses a video
    /// (`--pause-on`).
    pub hidden: bool,
    /// Surface size in logical pixels, while there is a configured surface.
    pub size: Option<Size>,
    /// The surface's scale, while there is a configured surface.
    pub scale: Option<f64>,
    /// The video's stream, while a video is attached.
    pub video: Option<VideoInfo>,
    /// A wallpaper that is loaded or started for this output and replaces the
    /// current one once it can be shown (`veil set` waits for that).
    pub next: Option<WallpaperInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// What kind of wallpaper an output shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Only the background colour.
    Color,
    Image,
    Video,
    /// A kind this build does not know (a newer daemon). Never sent.
    #[serde(other)]
    Unknown,
}

/// Where the wallpaper of an output is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The output is not connected.
    Disconnected,
    /// It is about to be shown: the surface is new, or a video waits for its
    /// first frame.
    Starting,
    /// An image or a colour is on screen.
    Shown,
    /// A video plays.
    Playing,
    /// A video is paused (`paused_by_user`, `hidden`), its frame on screen.
    Paused,
    /// A video was stopped because the compositor does not show it (a locked
    /// screen, a monitor that is off); it starts again when it does.
    Released,
    /// Not shown after an error (see the daemon's log); the next change of
    /// the output tries again.
    Stopped,
    /// A state this build does not know (a newer daemon). Never sent.
    #[serde(other)]
    Unknown,
}

/// A video's stream and what the compositor did with its frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoInfo {
    /// Size of the decoded frames, once known.
    pub size: Option<Size>,
    /// Frames per second, once known, if the stream has a fixed rate.
    pub fps: Option<f64>,
    /// Frames the compositor took, and frames it did not take in time,
    /// counted by the video sink since the video (or its latest loop) started.
    pub rendered: Option<u64>,
    pub dropped: Option<u64>,
}

/// The answer to `outputs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputList {
    /// The outputs the compositor has announced, then those the daemon waits
    /// for (named with `--output`, or unplugged with a wallpaper kept).
    pub outputs: Vec<OutputInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputInfo {
    /// `null` for an output the compositor gave no name.
    pub name: Option<String>,
    /// What the compositor says it is (make and model), if anything.
    pub description: Option<String>,
    pub connected: bool,
    /// Whether the daemon draws a wallpaper on it: every output, or with
    /// `--output` the ones named.
    pub wallpaper: bool,
}

/// The answer to `toggle`: what the outputs it was for are now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Toggled {
    /// `true`: paused by `veil pause`; `false`: resumed.
    pub paused: bool,
}

/// `veil status` without `--json`: one line about the daemon, one about the
/// default wallpaper, one per output.
pub fn status_text(status: &Status) -> String {
    let mut text = format!(
        "veil {} (pid {}), running for {}; videos pause on: {}\n",
        status.version,
        status.pid,
        duration(status.uptime_secs),
        status.pause_on.name(),
    );
    text.push_str(&format!(
        "default: {}\n",
        status
            .default
            .as_ref()
            .map_or_else(|| "the background colour".to_string(), wallpaper)
    ));
    if status.outputs.is_empty() {
        text.push_str("no outputs\n");
    }
    for output in &status.outputs {
        text.push_str(&output_line(output));
        text.push('\n');
    }
    text
}

fn output_line(output: &OutputStatus) -> String {
    let mut line = format!(
        "{}: {} {}",
        output
            .name
            .as_deref()
            .unwrap_or("(an output without a name)"),
        state(output),
        wallpaper(&output.wallpaper)
    );
    if let (Some(size), Some(scale)) = (output.size, output.scale) {
        line.push_str(&format!(
            "; {}x{} at scale {}",
            size.width,
            size.height,
            decimal(scale)
        ));
    }
    if let Some(video) = &output.video {
        let mut parts = Vec::new();
        if let Some(size) = video.size {
            parts.push(format!("{}x{}", size.width, size.height));
        }
        if let Some(fps) = video.fps {
            parts.push(format!("{} fps", decimal(fps)));
        }
        if let (Some(rendered), Some(dropped)) = (video.rendered, video.dropped) {
            parts.push(format!("{rendered} frames shown, {dropped} dropped"));
        }
        if !parts.is_empty() {
            line.push_str(&format!("; video {}", parts.join(", ")));
        }
    }
    if let Some(next) = &output.next {
        line.push_str(&format!("; next: {}", wallpaper(next)));
    }
    line
}

/// The state of an output in words, with the reasons of a pause.
fn state(output: &OutputStatus) -> String {
    let word = match output.state {
        State::Disconnected => "not connected,",
        State::Starting => "starting",
        State::Shown => "showing",
        State::Playing => "playing",
        State::Paused => "paused",
        State::Released => "stopped while the compositor does not show it,",
        State::Stopped => "stopped after an error (see the log),",
        State::Unknown => "in an unknown state,",
    };
    let mut reasons = Vec::new();
    if output.paused_by_user {
        reasons.push("by veil pause");
    }
    if output.hidden {
        reasons.push("hidden by a window");
    }
    match (output.state, reasons.is_empty()) {
        (State::Paused, false) => format!("{word} ({})", reasons.join(", ")),
        // A pause that has nothing to stop yet, or an image.
        (_, false) => format!("{word} (paused {})", reasons.join(", ")),
        (_, true) => word.to_string(),
    }
}

fn wallpaper(wallpaper: &WallpaperInfo) -> String {
    match &wallpaper.input {
        Some(input) => format!(
            "{input} ({}, {})",
            wallpaper.mode.name(),
            wallpaper.bg_color
        ),
        None => format!("the background colour {}", wallpaper.bg_color),
    }
}

/// `1.5`, `1`, `29.97`: at most two decimals, without trailing zeros.
fn decimal(value: f64) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// `45s`, `3m 05s`, `2h 03m`, `4d 2h`.
fn duration(secs: u64) -> String {
    let (days, hours, minutes, seconds) =
        (secs / 86_400, secs / 3_600 % 24, secs / 60 % 60, secs % 60);
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes:02}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}

/// `veil outputs` without `--json`: one line per output.
pub fn outputs_text(list: &OutputList) -> String {
    if list.outputs.is_empty() {
        return "no outputs\n".into();
    }
    list.outputs
        .iter()
        .map(|output| {
            let mut line = format!(
                "{}: {}, {}",
                output
                    .name
                    .as_deref()
                    .unwrap_or("(an output without a name)"),
                if output.connected {
                    "connected"
                } else {
                    "not connected"
                },
                if output.wallpaper {
                    "wallpaper"
                } else {
                    "no wallpaper (not one of the daemon's outputs)"
                }
            );
            if let Some(description) = &output.description {
                line.push_str(&format!(" ({description})"));
            }
            line.push('\n');
            line
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(input: &str) -> WallpaperInfo {
        WallpaperInfo {
            input: Some(input.into()),
            mode: ScaleMode::Fill,
            bg_color: Color::default(),
        }
    }

    fn playing() -> OutputStatus {
        OutputStatus {
            name: Some("DP-3".into()),
            connected: true,
            wallpaper: file("/w/a.mp4"),
            kind: Kind::Video,
            state: State::Playing,
            paused_by_user: false,
            hidden: false,
            size: Some(Size {
                width: 1280,
                height: 720,
            }),
            scale: Some(1.5),
            video: Some(VideoInfo {
                size: Some(Size {
                    width: 1920,
                    height: 1080,
                }),
                fps: Some(30000.0 / 1001.0),
                rendered: Some(120),
                dropped: Some(2),
            }),
            next: None,
        }
    }

    fn status(outputs: Vec<OutputStatus>) -> Status {
        Status {
            version: "0.1.0".into(),
            pid: 42,
            uptime_secs: 3_725,
            pause_on: PauseOn::Fullscreen,
            default: Some(file("/w/a.mp4")),
            outputs,
        }
    }

    #[test]
    fn the_status_json_is_pinned() {
        let json = serde_json::to_string(&status(vec![playing()])).expect("serialize");
        assert_eq!(
            json,
            concat!(
                r##"{"version":"0.1.0","pid":42,"uptime_secs":3725,"pause_on":"fullscreen","##,
                r##""default":{"input":"/w/a.mp4","mode":"fill","bg_color":"#000000"},"##,
                r##""outputs":[{"name":"DP-3","connected":true,"##,
                r##""wallpaper":{"input":"/w/a.mp4","mode":"fill","bg_color":"#000000"},"##,
                r##""kind":"video","state":"playing","paused_by_user":false,"hidden":false,"##,
                r##""size":{"width":1280,"height":720},"scale":1.5,"##,
                r##""video":{"size":{"width":1920,"height":1080},"fps":29.97002997002997,"##,
                r##""rendered":120,"dropped":2},"next":null}]}"##
            )
        );
        let read: Status = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(read, status(vec![playing()]));
    }

    #[test]
    fn a_newer_daemon_s_status_is_still_read() {
        let json = r##"{"version":"9.0.0","pid":1,"uptime_secs":0,"pause_on":"never",
            "default":null,"hint":"new","outputs":[{"name":null,"connected":true,
            "wallpaper":{"input":null,"mode":"fit","bg_color":"#102030","loop":true},
            "kind":"slideshow","state":"fading","paused_by_user":false,"hidden":false,
            "size":null,"scale":null,"video":null,"next":null,"volume":3}]}"##;
        let read: Status = serde_json::from_str(json).expect("newer status");
        assert_eq!(read.outputs[0].kind, Kind::Unknown);
        assert_eq!(read.outputs[0].state, State::Unknown);
        assert_eq!(read.outputs[0].wallpaper.input, None);
    }

    #[test]
    fn the_status_text_has_a_line_per_output() {
        let mut paused = playing();
        paused.name = Some("HDMI-A-1".into());
        paused.state = State::Paused;
        paused.paused_by_user = true;
        paused.hidden = true;
        paused.video = None;
        paused.next = Some(file("/w/b.png"));
        let mut away = playing();
        away.name = None;
        away.connected = false;
        away.state = State::Disconnected;
        away.wallpaper = WallpaperInfo {
            input: None,
            mode: ScaleMode::Fit,
            bg_color: Color {
                r: 0x10,
                g: 0x20,
                b: 0x30,
            },
        };
        away.size = None;
        away.scale = None;
        away.video = None;
        let mut image = playing();
        image.name = Some("eDP-1".into());
        image.state = State::Shown;
        image.kind = Kind::Image;
        image.wallpaper = file("/w/c.png");
        image.paused_by_user = true;
        image.scale = Some(1.0);
        image.video = None;

        assert_eq!(
            status_text(&status(vec![playing(), paused, away, image])),
            "veil 0.1.0 (pid 42), running for 1h 02m; videos pause on: fullscreen\n\
             default: /w/a.mp4 (fill, #000000)\n\
             DP-3: playing /w/a.mp4 (fill, #000000); 1280x720 at scale 1.5; \
             video 1920x1080, 29.97 fps, 120 frames shown, 2 dropped\n\
             HDMI-A-1: paused (by veil pause, hidden by a window) /w/a.mp4 (fill, #000000); \
             1280x720 at scale 1.5; next: /w/b.png (fill, #000000)\n\
             (an output without a name): not connected, the background colour #102030\n\
             eDP-1: showing (paused by veil pause) /w/c.png (fill, #000000); \
             1280x720 at scale 1\n"
        );
        let mut empty = status(Vec::new());
        empty.default = None;
        assert_eq!(
            status_text(&empty),
            "veil 0.1.0 (pid 42), running for 1h 02m; videos pause on: fullscreen\n\
             default: the background colour\n\
             no outputs\n"
        );
    }

    #[test]
    fn durations_are_short() {
        for (secs, text) in [
            (0, "0s"),
            (45, "45s"),
            (185, "3m 05s"),
            (7_380, "2h 03m"),
            (352_800, "4d 2h"),
        ] {
            assert_eq!(duration(secs), text, "{secs}");
        }
    }

    #[test]
    fn decimals_lose_their_trailing_zeros() {
        for (value, text) in [(1.0, "1"), (1.5, "1.5"), (1.25, "1.25"), (59.94, "59.94")] {
            assert_eq!(decimal(value), text, "{value}");
        }
    }

    #[test]
    fn the_output_list_is_pinned() {
        let list = OutputList {
            outputs: vec![
                OutputInfo {
                    name: Some("DP-3".into()),
                    description: Some("Dell U2720Q".into()),
                    connected: true,
                    wallpaper: true,
                },
                OutputInfo {
                    name: Some("eDP-1".into()),
                    description: None,
                    connected: true,
                    wallpaper: false,
                },
                OutputInfo {
                    name: Some("HDMI-A-1".into()),
                    description: None,
                    connected: false,
                    wallpaper: true,
                },
            ],
        };
        let json = serde_json::to_string(&list).expect("serialize");
        assert_eq!(
            json,
            concat!(
                r#"{"outputs":[{"name":"DP-3","description":"Dell U2720Q","connected":true,"#,
                r#""wallpaper":true},{"name":"eDP-1","description":null,"connected":true,"#,
                r#""wallpaper":false},{"name":"HDMI-A-1","description":null,"#,
                r#""connected":false,"wallpaper":true}]}"#
            )
        );
        assert_eq!(
            serde_json::from_str::<OutputList>(&json).expect("deserialize"),
            list
        );
        assert_eq!(
            outputs_text(&list),
            "DP-3: connected, wallpaper (Dell U2720Q)\n\
             eDP-1: connected, no wallpaper (not one of the daemon's outputs)\n\
             HDMI-A-1: not connected, wallpaper\n"
        );
        assert_eq!(
            outputs_text(&OutputList {
                outputs: Vec::new()
            }),
            "no outputs\n"
        );
    }

    #[test]
    fn the_toggle_answer_is_pinned() {
        assert_eq!(
            serde_json::to_string(&Toggled { paused: true }).expect("serialize"),
            r#"{"paused":true}"#
        );
    }
}

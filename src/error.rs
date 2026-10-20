use std::path::PathBuf;
use thiserror::Error;

/// Exit status for any error that has no status of its own.
pub const EXIT_FAILURE: u8 = 1;

/// Exit status of a command line that does not parse (`clap` uses it) and of
/// a configuration file that cannot be read or is wrong. A restart cannot mend
/// either, so the systemd unit does not restart on it.
pub const EXIT_USAGE: u8 = 2;

/// Exit status of `veil set` when no daemon is listening.
pub const EXIT_DAEMON_NOT_RUNNING: u8 = 3;

/// Exit status of `veil daemon` when another daemon already serves the compositor.
pub const EXIT_ALREADY_RUNNING: u8 = 4;

/// Exit status of `veil daemon` when its compositor went away while it ran. A
/// restart cannot bring the wallpaper back (the session is over, or a new one
/// starts its own daemon), so the systemd unit does not restart on it.
pub const EXIT_COMPOSITOR_GONE: u8 = 5;

#[derive(Error, Debug)]
pub enum VeilError {
    #[error("Failed to resolve path '{path}': {source}")]
    PathResolution {
        path: String,
        source: std::io::Error,
    },

    #[error("'{0}' is not a regular file")]
    NotAFile(String),

    #[error("'{0}' is empty")]
    EmptyFile(String),

    #[error("Cannot read '{path}': {source}")]
    Unreadable {
        path: String,
        source: std::io::Error,
    },

    #[error(
        "'{0}' is neither an image nor a video that veil recognises, \
         by its content or by its extension"
    )]
    UnknownMedia(String),

    #[error("'{path}' is a picture in the {format} format, which veil cannot decode")]
    UnsupportedImage { path: String, format: String },

    #[error("Failed to load the wallpaper: {0}")]
    Load(String),

    #[error("Failed to decode the image '{path}': {source}")]
    Image {
        path: String,
        source: image::ImageError,
    },

    #[error("Failed to scale image: {0}")]
    Scaling(String),

    #[error("Failed to create SHM buffer: {0}")]
    BufferCreation(String),

    #[error("Internal GLib/GStreamer error: {0}")]
    Glib(#[from] gstreamer::glib::Error),

    #[error("Failed to construct GStreamer element: {0}")]
    GstBool(#[from] gstreamer::glib::BoolError),

    #[error("Failed to change pipeline state: {0}")]
    GstStateChange(#[from] gstreamer::StateChangeError),

    #[error("GStreamer pipeline error: {0}")]
    Gstreamer(String),

    #[error("Failed to connect to Wayland display: {0}")]
    WaylandConnect(#[from] wayland_client::ConnectError),

    #[error("Failed to initialize Wayland registry globals: {0}")]
    WaylandGlobal(#[from] wayland_client::globals::GlobalError),

    #[error("Missing or incompatible Wayland global: {0}")]
    SctkGlobal(String),

    #[error("Error during Wayland event dispatch: {0}")]
    WaylandDispatch(#[from] wayland_client::DispatchError),

    /// None of the outputs requested with `--output` exists at start-up.
    #[error("{}; {}", describe_requested(.requested), describe_outputs(.available))]
    OutputNotFound {
        requested: Vec<String>,
        available: Vec<String>,
    },

    #[error("The compositor announced no outputs to draw on")]
    NoOutputs,

    #[error("Event loop error: {0}")]
    EventLoop(String),

    #[error("Lost the connection to the Wayland compositor: {0}")]
    ConnectionLost(String),

    #[error("IPC error: {0}")]
    Ipc(String),

    /// The daemon answered `superseded`: a newer `set` or `clear` took every
    /// output of this one before it was shown. The newer one is what the user
    /// asked for last, so this is no failure to announce.
    #[error("IPC error: {0}")]
    Superseded(String),

    #[error("XDG_RUNTIME_DIR is not set to an absolute path; veil keeps its control socket there")]
    NoRuntimeDir,

    #[error(
        "a veil daemon is already running for this compositor (control socket {}); \
         use `veil set` to change its wallpaper",
        .socket.display()
    )]
    AlreadyRunning { socket: PathBuf },

    #[error(
        "no veil daemon is running for this compositor (nothing listens on {}); \
         start one with `veil daemon`",
        .socket.display()
    )]
    DaemonNotRunning { socket: PathBuf },

    #[error("no command given; use `veil daemon` or `veil set` (see `veil --help`)")]
    MissingCommand,

    /// The configuration file cannot be read, or says something wrong: at
    /// `position` (line and column, from 1), where there is one.
    #[error("{}: {message}", describe_place(.path, *.position))]
    Config {
        path: PathBuf,
        position: Option<(usize, usize)>,
        message: String,
    },

    /// `veil config check` found wallpapers in the file that cannot be shown.
    #[error("{}", .0.join("; "))]
    ConfigInputs(Vec<String>),

    /// `veil completions` or `veil man-pages` could not write `what`.
    #[error("Cannot write {what}: {source}")]
    Generate {
        what: String,
        source: std::io::Error,
    },
}

impl VeilError {
    /// Whether a desktop notification makes sense for this error. It does not when
    /// the compositor is gone: there is no session left to show it in, and waiting
    /// for a notification service would only delay the exit.
    ///
    /// A second daemon is not announced either. Nothing is broken, the wallpaper
    /// is being shown by the first one, and the usual cause is a session that
    /// starts veil from two places, which would raise the notification at every
    /// login.
    ///
    /// Nor is a `veil set` that a newer one replaced (`Superseded`): the
    /// newer one is shown, and a script that changes wallpapers quickly would
    /// raise a notification for every request it sent too early.
    pub fn wants_notification(&self) -> bool {
        !matches!(
            self,
            Self::ConnectionLost(_) | Self::AlreadyRunning { .. } | Self::Superseded(_)
        )
    }

    /// Exit status of the process when it ends with this error. `0` is success;
    /// `2` is also a command line that could not be parsed (from `clap`).
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::DaemonNotRunning { .. } => EXIT_DAEMON_NOT_RUNNING,
            Self::AlreadyRunning { .. } => EXIT_ALREADY_RUNNING,
            Self::ConnectionLost(_) => EXIT_COMPOSITOR_GONE,
            Self::Config { .. } => EXIT_USAGE,
            _ => EXIT_FAILURE,
        }
    }
}

fn describe_place(path: &std::path::Path, position: Option<(usize, usize)>) -> String {
    match position {
        Some((line, column)) => format!("{}:{line}:{column}", path.display()),
        None => path.display().to_string(),
    }
}

fn describe_requested(requested: &[String]) -> String {
    let quoted: Vec<String> = requested.iter().map(|name| format!("'{name}'")).collect();
    match quoted.as_slice() {
        [one] => format!("Output {one} was not found"),
        several => format!("None of the outputs {} was found", several.join(", ")),
    }
}

fn describe_outputs(available: &[String]) -> String {
    if available.is_empty() {
        "the compositor announced no named outputs".into()
    } else {
        format!("available outputs: {}", available.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_facing_messages() {
        let cases = [
            (
                VeilError::MissingCommand,
                "no command given; use `veil daemon` or `veil set` (see `veil --help`)",
            ),
            (
                VeilError::OutputNotFound {
                    requested: vec!["DP-3".into()],
                    available: vec!["HDMI-A-1".into(), "eDP-1".into()],
                },
                "Output 'DP-3' was not found; available outputs: HDMI-A-1, eDP-1",
            ),
            (
                VeilError::OutputNotFound {
                    requested: vec!["DP-3".into()],
                    available: vec![],
                },
                "Output 'DP-3' was not found; the compositor announced no named outputs",
            ),
            (
                VeilError::OutputNotFound {
                    requested: vec!["DP-3".into(), "DP-4".into()],
                    available: vec!["eDP-1".into()],
                },
                "None of the outputs 'DP-3', 'DP-4' was found; available outputs: eDP-1",
            ),
            (
                VeilError::NoOutputs,
                "The compositor announced no outputs to draw on",
            ),
            (
                VeilError::Ipc("failed to connect".into()),
                "IPC error: failed to connect",
            ),
            (
                VeilError::NoRuntimeDir,
                "XDG_RUNTIME_DIR is not set to an absolute path; veil keeps its control socket there",
            ),
            (
                VeilError::AlreadyRunning {
                    socket: "/run/user/1000/veil-wayland-1.sock".into(),
                },
                "a veil daemon is already running for this compositor \
                 (control socket /run/user/1000/veil-wayland-1.sock); \
                 use `veil set` to change its wallpaper",
            ),
            (
                VeilError::DaemonNotRunning {
                    socket: "/run/user/1000/veil-wayland-1.sock".into(),
                },
                "no veil daemon is running for this compositor \
                 (nothing listens on /run/user/1000/veil-wayland-1.sock); \
                 start one with `veil daemon`",
            ),
            (
                VeilError::Load("no thread".into()),
                "Failed to load the wallpaper: no thread",
            ),
            (
                VeilError::NotAFile("/srv/walls".into()),
                "'/srv/walls' is not a regular file",
            ),
            (
                VeilError::ConnectionLost("Broken pipe (os error 32)".into()),
                "Lost the connection to the Wayland compositor: Broken pipe (os error 32)",
            ),
            (
                VeilError::Gstreamer("no sink".into()),
                "GStreamer pipeline error: no sink",
            ),
            (
                VeilError::Scaling("invalid crop box".into()),
                "Failed to scale image: invalid crop box",
            ),
            (
                VeilError::BufferCreation("out of memory".into()),
                "Failed to create SHM buffer: out of memory",
            ),
            (
                VeilError::SctkGlobal("zwlr_layer_shell_v1".into()),
                "Missing or incompatible Wayland global: zwlr_layer_shell_v1",
            ),
            (
                VeilError::EventLoop("closed".into()),
                "Event loop error: closed",
            ),
            (
                VeilError::Config {
                    path: "/c.toml".into(),
                    position: Some((2, 8)),
                    message: "unknown variant `zoom`".into(),
                },
                "/c.toml:2:8: unknown variant `zoom`",
            ),
            (
                VeilError::Config {
                    path: "/c.toml".into(),
                    position: None,
                    message: "No such file or directory (os error 2)".into(),
                },
                "/c.toml: No such file or directory (os error 2)",
            ),
        ];

        for (err, expected) in cases {
            assert_eq!(err.to_string(), expected);
        }
    }

    #[test]
    fn a_lost_connection_is_not_announced_on_the_desktop() {
        assert!(!VeilError::ConnectionLost("Broken pipe".into()).wants_notification());
        assert!(VeilError::NoOutputs.wants_notification());
        assert!(VeilError::EventLoop("closed".into()).wants_notification());
    }

    #[test]
    fn a_second_daemon_is_not_announced_but_a_missing_one_is() {
        let socket = PathBuf::from("/run/user/1000/veil-wayland-1.sock");
        assert!(!VeilError::AlreadyRunning {
            socket: socket.clone()
        }
        .wants_notification());
        // `veil set` is often run from a key binding, with no terminal to read.
        assert!(VeilError::DaemonNotRunning { socket }.wants_notification());
    }

    #[test]
    fn a_replaced_set_is_not_announced_but_a_failed_one_is() {
        let replaced = VeilError::Superseded("replaced by a newer `veil set`".into());
        assert!(!replaced.wants_notification());
        assert_eq!(replaced.exit_code(), 1);
        assert!(VeilError::Ipc("cannot decode".into()).wants_notification());
    }

    #[test]
    fn exit_codes() {
        let socket = PathBuf::from("/run/user/1000/veil-wayland-1.sock");
        assert_eq!(
            VeilError::DaemonNotRunning {
                socket: socket.clone()
            }
            .exit_code(),
            3
        );
        assert_eq!(VeilError::AlreadyRunning { socket }.exit_code(), 4);
        assert_eq!(
            VeilError::ConnectionLost("Broken pipe".into()).exit_code(),
            5
        );
        assert_eq!(
            VeilError::Config {
                path: "/c.toml".into(),
                position: Some((2, 8)),
                message: "unknown variant `zoom`".into(),
            }
            .exit_code(),
            EXIT_USAGE
        );
        for other in [
            VeilError::NoOutputs,
            VeilError::NoRuntimeDir,
            VeilError::MissingCommand,
            VeilError::Ipc("boom".into()),
            // Not connected at all: a restart may find the compositor.
            VeilError::EventLoop("boom".into()),
        ] {
            assert_eq!(other.exit_code(), 1, "{other}");
        }
    }

    #[test]
    fn path_resolution_includes_path_and_cause() {
        let err = VeilError::PathResolution {
            path: "missing.mp4".into(),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "No such file"),
        };
        assert_eq!(
            err.to_string(),
            "Failed to resolve path 'missing.mp4': No such file"
        );
    }
}

use crate::error::VeilError;
use crate::scaling::{Color, ScaleMode};
use serde::{Deserialize, Serialize};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Version of the wire protocol, sent with every response. It is raised when a
/// request or a response changes in a way that a peer built for the previous
/// version would misread.
pub const PROTOCOL_VERSION: u32 = 1;

/// A command for the daemon: one JSON object on one line. A field the daemon does
/// not know is an error, so a misspelt option is never silently ignored.
///
/// A command without fields is an empty struct variant (`Ping {}`) and not a
/// unit variant, because serde accepts unknown fields next to the tag of a
/// unit variant. `output` names one output; without it a command is for every
/// output the daemon draws on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Set {
        output: Option<String>,
        /// Absolute path of the media file. The daemon has its own working
        /// directory, so the client resolves relative paths before sending.
        input: String,
        /// Omitted means the daemon's `--mode`. Left out of the JSON when `None`,
        /// so requests without it keep the original wire format.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<ScaleMode>,
        /// `#rrggbb`; omitted means the daemon's `--bg-color`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bg_color: Option<Color>,
        /// Whether the wallpaper is saved, to be shown again after a restart
        /// (`veil set --no-persist` sends `false`). Omitted means `true`, and
        /// `true` is left out of the JSON, so requests that save keep the
        /// earlier wire format; a daemon older than the field refuses `false`
        /// as an unknown field instead of saving the wallpaper.
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        persist: bool,
    },
    /// Shows the background colour instead of the wallpaper, and lets go of
    /// the file (`veil clear`). Saved like a `set`, unless `persist` is false.
    Clear {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        /// `#rrggbb`; omitted means the daemon's `--bg-color`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bg_color: Option<Color>,
        #[serde(default = "yes", skip_serializing_if = "is_true")]
        persist: bool,
    },
    /// Pauses the wallpaper until `resume`, whatever the compositor shows.
    Pause {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// Ends a `pause`. A wallpaper the compositor hides stays paused (see
    /// `veil daemon --pause-on`).
    Resume {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// `resume` if every output it is for is paused by `pause`, `pause`
    /// otherwise. The answer says which (`Toggled`).
    Toggle {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// What the daemon shows on each output (answered with a `Status`).
    Status {},
    /// The outputs the compositor has announced (answered with an
    /// `OutputList`).
    Outputs {},
    /// Ends the daemon, as SIGTERM does. It is answered before the daemon
    /// shuts down.
    Quit {},
    /// Health check.
    Ping {},
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

impl Request {
    /// Parses one request line and checks its values (see [`Request::validate`]).
    /// The error is a message for the client.
    pub fn decode(line: &[u8]) -> Result<Self, String> {
        let text =
            std::str::from_utf8(line).map_err(|_| "the request is not valid UTF-8".to_string())?;
        let request: Request =
            serde_json::from_str(text).map_err(|e| format!("invalid request: {e}"))?;
        request.validate()?;
        Ok(request)
    }

    /// Checks what the JSON structure cannot express. It does not look at the
    /// file system: whether the file exists is found out when it is loaded.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Request::Ping {} | Request::Status {} | Request::Outputs {} | Request::Quit {} => {
                Ok(())
            }
            Request::Clear { output, .. }
            | Request::Pause { output }
            | Request::Resume { output }
            | Request::Toggle { output } => validate_output(output.as_deref()),
            Request::Set { output, input, .. } => {
                if input.is_empty() {
                    return Err("input must not be empty".into());
                }
                if input.contains('\0') {
                    return Err("input must not contain a NUL character".into());
                }
                if !Path::new(input).is_absolute() {
                    return Err(format!("input must be an absolute path, got '{input}'"));
                }
                validate_output(output.as_deref())
            }
        }
    }

    /// The name of the command, as in `cmd`.
    pub fn name(&self) -> &'static str {
        match self {
            Request::Set { .. } => "set",
            Request::Clear { .. } => "clear",
            Request::Pause { .. } => "pause",
            Request::Resume { .. } => "resume",
            Request::Toggle { .. } => "toggle",
            Request::Status {} => "status",
            Request::Outputs {} => "outputs",
            Request::Quit {} => "quit",
            Request::Ping {} => "ping",
        }
    }
}

fn validate_output(output: Option<&str>) -> Result<(), String> {
    match output {
        Some("") => Err("output must not be empty".into()),
        _ => Ok(()),
    }
}

/// The daemon's answer to one request: one JSON object on one line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    /// [`PROTOCOL_VERSION`] of the daemon. A response without the field comes
    /// from a daemon older than the field and reads as `0`.
    #[serde(default)]
    pub version: u32,
    #[serde(flatten)]
    pub outcome: Outcome,
    /// What a command that reports something answers, with `ok`: a
    /// `Status`, an `OutputList`, `Toggled` (`ipc::report`). Left out of the
    /// JSON otherwise, so the other answers keep their wire format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ok,
    Error {
        /// What kind of error this is, for programs.
        #[serde(default)]
        code: ErrorCode,
        /// The same, for people.
        message: String,
    },
}

/// Machine-readable reason of an error response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request is not valid JSON, names an unknown command or field, or has
    /// a value that is not accepted.
    InvalidRequest,
    /// The request is longer than the daemon accepts.
    TooLarge,
    /// The client did not send a complete request in time.
    Timeout,
    /// The client runs as another user.
    Forbidden,
    /// Too many clients are connected at once.
    Busy,
    /// A newer request replaced this one before it took effect.
    Superseded,
    /// The request names an output the daemon does not draw on.
    UnknownOutput,
    /// The file does not exist.
    NotFound,
    /// The file cannot be a wallpaper: not a regular file, empty, or neither
    /// an image nor a video veil can show.
    UnsupportedMedia,
    /// The request was understood but could not be carried out.
    #[default]
    Failed,
    /// A code this build does not know (a newer daemon). Never sent.
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    /// The code of a request that failed with `error`.
    pub fn of(error: &VeilError) -> Self {
        match error {
            VeilError::PathResolution { .. } => Self::NotFound,
            VeilError::NotAFile(_)
            | VeilError::EmptyFile(_)
            | VeilError::UnknownMedia(_)
            | VeilError::UnsupportedImage { .. } => Self::UnsupportedMedia,
            _ => Self::Failed,
        }
    }
}

impl Response {
    pub fn ok() -> Self {
        Self {
            version: PROTOCOL_VERSION,
            outcome: Outcome::Ok,
            result: None,
        }
    }

    /// `ok`, with `result` as what the command reports.
    pub fn report(result: &impl Serialize) -> Self {
        match serde_json::to_value(result) {
            Ok(value) => Self {
                result: Some(value),
                ..Self::ok()
            },
            Err(e) => Self::error(ErrorCode::Failed, format!("cannot encode the answer: {e}")),
        }
    }

    pub fn error(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            outcome: Outcome::Error {
                code,
                message: message.into(),
            },
            result: None,
        }
    }

    /// The error a request that failed with `error` is answered with.
    pub fn failed(error: &VeilError) -> Self {
        Self::error(ErrorCode::of(error), error.to_string())
    }
}

/// Path of the daemon's control socket: `$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock`.
///
/// `XDG_RUNTIME_DIR` is required. It is the only directory that is private to the
/// user by definition, and there is no fallback to a shared one such as `/tmp`,
/// where any user could create the socket first. `WAYLAND_DISPLAY` may be a full
/// path; only its last component is used.
pub fn socket_path() -> Result<PathBuf, VeilError> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .ok_or(VeilError::NoRuntimeDir)?;

    let display = std::env::var_os("WAYLAND_DISPLAY");
    let display = display
        .as_deref()
        .and_then(|display| Path::new(display).file_name())
        .unwrap_or(OsStr::new("wayland-0"));

    let mut name = OsString::from("veil-");
    name.push(display);
    name.push(".sock");
    Ok(runtime_dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialises tests that mutate process-wide environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn round_trip_request(req: Request, wire: &str) {
        assert_eq!(serde_json::to_string(&req).expect("serialize"), wire);
        assert_eq!(
            serde_json::from_str::<Request>(wire).expect("deserialize"),
            req
        );
    }

    fn round_trip_response(resp: Response, wire: &str) {
        assert_eq!(serde_json::to_string(&resp).expect("serialize"), wire);
        assert_eq!(
            serde_json::from_str::<Response>(wire).expect("deserialize"),
            resp
        );
    }

    fn set(input: &str) -> Request {
        Request::Set {
            output: None,
            input: input.into(),
            mode: None,
            bg_color: None,
            persist: true,
        }
    }

    #[test]
    fn request_ping_round_trip() {
        round_trip_request(Request::Ping {}, r#"{"cmd":"ping"}"#);
    }

    #[test]
    fn request_set_round_trip() {
        round_trip_request(
            Request::Set {
                output: Some("HDMI-A-1".into()),
                input: "/tmp/a.mp4".into(),
                mode: None,
                bg_color: None,
                persist: true,
            },
            r#"{"cmd":"set","output":"HDMI-A-1","input":"/tmp/a.mp4"}"#,
        );
        round_trip_request(
            set("/tmp/a.png"),
            r#"{"cmd":"set","output":null,"input":"/tmp/a.png"}"#,
        );
    }

    #[test]
    fn request_set_with_mode_and_bg_color_round_trip() {
        round_trip_request(
            Request::Set {
                output: None,
                input: "/tmp/a.png".into(),
                mode: Some(ScaleMode::Fit),
                bg_color: Some(Color {
                    r: 0x11,
                    g: 0x22,
                    b: 0x33,
                }),
                persist: true,
            },
            r##"{"cmd":"set","output":null,"input":"/tmp/a.png","mode":"fit","bg_color":"#112233"}"##,
        );
    }

    #[test]
    fn request_set_that_is_not_saved_round_trip() {
        round_trip_request(
            Request::Set {
                output: Some("DP-3".into()),
                input: "/tmp/a.png".into(),
                mode: None,
                bg_color: None,
                persist: false,
            },
            r#"{"cmd":"set","output":"DP-3","input":"/tmp/a.png","persist":false}"#,
        );
        // `true` is what a request without the field means.
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"cmd":"set","input":"/a.png","persist":true}"#)
                .expect("explicit true"),
            set("/a.png")
        );
        assert!(serde_json::from_str::<Request>(
            r#"{"cmd":"set","input":"/a.png","persist":"no"}"#
        )
        .is_err());
    }

    #[test]
    fn request_set_optional_fields_may_be_omitted() {
        let req: Request = serde_json::from_str(r#"{"cmd":"set","input":"/a.png"}"#)
            .expect("missing optional fields");
        assert_eq!(req, set("/a.png"));
    }

    #[test]
    fn response_round_trip() {
        round_trip_response(Response::ok(), r#"{"version":1,"status":"ok"}"#);
        round_trip_response(
            Response::error(ErrorCode::Failed, "boom"),
            r#"{"version":1,"status":"error","code":"failed","message":"boom"}"#,
        );
    }

    #[test]
    fn every_error_code_has_a_pinned_name() {
        for (code, name) in [
            (ErrorCode::InvalidRequest, "invalid_request"),
            (ErrorCode::TooLarge, "too_large"),
            (ErrorCode::Timeout, "timeout"),
            (ErrorCode::Forbidden, "forbidden"),
            (ErrorCode::Busy, "busy"),
            (ErrorCode::Superseded, "superseded"),
            (ErrorCode::UnknownOutput, "unknown_output"),
            (ErrorCode::NotFound, "not_found"),
            (ErrorCode::UnsupportedMedia, "unsupported_media"),
            (ErrorCode::Failed, "failed"),
        ] {
            round_trip_response(
                Response::error(code, "m"),
                &format!(r#"{{"version":1,"status":"error","code":"{name}","message":"m"}}"#),
            );
        }
    }

    #[test]
    fn every_command_has_a_pinned_wire_format() {
        let color = Color {
            r: 0x11,
            g: 0x22,
            b: 0x33,
        };
        for (request, wire) in [
            (Request::Ping {}, r#"{"cmd":"ping"}"#),
            (Request::Status {}, r#"{"cmd":"status"}"#),
            (Request::Outputs {}, r#"{"cmd":"outputs"}"#),
            (Request::Quit {}, r#"{"cmd":"quit"}"#),
            (Request::Pause { output: None }, r#"{"cmd":"pause"}"#),
            (
                Request::Pause {
                    output: Some("DP-3".into()),
                },
                r#"{"cmd":"pause","output":"DP-3"}"#,
            ),
            (Request::Resume { output: None }, r#"{"cmd":"resume"}"#),
            (
                Request::Resume {
                    output: Some("DP-3".into()),
                },
                r#"{"cmd":"resume","output":"DP-3"}"#,
            ),
            (Request::Toggle { output: None }, r#"{"cmd":"toggle"}"#),
            (
                Request::Toggle {
                    output: Some("DP-3".into()),
                },
                r#"{"cmd":"toggle","output":"DP-3"}"#,
            ),
            (
                Request::Clear {
                    output: None,
                    bg_color: None,
                    persist: true,
                },
                r#"{"cmd":"clear"}"#,
            ),
            (
                Request::Clear {
                    output: Some("DP-3".into()),
                    bg_color: Some(color),
                    persist: false,
                },
                r##"{"cmd":"clear","output":"DP-3","bg_color":"#112233","persist":false}"##,
            ),
        ] {
            round_trip_request(request.clone(), wire);
            assert_eq!(Request::decode(wire.as_bytes()), Ok(request.clone()));
            assert!(wire.contains(&format!(r#""cmd":"{}""#, request.name())));
        }
    }

    #[test]
    fn commands_without_fields_take_none() {
        for wire in [
            r#"{"cmd":"status","output":"DP-3"}"#,
            r#"{"cmd":"outputs","json":true}"#,
            r#"{"cmd":"quit","now":true}"#,
            r#"{"cmd":"pause","input":"/a.png"}"#,
            r#"{"cmd":"clear","input":"/a.png"}"#,
        ] {
            let err = Request::decode(wire.as_bytes()).expect_err(wire);
            assert!(err.contains("unknown field"), "{wire:?}: {err}");
        }
    }

    #[test]
    fn an_output_must_not_be_empty_for_any_command() {
        for wire in [
            r#"{"cmd":"pause","output":""}"#,
            r#"{"cmd":"resume","output":""}"#,
            r#"{"cmd":"toggle","output":""}"#,
            r#"{"cmd":"clear","output":""}"#,
            r#"{"cmd":"set","output":"","input":"/a.png"}"#,
        ] {
            let err = Request::decode(wire.as_bytes()).expect_err(wire);
            assert!(err.contains("must not be empty"), "{wire:?}: {err}");
        }
    }

    #[test]
    fn a_result_goes_with_ok_and_is_left_out_without_one() {
        #[derive(Serialize)]
        struct Answer {
            paused: bool,
        }
        round_trip_response(
            Response::report(&Answer { paused: true }),
            r#"{"version":1,"status":"ok","result":{"paused":true}}"#,
        );
        // A daemon that has nothing to report.
        assert_eq!(Response::ok().result, None);
        let response: Response =
            serde_json::from_str(r#"{"version":1,"status":"ok"}"#).expect("ok without result");
        assert_eq!(response, Response::ok());
    }

    #[test]
    fn a_load_error_has_the_code_of_its_cause() {
        let io = || std::io::Error::from(std::io::ErrorKind::NotFound);
        for (error, code) in [
            (
                VeilError::PathResolution {
                    path: "/a".into(),
                    source: io(),
                },
                ErrorCode::NotFound,
            ),
            (
                VeilError::NotAFile("/a".into()),
                ErrorCode::UnsupportedMedia,
            ),
            (
                VeilError::EmptyFile("/a".into()),
                ErrorCode::UnsupportedMedia,
            ),
            (
                VeilError::UnknownMedia("/a".into()),
                ErrorCode::UnsupportedMedia,
            ),
            (
                VeilError::UnsupportedImage {
                    path: "/a".into(),
                    format: "HEIF".into(),
                },
                ErrorCode::UnsupportedMedia,
            ),
            (
                VeilError::Unreadable {
                    path: "/a".into(),
                    source: io(),
                },
                ErrorCode::Failed,
            ),
            (VeilError::Gstreamer("x".into()), ErrorCode::Failed),
        ] {
            assert_eq!(ErrorCode::of(&error), code, "{error:?}");
            let response = Response::failed(&error);
            assert_eq!(
                response.outcome,
                Outcome::Error {
                    code,
                    message: error.to_string()
                }
            );
        }
    }

    #[test]
    fn responses_carry_the_protocol_version() {
        assert_eq!(Response::ok().version, PROTOCOL_VERSION);
        assert_eq!(
            Response::error(ErrorCode::Busy, "m").version,
            PROTOCOL_VERSION
        );
    }

    #[test]
    fn response_of_a_daemon_without_version_or_code_is_still_read() {
        // The wire format before the version field existed.
        let response: Response = serde_json::from_str(r#"{"status":"ok"}"#).expect("old ok");
        assert_eq!(
            response,
            Response {
                version: 0,
                outcome: Outcome::Ok,
                result: None
            }
        );
        let response: Response =
            serde_json::from_str(r#"{"status":"error","message":"boom"}"#).expect("old error");
        assert_eq!(
            response,
            Response {
                version: 0,
                outcome: Outcome::Error {
                    code: ErrorCode::Failed,
                    message: "boom".into()
                },
                result: None
            }
        );
    }

    #[test]
    fn response_of_a_newer_daemon_is_still_read() {
        // An error code and a field this build does not know.
        let response: Response = serde_json::from_str(
            r#"{"version":7,"status":"error","code":"out_of_cheese","message":"m","hint":"h"}"#,
        )
        .expect("newer error");
        assert_eq!(
            response,
            Response {
                version: 7,
                outcome: Outcome::Error {
                    code: ErrorCode::Unknown,
                    message: "m".into()
                },
                result: None
            }
        );
    }

    #[test]
    fn unknown_cmd_is_rejected() {
        let err = serde_json::from_str::<Request>(r#"{"cmd":"explode"}"#)
            .expect_err("unknown cmd must fail");
        assert!(err.to_string().contains("unknown variant"), "{err}");
    }

    #[test]
    fn malformed_requests_are_rejected() {
        for wire in [
            r#"{}"#,
            r#"{"cmd":"set"}"#,
            r#"{"cmd":"SET","input":"/a.png"}"#,
            r#"{"cmd":"set","input":42}"#,
            r#"{"cmd":"set","input":"/a.png","mode":"zoom"}"#,
            r#"{"cmd":"set","input":"/a.png","bg_color":"red"}"#,
            r#"["set","/a.png"]"#,
            "not json",
            "",
        ] {
            assert!(serde_json::from_str::<Request>(wire).is_err(), "{wire:?}");
            assert!(Request::decode(wire.as_bytes()).is_err(), "{wire:?}");
        }
    }

    #[test]
    fn unknown_fields_are_rejected() {
        for wire in [
            r#"{"cmd":"set","input":"/a.png","colour":"red"}"#,
            r#"{"cmd":"set","input":"/a.png","Mode":"fit"}"#,
            r#"{"cmd":"ping","input":"/a.png"}"#,
        ] {
            let err = Request::decode(wire.as_bytes()).expect_err(wire);
            assert!(err.contains("unknown field"), "{wire:?}: {err}");
        }
    }

    #[test]
    fn decode_accepts_a_valid_line() {
        assert_eq!(Request::decode(br#"{"cmd":"ping"}"#), Ok(Request::Ping {}));
        // Surrounding whitespace, as left by `echo` or a CRLF line ending.
        assert_eq!(
            Request::decode(b"  {\"cmd\":\"ping\"}\r"),
            Ok(Request::Ping {})
        );
        assert_eq!(
            Request::decode(br#"{"cmd":"set","input":"/srv/a b.png"}"#),
            Ok(set("/srv/a b.png"))
        );
    }

    #[test]
    fn decode_rejects_bytes_that_are_not_utf8() {
        let err = Request::decode(b"{\"cmd\":\"ping\"}\xff").expect_err("invalid UTF-8");
        assert!(err.contains("UTF-8"), "{err}");
    }

    #[test]
    fn set_input_must_be_a_non_empty_absolute_path() {
        for (input, expected) in [
            ("", "must not be empty"),
            ("a.png", "absolute path"),
            ("./a.png", "absolute path"),
            ("~/a.png", "absolute path"),
            ("/srv/a\0.png", "NUL"),
        ] {
            let err = set(input).validate().expect_err(input);
            assert!(err.contains(expected), "{input:?}: {err}");
            // The same through the wire.
            let wire = serde_json::to_vec(&set(input)).expect("serialize");
            assert_eq!(Request::decode(&wire), Err(err), "{input:?}");
        }
        assert_eq!(set("/a.png").validate(), Ok(()));
        assert_eq!(Request::Ping {}.validate(), Ok(()));
    }

    #[test]
    fn set_output_must_not_be_empty() {
        let request = |output: Option<&str>| Request::Set {
            output: output.map(Into::into),
            input: "/a.png".into(),
            mode: None,
            bg_color: None,
            persist: true,
        };
        assert!(request(Some("")).validate().is_err());
        assert_eq!(request(Some("DP-1")).validate(), Ok(()));
        assert_eq!(request(None).validate(), Ok(()));
    }

    #[test]
    fn unknown_status_is_rejected() {
        assert!(serde_json::from_str::<Response>(r#"{"status":"maybe"}"#).is_err());
        assert!(serde_json::from_str::<Response>(r#"{"status":"error"}"#).is_err());
        assert!(serde_json::from_str::<Response>(r#"{"version":1}"#).is_err());
    }

    /// Runs `f` with the given values for `XDG_RUNTIME_DIR` and `WAYLAND_DISPLAY`
    /// (`None` unsets the variable), restoring the previous values afterwards.
    fn with_env(runtime_dir: Option<&str>, display: Option<&str>, f: impl FnOnce()) {
        const VARS: [&str; 2] = ["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY"];

        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved = VARS.map(std::env::var_os);
        for (key, value) in VARS.into_iter().zip([runtime_dir, display]) {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));

        for (key, value) in VARS.into_iter().zip(saved) {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
        if let Err(panic) = result {
            std::panic::resume_unwind(panic);
        }
    }

    #[test]
    fn socket_path_uses_both_env_vars() {
        with_env(Some("/run/user/1000"), Some("wayland-1"), || {
            assert_eq!(
                socket_path().expect("path"),
                PathBuf::from("/run/user/1000/veil-wayland-1.sock")
            );
        });
    }

    #[test]
    fn socket_path_needs_the_runtime_dir() {
        // No fallback to a directory that other users can write to.
        for display in [Some("wayland-1"), None] {
            with_env(None, display, || {
                assert!(matches!(socket_path(), Err(VeilError::NoRuntimeDir)));
            });
        }
    }

    #[test]
    fn socket_path_rejects_a_runtime_dir_that_is_not_absolute() {
        for runtime_dir in ["", "run/user/1000", "."] {
            with_env(Some(runtime_dir), Some("wayland-1"), || {
                assert!(
                    matches!(socket_path(), Err(VeilError::NoRuntimeDir)),
                    "{runtime_dir:?}"
                );
            });
        }
    }

    #[test]
    fn socket_path_falls_back_to_wayland_0_without_display() {
        for display in [None, Some("")] {
            with_env(Some("/run/user/1000"), display, || {
                assert_eq!(
                    socket_path().expect("path"),
                    PathBuf::from("/run/user/1000/veil-wayland-0.sock")
                );
            });
        }
    }

    #[test]
    fn socket_path_uses_only_the_name_of_an_absolute_display() {
        // WAYLAND_DISPLAY may be the full path of the compositor's socket.
        with_env(
            Some("/run/user/1000"),
            Some("/run/user/1000/sway/wayland-7"),
            || {
                assert_eq!(
                    socket_path().expect("path"),
                    PathBuf::from("/run/user/1000/veil-wayland-7.sock")
                );
            },
        );
    }
}

use super::protocol::{socket_path, ErrorCode, Outcome, Request, Response, PROTOCOL_VERSION};
use super::report::{self, OutputList, Status, Toggled};
use super::{instance, peer};
use crate::config::{ClearArgs, Command, SetArgs};
use crate::error::VeilError;
use serde::de::DeserializeOwned;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long a client waits for the daemon's response. The daemon answers a
/// `set` when the wallpaper is shown, so this has to cover loading the file.
/// `veil quit` waits as long again for the daemon to end.
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Runs a command that talks to the daemon of this session (every command but
/// `daemon` and `config`). Returns what the command prints on stdout, if
/// anything.
pub fn run(command: Command) -> Result<Option<String>, VeilError> {
    match command {
        Command::Daemon(_) => Err(VeilError::Ipc(
            "`veil daemon` is not a command for a running daemon".into(),
        )),
        Command::Config(_) => Err(VeilError::Ipc(
            "`veil config` is not a command for a running daemon".into(),
        )),
        Command::Completions(_) | Command::ManPages(_) => Err(VeilError::Ipc(
            "`veil completions` and `veil man-pages` are not commands for a running daemon".into(),
        )),
        Command::Set(args) => send_set(args).map(|()| None),
        Command::Clear(args) => send_clear(args).map(|()| None),
        Command::Pause(args) => ask(&Request::Pause {
            output: args.output,
        })
        .map(|_| None),
        Command::Resume(args) => ask(&Request::Resume {
            output: args.output,
        })
        .map(|_| None),
        Command::Toggle(args) => {
            let toggled: Toggled = answer_of(ask(&Request::Toggle {
                output: args.output,
            })?)?;
            Ok(Some(
                if toggled.paused {
                    "paused\n"
                } else {
                    "playing\n"
                }
                .into(),
            ))
        }
        Command::Status(args) => {
            let result = ask(&Request::Status {})?;
            let status: Status = answer_of(result.clone())?;
            Ok(Some(match args.json {
                true => json_line(result),
                false => report::status_text(&status),
            }))
        }
        Command::Outputs(args) => {
            let result = ask(&Request::Outputs {})?;
            let list: OutputList = answer_of(result.clone())?;
            Ok(Some(match args.json {
                true => json_line(result),
                false => report::outputs_text(&list),
            }))
        }
        Command::Quit => quit().map(|()| None),
        Command::Ping => ask(&Request::Ping {}).map(|_| Some("pong\n".into())),
    }
}

pub fn send_set(args: SetArgs) -> Result<(), VeilError> {
    ask(&Request::Set {
        output: args.output,
        input: absolute_input(&args.input)?,
        mode: args.mode,
        bg_color: args.bg_color,
        persist: !args.no_persist,
    })?;
    tracing::info!("wallpaper updated");
    Ok(())
}

fn send_clear(args: ClearArgs) -> Result<(), VeilError> {
    ask(&Request::Clear {
        output: args.output,
        bg_color: args.bg_color,
        persist: !args.no_persist,
    })?;
    tracing::info!("wallpaper cleared");
    Ok(())
}

/// Asks the daemon to end, and waits until it has.
fn quit() -> Result<(), VeilError> {
    let socket = socket_path()?;
    answered(&Request::Quit {}, send(&socket, &Request::Quit {})?)?;
    if !instance::wait_for_release(&socket, RESPONSE_TIMEOUT)? {
        return Err(VeilError::Ipc(format!(
            "the daemon is still shutting down after {RESPONSE_TIMEOUT:?}"
        )));
    }
    tracing::info!("the daemon has ended");
    Ok(())
}

/// Sends `request` to the daemon of this session. Returns what it reports.
fn ask(request: &Request) -> Result<Option<serde_json::Value>, VeilError> {
    let response = send(&socket_path()?, request)?;
    answered(request, response)
}

/// What `response` to `request` reports, or its error.
pub fn answered(
    request: &Request,
    response: Response,
) -> Result<Option<serde_json::Value>, VeilError> {
    if response.version != PROTOCOL_VERSION {
        tracing::warn!(
            "the daemon speaks protocol version {} and this client version {PROTOCOL_VERSION}; \
             restart the daemon if veil was updated",
            response.version
        );
    }
    match response.outcome {
        Outcome::Ok => Ok(response.result),
        // A daemon from before the command.
        Outcome::Error {
            code: ErrorCode::InvalidRequest,
            message,
        } if message.contains("unknown variant") => Err(VeilError::Ipc(format!(
            "the daemon does not know `veil {}`; it is older than this client, restart it \
             ({message})",
            request.name()
        ))),
        Outcome::Error {
            code: ErrorCode::Superseded,
            message,
        } => Err(VeilError::Superseded(message)),
        Outcome::Error { message, .. } => Err(VeilError::Ipc(message)),
    }
}

/// The `result` of an answer, as the type the command answers with.
pub fn answer_of<T: DeserializeOwned>(result: Option<serde_json::Value>) -> Result<T, VeilError> {
    let value =
        result.ok_or_else(|| VeilError::Ipc("the daemon answered without the result".into()))?;
    serde_json::from_value(value)
        .map_err(|e| VeilError::Ipc(format!("unreadable answer from the daemon: {e}")))
}

/// The JSON of `result` on one line, as `--json` prints it.
fn json_line(result: Option<serde_json::Value>) -> String {
    let mut line = result.map(|value| value.to_string()).unwrap_or_default();
    line.push('\n');
    line
}

/// The path as the daemon has to see it. The daemon runs in its own working
/// directory, so a relative path is resolved here, against the client's.
/// Symbolic links are kept; the daemon follows them when it opens the file.
fn absolute_input(input: &str) -> Result<String, VeilError> {
    let path = std::path::absolute(input).map_err(|source| VeilError::PathResolution {
        path: input.to_string(),
        source,
    })?;
    path.into_os_string().into_string().map_err(|path| {
        VeilError::Ipc(format!(
            "the path {} is not valid UTF-8",
            Path::new(&path).display()
        ))
    })
}

/// Sends one request to the daemon listening on `socket` and waits for its
/// response, for at most [`RESPONSE_TIMEOUT`].
pub fn send(socket: &Path, request: &Request) -> Result<Response, VeilError> {
    send_with_timeout(socket, request, RESPONSE_TIMEOUT)
}

/// [`send`] with another limit for the response. `timeout` must not be zero.
pub fn send_with_timeout(
    socket: &Path,
    request: &Request,
    timeout: Duration,
) -> Result<Response, VeilError> {
    let ipc = |e: std::io::Error| VeilError::Ipc(e.to_string());

    let mut stream = UnixStream::connect(socket).map_err(|e| match e.kind() {
        // No socket file, or one that a killed daemon left behind.
        ErrorKind::NotFound | ErrorKind::ConnectionRefused => VeilError::DaemonNotRunning {
            socket: socket.to_path_buf(),
        },
        _ => VeilError::Ipc(format!("failed to connect to {}: {e}", socket.display())),
    })?;

    // The mirror image of the daemon's check: do not hand requests to a socket
    // that some other user is listening on.
    let daemon_uid = peer::peer_uid(&stream).map_err(ipc)?;
    if daemon_uid != peer::own_uid() {
        return Err(VeilError::Ipc(format!(
            "{} belongs to a process of another user (uid {daemon_uid}); not using it",
            socket.display()
        )));
    }

    stream.set_read_timeout(Some(timeout)).map_err(ipc)?;
    stream.set_write_timeout(Some(timeout)).map_err(ipc)?;

    let mut payload = serde_json::to_string(request).map_err(|e| VeilError::Ipc(e.to_string()))?;
    payload.push('\n');
    // A daemon that turns the client away answers without reading the request,
    // and the write may then fail. Its answer is the better error, so read first.
    let sent = stream
        .write_all(payload.as_bytes())
        .and_then(|()| stream.shutdown(Shutdown::Write));

    let mut line = String::new();
    let received = BufReader::new(stream).read_line(&mut line);
    if line.trim().is_empty() {
        sent.map_err(ipc)?;
        return Err(VeilError::Ipc(match received {
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => format!(
                "the daemon did not answer within {timeout:?}; it may still carry out the request"
            ),
            Err(e) => e.to_string(),
            Ok(_) => "the daemon closed the connection without a response".into(),
        }));
    }

    serde_json::from_str(&line)
        .map_err(|e| VeilError::Ipc(format!("unreadable response from the daemon: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_paths_are_sent_unchanged() {
        assert_eq!(
            absolute_input("/srv/wall paper.png").expect("absolute"),
            "/srv/wall paper.png"
        );
    }

    #[test]
    fn relative_paths_are_resolved_against_the_working_directory() {
        let cwd = std::env::current_dir().expect("working directory");
        let expected = cwd.join("walls/a.png");
        assert_eq!(
            absolute_input("walls/a.png").expect("relative"),
            expected.to_str().expect("UTF-8 working directory")
        );
        assert_eq!(
            absolute_input("./walls/a.png").expect("relative with a dot"),
            expected.to_str().expect("UTF-8 working directory")
        );
    }

    #[test]
    fn the_result_is_accepted_by_the_request_validation() {
        let request = Request::Set {
            output: None,
            input: absolute_input("a.png").expect("relative"),
            mode: None,
            bg_color: None,
            persist: true,
        };
        assert_eq!(request.validate(), Ok(()));
    }

    #[test]
    fn an_empty_path_is_an_error() {
        assert!(matches!(
            absolute_input(""),
            Err(VeilError::PathResolution { .. })
        ));
    }

    #[test]
    fn a_daemon_that_does_not_know_a_command_is_said_to_be_older() {
        let refused = Response::error(
            ErrorCode::InvalidRequest,
            "invalid request: unknown variant `status`, expected `set` or `ping`",
        );
        let error = answered(&Request::Status {}, refused).expect_err("refused");
        let message = error.to_string();
        assert!(message.contains("does not know `veil status`"), "{message}");
        assert!(message.contains("restart it"), "{message}");

        let other = Response::error(ErrorCode::UnknownOutput, "output 'X' is not drawn on");
        let error = answered(&Request::Pause { output: None }, other).expect_err("refused");
        assert_eq!(error.to_string(), "IPC error: output 'X' is not drawn on");

        let replaced = Response::error(ErrorCode::Superseded, "replaced by a newer `veil set`");
        let error = answered(&Request::Status {}, replaced).expect_err("replaced");
        assert!(matches!(error, VeilError::Superseded(_)), "{error:?}");
        assert_eq!(
            error.to_string(),
            "IPC error: replaced by a newer `veil set`"
        );
    }

    #[test]
    fn an_answer_is_read_as_the_type_of_its_command() {
        let answer = Response::report(&Toggled { paused: true });
        let result = answered(&Request::Toggle { output: None }, answer).expect("ok");
        assert_eq!(
            answer_of::<Toggled>(result).expect("a toggle answer"),
            Toggled { paused: true }
        );
        assert!(answer_of::<Toggled>(None).is_err());
        assert!(answer_of::<Status>(Some(serde_json::json!({"paused": true}))).is_err());
    }

    #[test]
    fn without_a_daemon_the_error_says_so_and_names_the_socket() {
        let socket =
            std::env::temp_dir().join(format!("veil-no-daemon-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&socket);

        // No socket file at all.
        let error = send(&socket, &Request::Ping {}).expect_err("nothing listens there");
        assert!(
            matches!(&error, VeilError::DaemonNotRunning { socket: s } if *s == socket),
            "{error:?}"
        );
        assert!(
            error.to_string().contains(socket.to_str().expect("UTF-8")),
            "{error}"
        );

        // A socket file left behind by a daemon that was killed.
        drop(std::os::unix::net::UnixListener::bind(&socket).expect("bind"));
        let error = send(&socket, &Request::Ping {}).expect_err("nobody listens there");
        std::fs::remove_file(&socket).expect("remove the socket");
        assert!(
            matches!(error, VeilError::DaemonNotRunning { .. }),
            "{error:?}"
        );
    }
}

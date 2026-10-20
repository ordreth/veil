//! Shared by the IPC integration tests: a real `ipc::server` on a socket in
//! the temp directory, driven by a calloop event loop on its own thread, with a
//! stand-in for the daemon. No compositor is needed.

// Each test binary uses its own part of this module.
#![allow(dead_code)]

use calloop::{EventLoop, LoopSignal};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use veil::ipc::client;
use veil::ipc::instance::{self, SocketGuard};
use veil::ipc::protocol::{ErrorCode, Outcome, Request, Response, PROTOCOL_VERSION};
use veil::ipc::report::{
    Kind, OutputInfo, OutputList, OutputStatus, Size, State, Status, Toggled, WallpaperInfo,
};
use veil::ipc::server::{self, Reply, RequestHandler, ServerConfig};
use veil::policy::PauseOn;
use veil::scaling::{Color, ScaleMode};

/// How long a test client waits for the server before the test fails instead
/// of hanging.
pub const PATIENCE: Duration = Duration::from_secs(20);

pub const PING: &[u8] = b"{\"cmd\":\"ping\"}\n";

/// The one output the stand-in daemon draws on. A command for another one is
/// refused with `unknown_output`, as the daemon refuses it.
pub const OUTPUT: &str = "DP-3";

/// Stands in for the daemon: it records every request it is handed and answers
/// it as the daemon would, with made-up state: `status` with [`status`],
/// `outputs` with [`outputs`], `toggle` with a pause, the rest with `ok`.
/// A command for an output other than [`OUTPUT`] is refused with
/// `unknown_output`. A `set` of one of these file names is answered otherwise:
///
/// * `broken.png`: answered with an error (`failed`).
/// * `missing.png`: `not_found`.
/// * `notes.txt`: `unsupported_media`.
/// * `hold.png`: not answered yet. The reply is kept, as the daemon keeps it
///   while a wallpaper loads.
/// * `release.png`: every kept reply is answered with `ok`, then this one.
/// * `forget.png`: every kept reply is dropped unanswered, then this one is
///   answered with `ok`.
struct Recorder {
    requests: Arc<Mutex<Vec<Request>>>,
    held: Vec<Reply>,
}

impl RequestHandler for Recorder {
    fn handle_request(&mut self, request: Request, reply: Reply) {
        let (file, output) = match &request {
            Request::Set { input, output, .. } => (
                input.rsplit('/').next().unwrap_or_default().to_string(),
                output.clone(),
            ),
            Request::Clear { output, .. }
            | Request::Pause { output }
            | Request::Resume { output }
            | Request::Toggle { output } => (String::new(), output.clone()),
            Request::Status {} | Request::Outputs {} | Request::Quit {} | Request::Ping {} => {
                (String::new(), None)
            }
        };
        let answer = match &request {
            Request::Status {} => Some(Response::report(&status())),
            Request::Outputs {} => Some(Response::report(&outputs())),
            Request::Toggle { .. } => Some(Response::report(&Toggled { paused: true })),
            _ => None,
        };
        self.requests.lock().expect("request log").push(request);

        if let Some(name) = output.filter(|name| name != OUTPUT) {
            return reply.send(Response::error(
                ErrorCode::UnknownOutput,
                format!("output '{name}' is not driven by this daemon; it draws on {OUTPUT}"),
            ));
        }
        if let Some(answer) = answer {
            return reply.send(answer);
        }
        match file.as_str() {
            "hold.png" => self.held.push(reply),
            "broken.png" => reply.send(Response::error(ErrorCode::Failed, "cannot decode")),
            "missing.png" => reply.send(Response::error(ErrorCode::NotFound, "no such file")),
            "notes.txt" => reply.send(Response::error(
                ErrorCode::UnsupportedMedia,
                "neither an image nor a video",
            )),
            "release.png" => {
                for held in self.held.drain(..) {
                    held.send(Response::ok());
                }
                reply.send(Response::ok());
            }
            "forget.png" => {
                self.held.clear();
                reply.send(Response::ok());
            }
            _ => reply.send(Response::ok()),
        }
    }
}

/// A server running on its own thread until the value is dropped.
pub struct TestServer {
    pub socket: PathBuf,
    requests: Arc<Mutex<Vec<Request>>>,
    /// How often the server's event loop has woken up.
    wakeups: Arc<AtomicUsize>,
    /// Set to end the server thread; `wake` then gets its loop out of the wait.
    stop: Arc<AtomicBool>,
    wake: LoopSignal,
    thread: Option<JoinHandle<()>>,
    /// The server's claim on the socket. Dropped last, like the daemon's: it
    /// removes the socket file.
    guard: Option<SocketGuard>,
}

impl TestServer {
    /// `name` must be unique among the tests of a binary: it is part of the
    /// socket path, together with the process id.
    pub fn start(name: &str, config: ServerConfig) -> Self {
        let socket =
            std::env::temp_dir().join(format!("veil-ipc-{}-{name}.sock", std::process::id()));
        Self::start_at(socket, config)
    }

    /// A server on the socket at `socket`.
    pub fn start_at(socket: PathBuf, config: ServerConfig) -> Self {
        let (listener, guard) = instance::claim(&socket).expect("claim the test socket");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let wakeups = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));

        let (ready, started) = mpsc::channel();
        let log = Arc::clone(&requests);
        let (counted, stopped) = (Arc::clone(&wakeups), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            // Built here: a kept `Reply` belongs to the thread of the server.
            let mut daemon = Recorder {
                requests: log,
                held: Vec::new(),
            };
            let mut event_loop: EventLoop<Recorder> = EventLoop::try_new().expect("event loop");
            server::insert(&event_loop.handle(), listener, config).expect("insert the server");
            ready
                .send(event_loop.get_signal())
                .expect("report the loop");
            // Not `EventLoop::run` with `LoopSignal::stop`: `run` clears that
            // flag when it starts, so a stop sent before then would be lost.
            while !stopped.load(Ordering::Acquire) {
                event_loop
                    .dispatch(None, &mut daemon)
                    .expect("the event loop must survive every client");
                counted.fetch_add(1, Ordering::Relaxed);
            }
        });
        let wake = started.recv().expect("the server thread started");

        Self {
            socket,
            requests,
            wakeups,
            stop,
            wake,
            thread: Some(thread),
            guard: Some(guard),
        }
    }

    pub fn connect(&self) -> UnixStream {
        let stream = UnixStream::connect(&self.socket).expect("connect");
        stream.set_read_timeout(Some(PATIENCE)).expect("timeout");
        stream.set_write_timeout(Some(PATIENCE)).expect("timeout");
        stream
    }

    /// Sends `request` as it is and returns the response.
    pub fn exchange(&self, request: &[u8]) -> Response {
        let mut stream = self.connect();
        stream.write_all(request).expect("write the request");
        read_response(&stream)
    }

    /// The requests the daemon stand-in was handed so far.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("request log").clone()
    }

    /// How often the server's event loop has woken up so far. A loop that is
    /// waiting does not wake up.
    pub fn wakeups(&self) -> usize {
        self.wakeups.load(Ordering::Relaxed)
    }

    /// The server still answers: whatever the test did to it, it is not stuck
    /// and its event loop is running.
    pub fn assert_alive(&self) {
        let response = client::send(&self.socket, &Request::Ping {}).expect("ping");
        assert_eq!(response, Response::ok());
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake.wakeup();
        let joined = self.thread.take().map(JoinHandle::join);
        // What a daemon does when it shuts down.
        drop(self.guard.take());
        let _ = std::fs::remove_file(instance::lock_path(&self.socket));
        if !std::thread::panicking() {
            if let Some(Err(_)) = joined {
                panic!("the server thread panicked");
            }
        }
    }
}

/// Everything the server sent before it closed the connection.
pub fn read_to_close(mut stream: &UnixStream) -> Vec<u8> {
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return received,
            Ok(len) => received.extend_from_slice(&chunk[..len]),
            // The server closed a connection that still had unread input.
            Err(e) if e.kind() == ErrorKind::ConnectionReset => return received,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => panic!("the server did not close the connection: {e}"),
        }
    }
}

/// The single response line of a connection.
pub fn read_response(stream: &UnixStream) -> Response {
    let received = read_to_close(stream);
    let text = std::str::from_utf8(&received).expect("UTF-8 response");
    let line = text
        .strip_suffix('\n')
        .unwrap_or_else(|| panic!("no newline-terminated response: {text:?}"));
    assert!(!line.contains('\n'), "more than one line: {text:?}");
    serde_json::from_str(line).unwrap_or_else(|e| panic!("unreadable response {line:?}: {e}"))
}

/// The error code of `response`, which must be an error of this protocol version.
pub fn code_of(response: &Response) -> ErrorCode {
    assert_eq!(response.version, PROTOCOL_VERSION);
    match &response.outcome {
        Outcome::Error { code, .. } => *code,
        Outcome::Ok => panic!("expected an error response"),
    }
}

/// What the stand-in daemon answers `status` with.
pub fn status() -> Status {
    Status {
        version: "0.1.0".into(),
        pid: 7,
        uptime_secs: 65,
        pause_on: PauseOn::Fullscreen,
        default: None,
        outputs: vec![OutputStatus {
            name: Some(OUTPUT.into()),
            connected: true,
            wallpaper: WallpaperInfo {
                input: Some("/srv/a.png".into()),
                mode: ScaleMode::Fit,
                bg_color: Color::default(),
            },
            kind: Kind::Image,
            state: State::Shown,
            paused_by_user: false,
            hidden: false,
            size: Some(Size {
                width: 1920,
                height: 1080,
            }),
            scale: Some(1.0),
            video: None,
            next: None,
        }],
    }
}

/// What the stand-in daemon answers `outputs` with.
pub fn outputs() -> OutputList {
    OutputList {
        outputs: vec![OutputInfo {
            name: Some(OUTPUT.into()),
            description: Some("Test monitor".into()),
            connected: true,
            wallpaper: true,
        }],
    }
}

pub fn set(input: &str) -> Request {
    Request::Set {
        output: None,
        input: input.into(),
        mode: None,
        bg_color: None,
        persist: true,
    }
}

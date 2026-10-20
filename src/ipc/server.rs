//! Daemon side of the control socket: the transport.
//!
//! Nothing here blocks the event loop, whatever a client does. The listening
//! socket and every accepted connection are non-blocking event sources. A
//! connection carries one request and one response and is then closed. It is
//! also closed when it sends too much, or nothing complete within the read
//! timeout.
//!
//! What a request does is not decided here: a parsed and validated [`Request`]
//! goes to the [`RequestHandler`] the event loop runs with, together with the
//! [`Reply`] to answer it. The handler may answer later (a wallpaper is loaded
//! on another thread); the connection then waits in the reply, outside the
//! loop.

use super::peer;
use super::protocol::{ErrorCode, Request, Response};
use crate::error::VeilError;
use calloop::{
    generic::Generic,
    timer::{TimeoutAction, Timer},
    EventSource, Interest, LoopHandle, Mode, Poll, PostAction, Readiness, RegistrationToken, Token,
    TokenFactory,
};
use std::cell::Cell;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::rc::Rc;
use std::time::Duration;

/// How long a client may take to send its request, counted from the moment it
/// connects.
pub const READ_TIMEOUT: Duration = Duration::from_secs(2);

/// Longest accepted request line in bytes, without the newline.
pub const MAX_REQUEST_LEN: usize = 64 * 1024;

/// How many clients may be connected at the same time.
pub const MAX_CONNECTIONS: usize = 16;

/// How long the server stops accepting after `accept` failed.
pub const ACCEPT_RETRY: Duration = Duration::from_secs(1);

/// Carries out the requests the server receives. Implemented by the data the
/// event loop is dispatched with.
pub trait RequestHandler {
    /// Carries out `request` and answers it through `reply`: before returning,
    /// or later, when the result is known. This is called on the event loop, so
    /// what takes long must not be done here.
    fn handle_request(&mut self, request: Request, reply: Reply);
}

/// Limits of the server. [`ServerConfig::default`] is what the daemon uses;
/// tests shorten the timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerConfig {
    /// A connection that has not sent a complete request after this long is
    /// closed. The time is not extended by sending a little more.
    pub read_timeout: Duration,
    /// Longest accepted request line in bytes, without the newline.
    pub max_request_len: usize,
    /// Connections beyond this number are turned away at once.
    pub max_connections: usize,
    /// Pause before the next attempt after `accept` failed, which happens when
    /// the daemon has no file descriptor left for the connection.
    pub accept_retry: Duration,
    /// The only user whose connections are accepted.
    pub allowed_uid: u32,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            read_timeout: READ_TIMEOUT,
            max_request_len: MAX_REQUEST_LEN,
            max_connections: MAX_CONNECTIONS,
            accept_retry: ACCEPT_RETRY,
            allowed_uid: peer::own_uid(),
        }
    }
}

/// Serves `listener` (from `ipc::instance::claim`) from the event loop of `handle`.
///
/// The server keeps a handle to the loop (it adds one event source per
/// connection), so it lives as long as the loop's sources do.
pub fn insert<'l, D>(
    handle: &LoopHandle<'l, D>,
    listener: UnixListener,
    config: ServerConfig,
) -> Result<(), VeilError>
where
    D: RequestHandler + 'l,
{
    let connections = handle.clone();
    let open = Rc::new(Cell::new(0));
    // The listener's own registration, which its callback needs to pause it.
    let registration = Rc::new(Cell::new(None));
    let own_registration = Rc::clone(&registration);

    let token = handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            move |_, listener, _| {
                let Err(e) = accept_all(listener, &connections, &config, &open) else {
                    return Ok(PostAction::Continue);
                };
                // The connection that could not be accepted is still waiting, so
                // the listener stays readable: carrying on would wake the loop
                // again at once, for as long as the cause lasts. The listener is
                // taken out of the loop for a while instead.
                let paused = own_registration
                    .get()
                    .is_some_and(|own| resume_later(&connections, own, config.accept_retry));
                tracing::warn!(
                    "IPC accept failed: {e}; {}",
                    if paused {
                        format!("trying again in {:?}", config.accept_retry)
                    } else {
                        "trying again".into()
                    }
                );
                Ok(if paused {
                    PostAction::Disable
                } else {
                    PostAction::Continue
                })
            },
        )
        .map_err(|e| VeilError::EventLoop(e.to_string()))?;
    registration.set(Some(token));
    Ok(())
}

/// Puts the disabled `listener` back into the loop after `delay`. Returns whether
/// that could be arranged; if not, the listener must not be disabled.
fn resume_later<'l, D: 'l>(
    handle: &LoopHandle<'l, D>,
    listener: RegistrationToken,
    delay: Duration,
) -> bool {
    let resume = handle.clone();
    handle
        .insert_source(Timer::from_duration(delay), move |_, _, _| {
            if let Err(e) = resume.enable(&listener) {
                tracing::error!("IPC server cannot accept connections any more: {e}");
            }
            TimeoutAction::Drop
        })
        .is_ok()
}

/// Accepts every connection that is waiting. A connection of another user, or
/// one over the limit, is answered with an error and closed straight away. An
/// error means that `accept` itself failed and a connection is still waiting
/// (typically the process is out of file descriptors).
fn accept_all<'l, D>(
    listener: &UnixListener,
    handle: &LoopHandle<'l, D>,
    config: &ServerConfig,
    open: &Rc<Cell<usize>>,
) -> std::io::Result<()>
where
    D: RequestHandler + 'l,
{
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
            // Interrupted by a signal, or the client gave up while it waited.
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == ErrorKind::ConnectionAborted => continue,
            Err(e) => return Err(e),
        };
        // An accepted socket does not inherit the listener's non-blocking mode.
        if let Err(e) = stream.set_nonblocking(true) {
            tracing::warn!("IPC connection dropped, it cannot be made non-blocking: {e}");
            continue;
        }

        match peer::peer_uid(&stream) {
            Ok(uid) if uid == config.allowed_uid => {}
            Ok(uid) => {
                tracing::warn!("IPC connection from user id {uid} rejected");
                let message = "this daemon belongs to another user";
                send(&stream, &Response::error(ErrorCode::Forbidden, message));
                continue;
            }
            Err(e) => {
                tracing::warn!("IPC connection dropped, its user is unknown: {e}");
                continue;
            }
        }

        if open.get() >= config.max_connections {
            tracing::warn!(
                "IPC connection rejected: {} clients are already connected",
                open.get()
            );
            let message = "too many clients are connected; try again";
            send(&stream, &Response::error(ErrorCode::Busy, message));
            continue;
        }

        let connection = Connection::new(stream, config, Slot::take(open));
        if let Err(e) = handle.insert_source(connection, |(request, reply), _, daemon| {
            daemon.handle_request(request, reply)
        }) {
            tracing::warn!("IPC connection dropped, it cannot be watched: {e}");
        }
    }
}

/// One of the `max_connections` places. It is given back when the connection is
/// dropped, whatever the reason.
#[derive(Debug)]
struct Slot(Rc<Cell<usize>>);

impl Slot {
    fn take(open: &Rc<Cell<usize>>) -> Self {
        open.set(open.get() + 1);
        Self(Rc::clone(open))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.set(self.0.get().saturating_sub(1));
    }
}

/// The answer a client is waiting for. The handler gets one with every request
/// and may keep it for as long as the request takes: the connection stays open,
/// with no time limit, and keeps its place among the `max_connections`.
///
/// Sending never waits for the client. A reply that is dropped unanswered
/// tells the client so, which makes it impossible to leave a client hanging by
/// forgetting it.
#[derive(Debug)]
pub struct Reply {
    stream: Option<UnixStream>,
    _slot: Slot,
}

impl Reply {
    /// Sends the response and closes the connection.
    pub fn send(mut self, response: Response) {
        if let Some(stream) = self.stream.take() {
            send(&stream, &response);
        }
    }
}

impl Drop for Reply {
    fn drop(&mut self) {
        if let Some(stream) = self.stream.take() {
            let message = "the daemon ended the request without an answer";
            send(&stream, &Response::error(ErrorCode::Failed, message));
        }
    }
}

/// One client: an event source that yields the client's request together with
/// the [`Reply`] to answer it. It removes itself from the loop once it has
/// handed the request over or refused it, the client is gone, or the time is up.
struct Connection {
    /// `None` once the stream has been handed over in a [`Reply`].
    stream: Option<Generic<UnixStream>>,
    deadline: Timer,
    /// The request line received so far.
    request: Vec<u8>,
    max_request_len: usize,
    read_timeout: Duration,
    slot: Option<Slot>,
}

impl Connection {
    fn new(stream: UnixStream, config: &ServerConfig, slot: Slot) -> Self {
        Self {
            stream: Some(Generic::new(stream, Interest::READ, Mode::Level)),
            deadline: Timer::from_duration(config.read_timeout),
            request: Vec::new(),
            max_request_len: config.max_request_len,
            read_timeout: config.read_timeout,
            slot: Some(slot),
        }
    }

    /// Answers the client from the transport itself, without the handler.
    fn refuse(&self, code: ErrorCode, message: impl Into<String>) {
        if let Some(stream) = &self.stream {
            send(stream.get_ref(), &Response::error(code, message));
        }
    }
}

impl EventSource for Connection {
    type Event = (Request, Reply);
    type Metadata = ();
    type Ret = ();
    type Error = std::io::Error;

    // No error of a client is returned from here: an error would end the
    // daemon's event loop.
    fn process_events<F>(
        &mut self,
        readiness: Readiness,
        token: Token,
        mut callback: F,
    ) -> Result<PostAction, Self::Error>
    where
        F: FnMut(Self::Event, &mut Self::Metadata) -> Self::Ret,
    {
        let mut timed_out = false;
        self.deadline.process_events(readiness, token, |_, _| {
            timed_out = true;
            TimeoutAction::Drop
        })?;
        if timed_out {
            tracing::warn!(
                "IPC client sent no complete request within {:?}; closing the connection",
                self.read_timeout
            );
            self.refuse(ErrorCode::Timeout, "timed out waiting for the request");
            return Ok(PostAction::Remove);
        }

        let Some(stream) = self.stream.as_mut() else {
            return Ok(PostAction::Remove);
        };
        let mut progress = None;
        let (request, limit) = (&mut self.request, self.max_request_len);
        stream.process_events(readiness, token, |_, stream| {
            progress = Some(read_request(&**stream, request, limit));
            Ok(PostAction::Continue)
        })?;

        match progress {
            // The event was not for this connection, or more is to come.
            None | Some(Progress::Pending) => return Ok(PostAction::Continue),
            Some(Progress::Closed) => tracing::debug!("IPC client disconnected without a request"),
            Some(Progress::Failed(e)) => tracing::debug!("IPC read failed: {e}"),
            Some(Progress::TooLarge) => {
                tracing::warn!("IPC request over {limit} bytes rejected");
                self.refuse(
                    ErrorCode::TooLarge,
                    format!("the request is longer than {limit} bytes"),
                );
            }
            Some(Progress::Complete) => match Request::decode(&self.request) {
                Err(message) => self.refuse(ErrorCode::InvalidRequest, message),
                Ok(request) => {
                    // The request is in: from here on the connection belongs to
                    // whoever answers it. It leaves the loop (and its read
                    // timeout) and lives on in the reply.
                    if let (Some(stream), Some(slot)) = (self.stream.take(), self.slot.take()) {
                        let reply = Reply {
                            // `Generic::unwrap` gives the stream back out of
                            // its event source; it is not one that can fail.
                            stream: Some(stream.unwrap()),
                            _slot: slot,
                        };
                        callback((request, reply), &mut ());
                    }
                }
            },
        }
        Ok(PostAction::Remove)
    }

    fn register(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        if let Some(stream) = &mut self.stream {
            stream.register(poll, factory)?;
        }
        self.deadline.register(poll, factory)
    }

    fn reregister(&mut self, poll: &mut Poll, factory: &mut TokenFactory) -> calloop::Result<()> {
        if let Some(stream) = &mut self.stream {
            stream.reregister(poll, factory)?;
        }
        self.deadline.reregister(poll, factory)
    }

    fn unregister(&mut self, poll: &mut Poll) -> calloop::Result<()> {
        if let Some(stream) = &mut self.stream {
            stream.unregister(poll)?;
        }
        self.deadline.unregister(poll)
    }
}

/// Where the request of a connection stands after reading what was available.
#[derive(Debug)]
enum Progress {
    /// No complete line yet, and nothing more to read right now.
    Pending,
    /// The line is complete (it ended with a newline, or the client finished
    /// sending) and is in the buffer, without the newline.
    Complete,
    /// The line is longer than the limit.
    TooLarge,
    /// The client finished without sending anything.
    Closed,
    Failed(std::io::Error),
}

/// Appends what `reader` has ready to `line`, up to the first newline, and never
/// keeps more than `max_len` bytes plus one read. It returns when the reader
/// would block, so a non-blocking socket never stalls the caller. Bytes after
/// the newline are ignored: a connection carries a single request.
fn read_request(mut reader: impl Read, line: &mut Vec<u8>, max_len: usize) -> Progress {
    let mut chunk = [0u8; 4096];
    loop {
        let received = match reader.read(&mut chunk) {
            Ok(0) if line.is_empty() => return Progress::Closed,
            Ok(0) => return Progress::Complete,
            Ok(len) => &chunk[..len],
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Progress::Pending,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => return Progress::Failed(e),
        };

        let newline = received.iter().position(|&byte| byte == b'\n');
        line.extend_from_slice(&received[..newline.unwrap_or(received.len())]);
        if line.len() > max_len {
            return Progress::TooLarge;
        }
        if newline.is_some() {
            return Progress::Complete;
        }
    }
}

/// Sends `response` and does not wait for the client. The socket is
/// non-blocking and nothing else was written to it, so a response fits in its
/// buffer; if the client is gone, the response is dropped.
fn send(mut stream: &UnixStream, response: &Response) {
    let mut payload = match serde_json::to_vec(response) {
        Ok(payload) => payload,
        Err(e) => {
            tracing::warn!("IPC response could not be serialized: {e}");
            return;
        }
    };
    payload.push(b'\n');
    if let Err(e) = stream.write_all(&payload) {
        tracing::debug!("IPC response not delivered: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::io;

    /// A reader that returns the scripted results in order, then blocks.
    struct Script(VecDeque<io::Result<Vec<u8>>>);

    impl Script {
        fn new(steps: impl IntoIterator<Item = io::Result<&'static [u8]>>) -> Self {
            Self(
                steps
                    .into_iter()
                    .map(|step| step.map(<[u8]>::to_vec))
                    .collect(),
            )
        }
    }

    impl Read for Script {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.0.pop_front() {
                Some(Ok(mut data)) => {
                    let len = data.len().min(buf.len());
                    buf[..len].copy_from_slice(&data[..len]);
                    if len < data.len() {
                        self.0.push_front(Ok(data.split_off(len)));
                    }
                    Ok(len)
                }
                Some(Err(e)) => Err(e),
                None => Err(ErrorKind::WouldBlock.into()),
            }
        }
    }

    const EOF: io::Result<&[u8]> = Ok(b"");

    fn would_block() -> io::Result<&'static [u8]> {
        Err(ErrorKind::WouldBlock.into())
    }

    #[test]
    fn a_line_ends_at_the_newline() {
        let mut line = Vec::new();
        let progress = read_request(
            Script::new([Ok(&b"{\"cmd\":\"ping\"}\n"[..])]),
            &mut line,
            64,
        );
        assert!(matches!(progress, Progress::Complete), "{progress:?}");
        assert_eq!(line, b"{\"cmd\":\"ping\"}");
    }

    #[test]
    fn a_line_may_arrive_in_pieces_across_wakeups() {
        let mut line = Vec::new();
        let mut reader = Script::new([
            Ok(&b"{\"cmd\""[..]),
            would_block(),
            Ok(b":\"pi"),
            Ok(b"ng\"}"),
            would_block(),
            Ok(b"\n"),
        ]);
        for expected_len in [6, 14] {
            let progress = read_request(&mut reader, &mut line, 64);
            assert!(matches!(progress, Progress::Pending), "{progress:?}");
            assert_eq!(line.len(), expected_len);
        }
        let progress = read_request(&mut reader, &mut line, 64);
        assert!(matches!(progress, Progress::Complete), "{progress:?}");
        assert_eq!(line, b"{\"cmd\":\"ping\"}");
    }

    #[test]
    fn end_of_stream_completes_a_line_without_newline() {
        let mut line = Vec::new();
        let progress = read_request(Script::new([Ok(&b"{}"[..]), EOF]), &mut line, 64);
        assert!(matches!(progress, Progress::Complete), "{progress:?}");
        assert_eq!(line, b"{}");
    }

    #[test]
    fn end_of_stream_without_data_is_a_closed_connection() {
        let mut line = Vec::new();
        let progress = read_request(Script::new([EOF]), &mut line, 64);
        assert!(matches!(progress, Progress::Closed), "{progress:?}");
    }

    #[test]
    fn nothing_to_read_is_pending() {
        let mut line = Vec::new();
        let progress = read_request(Script::new([]), &mut line, 64);
        assert!(matches!(progress, Progress::Pending), "{progress:?}");
        assert!(line.is_empty());
    }

    #[test]
    fn bytes_after_the_newline_are_ignored() {
        let mut line = Vec::new();
        let progress = read_request(Script::new([Ok(&b"one\ntwo\nthree"[..])]), &mut line, 64);
        assert!(matches!(progress, Progress::Complete), "{progress:?}");
        assert_eq!(line, b"one");
    }

    #[test]
    fn the_limit_counts_the_line_without_its_newline() {
        let mut line = Vec::new();
        let progress = read_request(Script::new([Ok(&b"12345678\n"[..])]), &mut line, 8);
        assert!(matches!(progress, Progress::Complete), "{progress:?}");

        let mut line = Vec::new();
        let progress = read_request(Script::new([Ok(&b"123456789\n"[..])]), &mut line, 8);
        assert!(matches!(progress, Progress::TooLarge), "{progress:?}");
    }

    #[test]
    fn a_line_without_end_stops_at_the_limit() {
        // The reader never runs dry, like a client that keeps sending.
        struct Endless(usize);
        impl Read for Endless {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                buf.fill(b'x');
                self.0 += buf.len();
                Ok(buf.len())
            }
        }

        let mut reader = Endless(0);
        let mut line = Vec::new();
        let progress = read_request(&mut reader, &mut line, MAX_REQUEST_LEN);
        assert!(matches!(progress, Progress::TooLarge), "{progress:?}");
        // It stopped reading (and buffering) right after passing the limit.
        assert!(reader.0 <= MAX_REQUEST_LEN + 4096, "{}", reader.0);
        assert!(line.len() <= MAX_REQUEST_LEN + 4096, "{}", line.len());
    }

    #[test]
    fn an_interrupted_read_is_retried_and_other_errors_fail() {
        let mut line = Vec::new();
        let progress = read_request(
            Script::new([Err(ErrorKind::Interrupted.into()), Ok(&b"a\n"[..])]),
            &mut line,
            64,
        );
        assert!(matches!(progress, Progress::Complete), "{progress:?}");

        let mut line = Vec::new();
        let progress = read_request(
            Script::new([Ok(&b"a"[..]), Err(ErrorKind::ConnectionReset.into())]),
            &mut line,
            64,
        );
        assert!(matches!(progress, Progress::Failed(_)), "{progress:?}");
    }

    #[test]
    fn default_limits() {
        let config = ServerConfig::default();
        assert_eq!(config.read_timeout, Duration::from_secs(2));
        assert_eq!(config.max_request_len, 64 * 1024);
        assert_eq!(config.max_connections, 16);
        assert_eq!(config.accept_retry, Duration::from_secs(1));
        assert_eq!(config.allowed_uid, peer::own_uid());
    }
}

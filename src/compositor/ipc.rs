//! Following a compositor's IPC: an event socket that says when something may
//! have changed, and requests that say what the state is now.
//!
//! Every backend works the same way, whatever its wire format: the events are
//! not trusted to describe the state (they often do not say which output they
//! are about, and some changes come without an event). An event that may
//! change what an output shows schedules a resync, which asks the compositor
//! for its whole state and computes every output's [`super::OutputView`] from
//! the answers (pure, per backend). What differs from the last resync is
//! reported ([`super::diff`]). Events that come in a burst are taken together,
//! and a resync never runs twice at the same time.
//!
//! What a backend contributes is its [`Protocol`]: how its events are framed,
//! what it sends to subscribe, which requests a resync makes, when an answer is
//! complete, and the state the answers describe.
//!
//! Every socket is a non-blocking `calloop` source, and no thread is started.
//! A request that is not answered in time is given up. When the event socket
//! closes (or its stream makes no sense), [`CompositorEvent::Disconnected`] is
//! reported and the connection is tried again, with a growing pause. Nothing
//! here ends the daemon or returns an error from an event source.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use calloop::{
    generic::Generic,
    timer::{TimeoutAction, Timer},
    Interest, LoopHandle, Mode, PostAction, RegistrationToken,
};
use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};

use super::{diff, CompositorEvent, CompositorEvents, Snapshot};

/// Where a compositor listens. The two may be the same socket (sway): every
/// request then has a connection of its own, as with separate sockets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sockets {
    /// The socket whose connection streams the events.
    pub events: PathBuf,
    /// The socket requests are sent to, one connection per request.
    pub requests: PathBuf,
}

/// Timing of a backend. [`Config::default`] is what the daemon uses; tests
/// shorten it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// How long after an event the state is asked for, so that the events of
    /// one change are taken together.
    pub resync_delay: Duration,
    /// How long the compositor may take to answer one request.
    pub request_timeout: Duration,
    /// First pause before the event socket is connected again, or a failed
    /// resync is tried again. It doubles with every failure in a row.
    pub retry_min: Duration,
    /// Longest pause.
    pub retry_max: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            resync_delay: Duration::from_millis(30),
            request_timeout: Duration::from_secs(2),
            retry_min: Duration::from_secs(1),
            retry_max: Duration::from_secs(30),
        }
    }
}

/// Largest answer to a request.
pub const MAX_REPLY: usize = 8 * 1024 * 1024;

/// What a compositor's IPC looks like on the wire. Everything here is pure.
pub trait Protocol: 'static {
    /// The compositor's name, for the log.
    fn name(&self) -> &'static str;

    /// What is sent on the event socket right after it is connected (a
    /// subscription). Empty: nothing.
    fn subscription(&self) -> Vec<u8> {
        Vec::new()
    }

    /// Takes the complete events out of `pending`, the bytes read from the
    /// event socket and not taken yet, and leaves an incomplete one there.
    /// `Ok(true)` if one of them may have changed what an output shows. An
    /// error ends the connection: the stream makes no sense, or the compositor
    /// says that it is leaving.
    fn take_events(&self, pending: &mut Vec<u8>) -> Result<bool, String>;

    /// The requests of a resync, in order: a name for the log, and the bytes.
    fn requests(&self) -> Vec<(&'static str, Vec<u8>)>;

    /// Whether `reply` is a whole answer. An answer is also whole when the
    /// compositor closes the connection.
    fn reply_complete(&self, _reply: &[u8]) -> bool {
        false
    }

    /// What every output shows, from the answers, in the order of
    /// [`Protocol::requests`].
    fn snapshot(&self, replies: &[Vec<u8>]) -> Result<Snapshot, String>;
}

/// Connects to a Unix socket without blocking: a listener whose backlog is
/// full is an error, not a wait.
fn open(path: &Path) -> std::io::Result<UnixStream> {
    let socket = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
        None,
    )?;
    rustix::net::connect(&socket, &SocketAddrUnix::new(path)?)?;
    Ok(UnixStream::from(socket))
}

/// The connection to one compositor, shared by the event sources.
struct Link<'l, D, P> {
    handle: LoopHandle<'l, D>,
    protocol: P,
    sockets: Sockets,
    config: Config,
    /// Whether the event socket is connected.
    connected: bool,
    /// Counts the connections, so that a resync that began on an earlier
    /// connection is not reported.
    connection: u64,
    /// What was last reported. Empty while there is no connection.
    reported: Snapshot,
    resync: Resync,
    /// The requests of the running resync.
    query: Option<Query>,
    /// Pause before the next attempt after a failure.
    retry: Duration,
    /// Whether the last connection attempt failed, so that a compositor that
    /// stays away is logged once and not every time.
    failing: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resync {
    Idle,
    /// Its timer runs.
    Scheduled,
    /// Its requests run. `again`: something changed meanwhile, so another one
    /// follows.
    Running {
        again: bool,
    },
}

struct Query {
    connection: u64,
    waiting: VecDeque<(&'static str, Vec<u8>)>,
    replies: Vec<Vec<u8>>,
    /// The source that reads the answer to the current request.
    reader: Option<RegistrationToken>,
    timeout: Option<RegistrationToken>,
}

type Shared<'l, D, P> = Rc<RefCell<Link<'l, D, P>>>;

/// Follows the compositor that listens on `sockets` and speaks `protocol`,
/// from the event loop of `handle`, reporting to its [`CompositorEvents`].
pub fn insert<'l, D, P>(handle: &LoopHandle<'l, D>, protocol: P, sockets: Sockets, config: Config)
where
    D: CompositorEvents + 'l,
    P: Protocol,
{
    let shared = Rc::new(RefCell::new(Link {
        handle: handle.clone(),
        protocol,
        sockets,
        config,
        connected: false,
        connection: 0,
        reported: Snapshot::new(),
        resync: Resync::Idle,
        query: None,
        retry: config.retry_min,
        failing: false,
    }));
    connect(&shared);
}

/// Connects to the event socket and subscribes, or tries again later.
fn connect<'l, D: CompositorEvents + 'l, P: Protocol>(shared: &Shared<'l, D, P>) {
    let mut link = shared.borrow_mut();
    let name = link.protocol.name();
    let path = link.sockets.events.clone();
    let subscription = link.protocol.subscription();
    let opened = open(&path).and_then(|stream| {
        // A few bytes on a new connection do not have to wait.
        (&stream).write_all(&subscription)?;
        Ok(stream)
    });
    let stream = match opened {
        Ok(stream) => stream,
        Err(e) => {
            if link.failing {
                tracing::debug!("{name}'s event socket is still not there: {e}");
            } else {
                tracing::warn!(
                    "cannot connect to {name}'s event socket {}: {e}; trying again",
                    path.display()
                );
            }
            link.failing = true;
            drop(link);
            return retry_later(shared, connect);
        }
    };

    let reader = Rc::clone(shared);
    let mut pending = Vec::new();
    let inserted = link.handle.insert_source(
        Generic::new(stream, Interest::READ, Mode::Level),
        move |_, stream, data| Ok(read_events(&reader, stream, &mut pending, data)),
    );
    if let Err(e) = inserted {
        tracing::error!("cannot follow {name}'s event socket: {e}");
        drop(link);
        return retry_later(shared, connect);
    }
    if link.failing || link.connection > 0 {
        tracing::info!("connected to {name}'s event socket again");
    } else {
        tracing::debug!("connected to {name}'s event socket");
    }
    link.connected = true;
    link.connection += 1;
    link.failing = false;
    link.retry = link.config.retry_min;
    drop(link);
    // What every output shows right now.
    schedule_resync(shared, Duration::ZERO);
}

/// Calls `attempt` after the current retry pause, which then doubles.
fn retry_later<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    attempt: fn(&Shared<'l, D, P>),
) {
    let mut link = shared.borrow_mut();
    let delay = link.retry;
    link.retry = (link.retry * 2).min(link.config.retry_max);
    let again = Rc::clone(shared);
    let scheduled = link
        .handle
        .insert_source(Timer::from_duration(delay), move |_, _, _| {
            attempt(&again);
            TimeoutAction::Drop
        });
    if let Err(e) = scheduled {
        tracing::error!("cannot follow {} any more: {e}", link.protocol.name());
    }
}

/// Reads what the event socket has, and schedules a resync if an event may have
/// changed an output.
fn read_events<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    mut stream: &UnixStream,
    pending: &mut Vec<u8>,
    data: &mut D,
) -> PostAction {
    let name = shared.borrow().protocol.name();
    let mut changed = false;
    let mut chunk = [0u8; 4096];
    // Bounded, so that a flood of events cannot keep the loop here; the
    // source is level-triggered and comes back for the rest.
    for _ in 0..64 {
        match stream.read(&mut chunk) {
            Ok(0) => {
                lost(shared, data, format!("{name} closed its event socket"));
                return PostAction::Remove;
            }
            Ok(n) => {
                pending.extend_from_slice(&chunk[..n]);
                let taken = shared.borrow().protocol.take_events(pending);
                match taken {
                    Ok(change) => changed |= change,
                    Err(reason) => {
                        lost(shared, data, reason);
                        return PostAction::Remove;
                    }
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => {
                lost(
                    shared,
                    data,
                    format!("reading {name}'s event socket failed: {e}"),
                );
                return PostAction::Remove;
            }
        }
    }
    if changed {
        let delay = shared.borrow().config.resync_delay;
        schedule_resync(shared, delay);
    }
    PostAction::Continue
}

/// The event socket is gone. What was reported no longer holds; the
/// connection is tried again.
fn lost<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    data: &mut D,
    reason: String,
) {
    {
        let mut link = shared.borrow_mut();
        link.connected = false;
        link.reported.clear();
    }
    tracing::warn!("{reason}; connecting again");
    data.compositor_event(CompositorEvent::Disconnected { reason });
    retry_later(shared, connect);
}

/// Asks for the state after `delay`, unless that is arranged already. A resync
/// that is running is followed by another one.
fn schedule_resync<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    delay: Duration,
) {
    let mut link = shared.borrow_mut();
    match link.resync {
        Resync::Scheduled | Resync::Running { again: true } => return,
        Resync::Running { again: false } => {
            link.resync = Resync::Running { again: true };
            return;
        }
        Resync::Idle => {}
    }
    let resync = Rc::clone(shared);
    let scheduled = link
        .handle
        .insert_source(Timer::from_duration(delay), move |_, _, data| {
            start_resync(&resync, data);
            TimeoutAction::Drop
        });
    match scheduled {
        Ok(_) => link.resync = Resync::Scheduled,
        Err(e) => tracing::error!("cannot ask {} for its state: {e}", link.protocol.name()),
    }
}

fn start_resync<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    data: &mut D,
) {
    {
        let mut link = shared.borrow_mut();
        if !link.connected {
            link.resync = Resync::Idle;
            return;
        }
        link.resync = Resync::Running { again: false };
        let requests: VecDeque<_> = link.protocol.requests().into_iter().collect();
        let count = u32::try_from(requests.len()).unwrap_or(u32::MAX);
        let timeout_after = link.config.request_timeout.saturating_mul(count);
        let name = link.protocol.name();
        let watchdog = Rc::clone(shared);
        let timeout = link
            .handle
            .insert_source(Timer::from_duration(timeout_after), move |_, _, _| {
                // Its own registration ends with this callback.
                if let Some(query) = watchdog.borrow_mut().query.as_mut() {
                    query.timeout = None;
                }
                resync_failed(
                    &watchdog,
                    format!("{name} did not answer within {timeout_after:?}"),
                );
                TimeoutAction::Drop
            })
            .ok();
        link.query = Some(Query {
            connection: link.connection,
            waiting: requests,
            replies: Vec::new(),
            reader: None,
            timeout,
        });
    }
    send_next_request(shared, data);
}

/// Sends the next request of the running resync, or finishes it.
fn send_next_request<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    data: &mut D,
) {
    let mut link = shared.borrow_mut();
    let name = link.protocol.name();
    let Some((request, bytes)) = link.query.as_mut().and_then(|q| q.waiting.pop_front()) else {
        drop(link);
        return finish_resync(shared, data);
    };
    let sent = open(&link.sockets.requests).and_then(|stream| {
        // A request of a few bytes on a new connection does not have to wait.
        (&stream).write_all(&bytes)?;
        Ok(stream)
    });
    let stream = match sent {
        Ok(stream) => stream,
        Err(e) => {
            drop(link);
            return resync_failed(shared, format!("cannot send {request} to {name}: {e}"));
        }
    };
    let reader = Rc::clone(shared);
    let mut reply = Vec::new();
    let inserted = link.handle.insert_source(
        Generic::new(stream, Interest::READ, Mode::Level),
        move |_, stream, data| Ok(read_reply(&reader, stream, &mut reply, data)),
    );
    match inserted {
        Ok(token) => {
            if let Some(query) = link.query.as_mut() {
                query.reader = Some(token);
            }
        }
        Err(e) => {
            drop(link);
            resync_failed(shared, format!("cannot read {name}'s answer: {e}"));
        }
    }
}

/// Reads the answer to the current request, which is complete when the
/// protocol says so, or when the compositor closes the connection.
fn read_reply<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    mut stream: &UnixStream,
    reply: &mut Vec<u8>,
    data: &mut D,
) -> PostAction {
    let name = shared.borrow().protocol.name();
    let mut chunk = [0u8; 16 * 1024];
    let outcome = loop {
        match stream.read(&mut chunk) {
            Ok(0) => break Ok(std::mem::take(reply)),
            Ok(n) if reply.len() + n > MAX_REPLY => {
                break Err(format!("{name}'s answer is larger than {MAX_REPLY} bytes"))
            }
            Ok(n) => {
                reply.extend_from_slice(&chunk[..n]);
                if shared.borrow().protocol.reply_complete(reply) {
                    break Ok(std::mem::take(reply));
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => return PostAction::Continue,
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) => break Err(format!("reading {name}'s answer failed: {e}")),
        }
    };
    // This source ends here (and with it the connection); its token must not
    // be removed a second time.
    if let Some(query) = shared.borrow_mut().query.as_mut() {
        query.reader = None;
    }
    match outcome {
        Ok(answer) => {
            if let Some(query) = shared.borrow_mut().query.as_mut() {
                query.replies.push(answer);
            }
            send_next_request(shared, data);
        }
        Err(reason) => resync_failed(shared, reason),
    }
    PostAction::Remove
}

/// Every request is answered: reports what changed since the last resync.
fn finish_resync<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    data: &mut D,
) {
    let (events, again) = {
        let mut link = shared.borrow_mut();
        let Some(query) = end_query(&mut link) else {
            return;
        };
        let again = matches!(link.resync, Resync::Running { again: true });
        link.resync = Resync::Idle;
        if !link.connected || query.connection != link.connection {
            // Begun on a connection that is gone: the next one asks again.
            (Vec::new(), again)
        } else {
            match link.protocol.snapshot(&query.replies) {
                Ok(now) => {
                    let events = diff(&link.reported, &now);
                    link.reported = now;
                    link.retry = link.config.retry_min;
                    (events, again)
                }
                Err(reason) => {
                    drop(link);
                    return resync_failed(shared, reason);
                }
            }
        }
    };
    for event in events {
        data.compositor_event(event);
    }
    if again {
        schedule_resync(shared, Duration::ZERO);
    }
}

/// A resync could not be completed: it is tried again after a pause, while
/// the event socket is connected.
fn resync_failed<'l, D: CompositorEvents + 'l, P: Protocol>(
    shared: &Shared<'l, D, P>,
    reason: String,
) {
    let (name, delay) = {
        let mut link = shared.borrow_mut();
        end_query(&mut link);
        link.resync = Resync::Idle;
        if !link.connected {
            return;
        }
        let delay = link.retry;
        link.retry = (link.retry * 2).min(link.config.retry_max);
        (link.protocol.name(), delay)
    };
    tracing::warn!("cannot read {name}'s state: {reason}; trying again in {delay:?}");
    schedule_resync(shared, delay);
}

/// Takes the running query out, and removes the sources it still has in the
/// loop.
fn end_query<D, P>(link: &mut Link<'_, D, P>) -> Option<Query> {
    let mut query = link.query.take()?;
    for token in [query.reader.take(), query.timeout.take()]
        .into_iter()
        .flatten()
    {
        link.handle.remove(token);
    }
    Some(query)
}

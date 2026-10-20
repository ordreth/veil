//! The compositor backends against fake compositors. A fake Hyprland: its two
//! sockets in a temporary directory, an event stream the test writes to, and a
//! request socket that answers `j/monitors` and `j/clients` from what the test
//! sets. A fake sway: one socket, framed messages, a subscription that turns a
//! connection into the event stream, and answers to `get_outputs` and
//! `get_tree` on connections it keeps open, as sway does. The backend runs on a
//! real `calloop` loop on its own thread, as in the daemon.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use calloop::{EventLoop, LoopSignal};
use veil::compositor::hyprland::{self, Config, Sockets};
use veil::compositor::sway;
use veil::compositor::{CompositorEvent, CompositorEvents};

/// How long a test waits for something that must happen.
const PATIENCE: Duration = Duration::from_secs(10);

/// Timings short enough for a test.
const QUICK: Config = Config {
    resync_delay: Duration::from_millis(10),
    request_timeout: Duration::from_millis(300),
    retry_min: Duration::from_millis(50),
    retry_max: Duration::from_millis(200),
};

/// Two monitors, HDMI-A-1 on workspace 1 and DP-3 on workspace `dp_workspace`.
fn monitors(dp_workspace: i64) -> String {
    format!(
        r#"[{{"id":0,"name":"HDMI-A-1","x":0,"y":0,"width":1920,"height":1080,"scale":1,"transform":0,
             "activeWorkspace":{{"id":1,"name":"1"}},"specialWorkspace":{{"id":0,"name":""}},"disabled":false}},
            {{"id":1,"name":"DP-3","x":1920,"y":0,"width":1920,"height":1080,"scale":1,"transform":0,
             "activeWorkspace":{{"id":{dp_workspace},"name":"{dp_workspace}"}},"specialWorkspace":{{"id":0,"name":""}},"disabled":false}}]"#
    )
}

/// A fullscreen window on workspace `workspace` of DP-3.
fn fullscreen_on(workspace: i64) -> String {
    format!(
        r#"[{{"address":"0x1","mapped":true,"hidden":false,"at":[1920,0],"size":[1920,1080],
             "workspace":{{"id":{workspace},"name":"{workspace}"}},"floating":false,"monitor":1,
             "fullscreen":2,"fullscreenClient":2}}]"#
    )
}

/// What the fake Hyprland answers, and what it was asked.
#[derive(Default)]
struct State {
    monitors: String,
    clients: String,
    /// Every request it received, in order.
    requests: Vec<String>,
    /// Requests are accepted and never answered.
    silent: bool,
}

struct FakeHyprland {
    dir: PathBuf,
    state: Arc<Mutex<State>>,
    /// The listener of the event socket, until the backend has connected.
    events: Option<UnixListener>,
    stop: Arc<AtomicBool>,
    requests: Option<JoinHandle<()>>,
}

impl FakeHyprland {
    /// A directory for the sockets; nothing listens yet.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("veil-hy-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the socket directory");
        let state = Arc::new(Mutex::new(State {
            monitors: monitors(2),
            clients: "[]".into(),
            ..State::default()
        }));
        Self {
            dir,
            state,
            events: None,
            stop: Arc::new(AtomicBool::new(false)),
            requests: None,
        }
    }

    fn sockets(&self) -> Sockets {
        hyprland::sockets(&self.dir)
    }

    /// Starts listening on both sockets.
    fn listen(&mut self) {
        let sockets = self.sockets();
        self.events = Some(UnixListener::bind(&sockets.events).expect("bind the event socket"));
        let requests = UnixListener::bind(&sockets.requests).expect("bind the request socket");
        requests
            .set_nonblocking(true)
            .expect("non-blocking request socket");
        let (state, stop) = (Arc::clone(&self.state), Arc::clone(&self.stop));
        self.requests = Some(std::thread::spawn(move || {
            let mut held = Vec::new();
            while !stop.load(Ordering::Relaxed) {
                let Ok((stream, _)) = requests.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                if let Some(stream) = answer(stream, &state) {
                    held.push(stream);
                }
            }
        }));
    }

    /// Waits for the backend to connect to the event socket.
    fn accept_events(&mut self) -> UnixStream {
        let listener = self.events.as_ref().expect("listening");
        listener
            .set_nonblocking(true)
            .expect("non-blocking event socket");
        let start = Instant::now();
        loop {
            if let Ok((stream, _)) = listener.accept() {
                return stream;
            }
            assert!(start.elapsed() < PATIENCE, "the backend did not connect");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn set(&self, monitors: String, clients: String) {
        let mut state = self.state.lock().expect("state");
        state.monitors = monitors;
        state.clients = clients;
    }

    fn requests(&self) -> Vec<String> {
        self.state.lock().expect("state").requests.clone()
    }
}

/// Answers one request from `state`, as Hyprland does: read it, write the
/// answer, close. A silent fake keeps the connection instead.
fn answer(mut stream: UnixStream, state: &Mutex<State>) -> Option<UnixStream> {
    stream
        .set_nonblocking(false)
        .expect("blocking request connection");
    stream
        .set_read_timeout(Some(PATIENCE))
        .expect("read timeout");
    let mut request = [0u8; 256];
    let n = stream.read(&mut request).unwrap_or(0);
    let request = String::from_utf8_lossy(&request[..n]).into_owned();
    let mut state = state.lock().expect("state");
    state.requests.push(request.clone());
    if state.silent {
        return Some(stream);
    }
    let reply = match request.as_str() {
        "j/monitors" => state.monitors.clone(),
        "j/clients" => state.clients.clone(),
        _ => "unknown request".into(),
    };
    let _ = stream.write_all(reply.as_bytes());
    None
}

impl Drop for FakeHyprland {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.requests.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Passes every event on to the test.
struct Recorder(mpsc::Sender<CompositorEvent>);

impl CompositorEvents for Recorder {
    fn compositor_event(&mut self, event: CompositorEvent) {
        let _ = self.0.send(event);
    }
}

/// The backend on an event loop of its own, until the value is dropped.
struct Backend {
    events: mpsc::Receiver<CompositorEvent>,
    stop: Arc<AtomicBool>,
    wake: LoopSignal,
    thread: Option<JoinHandle<()>>,
    /// How often the loop woke up: a backend that spins shows here.
    wakeups: Arc<AtomicUsize>,
}

/// Which compositor a [`Backend`] follows.
enum Follow {
    Hyprland(Sockets),
    Sway(PathBuf),
}

impl Backend {
    fn start(sockets: Sockets) -> Self {
        Self::follow(Follow::Hyprland(sockets))
    }

    fn follow(compositor: Follow) -> Self {
        let (sender, events) = mpsc::channel();
        let (ready, signal) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let wakeups = Arc::new(AtomicUsize::new(0));
        let (stopped, woken) = (Arc::clone(&stop), Arc::clone(&wakeups));
        let thread = std::thread::spawn(move || {
            let mut event_loop: EventLoop<Recorder> = EventLoop::try_new().expect("event loop");
            match compositor {
                Follow::Hyprland(sockets) => hyprland::insert(&event_loop.handle(), sockets, QUICK),
                Follow::Sway(socket) => sway::insert(&event_loop.handle(), socket, QUICK),
            }
            let _ = ready.send(event_loop.get_signal());
            let mut recorder = Recorder(sender);
            while !stopped.load(Ordering::Relaxed) {
                woken.fetch_add(1, Ordering::Relaxed);
                event_loop
                    .dispatch(Some(Duration::from_millis(500)), &mut recorder)
                    .expect("dispatch");
            }
        });
        let wake = signal.recv_timeout(PATIENCE).expect("the loop starts");
        Self {
            events,
            stop,
            wake,
            thread: Some(thread),
            wakeups,
        }
    }

    fn next(&self) -> CompositorEvent {
        self.events
            .recv_timeout(PATIENCE)
            .expect("the backend reports an event")
    }

    /// The next `n` events.
    fn take(&self, n: usize) -> Vec<CompositorEvent> {
        (0..n).map(|_| self.next()).collect()
    }

    /// Asserts that nothing is reported for a while.
    fn quiet(&self) {
        if let Ok(event) = self.events.recv_timeout(Duration::from_millis(300)) {
            panic!("unexpected event {event:?}");
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.wake.wakeup();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn workspace(output: &str, workspace: &str) -> CompositorEvent {
    CompositorEvent::WorkspaceChanged {
        output: output.into(),
        workspace: workspace.into(),
    }
}

fn fullscreen(output: &str, fullscreen: bool) -> CompositorEvent {
    CompositorEvent::FullscreenChanged {
        output: output.into(),
        fullscreen,
    }
}

fn covered(output: &str, covered: bool) -> CompositorEvent {
    CompositorEvent::CoverageChanged {
        output: output.into(),
        covered,
    }
}

/// What a backend reports first, for the fake's starting state.
fn initial() -> Vec<CompositorEvent> {
    vec![
        workspace("DP-3", "2"),
        fullscreen("DP-3", false),
        covered("DP-3", false),
        workspace("HDMI-A-1", "1"),
        fullscreen("HDMI-A-1", false),
        covered("HDMI-A-1", false),
    ]
}

#[test]
fn the_state_of_every_output_is_reported_when_the_backend_connects() {
    let mut hyprland = FakeHyprland::new("initial");
    hyprland.listen();
    let backend = Backend::start(hyprland.sockets());
    let _events = hyprland.accept_events();

    assert_eq!(backend.take(6), initial());
    backend.quiet();
    assert_eq!(hyprland.requests(), ["j/monitors", "j/clients"]);
}

#[test]
fn switching_workspace_to_a_fullscreen_window_is_reported_for_its_output() {
    let mut hyprland = FakeHyprland::new("switch");
    hyprland.listen();
    let backend = Backend::start(hyprland.sockets());
    let mut events = hyprland.accept_events();
    backend.take(6);

    // What Hyprland sends when DP-3 switches to workspace 5, where a window
    // is fullscreen.
    hyprland.set(monitors(5), fullscreen_on(5));
    events
        .write_all(
            b"workspace>>5\nworkspacev2>>5,5\nfocusedmon>>DP-3,5\nfocusedmonv2>>DP-3,5\n\
              activewindow>>kitty,~\nactivewindowv2>>1\n",
        )
        .expect("send events");
    assert_eq!(
        backend.take(3),
        [
            workspace("DP-3", "5"),
            fullscreen("DP-3", true),
            covered("DP-3", true)
        ]
    );
    backend.quiet();
    // The burst of six lines was taken together: at most two resyncs.
    let requests = hyprland.requests().len();
    assert!(
        (4..=6).contains(&requests),
        "{requests} requests: {:?}",
        hyprland.requests()
    );

    // Leaving the fullscreen window.
    hyprland.set(monitors(5), "[]".into());
    events.write_all(b"fullscreen>>0\n").expect("send events");
    assert_eq!(
        backend.take(2),
        [fullscreen("DP-3", false), covered("DP-3", false)]
    );
}

#[test]
fn events_that_change_nothing_on_an_output_ask_for_nothing() {
    let mut hyprland = FakeHyprland::new("quiet");
    hyprland.listen();
    let backend = Backend::start(hyprland.sockets());
    let mut events = hyprland.accept_events();
    backend.take(6);

    events
        .write_all(b"activewindow>>kitty,~\nwindowtitlev2>>1,vim\nsubmap>>resize\nnot an event\n")
        .expect("send events");
    backend.quiet();
    assert_eq!(hyprland.requests().len(), 2);

    // An event line split across two writes is read as one.
    hyprland.set(monitors(7), "[]".into());
    events.write_all(b"works").expect("send part of an event");
    std::thread::sleep(Duration::from_millis(50));
    events.write_all(b"pace>>7\n").expect("send the rest");
    assert_eq!(backend.next(), workspace("DP-3", "7"));
    // The loop slept meanwhile instead of spinning.
    assert!(backend.wakeups.load(Ordering::Relaxed) < 200);
}

#[test]
fn a_closed_event_socket_is_reported_and_connected_again() {
    let mut hyprland = FakeHyprland::new("reconnect");
    hyprland.listen();
    let backend = Backend::start(hyprland.sockets());
    let events = hyprland.accept_events();
    backend.take(6);

    drop(events);
    assert!(matches!(
        backend.next(),
        CompositorEvent::Disconnected { .. }
    ));
    // It connects again by itself, and reports everything anew.
    let _events = hyprland.accept_events();
    assert_eq!(backend.take(6), initial());
}

#[test]
fn a_compositor_that_is_not_there_yet_is_waited_for() {
    let mut hyprland = FakeHyprland::new("late");
    let backend = Backend::start(hyprland.sockets());
    // Nothing to connect to: no event, and no error that ends the loop.
    backend.quiet();

    hyprland.listen();
    let _events = hyprland.accept_events();
    assert_eq!(backend.take(6), initial());
}

#[test]
fn a_request_that_is_not_answered_is_given_up_and_asked_again() {
    let mut hyprland = FakeHyprland::new("silent");
    hyprland.state.lock().expect("state").silent = true;
    hyprland.listen();
    let backend = Backend::start(hyprland.sockets());
    let _events = hyprland.accept_events();
    // The first request hangs; nothing is reported.
    backend.quiet();

    hyprland.state.lock().expect("state").silent = false;
    assert_eq!(backend.take(6), initial());
    let requests = hyprland.requests();
    assert!(requests.len() >= 3, "{requests:?}");
}

#[test]
fn an_answer_that_is_not_json_is_given_up_and_asked_again() {
    let mut hyprland = FakeHyprland::new("garbage");
    hyprland.set("unknown request".into(), "[]".into());
    hyprland.listen();
    let backend = Backend::start(hyprland.sockets());
    let _events = hyprland.accept_events();
    backend.quiet();

    hyprland.set(monitors(2), "[]".into());
    assert_eq!(backend.take(6), initial());
}

/// Two outputs as sway reports them: DP-1 on workspace `dp_workspace`, and
/// HDMI-A-1 to its right on workspace 9.
fn sway_outputs(dp_workspace: &str) -> String {
    format!(
        r#"[{{"id":3,"type":"output","name":"DP-1","active":true,"power":true,"scale":1.0,
             "current_workspace":"{dp_workspace}","rect":{{"x":0,"y":0,"width":1920,"height":1080}}}},
            {{"id":4,"type":"output","name":"HDMI-A-1","active":true,"power":true,"scale":1.0,
             "current_workspace":"9","rect":{{"x":1920,"y":0,"width":1280,"height":720}}}}]"#
    )
}

/// sway's tree with `windows` on the shown workspace of DP-1.
fn sway_tree(windows: &str) -> String {
    format!(
        r#"{{"id":1,"type":"root","name":"root","rect":{{"x":0,"y":0,"width":3200,"height":1080}},
            "nodes":[{{"id":3,"type":"output","name":"DP-1","rect":{{"x":0,"y":0,"width":1920,"height":1080}},
              "nodes":[{{"id":5,"type":"workspace","name":"1","fullscreen_mode":1,
                "rect":{{"x":0,"y":0,"width":1920,"height":1080}},"nodes":[{windows}],"floating_nodes":[]}}]}}],
            "floating_nodes":[]}}"#
    )
}

/// A fullscreen window on DP-1.
const SWAY_FULLSCREEN: &str = r#"{"id":7,"type":"con","name":"mpv","app_id":"mpv","visible":true,
    "fullscreen_mode":1,"rect":{"x":0,"y":0,"width":1920,"height":1080},"nodes":[],"floating_nodes":[]}"#;

/// What the fake sway answers, and what it was asked.
#[derive(Default)]
struct SwayState {
    outputs: String,
    tree: String,
    /// The types of the requests it received, in order (not the subscription).
    requests: Vec<u32>,
    /// Whether it refuses the next subscription.
    refuse: bool,
}

struct FakeSway {
    dir: PathBuf,
    state: Arc<Mutex<SwayState>>,
    /// Event connections, as the backend subscribes.
    subscribed: mpsc::Receiver<UnixStream>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeSway {
    fn start(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("veil-sw-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the socket directory");
        let listener = UnixListener::bind(dir.join("sway.sock")).expect("bind sway's socket");
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let state = Arc::new(Mutex::new(SwayState {
            outputs: sway_outputs("1"),
            tree: sway_tree(""),
            ..SwayState::default()
        }));
        let (subscriber, subscribed) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, stopped) = (Arc::clone(&state), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            // sway keeps request connections open after its answer.
            let mut held = Vec::new();
            while !stopped.load(Ordering::Relaxed) {
                let Ok((stream, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                };
                if let Some(stream) = serve_sway(stream, &shared, &subscriber) {
                    held.push(stream);
                }
            }
        });
        Self {
            dir,
            state,
            subscribed,
            stop,
            thread: Some(thread),
        }
    }

    fn socket(&self) -> PathBuf {
        self.dir.join("sway.sock")
    }

    /// Waits for the backend to subscribe, and returns its event connection.
    fn events(&self) -> UnixStream {
        self.subscribed
            .recv_timeout(PATIENCE)
            .expect("the backend subscribes")
    }

    fn set(&self, outputs: String, tree: String) {
        let mut state = self.state.lock().expect("state");
        state.outputs = outputs;
        state.tree = tree;
    }

    fn requests(&self) -> Vec<u32> {
        self.state.lock().expect("state").requests.clone()
    }
}

impl Drop for FakeSway {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Reads one message from `stream`.
fn read_sway_message(stream: &mut UnixStream) -> Option<(u32, Vec<u8>)> {
    let mut header = [0u8; 14];
    stream.read_exact(&mut header).ok()?;
    assert_eq!(&header[..6], b"i3-ipc", "the backend sends sway's magic");
    let length = u32::from_ne_bytes(header[6..10].try_into().expect("4 bytes"));
    let kind = u32::from_ne_bytes(header[10..14].try_into().expect("4 bytes"));
    let mut payload = vec![0u8; usize::try_from(length).expect("a size")];
    stream.read_exact(&mut payload).ok()?;
    Some((kind, payload))
}

/// Serves one connection as sway does. Returns it if it is to be kept open.
fn serve_sway(
    mut stream: UnixStream,
    state: &Mutex<SwayState>,
    subscriber: &mpsc::Sender<UnixStream>,
) -> Option<UnixStream> {
    stream.set_nonblocking(false).expect("blocking connection");
    stream
        .set_read_timeout(Some(PATIENCE))
        .expect("read timeout");
    let (kind, payload) = read_sway_message(&mut stream)?;
    let mut state = state.lock().expect("state");
    let reply = match kind {
        sway::SUBSCRIBE => {
            let wanted: Vec<String> = serde_json::from_slice(&payload).expect("a list of events");
            assert!(wanted.iter().any(|e| e == "window"), "{wanted:?}");
            let success = !std::mem::take(&mut state.refuse);
            let reply = format!(r#"{{"success":{success}}}"#);
            let _ = stream.write_all(&sway::encode(sway::SUBSCRIBE, reply.as_bytes()));
            let _ = subscriber.send(stream);
            return None;
        }
        sway::GET_OUTPUTS => state.outputs.clone(),
        sway::GET_TREE => state.tree.clone(),
        _ => r#"{"success":false}"#.into(),
    };
    state.requests.push(kind);
    let _ = stream.write_all(&sway::encode(kind, reply.as_bytes()));
    Some(stream)
}

/// Sends an event of `kind` with `payload` on the event connection.
fn sway_event(events: &mut UnixStream, kind: u32, payload: &str) {
    events
        .write_all(&sway::encode(kind, payload.as_bytes()))
        .expect("send an event");
}

/// What the sway backend reports first, for the fake's starting state.
fn sway_initial() -> Vec<CompositorEvent> {
    vec![
        workspace("DP-1", "1"),
        fullscreen("DP-1", false),
        covered("DP-1", false),
        workspace("HDMI-A-1", "9"),
        fullscreen("HDMI-A-1", false),
        covered("HDMI-A-1", false),
    ]
}

#[test]
fn sway_the_state_of_every_output_is_reported_when_the_backend_connects() {
    let sway = FakeSway::start("initial");
    let backend = Backend::follow(Follow::Sway(sway.socket()));
    let _events = sway.events();
    assert_eq!(backend.take(6), sway_initial());
    backend.quiet();
    // Answers on connections sway keeps open were read to their length.
    assert_eq!(sway.requests(), [sway::GET_OUTPUTS, sway::GET_TREE]);
}

#[test]
fn sway_a_fullscreen_window_is_reported_for_its_output() {
    let sway = FakeSway::start("fullscreen");
    let backend = Backend::follow(Follow::Sway(sway.socket()));
    let mut events = sway.events();
    backend.take(6);

    sway.set(sway_outputs("1"), sway_tree(SWAY_FULLSCREEN));
    sway_event(
        &mut events,
        sway::EVENT_WINDOW,
        r#"{"change":"fullscreen_mode","container":{"id":7,"app_id":"mpv"}}"#,
    );
    assert_eq!(
        backend.take(2),
        [fullscreen("DP-1", true), covered("DP-1", true)]
    );
    backend.quiet();

    // A workspace switch leaves it behind.
    sway.set(sway_outputs("2"), sway_tree(""));
    sway_event(
        &mut events,
        sway::EVENT_WORKSPACE,
        r#"{"change":"focus","current":{"name":"2"},"old":{"name":"1"}}"#,
    );
    assert_eq!(
        backend.take(3),
        [
            workspace("DP-1", "2"),
            fullscreen("DP-1", false),
            covered("DP-1", false)
        ]
    );
}

#[test]
fn sway_events_that_change_nothing_ask_for_nothing() {
    let sway = FakeSway::start("quiet");
    let backend = Backend::follow(Follow::Sway(sway.socket()));
    let mut events = sway.events();
    backend.take(6);

    sway_event(
        &mut events,
        sway::EVENT_WINDOW,
        r#"{"change":"title","container":{}}"#,
    );
    sway_event(
        &mut events,
        sway::EVENT_WORKSPACE,
        r#"{"change":"urgent","current":{}}"#,
    );
    backend.quiet();
    assert_eq!(sway.requests().len(), 2);

    // An event split across two writes is read as one.
    sway.set(sway_outputs("3"), sway_tree(""));
    let message = sway::encode(sway::EVENT_WORKSPACE, br#"{"change":"focus"}"#);
    events
        .write_all(&message[..10])
        .expect("send part of an event");
    std::thread::sleep(Duration::from_millis(50));
    events.write_all(&message[10..]).expect("send the rest");
    assert_eq!(backend.next(), workspace("DP-1", "3"));
    assert!(backend.wakeups.load(Ordering::Relaxed) < 200);
}

#[test]
fn sway_shutting_down_is_a_lost_connection_and_is_connected_again() {
    let sway = FakeSway::start("shutdown");
    let backend = Backend::follow(Follow::Sway(sway.socket()));
    let mut events = sway.events();
    backend.take(6);

    sway_event(&mut events, sway::EVENT_SHUTDOWN, r#"{"change":"exit"}"#);
    assert!(matches!(
        backend.next(),
        CompositorEvent::Disconnected { .. }
    ));
    let _events = sway.events();
    assert_eq!(backend.take(6), sway_initial());
}

#[test]
fn sway_a_refused_subscription_is_tried_again() {
    let sway = FakeSway::start("refused");
    sway.state.lock().expect("state").refuse = true;
    let backend = Backend::follow(Follow::Sway(sway.socket()));
    let _refused = sway.events();
    // Connected, the state asked for, and then the refusal read: what was
    // reported, if anything, is taken back.
    let mut seen = Vec::new();
    loop {
        let event = backend.next();
        if matches!(event, CompositorEvent::Disconnected { .. }) {
            break;
        }
        seen.push(event);
        assert!(seen.len() <= 6, "{seen:?}");
    }
    let _events = sway.events();
    assert_eq!(backend.take(6), sway_initial());
}

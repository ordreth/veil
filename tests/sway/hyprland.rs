//! A fake Hyprland IPC next to the headless sway: the two sockets of an
//! instance in `$XDG_RUNTIME_DIR/hypr/<signature>/`, answering `j/monitors`
//! and `j/clients` from what the test sets, and passing on the events the test
//! sends. With `HYPRLAND_INSTANCE_SIGNATURE` set to [`SIGNATURE`], the daemon
//! follows it instead of sway (`compositor::detect` asks for Hyprland first),
//! while it still draws on the sway.

use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

/// The instance signature of the fake.
pub const SIGNATURE: &str = "FAKE";

#[derive(Default)]
struct Shared {
    monitors: String,
    clients: String,
    /// The daemon's event connections.
    listeners: Vec<UnixStream>,
}

pub struct FakeHyprland {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl FakeHyprland {
    /// Listens in `runtime_dir` (the sway's) and answers with `monitors` and
    /// no clients.
    pub fn start(runtime_dir: &Path, monitors: String) -> Self {
        let dir = runtime_dir.join("hypr").join(SIGNATURE);
        std::fs::create_dir_all(&dir).expect("create the Hyprland instance directory");
        let requests = UnixListener::bind(dir.join(".socket.sock")).expect("bind .socket.sock");
        let events = UnixListener::bind(dir.join(".socket2.sock")).expect("bind .socket2.sock");
        for listener in [&requests, &events] {
            listener
                .set_nonblocking(true)
                .expect("non-blocking listener");
        }
        let shared = Arc::new(Mutex::new(Shared {
            monitors,
            clients: "[]".into(),
            listeners: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));

        let answering = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match requests.accept() {
                        Ok((stream, _)) => answer(stream, &shared),
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        let accepting = {
            let (shared, stop) = (Arc::clone(&shared), Arc::clone(&stop));
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match events.accept() {
                        Ok((stream, _)) => lock(&shared).listeners.push(stream),
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        Self {
            shared,
            stop,
            threads: vec![answering, accepting],
        }
    }

    /// What `j/clients` answers from now on.
    pub fn set_clients(&self, clients: String) {
        lock(&self.shared).clients = clients;
    }

    /// Sends an event line (`fullscreen>>1`) to every connected daemon.
    pub fn event(&self, line: &str) {
        let mut shared = lock(&self.shared);
        shared
            .listeners
            .retain_mut(|stream| stream.write_all(format!("{line}\n").as_bytes()).is_ok());
    }

    /// Whether a daemon follows the event socket.
    pub fn followed(&self) -> bool {
        !lock(&self.shared).listeners.is_empty()
    }

    /// Gives `command` the environment of a session of this Hyprland.
    pub fn env(command: &mut Command) {
        command.env("HYPRLAND_INSTANCE_SIGNATURE", SIGNATURE);
    }
}

impl Drop for FakeHyprland {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn lock(shared: &Mutex<Shared>) -> std::sync::MutexGuard<'_, Shared> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Answers one request and closes the connection, as Hyprland does.
fn answer(mut stream: UnixStream, shared: &Mutex<Shared>) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let mut request = [0u8; 256];
    let n = stream.read(&mut request).unwrap_or(0);
    let reply = match &request[..n] {
        b"j/monitors" => lock(shared).monitors.clone(),
        b"j/clients" => lock(shared).clients.clone(),
        _ => "unknown request".into(),
    };
    let _ = stream.write_all(reply.as_bytes());
}

/// One monitor as Hyprland describes it, on workspace 1.
pub fn monitor(name: &str, x: u32, width: u32, height: u32) -> String {
    format!(
        r#"{{"id":0,"name":"{name}","x":{x},"y":0,"width":{width},"height":{height},"scale":1,
            "transform":0,"activeWorkspace":{{"id":1,"name":"1"}},
            "specialWorkspace":{{"id":0,"name":""}},"disabled":false}}"#
    )
}

/// A fullscreen window on workspace 1 at `x`.
pub fn fullscreen_window(x: u32, width: u32, height: u32) -> String {
    format!(
        r#"{{"address":"0x1","mapped":true,"hidden":false,"at":[{x},0],"size":[{width},{height}],
            "workspace":{{"id":1,"name":"1"}},"floating":false,"monitor":0,
            "fullscreen":2,"fullscreenClient":2}}"#
    )
}

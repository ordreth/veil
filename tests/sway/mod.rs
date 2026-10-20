//! A headless sway for `tests/headless.rs`: one compositor per test, in a
//! runtime directory of its own, the real `veil` binary started in it, and
//! screenshots taken with `grim`.
//!
//! The tools come from `nix develop .#headless`: `sway`, `swaymsg`, `grim`,
//! `foot`, `swaylock` and `gst-launch-1.0` (the media). A tool that is missing
//! fails the test: these tests are `#[ignore]`d and run only when asked for.
//!
//! Nothing here touches the session the tests are started from: the
//! compositor, the daemon and the clients get their own `XDG_RUNTIME_DIR`,
//! `XDG_STATE_HOME` and `XDG_CONFIG_HOME`, a session bus that leads nowhere
//! (no desktop notifications), and neither `WAYLAND_DISPLAY` nor
//! `HYPRLAND_INSTANCE_SIGNATURE` of the session.

// The test binary uses all of it, but not every helper in every test.
#![allow(dead_code)]

pub mod hyprland;

use image::RgbImage;
use rustix::process::{Pid, Signal};
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};
use veil::ipc::protocol::Response;
use veil::ipc::report::{OutputStatus, State, Status};

/// How long a test waits for the compositor or the daemon before it fails.
pub const PATIENCE: Duration = Duration::from_secs(20);

/// How often a condition is looked at while a test waits for it.
const POLL: Duration = Duration::from_millis(100);

/// One compositor at a time. They are independent, but each renders on the
/// CPU (pixman), and the timings the tests rely on (the pause after a second,
/// the release after two) should not depend on how many run beside them.
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// A colour as `grim` sees it.
pub type Rgb = [u8; 3];

pub const BLACK: Rgb = [0, 0, 0];
pub const RED: Rgb = [255, 0, 0];
pub const GREEN: Rgb = [0, 255, 0];
pub const BLUE: Rgb = [0, 0, 255];
pub const YELLOW: Rgb = [255, 255, 0];
pub const MAGENTA: Rgb = [255, 0, 255];
pub const CYAN: Rgb = [0, 255, 255];

/// A colour as `--bg-color` takes it.
pub fn hex(colour: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", colour[0], colour[1], colour[2])
}

/// A headless sway with its outputs, killed and cleaned up when dropped.
pub struct Sway {
    dir: PathBuf,
    process: Child,
    display: String,
    swaysock: PathBuf,
    /// Dropped last, after the compositor is gone.
    _turn: MutexGuard<'static, ()>,
}

impl Sway {
    /// Starts a sway whose outputs `HEADLESS-1`, `HEADLESS-2`, ... have the
    /// given sizes, placed side by side from the left. `name` must be unique
    /// among the tests (it is part of the runtime directory).
    pub fn start(name: &str, outputs: &[(u32, u32)]) -> Self {
        assert!(!outputs.is_empty(), "a sway without outputs");
        let turn = ONE_AT_A_TIME.lock().unwrap_or_else(PoisonError::into_inner);
        let dir = std::env::temp_dir().join(format!("veil-hl-{}-{name}", std::process::id()));
        // A Unix socket path has at most 108 bytes; sway's is the longest.
        assert!(
            dir.as_os_str().len() < 60,
            "runtime directory {} is too long for the sockets in it; set TMPDIR to a shorter path",
            dir.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .expect("create the runtime directory");

        let mut config = String::from("xwayland disable\n");
        let mut x = 0;
        for (index, (width, height)) in outputs.iter().enumerate() {
            config += &format!(
                "output HEADLESS-{} resolution {width}x{height} position {x} 0\n",
                index + 1
            );
            x += width;
        }
        std::fs::write(dir.join("sway.conf"), config).expect("write the sway config");

        let log = std::fs::File::create(dir.join("sway.log")).expect("create the sway log");
        let mut command = Command::new("sway");
        command
            .arg("-c")
            .arg(dir.join("sway.conf"))
            .env("XDG_RUNTIME_DIR", &dir)
            .env("WLR_BACKENDS", "headless")
            .env("WLR_HEADLESS_OUTPUTS", outputs.len().to_string())
            .env("WLR_RENDERER", "pixman")
            .env("WLR_LIBINPUT_NO_DEVICES", "1")
            .env("LIBSEAT_BACKEND", "noop")
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("clone the log file"))
            .stderr(log);
        isolate(&mut command, &dir);
        let process = command
            .spawn()
            .expect("start sway (run the tests in `nix develop .#headless`)");

        let mut sway = Self {
            dir,
            process,
            display: String::new(),
            swaysock: PathBuf::new(),
            _turn: turn,
        };
        sway.connect(outputs.len());
        sway
    }

    /// Waits for both sockets of the compositor and for its outputs.
    fn connect(&mut self, outputs: usize) {
        let (display, swaysock) = wait_until("sway to listen", || {
            if let Some(status) = self.process.try_wait().expect("look at sway") {
                panic!("sway exited with {status}: {}", self.sway_log());
            }
            let mut display = None;
            let mut swaysock = None;
            for entry in std::fs::read_dir(&self.dir).expect("read the runtime directory") {
                let name = entry.expect("directory entry").file_name();
                let name = name.to_string_lossy().into_owned();
                if name.starts_with("wayland-") && !name.ends_with(".lock") {
                    display = Some(name);
                } else if name.starts_with("sway-ipc.") && name.ends_with(".sock") {
                    swaysock = Some(self.dir.join(name));
                }
            }
            display.zip(swaysock)
        });
        self.display = display;
        self.swaysock = swaysock;
        wait_until("sway's outputs", || {
            (self.output_names().len() == outputs).then_some(())
        });
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where the daemon keeps its saved wallpapers.
    pub fn state_file(&self) -> PathBuf {
        self.dir.join("state/veil/state.json")
    }

    /// The configuration file a daemon started here reads.
    pub fn config_file(&self) -> PathBuf {
        self.dir.join("config/veil/config.toml")
    }

    /// The control socket of a daemon started here.
    pub fn socket(&self) -> PathBuf {
        self.dir.join(format!("veil-{}.sock", self.display))
    }

    fn sway_log(&self) -> String {
        std::fs::read_to_string(self.dir.join("sway.log")).unwrap_or_default()
    }

    /// Gives `command` this compositor's environment: its display, its IPC
    /// socket, and directories of this test.
    pub fn env(&self, command: &mut Command) {
        isolate(command, &self.dir);
        command
            .env("WAYLAND_DISPLAY", &self.display)
            .env("SWAYSOCK", &self.swaysock);
    }

    /// `swaymsg` with `args`; panics if sway refuses it.
    pub fn msg(&self, args: &[&str]) -> Value {
        let mut command = Command::new("swaymsg");
        command.arg("-r").args(args);
        self.env(&mut command);
        let output = command.output().expect("run swaymsg");
        assert!(
            output.status.success(),
            "swaymsg {args:?}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap_or(Value::Null)
    }

    /// The names of the outputs sway has, enabled or not.
    pub fn output_names(&self) -> Vec<String> {
        let mut command = Command::new("swaymsg");
        command.args(["-r", "-t", "get_outputs"]);
        self.env(&mut command);
        let Ok(output) = command.output() else {
            return Vec::new();
        };
        let outputs: Value = serde_json::from_slice(&output.stdout).unwrap_or(Value::Null);
        outputs
            .as_array()
            .map(|outputs| {
                outputs
                    .iter()
                    .filter_map(|output| output["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Adds an output of `width` x `height` at `x` (logical pixels from the
    /// left) and returns its name.
    pub fn create_output(&self, width: u32, height: u32, x: u32) -> String {
        let before = self.output_names();
        self.msg(&["create_output"]);
        let name = wait_until("the new output", || {
            self.output_names()
                .into_iter()
                .find(|name| !before.contains(name))
        });
        self.msg(&[
            "output",
            &name,
            "resolution",
            &format!("{width}x{height}"),
            "position",
            &x.to_string(),
            "0",
        ]);
        name
    }

    /// A screenshot of one output.
    pub fn shot(&self, output: &str) -> RgbImage {
        let mut command = Command::new("grim");
        command.args(["-t", "ppm", "-o", output, "-"]);
        self.env(&mut command);
        let shot = command.output().expect("run grim");
        assert!(
            shot.status.success(),
            "grim -o {output}: {}",
            String::from_utf8_lossy(&shot.stderr)
        );
        image::load_from_memory(&shot.stdout)
            .expect("grim's screenshot")
            .to_rgb8()
    }

    /// The colour at the centre of an output.
    pub fn centre(&self, output: &str) -> Rgb {
        let shot = self.shot(output);
        let (width, height) = shot.dimensions();
        shot.get_pixel(width / 2, height / 2).0
    }

    /// The colour of an output's top left corner (a few pixels in).
    pub fn corner(&self, output: &str) -> Rgb {
        self.shot(output).get_pixel(4, 4).0
    }

    /// Waits until the centre of `output` has `colour`.
    pub fn wait_for_centre(&self, output: &str, colour: Rgb) {
        wait_for(
            &format!("{output} to show {colour:?} in its centre"),
            || {
                let centre = self.centre(output);
                if centre == colour {
                    Ok(())
                } else {
                    Err(format!("it shows {centre:?}"))
                }
            },
        );
    }

    /// Waits until `output` shows exactly `picture`.
    pub fn wait_for_picture(&self, output: &str, picture: &RgbImage) {
        wait_for(&format!("{output} to show the expected picture"), || {
            let shot = self.shot(output);
            if &shot == picture {
                Ok(())
            } else {
                Err(differences(&shot, picture))
            }
        });
    }

    /// Panics unless `output` shows exactly `picture`.
    pub fn assert_picture(&self, output: &str, picture: &RgbImage) {
        let shot = self.shot(output);
        assert!(
            &shot == picture,
            "{output} changed: {}",
            differences(&shot, picture)
        );
    }

    /// How many different pictures `count` screenshots of `output`, `apart`
    /// from each other, show.
    pub fn distinct_shots(&self, output: &str, count: usize, apart: Duration) -> usize {
        let mut shots: Vec<Vec<u8>> = Vec::new();
        for index in 0..count {
            if index > 0 {
                std::thread::sleep(apart);
            }
            let shot = self.shot(output).into_raw();
            if !shots.contains(&shot) {
                shots.push(shot);
            }
        }
        shots.len()
    }

    /// Whether the picture on `output` changes within about two seconds.
    pub fn moves(&self, output: &str) -> bool {
        self.distinct_shots(output, 8, Duration::from_millis(250)) > 1
    }

    /// A command of the `veil` binary in this compositor.
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_veil"));
        command.args(args);
        self.env(&mut command);
        command
    }

    /// Runs `veil` with `args` and returns what it did.
    pub fn veil(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run veil")
    }

    /// Runs `veil` with `args` and panics unless it succeeds.
    pub fn veil_ok(&self, args: &[&str]) -> String {
        let output = self.veil(args);
        assert!(
            output.status.success(),
            "veil {args:?} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// Starts `veil daemon` with `args` and waits until it answers.
    pub fn daemon(&self, args: &[&str]) -> Daemon {
        self.daemon_with(args, |_| {})
    }

    /// Starts `veil daemon` with `args`, its command changed by `adjust` first
    /// (an environment variable, another program in front of it), and waits
    /// until it answers.
    pub fn daemon_with(&self, args: &[&str], adjust: impl FnOnce(&mut Command)) -> Daemon {
        let log_path = self.dir.join(format!(
            "daemon-{}.log",
            std::fs::read_dir(&self.dir)
                .map(|entries| entries.count())
                .unwrap_or(0)
        ));
        let log = std::fs::File::create(&log_path).expect("create the daemon log");
        // Without a core file: one test expects a crash, and any other would
        // fail its test anyway. `exec` keeps the process id.
        let mut command = Command::new("sh");
        command
            .args(["-c", "ulimit -c 0 && exec \"$0\" \"$@\""])
            .arg(env!("CARGO_BIN_EXE_veil"))
            .arg("daemon")
            .args(args)
            .env("RUST_LOG", "veil=debug")
            .stdin(Stdio::null())
            .stdout(log.try_clone().expect("clone the log file"))
            .stderr(log);
        self.env(&mut command);
        adjust(&mut command);
        let process = command.spawn().expect("start veil daemon");
        let mut daemon = Daemon {
            process,
            log: log_path,
        };
        wait_for("the daemon to answer", || {
            if let Some(status) = daemon.try_wait() {
                panic!("the daemon exited with {status}:\n{}", daemon.log());
            }
            if self.veil(&["ping"]).status.success() {
                Ok(())
            } else {
                Err(daemon.log())
            }
        });
        daemon
    }

    /// Sends one raw request line to the daemon and returns its answer.
    pub fn request(&self, line: &[u8]) -> Response {
        let mut stream = self.connect_to_daemon();
        stream.write_all(line).expect("send the request");
        read_response(&mut stream)
    }

    /// A connection to the daemon's control socket, with a read timeout so a
    /// daemon that does not answer fails the test.
    pub fn connect_to_daemon(&self) -> UnixStream {
        let stream = UnixStream::connect(self.socket()).expect("connect to the daemon");
        stream
            .set_read_timeout(Some(PATIENCE))
            .expect("read timeout");
        stream
    }

    /// What the daemon says about itself and its outputs.
    pub fn status(&self) -> Status {
        let output = self.veil(&["status", "--json"]);
        assert!(
            output.status.success(),
            "veil status: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("the JSON of veil status")
    }

    /// The status of one output.
    pub fn output_status(&self, name: &str) -> OutputStatus {
        output_of(&self.status(), name)
    }

    /// Waits until `output` is in `state`, and returns its status then.
    pub fn wait_for_state(&self, output: &str, state: State) -> OutputStatus {
        wait_for(&format!("{output} to be {state:?}"), || {
            let status = self.output_status(output);
            if status.state == state {
                Ok(status)
            } else {
                Err(format!("last status: {status:?}"))
            }
        })
    }

    /// Starts a client of this compositor (`foot`, `swaylock`).
    pub fn client(&self, program: &str, args: &[&str]) -> Client {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.env(&mut command);
        Client(
            command
                .spawn()
                .unwrap_or_else(|error| panic!("start {program}: {error}")),
        )
    }

    /// A terminal window with the app id `app_id`, on the focused output.
    /// It runs `sleep`, so nothing of the user's shell is involved.
    pub fn window(&self, app_id: &str) -> Client {
        let client = self.client("foot", &["--app-id", app_id, "sleep", "600"]);
        wait_until(&format!("the window {app_id}"), || {
            let tree = self.msg(&["-t", "get_tree"]);
            tree.to_string()
                .contains(&format!("\"app_id\":\"{app_id}\""))
                .then_some(())
        });
        client
    }
}

impl Drop for Sway {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        if std::thread::panicking() {
            eprintln!(
                "--- sway log ---\n{}",
                self.sway_log()
                    .lines()
                    .rev()
                    .take(30)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// How two pictures differ, short enough for a test failure.
fn differences(found: &RgbImage, expected: &RgbImage) -> String {
    if found.dimensions() != expected.dimensions() {
        return format!(
            "{:?} instead of {:?} pixels",
            found.dimensions(),
            expected.dimensions()
        );
    }
    let mut differing = found
        .enumerate_pixels()
        .filter(|(x, y, pixel)| expected.get_pixel(*x, *y) != *pixel);
    let count = differing.clone().count();
    match differing.next() {
        Some((x, y, pixel)) => format!(
            "{count} pixels differ, the first at {x},{y}: {:?} instead of {:?}",
            pixel.0,
            expected.get_pixel(x, y).0
        ),
        None => "no difference".into(),
    }
}

/// Reads one response line from the daemon.
pub fn read_response(stream: &mut UnixStream) -> Response {
    let mut line = String::new();
    BufReader::new(stream)
        .read_line(&mut line)
        .expect("read the answer");
    serde_json::from_str(&line).unwrap_or_else(|error| panic!("an answer: {error}: {line:?}"))
}

/// The status of the output `name` in `status`.
pub fn output_of(status: &Status, name: &str) -> OutputStatus {
    status
        .outputs
        .iter()
        .find(|output| output.name.as_deref() == Some(name))
        .cloned()
        .unwrap_or_else(|| panic!("no output {name} in {status:?}"))
}

/// What every process of a test gets: directories of the test, and nothing of
/// the session the tests run in.
fn isolate(command: &mut Command, dir: &Path) {
    command
        .env("XDG_RUNTIME_DIR", dir)
        .env("XDG_STATE_HOME", dir.join("state"))
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("HOME", dir)
        .env(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/veil-test-bus",
        )
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("WAYLAND_SOCKET")
        .env_remove("SWAYSOCK")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .env_remove("DISPLAY")
        .env_remove("NOTIFY_SOCKET")
        .env_remove("WATCHDOG_USEC")
        .env_remove("JOURNAL_STREAM");
    // A registry of the tests' own: a stale one in ~/.cache can hide plugins
    // Shared by the tests, so the plugins are scanned once.
    if std::env::var_os("GST_REGISTRY").is_none() && std::env::var_os("GST_REGISTRY_1_0").is_none()
    {
        command.env(
            "GST_REGISTRY",
            Path::new(env!("CARGO_TARGET_TMPDIR")).join("headless-gst-registry.bin"),
        );
    }
}

/// A running `veil daemon`. Dropping it stops it (SIGTERM, then SIGKILL).
pub struct Daemon {
    process: Child,
    log: PathBuf,
}

impl Daemon {
    pub fn pid(&self) -> u32 {
        self.process.id()
    }

    /// What the daemon has logged so far.
    pub fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// How many lines of the log contain `text`.
    pub fn logged(&self, text: &str) -> usize {
        self.log()
            .lines()
            .filter(|line| line.contains(text))
            .count()
    }

    /// Waits until the log has a line with `text`.
    pub fn wait_for_log(&self, text: &str) {
        wait_for(&format!("the daemon to log {text:?}"), || {
            if self.logged(text) > 0 {
                Ok(())
            } else {
                Err(self.log())
            }
        });
    }

    fn proc(&self, file: &str) -> PathBuf {
        PathBuf::from(format!("/proc/{}/{file}", self.pid()))
    }

    /// The daemon's threads.
    pub fn threads(&self) -> usize {
        std::fs::read_dir(self.proc("task"))
            .expect("the daemon's threads")
            .count()
    }

    /// The daemon's open file descriptors.
    pub fn fds(&self) -> usize {
        std::fs::read_dir(self.proc("fd"))
            .expect("the daemon's file descriptors")
            .count()
    }

    /// CPU time the daemon has used, in clock ticks (user plus system).
    pub fn cpu_ticks(&self) -> u64 {
        let stat = std::fs::read_to_string(self.proc("stat")).expect("the daemon's stat");
        // The fields after the name, which is in parentheses and may have
        // spaces: state is field 3, utime 14 and stime 15.
        let after_name = stat.rsplit_once(')').map(|(_, rest)| rest).unwrap_or("");
        let fields: Vec<&str> = after_name.split_whitespace().collect();
        let field = |index: usize| -> u64 {
            fields
                .get(index)
                .and_then(|value| value.parse().ok())
                .expect("a number in /proc/<pid>/stat")
        };
        field(11) + field(12)
    }

    /// CPU ticks the daemon uses over `period`.
    pub fn cpu_over(&self, period: Duration) -> u64 {
        let before = self.cpu_ticks();
        std::thread::sleep(period);
        self.cpu_ticks() - before
    }

    pub fn signal(&self, signal: Signal) {
        let pid = Pid::from_raw(self.pid() as i32).expect("a process id is not 0");
        rustix::process::kill_process(pid, signal).expect("signal the daemon");
    }

    /// The exit status, if the daemon has ended.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.process.try_wait().expect("look at the daemon")
    }

    /// Waits for the daemon to end.
    pub fn wait(&mut self) -> ExitStatus {
        wait_for("the daemon to end", || {
            self.try_wait().ok_or_else(|| self.log())
        })
    }

    /// Stops the daemon with SIGTERM and returns its exit status.
    pub fn stop(mut self) -> ExitStatus {
        self.signal(Signal::TERM);
        self.wait()
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.try_wait().is_none() {
            self.signal(Signal::TERM);
            let deadline = Instant::now() + Duration::from_secs(10);
            while self.try_wait().is_none() && Instant::now() < deadline {
                std::thread::sleep(POLL);
            }
            let _ = self.process.kill();
            let _ = self.process.wait();
        }
        if std::thread::panicking() {
            eprintln!("--- daemon log ---\n{}", self.log());
        }
    }
}

/// Whether `status` is the end of a process killed by `signal`.
pub fn killed_by(status: ExitStatus, signal: Signal) -> bool {
    status.signal() == Some(signal.as_raw())
}

/// A client of the compositor, killed when dropped.
pub struct Client(Child);

impl Client {
    pub fn signal(&self, signal: Signal) {
        let pid = Pid::from_raw(self.0.id() as i32).expect("a process id is not 0");
        rustix::process::kill_process(pid, signal).expect("signal the client");
    }

    /// Waits for the client to end by itself.
    pub fn wait(mut self) -> ExitStatus {
        wait_until("the client to end", || {
            self.0.try_wait().expect("look at the client")
        })
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Polls `probe` until it returns something, for at most [`PATIENCE`].
pub fn wait_until<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    wait_for(what, || probe().ok_or_else(String::new))
}

/// Polls `probe` until it returns `Ok`, for at most [`PATIENCE`]. The failure
/// names what the last try found instead (its `Err`).
pub fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Result<T, String>) -> T {
    let start = Instant::now();
    loop {
        match probe() {
            Ok(found) => return found,
            Err(last) if start.elapsed() > PATIENCE => {
                panic!("waited {PATIENCE:?} for {what}; {last}")
            }
            Err(_) => std::thread::sleep(POLL),
        }
    }
}

/// Test media, made once per test run and kept in Cargo's directory for test
/// files (`target/tmp`), so a second run does not make them again.
pub struct Media;

impl Media {
    fn dir() -> PathBuf {
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("headless-media");
        std::fs::create_dir_all(&dir).expect("create the media directory");
        dir
    }

    /// A picture of `width` x `height` in one colour.
    pub fn png(width: u32, height: u32, colour: Rgb) -> PathBuf {
        let path = Self::dir().join(format!(
            "{}-{width}x{height}.png",
            hex(colour).trim_start_matches('#')
        ));
        if !path.exists() {
            let picture = RgbImage::from_pixel(width, height, image::Rgb(colour));
            let partial = path.with_extension(format!("{}.part", std::process::id()));
            picture
                .save_with_format(&partial, image::ImageFormat::Png)
                .expect("write a test picture");
            std::fs::rename(&partial, &path).expect("move the test picture into place");
        }
        path
    }

    /// A picture of `width` x `height` in four quarters: red, green, blue and
    /// yellow (top left, top right, bottom left, bottom right).
    pub fn quarters(width: u32, height: u32) -> PathBuf {
        let path = Self::dir().join(format!("quarters-{width}x{height}.png"));
        if !path.exists() {
            let picture = RgbImage::from_fn(width, height, |x, y| {
                image::Rgb(match (x < width / 2, y < height / 2) {
                    (true, true) => RED,
                    (false, true) => GREEN,
                    (true, false) => BLUE,
                    (false, false) => YELLOW,
                })
            });
            let partial = path.with_extension(format!("{}.part", std::process::id()));
            picture
                .save_with_format(&partial, image::ImageFormat::Png)
                .expect("write a test picture");
            std::fs::rename(&partial, &path).expect("move the test picture into place");
        }
        path
    }

    /// A VP8 clip of `frames` frames at 30 fps of `videotestsrc`'s `pattern`
    /// (`ball` moves), `width` x `height`.
    pub fn clip(pattern: &str, width: u32, height: u32, frames: u32) -> PathBuf {
        let path = Self::dir().join(format!("{pattern}-{width}x{height}-{frames}.webm"));
        if !path.exists() {
            let partial = path.with_extension(format!("{}.part", std::process::id()));
            let mut command = Command::new("gst-launch-1.0");
            command.arg("-q").args(
                format!(
                    "videotestsrc pattern={pattern} num-buffers={frames} \
                     ! video/x-raw,width={width},height={height},framerate=30/1 \
                     ! videoconvert ! vp8enc deadline=1 ! webmmux ! filesink"
                )
                .split_whitespace(),
            );
            command.arg(format!("location={}", partial.display()));
            if std::env::var_os("GST_REGISTRY").is_none()
                && std::env::var_os("GST_REGISTRY_1_0").is_none()
            {
                command.env(
                    "GST_REGISTRY",
                    Path::new(env!("CARGO_TARGET_TMPDIR")).join("headless-gst-registry.bin"),
                );
            }
            let output = command.output().expect("run gst-launch-1.0");
            assert!(
                output.status.success(),
                "make the clip {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
            std::fs::rename(&partial, &path).expect("move the clip into place");
        }
        path
    }

    /// A file of `bytes` with the name `name`.
    pub fn file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = Self::dir().join(name);
        let mut file = std::fs::File::create(&path).expect("create a test file");
        file.write_all(bytes).expect("write a test file");
        path
    }
}

/// The path as `veil` takes it.
pub fn arg(path: &Path) -> &str {
    path.to_str().expect("a UTF-8 path")
}

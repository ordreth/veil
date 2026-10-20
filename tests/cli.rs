//! The `veil` binary itself, where no compositor is needed: the commands for a
//! running daemon against a test server or against nothing, and a `veil
//! daemon` that finds the socket taken, which it finds out before it connects
//! to a compositor.

mod common;

use common::{set, TestServer};
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use veil::ipc::instance;
use veil::ipc::protocol::Request;
use veil::ipc::server::ServerConfig;
use veil::scaling::ScaleMode;

/// `WAYLAND_DISPLAY` of the tests. No compositor listens under this name.
const DISPLAY: &str = "veil-test";

/// A private `XDG_RUNTIME_DIR` for one test, removed when dropped.
struct RuntimeDir(PathBuf);

impl RuntimeDir {
    /// `name` must be unique among the tests of this file.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("veil-cli-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .expect("create the runtime directory");
        Self(dir)
    }

    /// Where a daemon started in this directory puts its socket.
    fn socket(&self) -> PathBuf {
        self.0.join(format!("veil-{DISPLAY}.sock"))
    }

    /// Where a daemon started in this directory keeps its saved wallpapers
    /// (its `XDG_STATE_HOME` is the directory `state` in here).
    fn state_file(&self) -> PathBuf {
        self.0.join("state/veil/state.json")
    }

    /// The configuration file a daemon started in this directory reads (its
    /// `XDG_CONFIG_HOME` is the directory `config` in here).
    fn config_file(&self) -> PathBuf {
        self.0.join("config/veil/config.toml")
    }

    /// Runs the `veil` binary with this runtime directory.
    fn veil(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("run veil")
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_veil"));
        command
            .args(args)
            .env("XDG_RUNTIME_DIR", &self.0)
            // Never the developer's own saved wallpapers or configuration.
            .env("XDG_STATE_HOME", self.0.join("state"))
            .env("XDG_CONFIG_HOME", self.0.join("config"))
            .env("WAYLAND_DISPLAY", DISPLAY)
            .env_remove("WAYLAND_SOCKET")
            .env_remove("NOTIFY_SOCKET")
            .env_remove("RUST_LOG")
            // No desktop notifications from tests: with no session bus to
            // reach, an attempt fails at once and is logged.
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                "unix:path=/nonexistent/veil-test-bus",
            );
        command
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Whether the process tried to raise a desktop notification (see `command`).
fn tried_to_notify(output: &Output) -> bool {
    stderr(output).contains("desktop notification")
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("UTF-8 temp path")
}

#[test]
fn set_without_a_daemon_says_that_none_is_running() {
    let runtime = RuntimeDir::new("no-daemon");

    let output = runtime.veil(&["set", "--input", "/srv/a.png"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains("no veil daemon is running"), "{message}");
    assert!(message.contains("veil daemon"), "{message}");
    assert!(
        message.contains(path_str(&runtime.socket())),
        "the socket path is named: {message}"
    );
}

#[test]
fn set_against_a_socket_left_behind_says_the_same() {
    let runtime = RuntimeDir::new("stale-set");
    drop(UnixListener::bind(runtime.socket()).expect("bind"));
    assert!(runtime.socket().exists());

    let output = runtime.veil(&["set", "--input", "/srv/a.png"]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("no veil daemon is running"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_second_daemon_exits_and_leaves_the_first_alone() {
    let runtime = RuntimeDir::new("second");
    // The first daemon: a server that holds the claim on the socket.
    let first = TestServer::start_at(runtime.socket(), ServerConfig::default());

    for args in [
        &["daemon"][..],
        &["daemon", "--bg-color", "#102030"],
        &["--daemon"],
    ] {
        let output = runtime.veil(args);
        assert_eq!(
            output.status.code(),
            Some(4),
            "{args:?}: {}",
            stderr(&output)
        );
        let message = stderr(&output);
        assert!(message.contains("already running"), "{message}");
        assert!(
            message.contains(path_str(&runtime.socket())),
            "the socket path is named: {message}"
        );
        // It stopped before it looked for a compositor, and quietly.
        assert!(!message.contains("Wayland"), "{message}");
        assert!(!tried_to_notify(&output), "{message}");
    }

    // The first daemon kept its socket and still answers.
    assert!(runtime.socket().exists());
    first.assert_alive();
    let output = runtime.veil(&["set", "--input", "/srv/a.png"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
}

#[test]
fn a_daemon_starts_over_what_a_killed_daemon_left_behind() {
    let runtime = RuntimeDir::new("stale-daemon");
    // After `kill -9`: the socket file with nobody listening, and the lock
    // file with nobody holding the lock.
    drop(UnixListener::bind(runtime.socket()).expect("bind"));
    std::fs::File::create(instance::lock_path(&runtime.socket())).expect("lock file");

    // It gets past the instance check and fails where it needs a compositor,
    // which the test does not have.
    let output = runtime.veil(&["daemon"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(!message.contains("already running"), "{message}");
    assert!(
        message.contains("Failed to connect to Wayland display"),
        "{message}"
    );

    // Leaving with an error, it took its socket with it.
    assert!(!runtime.socket().exists(), "the socket was removed");
    // And the next one starts the same way.
    let output = runtime.veil(&["daemon"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(!runtime.socket().exists());
}

#[test]
fn set_reaches_the_daemon_with_an_absolute_path() {
    let runtime = RuntimeDir::new("set");
    let server = TestServer::start_at(runtime.socket(), ServerConfig::default());

    // A relative path, from a working directory that is not the daemon's.
    let output = runtime
        .command(&["set", "--input", "walls/a.png", "--mode", "fit"])
        .current_dir(&runtime.0)
        .output()
        .expect("run veil");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    let cwd = std::fs::canonicalize(&runtime.0).expect("canonical runtime directory");
    assert_eq!(
        server.requests(),
        [Request::Set {
            output: None,
            input: path_str(&cwd.join("walls/a.png")).to_string(),
            mode: Some(ScaleMode::Fit),
            bg_color: None,
            persist: true,
        }]
    );
}

#[test]
fn an_error_of_the_daemon_is_an_ordinary_failure() {
    let runtime = RuntimeDir::new("set-error");
    let server = TestServer::start_at(runtime.socket(), ServerConfig::default());

    // The test server refuses this file name (see `common`).
    let output = runtime.veil(&["set", "--input", "/srv/broken.png"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("cannot decode"),
        "{}",
        stderr(&output)
    );
    assert_eq!(server.requests(), [set("/srv/broken.png")]);
}

#[test]
fn a_daemon_refuses_an_input_that_cannot_be_a_wallpaper_and_says_why() {
    let runtime = RuntimeDir::new("bad-input");
    // A daemon looks at its `--input` before it connects to a compositor, so
    // what it says about these files needs none.
    let cases: [(&str, &[u8], &str); 4] = [
        ("empty.png", b"", "is empty"),
        ("empty.mp4", b"", "is empty"),
        (
            "notes.txt",
            b"just some notes\n",
            "neither an image nor a video",
        ),
        (
            "notes",
            b"just some notes\n",
            "neither an image nor a video",
        ),
    ];
    for (name, content, reason) in cases {
        let input = runtime.0.join(name);
        std::fs::write(&input, content).expect("write the input");

        let output = runtime.veil(&["daemon", "--input", path_str(&input)]);
        assert_eq!(output.status.code(), Some(1), "{name}: {}", stderr(&output));
        assert!(
            stderr(&output).contains(reason) && stderr(&output).contains(name),
            "{name}: {}",
            stderr(&output)
        );
        // It left like any daemon that ends: without its socket.
        assert!(!runtime.socket().exists(), "{name}");
    }

    let output = runtime.veil(&["daemon", "--input", path_str(&runtime.0)]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("is not a regular file"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn set_without_saving_says_so_to_the_daemon() {
    let runtime = RuntimeDir::new("no-persist");
    let server = TestServer::start_at(runtime.socket(), ServerConfig::default());

    let output = runtime.veil(&["set", "--input", "/srv/a.png", "--no-persist"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(
        server.requests(),
        [Request::Set {
            output: None,
            input: "/srv/a.png".into(),
            mode: None,
            bg_color: None,
            persist: false,
        }]
    );
}

#[test]
fn a_saved_state_that_cannot_be_used_does_not_stop_the_daemon() {
    let runtime = RuntimeDir::new("bad-state");
    let state = runtime.state_file();
    std::fs::create_dir_all(state.parent().expect("parent")).expect("state directory");
    for content in [
        &b"{\"version\":1,\"outp"[..],
        b"{\"version\":99}",
        br#"{"version":1,"default":{"input":"walls/a.png","mode":"fill","bg_color":"000000"}}"#,
    ] {
        std::fs::write(&state, content).expect("write the state file");

        // It gets as far as the compositor, which the test does not have.
        let output = runtime.veil(&["daemon"]);
        assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
        let message = stderr(&output);
        assert!(
            message.contains("Failed to connect to Wayland display"),
            "{message}"
        );
        assert!(message.contains(path_str(&state)), "{message}");
        // Nothing was saved, so the file is as it was.
        assert_eq!(std::fs::read(&state).expect("read"), content);
    }
}

#[test]
fn a_daemon_that_does_not_start_saves_nothing() {
    let runtime = RuntimeDir::new("unsaved");
    let input = runtime.0.join("a.png");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
        .save(&input)
        .expect("write the input");

    // The `--input` is fine; the compositor is missing.
    let output = runtime.veil(&["daemon", "--input", path_str(&input)]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Failed to connect to Wayland display"),
        "{}",
        stderr(&output)
    );
    assert!(!runtime.state_file().exists());
}

#[test]
fn both_commands_need_the_runtime_directory() {
    let runtime = RuntimeDir::new("no-runtime-dir");
    for args in [&["daemon"][..], &["set", "--input", "/srv/a.png"]] {
        let output = runtime
            .command(args)
            .env_remove("XDG_RUNTIME_DIR")
            .output()
            .expect("run veil");
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).contains("XDG_RUNTIME_DIR"),
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

/// Every command for a running daemon, with the arguments it needs.
const CONTROL_COMMANDS: [&[&str]; 10] = [
    &["set", "--input", "/srv/a.png"],
    &["clear"],
    &["pause"],
    &["resume"],
    &["toggle"],
    &["status"],
    &["outputs", "--json"],
    &["quit"],
    &["stop"],
    &["ping"],
];

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn every_command_without_a_daemon_says_that_none_is_running() {
    let runtime = RuntimeDir::new("no-daemon-all");
    for args in CONTROL_COMMANDS {
        let output = runtime.veil(args);
        assert_eq!(
            output.status.code(),
            Some(3),
            "{args:?}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains("no veil daemon is running"),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{args:?}");
        // A status bar that asks while no daemon runs must not flood the
        // desktop; a key binding that acts is told.
        let asks = matches!(args[0], "status" | "outputs" | "ping");
        assert_eq!(
            tried_to_notify(&output),
            !asks,
            "{args:?}: {}",
            stderr(&output)
        );
    }
}

#[test]
fn the_commands_reach_the_daemon_and_print_its_answer() {
    let runtime = RuntimeDir::new("commands");
    let server = TestServer::start_at(runtime.socket(), ServerConfig::default());

    for (args, printed) in [
        (&["ping"][..], "pong\n"),
        (&["toggle", "-o", common::OUTPUT], "paused\n"),
        (&["pause"], ""),
        (&["resume", "--output", common::OUTPUT], ""),
        (&["clear", "--bg-color", "#102030", "--no-persist"], ""),
        (&["outputs"], "DP-3: connected, wallpaper (Test monitor)\n"),
        (
            &["status"],
            "veil 0.1.0 (pid 7), running for 1m 05s; videos pause on: fullscreen\n\
             default: the background colour\n\
             DP-3: showing /srv/a.png (fit, #000000); 1920x1080 at scale 1\n",
        ),
    ] {
        let output = runtime.veil(args);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), printed, "{args:?}");
    }

    // `--json` prints the daemon's report on one line, as it was sent.
    for (args, expected) in [
        (
            "status",
            serde_json::to_value(common::status()).expect("status JSON"),
        ),
        (
            "outputs",
            serde_json::to_value(common::outputs()).expect("outputs JSON"),
        ),
    ] {
        let output = runtime.veil(&[args, "--json"]);
        assert_eq!(output.status.code(), Some(0), "{args}: {}", stderr(&output));
        let printed = stdout(&output);
        assert_eq!(printed.lines().count(), 1, "{printed:?}");
        let value: serde_json::Value = serde_json::from_str(&printed).expect("JSON");
        assert_eq!(value, expected, "{args}");
    }

    assert_eq!(
        server.requests(),
        [
            Request::Ping {},
            Request::Toggle {
                output: Some(common::OUTPUT.into())
            },
            Request::Pause { output: None },
            Request::Resume {
                output: Some(common::OUTPUT.into())
            },
            Request::Clear {
                output: None,
                bg_color: Some(veil::scaling::Color {
                    r: 0x10,
                    g: 0x20,
                    b: 0x30
                }),
                persist: false
            },
            Request::Outputs {},
            Request::Status {},
            Request::Status {},
            Request::Outputs {},
        ]
    );
}

#[test]
fn a_command_for_an_output_the_daemon_does_not_draw_on_fails() {
    let runtime = RuntimeDir::new("unknown-output");
    let _server = TestServer::start_at(runtime.socket(), ServerConfig::default());
    for args in [
        &["pause", "-o", "HDMI-A-1"][..],
        &["toggle", "-o", "HDMI-A-1"],
        &["clear", "-o", "HDMI-A-1"],
        &["set", "-o", "HDMI-A-1", "-i", "/srv/a.png"],
    ] {
        let output = runtime.veil(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            stderr(&output).contains("not driven by this daemon"),
            "{args:?}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{args:?}");
    }
}

#[test]
fn quit_returns_once_the_daemon_has_ended() {
    let runtime = RuntimeDir::new("quit");
    let server = TestServer::start_at(runtime.socket(), ServerConfig::default());
    let mut quit = runtime
        .command(&["quit"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run veil quit");

    // The stand-in answers and goes on running: `veil quit` waits for it.
    let deadline = std::time::Instant::now() + common::PATIENCE;
    while server.requests().is_empty() {
        assert!(std::time::Instant::now() < deadline, "quit never arrived");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert_eq!(quit.try_wait().expect("still running?"), None);
    assert_eq!(server.requests(), [Request::Quit {}]);

    // The daemon ends: its claim on the socket is released.
    drop(server);
    let output = quit.wait_with_output().expect("veil quit ends");
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    // The next daemon could start now.
    drop(instance::claim(&runtime.socket()).expect("the socket is free"));
}

#[test]
fn a_command_line_that_does_not_parse_exits_with_2() {
    let runtime = RuntimeDir::new("usage");
    for args in [
        &[][..],
        &["set"],
        &["frobnicate"],
        &["pause", "--json"],
        &["status", "-o", "DP-3"],
        &["quit", "now"],
    ] {
        let output = runtime.veil(args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn a_daemon_with_a_wrong_configuration_file_exits_with_2() {
    let runtime = RuntimeDir::new("bad-config");
    let config = runtime.config_file();
    std::fs::create_dir_all(config.parent().expect("a directory")).expect("create the directory");
    std::fs::write(&config, "[defaults]\nmode = \"zoom\"\n").expect("write the file");

    let output = runtime.veil(&["daemon"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(
        message.contains(&format!("{}:2:8: ", path_str(&config))),
        "{message}"
    );
    // Under systemd nobody reads the terminal.
    assert!(tried_to_notify(&output), "{message}");
    // A flag does not make a wrong file right.
    let output = runtime.veil(&["daemon", "--mode", "fit"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));

    // A file named on the command line must exist; the default one need not.
    let missing = runtime.0.join("missing.toml");
    let output = runtime.veil(&["daemon", "--config", path_str(&missing)]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(
        stderr(&output).contains(path_str(&missing)),
        "{}",
        stderr(&output)
    );
    std::fs::remove_file(&config).expect("remove the file");
    let output = runtime.veil(&["daemon"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Failed to connect to Wayland display"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn a_daemon_with_a_configuration_file_saves_nothing_before_it_has_started() {
    let runtime = RuntimeDir::new("config-unsaved");
    let config = runtime.config_file();
    std::fs::create_dir_all(config.parent().expect("a directory")).expect("create the directory");
    std::fs::write(&config, "[output.\"*\"]\ninput = \"/srv/a.png\"\n").expect("write the file");

    // The file is fine; the compositor is missing.
    let output = runtime.veil(&["daemon"]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let message = stderr(&output);
    assert!(message.contains(path_str(&config)), "{message}");
    assert!(
        message.contains("Failed to connect to Wayland display"),
        "{message}"
    );
    assert!(!runtime.state_file().exists());
}

#[test]
fn config_check_says_what_is_wrong_and_raises_no_notification() {
    let runtime = RuntimeDir::new("config-check");

    // No file: the built-in defaults.
    let output = runtime.veil(&["config", "check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).starts_with("no configuration file at "),
        "{}",
        stdout(&output)
    );

    let config = runtime.config_file();
    let directory = config.parent().expect("a directory");
    std::fs::create_dir_all(directory).expect("create the directory");
    image::RgbaImage::from_pixel(2, 2, image::Rgba([255, 0, 0, 255]))
        .save(directory.join("a.png"))
        .expect("write the image");

    // A relative input is next to the file.
    std::fs::write(
        &config,
        "[defaults]\nmode = \"fit\"\n[output.DP-3]\ninput = \"a.png\"\n",
    )
    .expect("write the file");
    let output = runtime.veil(&["config", "check"]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    let printed = stdout(&output);
    assert!(
        printed.starts_with(&format!("{}: ok\n", path_str(&config))),
        "{printed}"
    );
    assert!(
        printed.contains(&format!(
            "[output.\"DP-3\"]: image {} (fit, #000000)\n",
            path_str(&directory.join("a.png"))
        )),
        "{printed}"
    );

    for (text, status, says) in [
        ("[output.DP-3]\ninput = \"b.png\"\n", 1, "b.png"),
        ("[defaults]\npause_on = \"sometimes\"\n", 2, ":2:12: "),
        ("[output.DP-3]\nloop = true\n", 2, ":2:1: "),
    ] {
        std::fs::write(&config, text).expect("write the file");
        let output = runtime.veil(&["config", "check"]);
        assert_eq!(
            output.status.code(),
            Some(status),
            "{text}: {}",
            stderr(&output)
        );
        assert!(
            stderr(&output).contains(says),
            "{text}: {}",
            stderr(&output)
        );
        assert_eq!(stdout(&output), "", "{text}");
        assert!(!tried_to_notify(&output), "{text}: {}", stderr(&output));
    }

    // `--config` names another file.
    let other = runtime.0.join("other.toml");
    std::fs::write(&other, "[defaults]\nbg_color = \"#102030\"\n").expect("write the file");
    let output = runtime.veil(&["config", "check", "--config", path_str(&other)]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("bg_color: #102030\n"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn completions_and_man_pages_need_no_daemon() {
    let runtime = RuntimeDir::new("generate");
    for shell in ["bash", "zsh", "fish", "nushell"] {
        let output = runtime.veil(&["completions", shell]);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{shell}: {}",
            stderr(&output)
        );
        assert!(stdout(&output).contains("pause-on"), "{shell}");
        assert_eq!(stderr(&output), "", "{shell}");
    }
    let output = runtime.veil(&["completions", "powershell"]);
    assert_eq!(output.status.code(), Some(2), "{}", stderr(&output));
    assert!(!tried_to_notify(&output), "{}", stderr(&output));

    let pages = runtime.0.join("man/man1");
    let output = runtime.veil(&["man-pages", path_str(&pages)]);
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(stdout(&output), "");
    for page in ["veil.1", "veil-set.1", "veil-config-check.1"] {
        let text = std::fs::read_to_string(pages.join(page)).expect("a page");
        assert!(text.contains(".SH SYNOPSIS"), "{page}");
    }

    // A directory that cannot be made: status 1, and no notification (a
    // package build has no desktop).
    let file = runtime.0.join("a-file");
    std::fs::write(&file, "").expect("write a file");
    let output = runtime.veil(&["man-pages", path_str(&file.join("man"))]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("Cannot write"),
        "{}",
        stderr(&output)
    );
    assert!(!tried_to_notify(&output), "{}", stderr(&output));
}

//! The real daemon in a real compositor: every test starts a headless sway of
//! its own (`tests/sway/`), runs `veil daemon` and the commands in it, and
//! looks at the outputs with `grim`. These are checks that were first made
//! by hand (`docs/manual-tests.md`).
//!
//! They need the tools of the `headless` dev shell, so a plain `cargo test`
//! skips them (ignored). Run them with
//!
//! ```text
//! nix develop .#headless -c cargo test --test headless -- --ignored
//! ```
//!
//! Output changes while a video plays crash GStreamer's `waylandsink`
//! (`docs/known-issues.md`, entry 1): every test that changes outputs does it
//! with images only, and one test expects the crash.

mod sway;

use rustix::process::Signal;
use std::io::Write;
use std::process::Stdio;
use std::time::Duration;
use sway::hyprland::{self, FakeHyprland};
use sway::{arg, hex, killed_by, Media, Sway, BLACK, BLUE, CYAN, GREEN, MAGENTA, RED, YELLOW};
use veil::ipc::protocol::{ErrorCode, Outcome, Response};
use veil::ipc::report::{Kind, OutputList, State, Status, Toggled};
use veil::ipc::server::ServerConfig;

/// A clip of 3 s whose picture changes in every frame.
fn ball() -> std::path::PathBuf {
    Media::clip("ball", 320, 180, 90)
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn an_image_on_every_output_and_another_on_one() {
    let sway = Sway::start("images", &[(640, 480), (800, 600)]);
    let daemon = sway.daemon(&[]);
    // Nothing given, nothing saved: the background colour.
    sway.wait_for_state("HEADLESS-1", State::Shown);
    assert_eq!(sway.centre("HEADLESS-1"), BLACK);

    sway.veil_ok(&["set", "--input", arg(&Media::png(64, 48, RED))]);
    assert_eq!(sway.centre("HEADLESS-1"), RED);
    assert_eq!(sway.centre("HEADLESS-2"), RED);

    // A picture narrower than the output, fitted: bars of the colour given.
    sway.veil_ok(&[
        "set",
        "--input",
        arg(&Media::png(300, 600, BLUE)),
        "--output",
        "HEADLESS-2",
        "--mode",
        "fit",
        "--bg-color",
        &hex(GREEN),
    ]);
    assert_eq!(sway.centre("HEADLESS-2"), BLUE);
    assert_eq!(sway.corner("HEADLESS-2"), GREEN);
    assert_eq!(
        sway.centre("HEADLESS-1"),
        RED,
        "the other output is untouched"
    );

    let status = sway.output_status("HEADLESS-2");
    assert_eq!(status.kind, Kind::Image);
    assert_eq!(status.state, State::Shown);
    assert_eq!(
        status.size.map(|size| (size.width, size.height)),
        Some((800, 600))
    );

    // A still picture costs nothing while it is shown.
    let ticks = daemon.cpu_over(Duration::from_secs(3));
    assert!(ticks <= 2, "{ticks} CPU ticks in 3 s with an image");
    assert_eq!(daemon.stop().code(), Some(0));
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn outputs_come_and_go_without_touching_the_others() {
    let sway = Sway::start("hotplug", &[(640, 480), (640, 480)]);
    let mut daemon = sway.daemon(&["--input", arg(&Media::quarters(64, 48))]);
    sway.wait_for_state("HEADLESS-2", State::Shown);
    sway.veil_ok(&[
        "set",
        "--input",
        arg(&Media::png(64, 48, BLUE)),
        "--output",
        "HEADLESS-2",
    ]);
    let first = sway.shot("HEADLESS-1");
    assert_eq!(first.get_pixel(10, 10).0, RED);
    assert_eq!(first.get_pixel(630, 470).0, YELLOW);

    // A new output gets the default wallpaper, not the one of HEADLESS-2
    // (first the background colour, while it loads).
    let third = sway.create_output(640, 480, 1280);
    sway.wait_for_picture(&third, &first);

    // Disabled and enabled again: back with its own wallpaper.
    sway.msg(&["output", "HEADLESS-2", "disable"]);
    sway.wait_for_state("HEADLESS-2", State::Disconnected);
    sway.assert_picture("HEADLESS-1", &first);
    sway.msg(&["output", "HEADLESS-2", "enable"]);
    sway.wait_for_centre("HEADLESS-2", BLUE);
    sway.assert_picture("HEADLESS-1", &first);
    sway.assert_picture(&third, &first);

    // Unplugged for good: the daemon keeps its wallpaper for it, and a new
    // output (another name) gets the default.
    sway.msg(&["output", &third, "unplug"]);
    sway.wait_for_state(&third, State::Disconnected);
    sway.assert_picture("HEADLESS-1", &first);
    let fourth = sway.create_output(640, 480, 1280);
    sway.wait_for_picture(&fourth, &first);
    assert_eq!(sway.centre("HEADLESS-2"), BLUE);

    assert!(daemon.try_wait().is_none(), "the daemon is still running");
    assert_eq!(daemon.stop().code(), Some(0));
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn two_videos_play_side_by_side() {
    let sway = Sway::start("videos", &[(640, 360), (640, 360)]);
    let daemon = sway.daemon(&[]);
    let snow = Media::clip("snow", 256, 144, 90);
    sway.veil_ok(&["set", "--input", arg(&ball()), "--output", "HEADLESS-1"]);
    sway.veil_ok(&["set", "--input", arg(&snow), "--output", "HEADLESS-2"]);

    for (output, size) in [("HEADLESS-1", (320, 180)), ("HEADLESS-2", (256, 144))] {
        let status = sway.wait_for_state(output, State::Playing);
        assert_eq!(status.kind, Kind::Video);
        let video = status.video.expect("a playing video reports its stream");
        assert_eq!(
            video.size.map(|size| (size.width, size.height)),
            Some(size),
            "{output} plays its own clip"
        );
        assert!(sway.moves(output), "the video on {output} moves");
    }
    assert_eq!(daemon.stop().code(), Some(0));
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn a_video_with_bars_sits_in_the_middle_and_plays() {
    // The parent surface shows around the video, and the daemon does not commit
    // it on every frame: the sink presents its frames by itself, and
    // the place of its subsurface takes effect with the commits veil does make.
    let sway = Sway::start("bars", &[(640, 360)]);
    // 4:3 on 16:9, fitted: 480x360 in the middle, bars of 80 pixels.
    let clip = Media::clip("ball", 240, 180, 90);
    let red = hex(RED);
    let fit = ["--mode", "fit", "--bg-color", &red];
    // On a surface that showed nothing: the bars are drawn before the frame
    // size is known, and the subsurface is moved afterwards.
    let daemon = sway.daemon(&[&["--input", arg(&clip)][..], &fit].concat());
    for after_an_image in [false, true] {
        if after_an_image {
            let image = Media::png(64, 48, BLUE);
            sway.veil_ok(&[&["set", "--input", arg(&image)][..], &fit].concat());
            sway.veil_ok(&[&["set", "--input", arg(&clip)][..], &fit].concat());
        }
        sway.wait_for_state("HEADLESS-1", State::Playing);
        let shot = sway.shot("HEADLESS-1");
        for x in [4, 76, 564, 636] {
            assert_eq!(shot.get_pixel(x, 180).0, RED, "a bar at x {x}");
        }
        for x in [84, 556] {
            assert_ne!(shot.get_pixel(x, 180).0, RED, "the video at x {x}");
        }
        assert!(sway.moves("HEADLESS-1"), "the video plays");
    }
    assert_eq!(daemon.stop().code(), Some(0));
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn the_latest_set_wins_on_each_output() {
    let sway = Sway::start("latest", &[(640, 480), (640, 480)]);
    let daemon = sway.daemon(&[]);
    sway.wait_for_state("HEADLESS-2", State::Shown);

    // Sent one after the other without waiting for the answers, so they queue
    // up behind the video, which takes longest to load.
    let requests: Vec<Vec<String>> = vec![
        vec!["set".into(), "--input".into(), arg(&ball()).into()],
        set_on(&Media::png(64, 48, RED), "HEADLESS-1"),
        set_on(&Media::png(64, 48, BLUE), "HEADLESS-2"),
        set_on(&Media::png(64, 48, GREEN), "HEADLESS-1"),
    ];
    let clients: Vec<_> = requests
        .iter()
        .map(|args| {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let client = sway
                .command(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start veil set");
            std::thread::sleep(Duration::from_millis(30));
            client
        })
        .collect();
    let answers: Vec<_> = clients
        .into_iter()
        .map(|client| client.wait_with_output().expect("veil set"))
        .collect();

    // The last request for each output succeeded; an earlier one either took
    // effect before the next came or was replaced, which is no failure to
    // raise a desktop notification for.
    for (index, answer) in answers.iter().enumerate() {
        let stderr = String::from_utf8_lossy(&answer.stderr);
        if index >= 2 {
            assert!(answer.status.success(), "request {index}: {stderr}");
        } else {
            assert!(
                answer.status.success() || stderr.contains("replaced by a newer"),
                "request {index}: {stderr}"
            );
        }
        assert!(
            !stderr.contains("desktop notification"),
            "request {index}: {stderr}"
        );
    }
    assert_eq!(sway.centre("HEADLESS-1"), GREEN);
    assert_eq!(sway.centre("HEADLESS-2"), BLUE);
    let status = sway.status();
    for output in &status.outputs {
        assert_eq!(output.kind, Kind::Image, "{output:?}");
        assert_eq!(output.next, None, "nothing is left to load: {output:?}");
    }
    assert_eq!(daemon.stop().code(), Some(0));
}

fn set_on(file: &std::path::Path, output: &str) -> Vec<String> {
    ["set", "--input", arg(file), "--output", output]
        .map(String::from)
        .to_vec()
}

/// `docs/known-issues.md`, entry 1. If this test fails because the daemon
/// survived, the upstream bug may be fixed: update that entry and this test.
#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn an_output_change_while_a_video_plays_kills_the_daemon() {
    let sway = Sway::start("crash", &[(640, 360)]);
    let mut daemon = sway.daemon(&["--input", arg(&ball())]);
    sway.wait_for_state("HEADLESS-1", State::Playing);

    sway.msg(&["output", "HEADLESS-1", "scale", "2"]);
    let status = daemon.wait();
    assert!(
        killed_by(status, Signal::SEGV),
        "known issue 1: waylandsink crashes on an output change; the daemon ended with {status}"
    );
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn a_video_pauses_behind_a_fullscreen_window() {
    let sway = Sway::start("fullscreen", &[(640, 360)]);
    // Only the pause: a covered video is not released meanwhile.
    let daemon = sway.daemon(&["--input", arg(&ball()), "--release-unseen-after", "0"]);
    sway.wait_for_state("HEADLESS-1", State::Playing);
    daemon.wait_for_log("following sway");
    let playing = daemon.cpu_over(Duration::from_secs(2));

    let window = sway.window("cover");
    sway.msg(&["[app_id=cover]", "fullscreen", "enable"]);
    let status = sway.wait_for_state("HEADLESS-1", State::Paused);
    assert!(status.hidden && !status.paused_by_user, "{status:?}");
    let paused = daemon.cpu_over(Duration::from_secs(2));
    assert!(
        paused * 4 <= playing && paused <= 10,
        "CPU ticks in 2 s: {playing} playing, {paused} paused"
    );

    drop(window);
    sway.wait_for_state("HEADLESS-1", State::Playing);
    assert!(sway.moves("HEADLESS-1"));

    // Toggled faster than the pause delay: never paused.
    let pauses = daemon.logged("pausing it");
    let _window = sway.window("flicker");
    for _ in 0..5 {
        sway.msg(&["[app_id=flicker]", "fullscreen", "enable"]);
        std::thread::sleep(Duration::from_millis(200));
        sway.msg(&["[app_id=flicker]", "fullscreen", "disable"]);
        std::thread::sleep(Duration::from_millis(200));
    }
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(daemon.logged("pausing it"), pauses, "{}", daemon.log());
    assert_eq!(sway.output_status("HEADLESS-1").state, State::Playing);
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn a_video_pauses_behind_a_fullscreen_window_on_hyprland() {
    let sway = Sway::start("hyprland", &[(640, 360)]);
    let hyprland = FakeHyprland::start(
        sway.dir(),
        format!("[{}]", hyprland::monitor("HEADLESS-1", 0, 640, 360)),
    );
    let daemon = sway.daemon_with(&["--input", arg(&ball())], FakeHyprland::env);
    sway.wait_for_state("HEADLESS-1", State::Playing);
    daemon.wait_for_log("following Hyprland");
    sway::wait_until("the daemon to follow the events", || {
        hyprland.followed().then_some(())
    });

    hyprland.set_clients(format!("[{}]", hyprland::fullscreen_window(0, 640, 360)));
    hyprland.event("fullscreen>>1");
    let status = sway.wait_for_state("HEADLESS-1", State::Paused);
    assert!(status.hidden, "{status:?}");

    hyprland.set_clients("[]".into());
    hyprland.event("fullscreen>>0");
    sway.wait_for_state("HEADLESS-1", State::Playing);
    assert!(sway.moves("HEADLESS-1"));
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn a_locked_screen_and_a_powered_off_output_release_the_video() {
    let sway = Sway::start("unseen", &[(640, 360)]);
    let mut daemon = sway.daemon(&["--input", arg(&ball())]);
    sway.wait_for_state("HEADLESS-1", State::Playing);
    let threads = daemon.threads();
    let fds = daemon.fds();

    let lock = sway.client("swaylock", &["--color", "000000"]);
    sway.wait_for_state("HEADLESS-1", State::Released);
    daemon.wait_for_log("has not been shown");
    let released = daemon.cpu_over(Duration::from_secs(2));
    assert!(released <= 2, "{released} CPU ticks in 2 s, released");
    assert!(
        daemon.threads() < threads && daemon.fds() < fds,
        "the pipeline is gone: {} threads (playing: {threads}), {} descriptors (playing: {fds})",
        daemon.threads(),
        daemon.fds()
    );

    // swaylock unlocks on SIGUSR1.
    lock.signal(Signal::USR1);
    assert!(lock.wait().success());
    sway.wait_for_state("HEADLESS-1", State::Playing);
    assert!(sway.moves("HEADLESS-1"));

    // Off for longer than the release delay, so no sink is alive at power on
    // (known issue 1).
    sway.msg(&["output", "HEADLESS-1", "power", "off"]);
    sway.wait_for_state("HEADLESS-1", State::Released);
    sway.msg(&["output", "HEADLESS-1", "power", "on"]);
    sway.wait_for_state("HEADLESS-1", State::Playing);
    assert!(sway.moves("HEADLESS-1"));
    assert!(daemon.try_wait().is_none(), "the daemon survived power on");
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn the_daemon_answers_every_command_and_error_code() {
    let sway = Sway::start("ipc", &[(640, 360)]);
    let mut daemon = sway.daemon(&[]);
    sway.wait_for_state("HEADLESS-1", State::Shown);
    let red = Media::png(64, 48, RED);
    let broken = Media::file("broken.png", b"\x89PNG\r\n\x1a\nthis is no picture");
    let empty = Media::file("empty.png", b"");

    let ok = |line: String| {
        let answer = sway.request(format!("{line}\n").as_bytes());
        assert_eq!(answer.outcome, Outcome::Ok, "{line}: {answer:?}");
        answer
    };
    let refused = |line: String, code: ErrorCode| {
        let answer = sway.request(format!("{line}\n").as_bytes());
        assert!(
            matches!(&answer.outcome, Outcome::Error { code: found, .. } if *found == code),
            "{line}: expected {code:?}, got {answer:?}"
        );
    };
    let quoted = |path: &std::path::Path| serde_json::to_string(arg(path)).expect("JSON string");

    ok(r#"{"cmd":"ping"}"#.into());
    let status: Status = result(ok(r#"{"cmd":"status"}"#.into()));
    assert_eq!(status.outputs.len(), 1);
    let outputs: OutputList = result(ok(r#"{"cmd":"outputs"}"#.into()));
    assert!(outputs
        .outputs
        .iter()
        .any(|output| output.name.as_deref() == Some("HEADLESS-1")));

    ok(format!(r#"{{"cmd":"set","input":{}}}"#, quoted(&red)));
    ok(format!(
        r##"{{"cmd":"set","input":{},"output":"HEADLESS-1","mode":"fit","bg_color":"#00ffff","persist":false}}"##,
        quoted(&red)
    ));
    assert_eq!(sway.corner("HEADLESS-1"), CYAN);
    refused(
        format!(
            r#"{{"cmd":"set","input":{},"output":"NOPE"}}"#,
            quoted(&red)
        ),
        ErrorCode::UnknownOutput,
    );
    refused(
        r#"{"cmd":"set","input":"/nonexistent/a.png"}"#.into(),
        ErrorCode::NotFound,
    );
    refused(
        r#"{"cmd":"set","input":"/tmp"}"#.into(),
        ErrorCode::UnsupportedMedia,
    );
    refused(
        format!(r#"{{"cmd":"set","input":{}}}"#, quoted(&empty)),
        ErrorCode::UnsupportedMedia,
    );
    refused(
        format!(r#"{{"cmd":"set","input":{}}}"#, quoted(&broken)),
        ErrorCode::Failed,
    );
    refused(
        r#"{"cmd":"set","input":"relative.png"}"#.into(),
        ErrorCode::InvalidRequest,
    );

    ok(format!(
        r#"{{"cmd":"clear","output":"HEADLESS-1","bg_color":"{}"}}"#,
        hex(MAGENTA)
    ));
    assert_eq!(sway.centre("HEADLESS-1"), MAGENTA);
    refused(
        r#"{"cmd":"clear","output":"NOPE"}"#.into(),
        ErrorCode::UnknownOutput,
    );

    ok(r#"{"cmd":"pause"}"#.into());
    assert!(sway.output_status("HEADLESS-1").paused_by_user);
    ok(r#"{"cmd":"resume","output":"HEADLESS-1"}"#.into());
    assert!(!sway.output_status("HEADLESS-1").paused_by_user);
    let toggled: Toggled = result(ok(r#"{"cmd":"toggle"}"#.into()));
    assert!(toggled.paused);
    let toggled: Toggled = result(ok(r#"{"cmd":"toggle","output":"HEADLESS-1"}"#.into()));
    assert!(!toggled.paused);
    for command in ["pause", "resume", "toggle"] {
        refused(
            format!(r#"{{"cmd":"{command}","output":"NOPE"}}"#),
            ErrorCode::UnknownOutput,
        );
    }

    refused("this is not JSON".into(), ErrorCode::InvalidRequest);
    refused(r#"{"cmd":"dance"}"#.into(), ErrorCode::InvalidRequest);
    refused(
        r#"{"cmd":"ping","loud":true}"#.into(),
        ErrorCode::InvalidRequest,
    );

    // Requests that queue behind a video: the second loses its only output
    // to the third before it is loaded, and the video loses it too.
    let mut first = sway.connect_to_daemon();
    let mut second = sway.connect_to_daemon();
    let mut third = sway.connect_to_daemon();
    for (stream, input) in [
        (&mut first, quoted(&ball())),
        (&mut second, quoted(&red)),
        (&mut third, quoted(&Media::png(64, 48, BLUE))),
    ] {
        stream
            .write_all(format!("{{\"cmd\":\"set\",\"input\":{input}}}\n").as_bytes())
            .expect("send a set");
    }
    assert_eq!(
        code(sway::read_response(&mut second)),
        Some(ErrorCode::Superseded)
    );
    assert_eq!(
        code(sway::read_response(&mut first)),
        Some(ErrorCode::Superseded)
    );
    assert_eq!(sway::read_response(&mut third).outcome, Outcome::Ok);
    assert_eq!(sway.centre("HEADLESS-1"), BLUE);

    let limits = ServerConfig::default();
    let mut long = vec![b'x'; limits.max_request_len + 1];
    long.push(b'\n');
    let answer = sway.request(&long);
    assert_eq!(code(answer), Some(ErrorCode::TooLarge));

    // A client that says nothing is sent away after the read timeout.
    let mut silent = sway.connect_to_daemon();
    assert_eq!(
        code(sway::read_response(&mut silent)),
        Some(ErrorCode::Timeout)
    );

    // Every place taken by a client that says nothing: the next is turned away.
    let idle: Vec<_> = (0..limits.max_connections)
        .map(|_| sway.connect_to_daemon())
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    let mut one_too_many = sway.connect_to_daemon();
    assert_eq!(
        code(sway::read_response(&mut one_too_many)),
        Some(ErrorCode::Busy)
    );
    drop(idle);

    // `forbidden` needs a client of another user, which a test cannot be.
    ok(r#"{"cmd":"quit"}"#.into());
    assert_eq!(daemon.wait().code(), Some(0));
}

fn result<T: serde::de::DeserializeOwned>(answer: Response) -> T {
    serde_json::from_value(answer.result.expect("a result")).expect("the result's type")
}

fn code(answer: Response) -> Option<ErrorCode> {
    match answer.outcome {
        Outcome::Ok => None,
        Outcome::Error { code, .. } => Some(code),
    }
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn saved_and_configured_wallpapers_come_back_after_a_restart() {
    let sway = Sway::start("restart", &[(640, 480), (640, 480)]);
    let mut daemon = sway.daemon(&[]);
    sway.wait_for_state("HEADLESS-2", State::Shown);
    sway.veil_ok(&["set", "--input", arg(&Media::png(64, 48, RED))]);
    sway.veil_ok(&[
        "set",
        "--input",
        arg(&Media::png(64, 48, BLUE)),
        "--output",
        "HEADLESS-2",
    ]);
    sway.veil_ok(&["quit"]);
    assert_eq!(daemon.wait().code(), Some(0));

    let daemon = sway.daemon(&[]);
    sway.wait_for_state("HEADLESS-2", State::Shown);
    assert_eq!(sway.centre("HEADLESS-1"), RED);
    assert_eq!(sway.centre("HEADLESS-2"), BLUE);

    // Killed: what was saved is all there is.
    daemon.signal(Signal::KILL);
    drop(daemon);
    let daemon = sway.daemon(&[]);
    sway.wait_for_state("HEADLESS-2", State::Shown);
    assert_eq!(sway.centre("HEADLESS-1"), RED);
    assert_eq!(sway.centre("HEADLESS-2"), BLUE);
    assert_eq!(daemon.stop().code(), Some(0));

    // The configuration file wins for the outputs it names.
    std::fs::create_dir_all(sway.config_file().parent().expect("a directory"))
        .expect("create the configuration directory");
    std::fs::write(
        sway.config_file(),
        format!(
            "[output.HEADLESS-1]\ninput = {}\n",
            serde_json::to_string(arg(&Media::png(64, 48, GREEN))).expect("a TOML string")
        ),
    )
    .expect("write the configuration file");
    let check = sway.veil(&["config", "check"]);
    assert!(
        check.status.success(),
        "{}",
        String::from_utf8_lossy(&check.stderr)
    );
    let daemon = sway.daemon(&[]);
    sway.wait_for_state("HEADLESS-2", State::Shown);
    assert_eq!(sway.centre("HEADLESS-1"), GREEN);
    assert_eq!(sway.centre("HEADLESS-2"), BLUE);
    assert_eq!(daemon.stop().code(), Some(0));
}

#[test]
#[ignore = "needs a headless sway: nix develop .#headless -c cargo test --test headless -- --ignored"]
fn a_video_that_cannot_start_does_not_become_the_default() {
    let sway = Sway::start("default", &[(640, 480)]);
    let mut daemon = sway.daemon(&[]);
    sway.wait_for_state("HEADLESS-1", State::Shown);
    let red = Media::png(64, 48, RED);
    sway.veil_ok(&["set", "--input", arg(&red)]);
    let default = || sway.status().default.and_then(|wallpaper| wallpaper.input);

    // Named like a video and nothing GStreamer can play: it loads, and fails
    // to start (it must not become the default).
    let broken = Media::file("cannot-play.webm", &[0x5a; 4096]);
    let refused = sway.veil(&["set", "--input", arg(&broken)]);
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    assert_eq!(sway.centre("HEADLESS-1"), RED);
    assert_eq!(default().as_deref(), Some(arg(&red)));
    // An output connected later gets the default: the picture.
    let added = sway.create_output(640, 480, 640);
    sway.wait_for_centre(&added, RED);

    // A video that plays is the default once it is shown.
    let clip = ball();
    sway.veil_ok(&["set", "--input", arg(&clip)]);
    assert_eq!(default().as_deref(), Some(arg(&clip)));
    sway.veil_ok(&["quit"]);
    assert_eq!(daemon.wait().code(), Some(0));
    let saved = std::fs::read_to_string(sway.state_file()).expect("read the state file");
    assert!(saved.contains(arg(&clip)), "{saved}");
    assert!(!saved.contains(arg(&broken)), "{saved}");
}

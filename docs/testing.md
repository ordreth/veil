# Testing

veil is checked by automated tests only. A behaviour or a number counts as checked when a test or a script checks it; nothing is checked by hand before a commit or a release. What no test reaches yet is listed below, under "Not covered", and is found through use and reports ([`CONTRIBUTING.md`](../CONTRIBUTING.md)).

## Running the tests

| What | Command | Where it runs |
|---|---|---|
| Unit and integration tests | `cargo test` (inside `nix develop`) | CI, job `fmt / clippy / test / build` |
| The real daemon in a headless sway | `nix develop .#headless -c cargo test --test headless -- --ignored` | CI, job `headless compositor tests` |
| Short soaks: memory, threads and descriptors stay flat | `scripts/bench/soak.sh loop --duration 180` and `scripts/bench/soak.sh switch --switches 200` (inside `nix develop .#headless`, after `cargo build --release`) | CI, job `soak tests (short)`; fails on growth, a failed `veil set` or missing frames |
| The package, its plugins, completions and man pages | `nix flake check` | CI, job `nix flake check` |
| The oldest supported Rust | `nix develop .#msrv -c cargo check --locked --all-targets` | CI, job `build with the oldest supported Rust` |
| The release build against Debian 12's GStreamer | `scripts/release/build.sh --out dist` (Docker) | CI, job `release build (Debian 12 container)` |
| Licences and advisories of the dependencies | `cargo deny --locked check` | CI, job `cargo deny` |

`cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` run in CI as well. The headless tests are ignored by a plain `cargo test` because they need sway, `grim` and the other tools of the `headless` dev shell.

## What the tests cover

| Test | What it covers |
|---|---|
| unit tests (`src/`) | parsing (configuration file, IPC requests, the compositors' answers and events, among them Hyprland 0.56's and 0.57's), the scaling geometry, the state file, the colour coding of video frames and their conversion, the dmabuf formats a compositor takes |
| `tests/cli.rs` | the `veil` binary without a compositor: the commands against a test server or no daemon, exit statuses, a `veil daemon` that finds the socket taken |
| `tests/compositor.rs` | the Hyprland and sway backends against fake compositors: events, `j/monitors` and `j/clients`, sway's tree and subscriptions |
| `tests/ipc.rs`, `tests/ipc_fd_limit.rs` | the control socket end to end: framing, limits, timeouts, permissions, concurrent clients, and a daemon out of file descriptors |
| `tests/ipc_reference.rs` | [`ipc.md`](ipc.md) against the protocol types: every command, field, error code and example of the document |
| `tests/systemd.rs` | the systemd unit agrees with the binary: readiness, the exit statuses not worth a restart, the allocator setting |
| `tests/video_threads.rs` | video engines that are thrown away leave no threads or descriptors behind |
| `tests/notices.rs` | `THIRD-PARTY-NOTICES` matches `Cargo.lock` |
| `nix flake check` | the package builds and passes its tests; the GStreamer elements veil uses are found through the package's wrapper, where its registry goes, the allocator default; the completions load in bash, zsh, fish and nushell; the man pages have no roff errors |

The headless tests (`tests/headless.rs`; the harness is `tests/sway/`) start a headless sway of their own, run the real `veil daemon` and commands in it, and read the outputs with `grim`:

| Test | What it covers |
|---|---|
| `an_image_on_every_output_and_another_on_one` | `set` for every output and for one, `fit` bars in the colour given, `status` of an image, an image costs no CPU (at most 2 ticks in 3 s) |
| `outputs_come_and_go_without_touching_the_others` | a new output gets the default wallpaper; disable/enable and unplug leave the other outputs pixel-identical; a returning output gets its own wallpaper back |
| `two_videos_play_side_by_side` | two outputs, two clips, both move, each output reports its own stream |
| `the_latest_set_wins_on_each_output` | four `veil set` sent before the first is answered: the last per output is shown, the others succeed or are replaced, and no replaced one raises a desktop notification |
| `output_changes_while_videos_play_leave_them_playing` | known issue 1: with a video on two outputs, a scale change, a move, a disable and enable, a power off and on within a second and a new output leave the daemon running and every video moving |
| `a_video_pauses_behind_a_fullscreen_window` | the sway backend: a fullscreen `foot` pauses the video after a second (CPU a quarter of playing at most), closing it plays it again, five fullscreen toggles 200 ms apart pause nothing |
| `a_video_pauses_behind_a_fullscreen_window_on_hyprland` | the Hyprland backend against a fake Hyprland: `fullscreen>>1` pauses, `fullscreen>>0` plays |
| `a_locked_screen_and_a_powered_off_output_release_the_video` | `swaylock` and `power off`/`on` release the video (no bus thread, fewer descriptors, at most 2 CPU ticks in 2 s) and it plays again after unlock and power on |
| `the_daemon_answers_every_command_and_error_code` | the IPC table against the real daemon: every command, and every error code but `forbidden` (which needs a client of another user) |
| `saved_and_configured_wallpapers_come_back_after_a_restart` | `veil quit` and `kill -9`, then the saved wallpapers; a configuration file wins for the output it names, and `veil config check` accepts it |
| `a_video_that_cannot_start_does_not_become_the_default` | a `veil set` of a file GStreamer cannot play fails and changes nothing: not the screen, not the default, not the state file, and an output connected later gets the previous default |

## Numbers

Every number veil states about itself (CPU, memory, frames shown) comes from `scripts/bench/` ([`measurements.md`](measurements.md)): `bench.sh` for the cost of a wallpaper, `soak.sh` for growth over time. A change that could alter a number is benched against the build before it, alternating the two builds, and the result goes into `measurements.md` with its date and setup. The soaks fail on growth by themselves; `bench.sh` reports, and its numbers are compared by whoever runs it. The bench needs no GPU (a headless sway that renders on the CPU) unless it is given one (`--renderer`, `--render-device`).

## Not covered

### Checks to be turned into tests

These were checked once, by hand, when the feature was added (their procedures are in the history of `docs/manual-tests.md`), and are to become tests, one at a time:

- **Frames in dmabufs on an Intel or AMD GPU:** that a VA-API decoder's frames go to a headless sway in dmabufs, and that their colours match ffmpeg's decode, with `vulkan` (told the coding) and `gles2` (not told: BT.709 videos converted on the CPU). CI has no such GPU, so this will be an ignored test run where the GPU is.
- **The sway backend beyond fullscreen:** `--pause-on covered` with tiled windows, and switching workspaces.
- **The configuration file in a running daemon:** the precedence of flags, the file and `veil set` on several outputs, an output whose file is missing, `outputs = [...]`. The parsing and merging are unit tests (`src/config.rs`).
- **The systemd unit at run time:** readiness once every output shows its wallpaper, a restart after a crash, the watchdog, no restart for exit statuses 2, 4 and 5.
- **The Nix package outside the dev shell:** a video plays with the package's wrapper in an empty environment.
- **What each shell offers for completion** (the flake check only loads the scripts).
- **The failures of [`troubleshooting.md`](troubleshooting.md):** a missing plugin set, demuxer or decoder, a stale registry, another wallpaper program, an unknown output name, with the messages the guide quotes.
- **The installation on distributions:** the README's packages and build from source, and the release tarball with `install.sh`, on each distribution the README names. CI builds and tests on Debian 12 only.

### Not covered by any test

What CI and a headless sway cannot reach. Problems here are found by using veil (the developer uses it daily on Hyprland) and through reports:

- **Hyprland itself.** Its backend is tested against a fake Hyprland, not against Hyprland's rendering: whether videos play smoothly there ([known issue 6](known-issues.md)), whether a video in `fill` stands still at its loop point ([known issue 5](known-issues.md)), its lock screen, DPMS and logout.
- **Graphics cards CI does not have:** CI has no GPU, so hardware decoding (NVIDIA, Intel, AMD) and the dmabuf path are not tested there.
- **aarch64 with a compositor:** the release workflow builds for aarch64 and runs the tests there, but nothing has shown a wallpaper on it.
- **Compositors other than sway and Hyprland**, and the GStreamer of distributions other than Debian 12 ([known issue 8](known-issues.md)).

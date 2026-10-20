# Manual tests

Checks that need a compositor. Each was run by hand when its feature was added. Everything except the last section runs in a nested headless sway, which leaves the desktop session alone.

## What is automated

The core of these checks runs as tests: `tests/headless.rs` starts a headless sway of its own for each test (the harness is `tests/sway/`), runs the real `veil daemon` and commands in it, and reads the outputs with `grim`. They are ignored by a plain `cargo test` and run in CI as a job of their own:

```bash
nix develop .#headless -c cargo test --test headless -- --ignored
```

| Test | What it covers from the sections below |
|---|---|
| `an_image_on_every_output_and_another_on_one` | `set` for every output and for one, `fit` bars in the colour given, `status` of an image, an image costs no CPU (at most 2 ticks in 3 s) |
| `outputs_come_and_go_without_touching_the_others` | a new output gets the default wallpaper; disable/enable and unplug leave the other outputs pixel-identical; a returning output gets its own wallpaper back |
| `two_videos_play_side_by_side` | two outputs, two clips, both move, each output reports its own stream |
| `the_latest_set_wins_on_each_output` | four `veil set` sent before the first is answered: the last per output is shown, the others succeed or are replaced, and no replaced one raises a desktop notification |
| `an_output_change_while_a_video_plays_kills_the_daemon` | known issue 1: a scale change kills the daemon with SIGSEGV (the test fails once the upstream bug is fixed) |
| `a_video_pauses_behind_a_fullscreen_window` | the sway backend: a fullscreen `foot` pauses the video after a second (CPU a quarter of playing at most), closing it plays it again, five fullscreen toggles 200 ms apart pause nothing |
| `a_video_pauses_behind_a_fullscreen_window_on_hyprland` | the Hyprland backend against a fake Hyprland written in the test (in place of `socat`): `fullscreen>>1` pauses, `fullscreen>>0` plays |
| `a_locked_screen_and_a_powered_off_output_release_the_video` | `swaylock` and `power off`/`on` release the video (fewer threads and descriptors, at most 2 CPU ticks in 2 s) and it plays again after unlock and power on |
| `the_daemon_answers_every_command_and_error_code` | the IPC table against the real daemon: every command, and every error code but `forbidden` (which needs a client of another user) |
| `saved_and_configured_wallpapers_come_back_after_a_restart` | `veil quit` and `kill -9`, then the saved wallpapers; a configuration file wins for the output it names, and `veil config check` accepts it |
| `a_video_that_cannot_start_does_not_become_the_default` | a `veil set` of a file GStreamer cannot play fails and changes nothing: not the screen, not the default (`veil status`), not the state file, and an output connected later gets the previous default; a video that plays becomes the default |

Still by hand: the rows below that need timing or a person to judge (frame-callback counts, peak memory, `--pause-on covered`, the systemd unit, the package outside the dev shell), and the last section, in a real Hyprland session. CPU and memory over time are measured by `scripts/bench/` ([`measurements.md`](measurements.md)).

## Setup: a headless sway

Inside `nix develop`, with a private runtime directory (a short path: Unix socket paths are limited to 108 bytes):

```bash
export XDG_RUNTIME_DIR=/run/user/$(id -u)/veil-test
mkdir -m 700 -p "$XDG_RUNTIME_DIR"
unset WAYLAND_DISPLAY SWAYSOCK HYPRLAND_INSTANCE_SIGNATURE
# No desktop notifications from the test daemon on the real desktop.
export DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent
# A registry of our own: a stale ~/.cache/gstreamer-1.0 cache can hide plugins.
export GST_REGISTRY=$XDG_RUNTIME_DIR/gst-registry.bin
# Saved wallpapers of our own: the test daemon must not replace the real ones.
export XDG_STATE_HOME=$XDG_RUNTIME_DIR/state
# No configuration file but the test's own: the daemon reads ~/.config/veil/config.toml.
export XDG_CONFIG_HOME=$XDG_RUNTIME_DIR/config
printf 'output HEADLESS-1 resolution 1920x1080 scale 1\n' > "$XDG_RUNTIME_DIR/sway.conf"
WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 LIBSEAT_BACKEND=noop \
  nix shell nixpkgs#sway -c sway -c "$XDG_RUNTIME_DIR/sway.conf" &
export WAYLAND_DISPLAY=wayland-1   # the socket sway created in $XDG_RUNTIME_DIR
export SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*.sock)
```

A test clip: `gst-launch-1.0 videotestsrc pattern=ball num-buffers=300 ! video/x-raw,width=1280,height=720,framerate=30/1 ! videoconvert ! vp8enc deadline=1 ! webmmux ! filesink location=clip.webm`.

Measuring: the daemon's CPU is `utime + stime` from `/proc/<pid>/stat` (fields 14 and 15, in ticks of 10 ms) before and after a `sleep`; its threads and file descriptors are the entries of `/proc/<pid>/task` and `/proc/<pid>/fd`. Whether the picture moves: a few `grim` screenshots 0.3 s apart, and the number of distinct ones (`magick identify -format '%#\n' shot*.png | sort -u | wc -l`).

When counting frame callbacks with `WAYLAND_DEBUG=1`, leave out the lines that start with `{GStreamer display queue}`: those are the video sink's own, for its subsurface, not veil's.

## The Nix package outside the dev shell

The package must play a video with nothing from the dev shell. Build it with `nix build`, start the headless sway as above, and run the daemon with an almost empty environment and a home of its own (so the registry it writes is visible, and the real one is not used):

```bash
mkdir -p /tmp/veil-home
env -i HOME=/tmp/veil-home PATH=/run/current-system/sw/bin \
  XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" WAYLAND_DISPLAY=wayland-1 \
  DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent \
  ./result/bin/veil daemon --input clip.mp4 --mode fit
```

| Check | Expected |
|---|---|
| eight `grim` screenshots 0.25 s apart, after 5 s | all different (with an animated GIF of four frames shown 0.2 s each: four different) |
| `/tmp/veil-home/.cache/veil/gstreamer-registry.bin` | exists, about 1.5 MB (the whole plugin set; the core alone is about 7 KB) |
| time from "binding to output" to the next log line | about 2 s on the first start (the plugins are scanned), well under a second after |
| `kill -TERM` | exit status 0 |

Without the wrapper (a bare binary), the same command exits with status 1 and `no element "videoconvert"`. The warning `Could not bind to zwp_linux_dmabuf_v1` comes from the pixman renderer of the headless sway.

## Automatic pause behind a fullscreen window

Needs a compositor whose IPC veil follows. A fake Hyprland is enough: set `HYPRLAND_INSTANCE_SIGNATURE=FAKE` and serve the two sockets in `$XDG_RUNTIME_DIR/hypr/FAKE/` with `socat`:

```bash
H=$XDG_RUNTIME_DIR/hypr/FAKE; mkdir -p "$H"
# Requests: answer j/monitors and j/clients from files.
cat > "$H/answer.sh" <<'SH'
#!/usr/bin/env bash
case "$(head -c 9)" in j/monitor) cat "$(dirname "$0")/monitors.json" ;; j/clients) cat "$(dirname "$0")/clients.json" ;; esac
SH
chmod +x "$H/answer.sh"
echo '[{"name":"HEADLESS-1","x":0,"y":0,"width":1920,"height":1080,"scale":1.0,"activeWorkspace":{"id":1,"name":"1"}}]' > "$H/monitors.json"
echo '[]' > "$H/clients.json"
socat UNIX-LISTEN:"$H/.socket.sock",fork EXEC:"$H/answer.sh" &
# Events: whatever is appended to events.txt.
: > events.txt
socat UNIX-LISTEN:"$H/.socket2.sock",fork SYSTEM:'tail -n0 -f events.txt' &
```

A fullscreen window: put `[{"mapped":true,"at":[0,0],"size":[1920,1080],"workspace":{"id":1,"name":"1"},"fullscreen":2}]` into `clients.json` and append `fullscreen>>1` to `events.txt`; back with `[]` and `fullscreen>>0`. Expected with `veil daemon --input clip.webm`: "pausing it" one second after the event, CPU near 0, identical screenshots; "playing it" at once when the window goes. Toggling faster than once a second pauses nothing. Killing both `socat` processes is a lost IPC: the video plays.

## The sway backend

The headless sway is a real sway: keep `SWAYSOCK` set and `HYPRLAND_INSTANCE_SIGNATURE` unset, and the daemon follows it (`following sway through ...`). With `RUST_LOG=veil=debug veil daemon --input clip.webm`:

| Action | Expected |
|---|---|
| `swaymsg exec "foot -a one"` | "now cover it" (one tiled window, title bar included, covers the output); after 2 s the video is released (sway takes no frames of a covered surface) |
| `swaymsg '[app_id=one] fullscreen enable'` | "shows a fullscreen window", "pausing it" a second later |
| `fullscreen disable` | "playing it" at once |
| `swaymsg workspace 2` (empty), then `workspace 1` | plays on workspace 2, paused again on 1 |
| fullscreen on and off five times, 200 ms apart | no pause |
| `--pause-on covered` and `foot -a one` | paused after a second, not released |

To record replies for the tests: `swaymsg -t get_outputs -r`, `swaymsg -t get_tree -r` and `swaymsg -t subscribe -m '["window","workspace","output"]'` (with `layout tabbed`, `layout stacking`, `floating enable`, `move scratchpad` for the other shapes).

## A video nobody is shown

`veil daemon --input clip.webm` (and again with `--mode fit` and a 4:3 clip, so that the surface shows around the video):

| Action | Expected |
|---|---|
| `nix shell nixpkgs#swaylock -c swaylock -c 000000 &`, wait 4 s | "has not been shown for 2 s", CPU 0, about 7 threads and 28 file descriptors instead of 14 and 63 |
| `kill -USR1 <swaylock pid>` (swaylock unlocks on SIGUSR1) | "drawn again; starting its video", the video moves again |
| `swaymsg output HEADLESS-1 power off`, wait 4 s, `power on` | stopped while off, plays after power on, the daemon survives |
| `swaymsg exec foot`, `swaymsg '[app_id=foot] fullscreen enable'`, wait 4 s, `swaymsg '[app_id=foot] kill'` | stopped while the window covers it (sway sends a covered video no frame callbacks), plays when it is closed |
| `power off`, then `power on` within 1 s | the daemon dies in GStreamer (known issue 1): the video was not stopped yet |
| Any of the above with `--release-unseen-after 0` | nothing is stopped |

In the log, at no time while the video is shown: "has not been shown". With `RUST_LOG=veil=info,veil::wayland::visibility=trace` the frame counts are logged every second: `rendered` grows while the video is shown, `dropped` while it is not.

## Saved wallpapers

Two outputs (`swaymsg create_output` adds `HEADLESS-2`), an image and a clip, and `XDG_STATE_HOME` from the setup. Look at each output's centre pixel with `grim -o NAME shot.png` and `magick shot.png -format '%[pixel:p{960,540}]' info:`, and at `$XDG_STATE_HOME/veil/state.json`.

| Action | Expected |
|---|---|
| `veil daemon` (nothing saved) | "no wallpaper saved", black on both, no state file |
| `veil set -i red.png --bg-color '#00ff00'`, `veil set -i clip.webm -o HEADLESS-2`, `veil set -i blue.png -o HEADLESS-1 --no-persist` | blue and the clip; the file has the red default, red for `HEADLESS-1`, the clip for `HEADLESS-2` |
| `kill -9` the daemon, `veil daemon` | "showing the saved wallpapers": red and the clip, moving |
| `swaymsg output HEADLESS-2 scale 2` (known issue 1), then `scale 1` and `veil daemon` | exit status 139, then red and the clip again |
| An image on `HEADLESS-2` (no video while outputs are added), stop, add an entry for `HEADLESS-3` with another image to the file by hand, start, `swaymsg create_output` twice | `HEADLESS-3` shows its own image, `HEADLESS-4` the red default |
| Stop, move `red.png` away, start | a warning that the saved wallpaper of each output cannot be shown; green (its `--bg-color`) where it was red |
| Stop, `veil daemon --input blue.png`, stop, `veil daemon` | blue on both, both times; the file has only the blue default |
| Stop, write `{"version":1,"outp` into the file, start, `veil set -i red.png -o HEADLESS-1` | a warning that the file cannot be used, black; then the file has red for `HEADLESS-1` only |

Done on 2026-10-16 with every row as expected.

## The commands for a running daemon

Two outputs (`swaymsg create_output`, then `swaymsg output HEADLESS-2 resolution 1280x720 position 1920 0`), a clip and an image, `XDG_STATE_HOME` from the setup, and `veil daemon --input clip.webm` started inside `nix develop` (outside it the debug build finds no GStreamer plugins). CPU ticks of the daemon are `utime+stime` from `/proc/<pid>/stat` over 3 seconds; raw answers come from `socat` as in `docs/ipc.md`.

| Action | Expected |
|---|---|
| `veil status` before the daemon runs | exit status 3, "no veil daemon is running", no notification attempt |
| `veil ping`, `veil status`, `veil status --json`, `veil outputs`, `veil outputs --json` | `pong`; both outputs `playing` with the clip, the surface size, `video 640x480, 30 fps`; the same as JSON on one line; both outputs `connected, wallpaper` with sway's descriptions |
| `veil pause` | both `paused (by veil pause)`; CPU ticks drop to about 0; two screenshots 0.4 s apart are identical |
| `veil toggle` | prints `playing`; both play; two screenshots differ |
| `veil pause -o HEADLESS-2`, then `veil toggle`, then `veil toggle -o HEADLESS-1`, then `veil resume` | only HEADLESS-2 paused; `paused` (both held); `playing` (HEADLESS-1 plays); both play |
| `veil pause -o NOPE`; raw `pause` for `NOPE`, `set` of a missing file and of `/tmp`, `toggle` twice | exit status 1 and the list of outputs; codes `unknown_output`, `not_found`, `unsupported_media`; `"result":{"paused":true}`, then `false` |
| A fullscreen `foot` on HEADLESS-1; `veil pause -o HEADLESS-1`; `veil resume -o HEADLESS-1`; close `foot` | `paused (hidden by a window)`; `(by veil pause, hidden by a window)`; `(hidden by a window)` again; `playing` |
| `veil clear -o HEADLESS-2 --bg-color '#ff0000'` | HEADLESS-2 red, `showing the background colour #ff0000`; the state file has version 2 and an entry for HEADLESS-2 without `input` |
| `veil quit`, then `veil daemon` | `veil quit` returns once the daemon has exited with status 0, the socket is gone (its lock file stays); the new daemon plays the clip on HEADLESS-1 and shows red on HEADLESS-2 |
| `veil clear`, then `swaymsg create_output` | both black, the default is the colour (no `input`); HEADLESS-3 shows black |
| `veil set -i image.png -o HEADLESS-3 --mode fit`, then `veil stop` | the image on HEADLESS-3; the daemon ends with status 0 |

Done on 2026-10-16 with every row as expected: 27 CPU ticks per 3 seconds playing, 1 paused; `veil quit` returned 87 ms after the request.

## The examples of `docs/ipc.md`

Two outputs (`swaymsg create_output`), a picture, a clip and a text file, and `veil daemon` (the release build) started inside `nix develop .#headless` with the runtime, state, configuration and cache directories from the setup and a session bus that leads nowhere. Every request of `docs/ipc.md` was sent with `socat` as written there (with the test's paths and output names), and its answer compared with the one the document shows: `set` of a picture for every output, for one output with `fit` and a colour, of a clip with `"persist":false`, of a relative path, a missing file, a text file, a directory and an unknown output; unknown fields, an unknown command, `mode`, a colour, `status` with a field, `{}`, `not json` and an empty `output`; `status`, `outputs`, `pause`, `toggle` twice, `resume`, `clear` with and without `#`, `pause` for an unknown output; a `set` of a clip replaced by a `set` of a picture 50 ms later; a client that sends nothing for 4 s, a request of 70000 bytes, a request with CRLF, one without a newline and two on one line; `quit`. Then `veil status`, `veil status --json`, `veil outputs`, `veil outputs --json` and `veil toggle` twice, and `veil ping` after the `quit`.

Done on 2026-10-19: every answer as the document shows it, apart from paths, names, the pid and the counts (`superseded` for the replaced `set` and then `ok`; `timeout`, `too_large`; `ok` for the CRLF line, the line without a newline, and the first of the two lines, the second never read; the daemon ended with status 0, its socket gone and its lock file kept; `veil ping` then exited with status 3). After the `too_large` answer `socat` reported `Connection reset by peer`, since the daemon closed the connection with the rest of the request unread; the document says so.

## The configuration file

Two outputs of 1920x1080 (`swaymsg create_output`, then `swaymsg output HEADLESS-2 resolution 1920x1080 position 1920 0`), created before the daemon starts, and `XDG_STATE_HOME` and `XDG_CONFIG_HOME` from the setup (without the latter the test daemon reads your own configuration file). Pictures of 960x1080, so `fit` leaves bars: `red.png` next to the configuration file, `blue.png` elsewhere, and a clip. Read each output's centre pixel and its corner (`10,10`) as in "Saved wallpapers".

| Action | Expected |
|---|---|
| A file with `[defaults]` `mode = "fit"`, `bg_color = "#00ff00"`; `[output.HEADLESS-1]` `input = "red.png"` (relative); `[output."*"]` `input = "$XDG_RUNTIME_DIR/media/blue.png"`, `bg_color = "#ffff00"`. `veil config check`, then `veil daemon` with no flags | the check prints both sections with absolute paths, `image`, `fit`; HEADLESS-1 red with green bars, HEADLESS-2 blue with yellow bars; the log names the file; the state file has blue as the default and red for HEADLESS-1 |
| `veil set -i blue.png -o HEADLESS-1`, `veil quit`, `veil daemon` | blue with green bars (the file's defaults are `veil set`'s); after the restart red again (the file wins) |
| Start once with `--input blue.png --bg-color '#ff00ff'`, stop; a file with only `[output.HEADLESS-1]`, start | HEADLESS-1 red (`fill`, no bars), HEADLESS-2 the saved blue with magenta bars; the log says "and the saved ones on other outputs" |
| A file with `fit`, HEADLESS-1 red, and `[output.HEADLESS-2]` with only `bg_color = "#ff00ff"`; `veil daemon --mode stretch` | HEADLESS-1 red to the corners (the flag wins), HEADLESS-2 all magenta |
| `veil daemon --input blue.png` with the same file | blue on both, with the file's green bars; "the wallpapers of the configuration file are not used" |
| `[output.HEADLESS-1]` the clip, `[output.HEADLESS-2]` `input = "/nonexistent/x.png"`, `bg_color = "#00ffff"`; `veil config check`, then `veil daemon` | the check exits 1 naming `[output."HEADLESS-2"]`; the daemon starts, the clip moves on HEADLESS-1, HEADLESS-2 is cyan, and a warning names the missing file |
| `[defaults]` `outputs = ["HEADLESS-2"]` | `veil outputs`: HEADLESS-1 `no wallpaper (not one of the daemon's outputs)` |
| A misspelt key (`imput`) | `veil daemon` exits with status 2 and ``config.toml:4:1: unknown field `imput`, expected one of ...`` |

Done on 2026-10-16 with every row as expected.

## The systemd unit

The daemon runs as a transient unit of your own systemd user manager, with the properties of `contrib/systemd/veil.service` and the environment of the headless sway. `systemctl --user` finds its manager through `XDG_RUNTIME_DIR`, so give it the real one:

```bash
uctl() { env XDG_RUNTIME_DIR=/run/user/$(id -u) DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/$(id -u)/bus "$@"; }
start_unit() {   # blocks until READY=1
  uctl systemd-run --user --unit=veil-test -p Type=notify -p Restart=on-failure -p RestartSec=2 \
    -p 'RestartPreventExitStatus=2 4 5' -p WatchdogSec=30 -p UnsetEnvironment=HYPRLAND_INSTANCE_SIGNATURE \
    --setenv=XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR --setenv=WAYLAND_DISPLAY=$WAYLAND_DISPLAY \
    --setenv=SWAYSOCK=$SWAYSOCK --setenv=XDG_STATE_HOME=$XDG_STATE_HOME \
    --setenv=DBUS_SESSION_BUS_ADDRESS=$DBUS_SESSION_BUS_ADDRESS \
    --setenv=GST_PLUGIN_SYSTEM_PATH_1_0=$GST_PLUGIN_SYSTEM_PATH_1_0 --setenv=GST_REGISTRY=$GST_REGISTRY \
    "$@"
}
start_unit $PWD/target/release/veil daemon --input clip.webm
uctl systemctl --user show veil-test -p ActiveState -p StatusText -p NRestarts -p MainPID
uctl journalctl --user -u veil-test
uctl systemctl --user stop veil-test; uctl systemctl --user reset-failed veil-test
```

`systemd-analyze --user verify` checks the unit file itself (with the package's unit: `result/share/systemd/user/veil.service`; the file in `contrib/` names `/usr/bin/veil`, which NixOS does not have).

| Action | Expected |
|---|---|
| Two outputs, `start_unit ... --input clip.webm` | `systemd-run` returns once both outputs show the video (screenshots right after it returns); the journal has the second "has its first frame" (`RUST_LOG=veil=debug`) just before `Started`; `StatusText` names both outputs |
| The same with a file of random bytes named `.webm` | started all the same; `StatusText` has `not shown on` each output with the reason |
| `start_unit ... daemon` with nothing saved, `veil set -i clip.webm`, `kill -9` the main PID | active again with another PID after about 2.5 s, the clip moving; `NRestarts=1` |
| `kill -SEGV` the main PID once | nothing happens (the Rust runtime takes it); a second one kills it |
| `swaymsg output HEADLESS-1 scale 2` with the clip playing | `code=dumped, status=11/SEGV` in the journal, active again after about 3 s with the clip |
| A second unit with the same environment | exit status 4, `NRestarts=0`, the first one untouched |
| `-p WatchdogSec=3`, wait 5 s, `kill -STOP` the main PID | still up after 5 s; then `Watchdog timeout`, killed with SIGABRT, restarted |
| `systemctl --user stop` | status 0 in a fraction of a second |
| Kill the sway | exit status 5, not restarted |
| The journal | no escape codes, one time stamp per line, `veil 0.1.0 (GStreamer 1.28.7)` first |

Done on 2026-10-16 with every row as expected (systemd 261).

## Shell completions and man pages

`nix flake check` (the check `completions`) loads the scripts the package installs and lints its man pages, but does not ask a shell what it offers. That was done by hand, with the scripts of `veil completions` in a scratch directory; zsh, fish and nushell come from `nix shell nixpkgs#zsh nixpkgs#fish nixpkgs#nushell`.

- **bash:** `source veil.bash`, then set `COMP_WORDS`, `COMP_CWORD`, `COMP_LINE` and `COMP_POINT` for a command line and call `_veil veil "$cur" "$prev"`; `COMPREPLY` is what bash offers (it prints a `compopt` complaint outside a real completion, which is harmless).
- **zsh:** an interactive `zsh -f -i` in a `zpty` (module `zsh/zpty`), with the script as `_veil` in `fpath`, `compinit`, and `compadd` wrapped to print its matches (`builtin compadd -A h "$@"; print -r -- "HITS ${h[*]}"`); write the command line and a TAB, and read the `HITS` lines (they come on the same line as the echoed input).
- **fish:** `fish -c 'source veil.fish; complete -C "veil daemon --pause-on "'`.
- **nushell:** `nu -c 'source veil.nu; scope commands | where name starts-with "veil"'`; for the autoload directory, an interactive `nu -e '...; exit'` with a home of its own.

| Command line | Expected in every shell |
|---|---|
| `veil ` | the commands (bash, zsh and fish also `stop`), no `man-pages` |
| `veil daemon --pause-on ` | `never fullscreen covered` |
| `veil set --` | `--output --input --mode --bg-color --no-persist` (and `--help`) |
| `veil set --input <the start of a file name>` | the file |
| `veil set --output ` | nothing |
| `veil config ` | `check` (and `help`) |
| `veil completions ` | the four shells (fish: file names, its generator skips positional arguments) |

`man -l veil.1` (and the other pages) to read them; `nix shell nixpkgs#mandoc -c mandoc -T lint -W error *.1` reports nothing. Done on 2026-10-19 with every row as expected (bash 5.3, zsh 5.9, fish 4.8.1, nushell 0.115.1).

## Troubleshooting

Every failure in [`troubleshooting.md`](troubleshooting.md) was produced on purpose, in a headless sway as above (the release build, one output of 1920x1080, a fresh `GST_REGISTRY` per plugin set), to quote what the daemon really says. Done on 2026-10-19 with GStreamer 1.28.7. To do it again:

| Failure | How it was produced |
|---|---|
| A plugin set missing | `GST_PLUGIN_SYSTEM_PATH_1_0` of the dev shell without the entry of `gst-plugins-bad` (then `-good`, `-base`, `gst-libav`): `veil daemon --input clip.webm`, `veil daemon --input picture.png` and then `veil set --input clip.webm`. |
| A demuxer or decoder missing | `GST_PLUGIN_FEATURE_RANK=matroskademux:0` on the daemon, a WebM clip; `avdemux_gif:0,avdec_gif:0`, an animated GIF (`magick -size 64x48 xc:red xc:green xc:blue -set delay 20 -loop 0 anim.gif`). |
| A file GStreamer cannot read | 200 KB of `/dev/urandom` named `.webm`, as `--input` and with `veil set` (it once became the default; now guarded by `a_video_that_cannot_start_does_not_become_the_default`). |
| Forced software decoding | `GST_PLUGIN_FEATURE_RANK=nvh264dec:0` (and `:NONE`) with `h264-1080p30.mp4`, `nvvp9dec:0` with `vp9-1080p60.webm` (the clips of `scripts/bench/clips.sh`); the `video decoder:` line of the log. |
| A stale registry | A copy of the base plugins' directory on the plugin path; `libgstvideoconvertscale.so` damaged with `dd if=/dev/urandom of=... bs=4096 count=1 conv=notrunc` and given back its time stamp with `touch -r`; one start; the file repaired the same way; a second start with the same registry; `gst-inspect-1.0 -b` with the daemon's environment; a third start after deleting the registry. |
| The NVIDIA decoders kept out | A start with `CUDA_VISIBLE_DEVICES=` and a fresh registry, then one without it and the same registry. |
| Plugins of another GStreamer | `GST_PLUGIN_PATH` set to the `lib/gstreamer-1.0` directories of a GStreamer 1.26.11 `bad` from the Nix store (and of a `base`, which was a 32-bit build and did not load at all), with `GST_DEBUG=GST_PLUGIN_LOADING:4`; then a start without it and the same registry; then a fresh registry. |
| The display context | A debug build with the field of `engine::video::display_context` renamed to `display`, and one with a name GStreamer does not know (`src/engine/video.rs` restored from a copy afterwards); `WAYLAND_DEBUG=1`, `swaymsg -t get_tree` and a screenshot. |
| No buffer under a video | A debug build without the three `fill` calls of `engine/video.rs`, sway's background set to a colour (`swaymsg output HEADLESS-1 bg '#00ff00' solid_color`), screenshots and `veil status`. |
| Another wallpaper program | `swaybg -c '#0000ff' -m solid_color` (`nix shell nixpkgs#swaybg`) started after veil, then a `veil set`, then `swaybg` stopped; and `swaybg` started before veil. |
| Output names | `veil daemon --output DP-9`, `--output DP-9 --output HEADLESS-1`, `--output headless-1`; `veil set --output DP-9`, `veil pause --output DP-9`. |

## Governance files

veil takes no issues or pull requests, so there are no issue forms or templates (removed 2026-10-20; `CONTRIBUTING.md` says so). Reports go to GitHub Discussions, category "Reports". The links in `SECURITY.md`, `CONTRIBUTING.md` and the README to GitHub pages work only once the repository is public.

**Found while writing `SECURITY.md`: a video whose path has a `#`, `?` or `%` in it could not be played.** The path went into the `file://` URI as it was. Checked in a headless sway as above (a 320x180 VP8 clip copied under several names, `veil set` of each, then `veil status --json`): the build before the fix answered `the video cannot be played: Resource not found` for `wall#2.webm` and `sea?.webm`, and `Could not open resource for reading` for `100%.webm`, while `a b.webm` played; with `engine::video::file_uri` all of them play. The unit tests `the_uri_of_a_path_is_escaped` and `a_video_whose_name_is_not_a_uri_plays` fail on the old code.

**Left to the project owner, on GitHub:**

- Turn on private vulnerability reporting (Settings, Code security, "Private vulnerability reporting"), which `SECURITY.md` links to. Until then the link leads nowhere.
- Turn on Discussions, with one category, "Reports" (slug `reports`), and turn off Issues. Restrict or auto-close pull requests.

## The README's installation and quick start

**From source, per distribution** (2026-10-19). One container per distribution with Docker (`archlinux:latest`, `debian:stable` = Debian 13, `ubuntu:latest` = Ubuntu 26.04.1, `fedora:latest` = Fedora 44), the tree of `git archive HEAD` mounted read-only, the media directory, and the host directory `/run/user/1000/vd` mounted as the container's `/run/veil`. In each, as root: the README's package list (Debian and Ubuntu with `--no-install-recommends`, plus `gstreamer1.0-tools` for `gst-inspect-1.0`; Debian with rustup), `cargo build --release --locked`, and `gst-inspect-1.0 --exists` for every element veil uses. Then, on the host, a headless sway with its runtime directory in `/run/user/1000/vd`, and the container's binary run against it as uid 1000 (`docker exec -u 1000:1000 -e XDG_RUNTIME_DIR=/run/veil -e WAYLAND_DISPLAY=wayland-1 ...`): `veil daemon`, `veil set` of a PNG (screenshot with 13 colours, unchanged 0.5 s later), of an H.264 MP4, a VP9 WebM and an animated GIF (three screenshots 0.4 s apart all different, `veil status --json` `playing` with frames rendered), `veil status`, `veil quit` (status 0). All four built (5 to 6.5 minutes, release with LTO) and passed. The decoders, all software in a container: `avdec_h264`, `vp9dec`, `avdec_gif`; on Fedora `openh264dec`, because Fedora's `gst-libav` (built on `ffmpeg-free`) has no `avdec_h264` and no `avdec_h265`. Ubuntu was built a second time with Ubuntu's own `cargo` (Rust 1.93) instead of rustup. Debian's Rust is 1.85, too old for veil's 1.89. VA-API elements could not be checked (no GPU in the containers); which package holds `libgstva.so` was read from the package databases (`gst-plugin-va` on Arch, the `bad` package elsewhere).

**Found: the `force-aspect-ratio` warning named the wrong version and a problem that does not exist.** Debian's GStreamer 1.26.2 has no `force-aspect-ratio` on `waylandsink`, and veil warned `GStreamer < 1.24?; stretch and fill may be letterboxed`. The property is from 1.28 (`Since: 1.28` in `gstwaylandsink.c`). A 4:3 VP9 clip on a 16:9 output with `--bg-color '#ff0000'`: the share of exactly red pixels in the leftmost 200 columns was 0 in `fill` and `stretch` and 1 in `fit`, on Debian (1.26.2) exactly as on Arch (1.28.8). The sink before 1.28 scales to the render rectangle by itself. The line is now a `debug` one, and the README and the troubleshooting guide no longer list it as a problem.

**The install block for one user** (in the Arch container, as a user without privileges): the README's `install`, `sed` and `veil man-pages` lines, then `systemd-analyze --user verify` on the unit (passes; `ExecStart=/home/tester/.local/bin/veil daemon`), the 14 man pages, and `veil completions bash` into its directory.

**Nix:** `nix profile add --profile <scratch directory> .` (the README's command, against a profile of its own instead of the user's) installs `bin/veil` and `share/{bash-completion,doc,fish,man,nushell,systemd,zsh}`; `nix run . -- --version` prints `veil 0.1.0`. The NixOS lines of the README in a minimal `nixosSystem` (with the flake as `veil`), built up to `environment.etc."systemd/user".source`: `veil.service` is the package's unit, with the package's `ExecStart`, and `graphical-session.target.wants/veil.service` exists. Not switched to on a running NixOS. `nix run github:ordreth/veil` and `nix profile add github:...` need the repository to be public.

**The quick start:** the three commands as written, with the release build first on `PATH`, in a headless sway as in "Setup" above, with a video as `--input`.

## The release tarball

The release workflow runs only on GitHub, on a tag, so its parts were checked one by one (2026-10-19, x86_64 only: there was no ARM machine and no `qemu`).

**The build.** `scripts/release/build.sh` twice (Docker 29.7, the pinned `rust:1.98.1-slim-bookworm`): with the tests, every test passed against Debian 12's GStreamer 1.22.0 (411 unit tests in 12.5 s, so the GStreamer ones ran, and every integration test file); the binary needs `libwayland-client`, `libgstreamer-1.0`, `libgstvideo-1.0`, GLib, `libgcc_s`, `libm` and `libc`, and GLIBC_2.34 at most. The two tarballs differ only in the binary: 779 bytes of machine code (instructions in another order) and its build id; with `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1`, two more builds still differed in 573 bytes. Everything else in the tarball was byte for byte the same.

**The release notes.** `scripts/release/notes.sh` in a scratch copy of the tree: refused `v0.1.0` while `CHANGELOG.md` had only `[Unreleased]`, `0.1.0` and `v0.1` (no `v`, no patch number), `v0.2.0` (not the version of `Cargo.toml`), a section without a link reference, a version in `Cargo.toml` that `Cargo.lock` does not have yet; with `## [0.1.0] - 2026-10-20` and its link reference it printed the section (32 lines, from `### Added` to the last known issue). `actionlint` 1.7.12 and `shellcheck` 0.11 found nothing in the workflows and the scripts (`install.sh` also as POSIX `sh`).

**On each distribution.** The headless sway of "Setup" on the host, with its runtime directory in `/run/user/1000/vd`, and one container per distribution (`debian:12`, `debian:13`, `ubuntu:22.04`, `ubuntu:24.04`, `fedora:44`, `archlinux:latest`), with that directory as `/run/veil` and the media read-only. In each, as root: the README's runtime packages (Debian and Ubuntu with `--no-install-recommends`), the tarball extracted, `./install.sh` (into `/usr/local`). Then, as uid 1000 (`docker exec -u 1000:1000 -e XDG_RUNTIME_DIR=/run/veil -e WAYLAND_DISPLAY=wayland-1`, a home, state and configuration directories of its own, a session bus that leads nowhere): `veil daemon`, and `veil set` of a 1080p PNG (two `grim` screenshots, one picture), the H.264 1080p30 MP4 and the VP9 1080p60 WebM of `scripts/bench/clips.sh`, an animated GIF (seven colours, 0.3 s each) and the same animation as VP9 WebM (three screenshots about 0.5 s apart: three different pictures, `veil status --json` `playing`), then `veil status` and `veil quit` (status 0). Results are in the README ("Prebuilt binaries") and in known issue 8: everything passed on Debian 13, Fedora 44 and Arch; on Debian 12, Ubuntu 22.04 and 24.04 everything but the GIF, and Ubuntu 24.04 dropped frames of the 1080p clips. The decoders were the software ones (`avdec_h264`, on Fedora `openh264dec`; `vp9dec`; `avdec_gif`).

**What the GIF does without veil:** `gst-launch-1.0 -v filesrc location=anim.gif ! decodebin ! identity silent=false ! fakesink sync=true` in the container of the distribution; the time stamps of the buffers (`pts:`) and whether it prerolls. The CPU of the daemon: `utime + stime` of `/proc/<pid>/stat` over 5 s, and per thread from `/proc/<pid>/task/*/stat` with its `comm`. Plain `waylandsink` for comparison: `gst-launch-1.0 -v filesrc location=h264-1080p30.mp4 ! decodebin ! videoconvert ! fpsdisplaysink video-sink=waylandsink text-overlay=false`, which prints `rendered` and `dropped`.

**`install.sh`.** For one user (`--prefix` a scratch home's `.local`): 29 files, the unit's `ExecStart` pointing at that prefix (`systemd-analyze --user verify` passes), `man -w veil-set` finds the page with that `MANPATH`, fish (`complete -C "veil daemon --pause-on "` with that `HOME`) offers the three values from `~/.local/share/fish/vendor_completions.d`, and nushell lists `~/.local/share/nushell/vendor/autoload` among `$nu.vendor-autoload-dirs`. A prefix with `&` and `|` in it is written into `ExecStart` as it is; one with a space, `%` or a quote is refused (status 2), as are a relative prefix and an unknown argument. `--uninstall` removed all 29 files. The same with `dash`.

**Left to the project owner, on GitHub** (the repository was not public on 2026-10-19; GitHub's ARM runners and attestations are free only for public repositories):

- Push a pre-release tag first, to see the workflow run before 1.0: set `version = "1.0.0-rc.1"` in `Cargo.toml` (and `cargo build`, for `Cargo.lock`), give `CHANGELOG.md` a `## [1.0.0-rc.1] - <date>` section and its link reference, commit, `git tag v1.0.0-rc.1 && git push origin v1.0.0-rc.1`. Expected: the jobs `notes`, `build (x86_64)`, `build (aarch64)` and `publish` pass, and a pre-release appears with two tarballs and `SHA256SUMS`, the changelog's section as its text. Then delete the pre-release and its tag if it should not stay.
- On an ARM machine (a Raspberry Pi 4 or 5 with a Wayland compositor, or an ARM laptop), install the aarch64 tarball and show a picture and a video; nothing has run it with a compositor.
- `sha256sum --check --ignore-missing SHA256SUMS` and `gh attestation verify <tarball> --repo ordreth/veil` on the downloaded files.
- In CI, the new job `release build (Debian 12 container)` passes on `main`.

## In a real Hyprland session (for the project owner)

**Run so far (2026-10-20, Hyprland 0.56.2, NVIDIA RTX 3060 Ti, two 1080p monitors, a release build of the tree, run by an agent at the owner's request; the owner's own service was stopped for about 90 seconds and started again):** the daemon followed Hyprland's events and drew an H.264 1080p60 video on both monitors; a fullscreen window on one monitor paused that output after about a second ("cannot be seen; pausing it") and closing it played the video again, the other monitor kept playing. About 90 to 99 % of one core for the two streams (known issue 7). **Not run, and still open:** lock and DPMS (the agent could not unlock the session), known issues 5 and 6 (they need someone to watch), the systemd unit and crash recovery, logout, completions, and a monitor change. This Hyprland takes Lua-style dispatchers (`hyprctl dispatch 'hl.dsp.window.fullscreen({ window = "class:x" })'`); the old `hyprctl dispatch dpms off` and `hyprctl keyword monitor ...` forms below may need translating.

These need the desktop session, so no test or script runs them. They are made by hand on the build that is going to be released, before a release (see [`roadmap.md`](roadmap.md)).

- **Pause behind windows:** `RUST_LOG=veil=debug veil daemon --input <video>`; switch workspaces on each monitor, make a window fullscreen and back, open and close a window: the `compositor:` lines follow, "pausing it" about a second after a fullscreen window appears, `top` shows the daemon near 0%, "playing it" when it goes. Switching quickly between a workspace with a fullscreen window and one without pauses nothing. `--pause-on covered` with tiled windows and no gaps pauses as well.
- **Lock and DPMS:** lock the session (hyprlock) for a few seconds: "has not been shown for 2 s" and the daemon near 0%; unlock: the video plays again. Then `hyprctl dispatch dpms off`, wait 5 s, `hyprctl dispatch dpms on`: the video plays again and the daemon is still running. If the daemon dies at `dpms on`, Hyprland triggers the GStreamer crash of known issue 1 like sway does; please note whether it also dies with `dpms off` and `on` within one second.
- **Known issue 5:** with `--mode fill` and a video whose loop is a few seconds long, watch a few loops: does the picture stand still for a moment each time the video starts again? (A headless sway showed it once, for about 0.7 s, and has not since, with any build: `known-issues.md`, entry 5.) If it does here, record it in that entry with the steps to reproduce it.
- **Known issue 6:** a video in `fill` and in `fit`: does it play smoothly, without lag and without parts of older frames showing? (It did not with a build that stopped committing veil's surface on every frame; the loop is back since.)
- **The systemd unit:** install the unit (see the README, "Run as a systemd user service"), `systemctl --user enable --now veil`. `systemctl --user status veil` says `active (running)` with `Status: "Started; wallpaper shown on ..."`, naming every monitor, and the wallpaper is visible on all of them when the command returns. `journalctl --user -u veil` starts with `veil 0.1.0 (GStreamer ...)` and has no escape codes.
- **Crash recovery:** with veil running under its systemd unit, `kill -ABRT $(pidof veil)` (not `kill -SEGV`: the Rust runtime takes the first SIGSEGV sent that way and the daemon goes on): the wallpaper is back within about 3 seconds, the one that was set last. Then the real crash, if a video plays: change a monitor's scale or mode (`hyprctl keyword monitor ...`), and the same should happen. `systemctl --user show veil -p NRestarts` counts the restarts.
- **Logout:** log out and back in: the unit is stopped with the session (or, started with `exec-once`, ends with status 5 and is not restarted), and the next session starts it again with the same wallpapers.
- **Completions:** with the package installed (or `veil completions zsh > ~/.zfunc/_veil` and the `fpath` line of `veil completions --help`), in a new shell: `veil <TAB>`, `veil daemon --pause-on <TAB>` and `veil set --input <TAB>` offer the commands, `never fullscreen covered` and files; `man veil` and `man veil-set` open.
- **Compatibility:** fill in the Hyprland row of the compatibility table in [`troubleshooting.md`](troubleshooting.md#compatibility): whether images show, the `video decoder:` line of the log for a video, whether the automatic pause works, and the versions (the first line of the log, `hyprctl version`).
- **The README:** once the repository is public, `nix run github:ordreth/veil -- daemon` and `nix profile add github:ordreth/veil` as the README has them (checked only against the local flake); the quick start in your session; and, if Hyprland starts the unit through `exec-once`, that line of "Run as a systemd user service".

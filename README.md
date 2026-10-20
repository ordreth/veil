<p align="center"><img src="assets/icon/veil-256.png" alt="veil" width="160"></p>

# veil

**veil** is a wallpaper daemon for Wayland compositors with the layer-shell protocol (Hyprland, sway and other wlroots compositors). It shows a picture or a looping video on each monitor, can give every monitor a wallpaper of its own, pauses a video nobody can see, and brings back what each monitor showed after a restart. It is written in Rust, plays videos with GStreamer, and is controlled from the command line, a configuration file, or a socket that scripts and status bars can use.

> **Status: beta (1.0.0-beta.1).** Free to use, source-available, not open source (see [License](#license)). What was run, where and when is in the [support matrix](docs/troubleshooting.md#support-matrix): the headless sway tests and the measurements on an NVIDIA card, and a first pass on Hyprland 0.56.2 (images, video, automatic pause). Not run: Intel and AMD cards, aarch64, other compositors, and on Hyprland the lock screen, DPMS, logout, crash recovery and a long soak. What is known to be wrong is under [Known limitations](#known-limitations).

## Quick start

Once veil is [installed](#installation), in your Wayland session:

```bash
veil daemon &                          # draws on every monitor (black until a wallpaper is set)
veil set --input ~/Videos/waves.mp4    # a video or a picture; saved, and shown again after a restart
veil status                            # what each monitor shows
```

To start veil with your session, use its [systemd user unit](#run-as-a-systemd-user-service) instead of `veil daemon &`.

## Features

**Supported** means: implemented, covered by automated tests, and checked in a compositor (so far a headless sway, see the status above). **Experimental** means that it works, with a limit you should know about, or that it has not been checked where it matters. **Planned** features come after 1.0.

| Feature | Status | Notes |
|---|---|---|
| Pictures: PNG, JPEG, WebP, BMP, TIFF, GIF and the other formats of the [`image`](https://crates.io/crates/image) crate | Supported | Drawn at the monitor's physical resolution, then the decoded copy is freed. Pictures over 16384 pixels per side, or that would need more than 512 MiB, are refused. |
| Videos, looped, without audio | Supported | Any format the installed GStreamer plugins play. Checked: H.264, HEVC, VP8, VP9 and AV1, and ten containers (MP4, WebM, Matroska, QuickTime, AVI, Ogg, ...). |
| Scaling: `fill`, `fit`, `stretch`, `center`, `tile`, and a background colour | Supported | For pictures and videos (`tile` acts as `center` for a video). |
| HiDPI and fractional scaling | Supported | Checked pixel by pixel at scales 1, 1.25, 1.5 and 2. |
| Every monitor, or the ones you name; a wallpaper per monitor; monitors plugged in later | Supported | With pictures. With a video, see the next row. |
| Monitor changes while a video plays | Experimental | A change of any monitor (plugged in, unplugged, rescaled, powered on) crashes the daemon inside GStreamer ([known issue 1](docs/known-issues.md)). Under the systemd unit it is back with its wallpapers in about three seconds. |
| `veil set`, `clear`, `pause`, `resume`, `toggle`, `status`, `outputs` (also as JSON), `quit`, `ping` | Supported | Stable exit statuses. A control socket for scripts and status bars ([`docs/ipc.md`](docs/ipc.md)). |
| A new wallpaper replaces the old one only once it is ready | Supported | A video takes over when its first frame is on screen; a file that cannot be shown leaves the old wallpaper alone. |
| Wallpapers saved and shown again after a restart | Supported | Per monitor, in `~/.local/state/veil/state.json`. |
| Configuration file | Supported | `~/.config/veil/config.toml` ([`docs/configuration.md`](docs/configuration.md)). |
| systemd user unit | Supported | Ready once every monitor shows its wallpaper, restarted after a crash, a watchdog. |
| Pause a video behind a fullscreen window (or windows that cover it) | Supported on sway, experimental on Hyprland | Read from the compositor's IPC. On Hyprland 0.56.2 a fullscreen window paused its output and closing it played the video again (2026-10-20); with `--pause-on covered` and other cases it was checked against a fake Hyprland only. |
| Stop a video while the screen is locked or the monitor is off | Supported on sway, experimental elsewhere | Works on any compositor that stops taking the video's frames, as sway does; not checked on Hyprland (lock and DPMS). The video starts again from its beginning. |
| Hardware video decoding | Experimental | NVIDIA (`nvcodec`) decodes on the GPU, but every frame is converted to RGB on the CPU ([known issue 7](docs/known-issues.md)). Intel and AMD (VA-API) should avoid that conversion; not checked yet. Software decoding always works. |
| Animated GIFs | Supported | Played as videos; needs GStreamer's `libav` plugins. |
| Hyprland | Experimental | veil's main target. Images, video on two monitors and the automatic pause ran on Hyprland 0.56.2 (2026-10-20); smoothness, scales, lock, power-off and crash recovery were not checked there. |
| Other compositors with layer-shell (river, niri, Wayfire, labwc, KDE Plasma) | Experimental | Should work; nobody has reported yet. No pause behind windows there (the lock and power-off stop still applies). |
| Shell completions (bash, zsh, fish, nushell) and man pages | Supported | Generated by the binary itself. |
| Nix flake and package | Supported | The package brings its GStreamer plugins. |
| Prebuilt binaries for x86_64 and aarch64 Linux | Experimental | See [Prebuilt binaries](#prebuilt-binaries); the release workflow has not run yet. The x86_64 tarball ran on Debian 12 and 13, Ubuntu 22.04 and 24.04, Fedora 44 and Arch; aarch64 was not run. With GStreamer before 1.26, animated GIFs do not play (known issue 8). |
| GNOME, X11 | No | GNOME has no layer-shell. |
| HEIF/HEIC, AVIF, JPEG XL; animated WebP and PNG | No | Refused with the reason; an animated WebP or PNG shows its first frame. |
| Audio, frame-rate cap, playback speed, loop points | Planned | After 1.0. |
| Playlists, directory rotation, transitions | Planned | After 1.0. `veil set` from a script or a timer works today. |
| Pause when idle or on battery; pause behind windows on niri and river | Planned | After 1.0. |
| NixOS and home-manager modules, AUR, `.deb` and `.rpm` packages | Planned | After 1.0. |

## Performance

Measured with [`scripts/bench/`](scripts/bench) on the build of 2026-10-19, in a headless sway 1.12 that renders on the CPU, one 1920x1080 output in `fill`, 60 seconds after a 5-second warm-up, with `MALLOC_ARENA_MAX=1` as the package and the unit start it. The clips are ffmpeg test patterns in which every frame differs. The machine: Intel Core i5-11400F, NVIDIA GeForce RTX 3060 Ti (driver 595.91.07, so GStreamer picks the `nvcodec` decoders), 32 GB, NixOS with Linux 7.2 and GStreamer 1.28.7. CPU is in percent of one core, for the daemon's whole process.

| Wallpaper | Decoder | CPU | Frames shown per second | Memory (RSS) | Threads |
|---|---|---|---|---|---|
| A 1920x1080 PNG | none | 0.02 % (one tick of 10 ms in 60 s) | | 20 MiB | 1 |
| H.264, 1080p, 30 fps | `nvh264dec` (GPU) | 19.5 % | 30.0 of 30 | 203 MiB | 15 |
| H.264, 1080p, 60 fps | `nvh264dec` (GPU) | 39.7 % | 59.8 of 60 | 203 MiB | 15 |
| VP9, 1080p, 60 fps | `nvvp9dec` (GPU) | 37.9 % | 59.9 of 60 | 251 MiB | 15 |
| H.264, 2160p, 60 fps | `nvh264dec` (GPU) | 136 % | 60.2 of 60 | 297 MiB | 15 |
| H.264, 1080p, 30 fps | `avdec_h264` (software) | 24.5 % | 30.0 of 30 | 161 MiB | 22 |
| H.264, 2160p, 60 fps | `avdec_h264` (software) | 174 % | 60.1 of 60 | 458 MiB | 22 |

- **A still picture costs next to nothing** after it is drawn: one thread, and one tick of CPU time in a minute, while the bench asked `veil status` every second.
- **A video costs CPU in proportion to its pixels per second**, whatever the codec. The decoder on the GPU costs about 1 %; the rest is the conversion of every frame from NV12 to RGB, which the compositor needs and which happens on the CPU with NVIDIA's decoders. veil's design goal of under 1 % for a video is **not** met on this hardware. On Intel and AMD, VA-API could hand the frames to the compositor without that conversion; that has not been measured ([known issue 7](docs/known-issues.md)).
- **Memory, threads and file descriptors stay flat**: a video that looped once a second for ten minutes kept nothing (with the hardware decoder; the software decoder kept about 500 KiB over its first minutes, then nothing), and 500 wallpaper switches (250 of them videos) left nothing behind. Memory stays flat over switches only with one allocator arena (`MALLOC_ARENA_MAX=1`), which the Nix package and the systemd unit set; [known issue 3](docs/known-issues.md) has the numbers without it. Runs of hours and days are still to be done.
- **A paused or stopped video uses next to no CPU**: behind a fullscreen window it keeps its frame on screen and decodes nothing (measured: at most one tick in 5 seconds); while the screen is locked its pipeline is released.

How each number was measured, and what was measured before, is in [`docs/measurements.md`](docs/measurements.md). Nothing has been measured in a real session yet.

## Installation

veil needs a Wayland compositor with `wlr-layer-shell` and GStreamer 1.x with the plugin sets `base`, `good`, `bad` and `libav` at run time (`waylandsink` is in `bad`). `XDG_RUNTIME_DIR` must be set, as it is in any desktop session.

### Prebuilt binaries

Each [release](https://github.com/ordreth/veil/releases) has a tarball for x86_64 and one for aarch64 Linux, with the binary, the systemd user unit, the man pages, the shell completions, the documents and an install script, and `SHA256SUMS`:

```bash
version=1.0.0    # the release you want
tarball=veil-$version-$(uname -m)-unknown-linux-gnu.tar.gz
curl -LO https://github.com/ordreth/veil/releases/download/v$version/$tarball
curl -LO https://github.com/ordreth/veil/releases/download/v$version/SHA256SUMS
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "$tarball" && cd "${tarball%.tar.gz}"
sudo ./install.sh                  # into /usr/local
./install.sh --prefix ~/.local     # or for your user only, without root
```

`./install.sh --help` lists what goes where, and `--uninstall` (with the same `--prefix`) removes it again. Each tarball also has a build-provenance attestation, which says that GitHub's release workflow built it from the tagged commit: `gh attestation verify "$tarball" --repo ordreth/veil`. Then [start the daemon](#start-the-daemon), or [the systemd unit](#run-as-a-systemd-user-service), which the script installs with the path of the binary in it. For `~/.local`, `~/.local/bin` must be on your `PATH`, and zsh needs the `fpath` line the script prints; bash, fish, nushell, `man` and systemd look there by themselves. Started by hand, give the daemon `MALLOC_ARENA_MAX=1` as the unit does (as in `MALLOC_ARENA_MAX=1 veil daemon`), or its memory grows with every video it switches to.

The binary uses the system's GStreamer, GLib and Wayland client library, and needs glibc 2.34 or newer. Install the GStreamer plugins (no development files, no Rust):

```bash
sudo apt install libwayland-client0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good \
  gstreamer1.0-plugins-bad gstreamer1.0-libav                      # Debian, Ubuntu
sudo dnf install libwayland-client gstreamer1-plugins-base gstreamer1-plugins-good \
  gstreamer1-plugins-bad-free gstreamer1-plugin-libav gstreamer1-plugin-openh264   # Fedora
sudo pacman -S --needed wayland gstreamer gst-plugins-base gst-plugins-good \
  gst-plugins-bad gst-libav                                        # Arch Linux
```

The x86_64 tarball was installed with these packages and run in a headless sway on Debian 12 (GStreamer 1.22.0), Debian 13 (1.26.2), Ubuntu 22.04 (1.20.3), Ubuntu 24.04 (1.24.2), Fedora 44 and Arch Linux (both 1.28.8), on 2026-10-19: a picture, an H.264 MP4 and a VP9 WebM showed and played everywhere. Animated GIFs need GStreamer 1.26 or newer: on Ubuntu 22.04 and 24.04 and Debian 12 they stand still or do not start (on Debian 12 also with a whole CPU core busy), because of GStreamer's GIF demuxer; [convert them to a video](docs/troubleshooting.md#an-animated-gif-stands-still-does-not-start-or-keeps-a-core-busy). On Ubuntu 24.04, 1080p videos showed only a half to a quarter of their frames in the headless sway, for a reason not found yet ([known issue 8](docs/known-issues.md#8-older-gstreamer-versions-animated-gifs-and-frames-dropped-on-ubuntu-2404)). The release workflow builds the aarch64 tarball with the same script on an ARM machine and runs the tests there; it has not been run with a compositor.

### Nix and NixOS

```bash
nix run github:ordreth/veil -- daemon     # try it without installing
nix profile add github:ordreth/veil       # install it (`nix profile install` in older Nix)
```

In a NixOS configuration, through the flake's overlay:

```nix
# flake inputs: veil.url = "github:ordreth/veil";
nixpkgs.overlays = [ veil.overlays.default ];
environment.systemPackages = [ pkgs.veil ];
# The systemd user unit, started with the graphical session:
systemd.packages = [ pkgs.veil ];
systemd.user.services.veil.wantedBy = [ "graphical-session.target" ];
```

Because of the license, nixpkgs counts veil as unfree. The flake's own packages allow it by themselves; with the overlay, allow it in your configuration: `nixpkgs.config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) [ "veil" ];`.

The package is a wrapper around the binary that brings its own GStreamer plugin set and a GStreamer plugin cache of its own (`~/.cache/veil/gstreamer-registry.bin`), and sets `MALLOC_ARENA_MAX=1` unless you set it. It includes the systemd user unit, the shell completions and the man pages. Hardware decoding uses the system's drivers (`hardware.graphics`, and `hardware.nvidia` for NVIDIA); with Nix on another distribution it has not been tried.

### From source

You need Rust 1.89 or newer, a C compiler and `pkg-config`, the development files of Wayland, `libxkbcommon` and GStreamer, and the GStreamer plugins. These are the packages, checked on 2026-10-19: veil was built with them in a container of each distribution, and the result showed a picture and played an H.264 video, a VP9 video and an animated GIF in a headless sway. Other versions of these distributions were not tried.

**Arch Linux** (GStreamer 1.28.8, Rust 1.99):

```bash
sudo pacman -S --needed base-devel git rust pkgconf wayland libxkbcommon \
  gstreamer gst-plugins-base gst-plugins-good gst-plugins-bad gst-libav
sudo pacman -S gst-plugin-va    # VA-API hardware decoding (Intel, AMD)
```

**Debian 13** (GStreamer 1.26.2). Debian's Rust (1.85) is too old, so Rust comes from [rustup](https://rustup.rs):

```bash
sudo apt install build-essential git pkg-config curl libwayland-dev libxkbcommon-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"    # or open a new shell
```

**Ubuntu 26.04** (GStreamer 1.28.2, Rust 1.93):

```bash
sudo apt install build-essential git pkg-config cargo libwayland-dev libxkbcommon-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libav
```

**Fedora 44** (GStreamer 1.28.8, Rust 1.99):

```bash
sudo dnf install gcc git rust cargo pkgconf-pkg-config wayland-devel libxkbcommon-devel \
  gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good \
  gstreamer1-plugins-bad-free gstreamer1-plugin-libav gstreamer1-plugin-openh264
```

Fedora's own GStreamer has no software decoder for HEVC, and decodes H.264 with `openh264dec` (from the Cisco repository Fedora enables). For HEVC in software, the full FFmpeg of RPM Fusion is the usual addition (not tried here). On Debian, Ubuntu and Fedora the VA-API and NVIDIA decoders are in the `bad` plugins; Arch has VA-API in `gst-plugin-va`. Hardware decoding was not tried in the containers (they had no GPU).

Then build veil and install the binary, the systemd unit and the man pages (here for your user; adjust the paths for a system-wide install):

```bash
git clone https://github.com/ordreth/veil && cd veil
cargo build --release --locked
install -Dm755 target/release/veil ~/.local/bin/veil
mkdir -p ~/.config/systemd/user ~/.local/share/man/man1
sed "s|/usr/bin/veil|$HOME/.local/bin/veil|" contrib/systemd/veil.service > ~/.config/systemd/user/veil.service
~/.local/bin/veil man-pages ~/.local/share/man/man1
```

`~/.local/bin` must be on your `PATH`. The completions are under [Shell completions and man pages](#shell-completions-and-man-pages). Started by hand rather than through the unit, give a binary you built yourself `MALLOC_ARENA_MAX=1` (as in `MALLOC_ARENA_MAX=1 veil daemon`), or its memory grows with every video it switches to.

### Hardware video decoding

GStreamer picks a hardware decoder when one works, and veil logs which one it got for every video (`video decoder: nvh264dec (hardware)`). Intel and AMD need a VA-API driver (`intel-media-driver`, or Mesa for AMD) and GStreamer's `va` plugin; NVIDIA needs the proprietary driver, used through GStreamer's `nvcodec`. Without either, videos are decoded in software and still play, at a higher CPU cost. [`docs/troubleshooting.md`](docs/troubleshooting.md#no-hardware-decoding) says how to check, and how to force software decoding.

## Usage

### Start the daemon

```bash
veil daemon                                           # every monitor: its wallpaper from the configuration file, else the saved one
veil daemon --input ~/Videos/waves.mp4                # this wallpaper on every monitor
veil daemon --output DP-3 --output HDMI-A-1           # only these monitors
veil daemon --mode fit --bg-color '#202020'           # letterbox on dark grey (quote the #)
```

Without `--input`, each monitor shows its wallpaper from the [configuration file](#configuration-file), else the one it [showed before](#saved-wallpapers), else a solid `--bg-color` (black by default) until `veil set`. Monitor names are the compositor's (`veil outputs`, `hyprctl monitors`, `swaymsg -t get_outputs`). A monitor keeps its wallpaper by name: unplugged and plugged in again, it shows what it showed. A monitor connected for the first time gets the default wallpaper: that of the latest `veil set` for every monitor, or else `--input` or the configuration file's `[output."*"]`.

`--mode` says how a wallpaper is fitted to its monitor; the daemon's `--mode` and `--bg-color` are also the defaults of `veil set`:

| Mode | Result |
|---|---|
| `fill` (default) | Covers the whole monitor, keeping the aspect ratio. The overflow is cropped evenly from both sides. |
| `fit` | The whole picture is visible, keeping the aspect ratio. Bars of `--bg-color` fill the rest. |
| `stretch` | Scaled to exactly the monitor's size; the aspect ratio is not kept. |
| `center` | Native size (one pixel per device pixel), centred. Cropped if larger than the monitor, surrounded by `--bg-color` if smaller. |
| `tile` | Native size, repeated from the top-left corner. Videos fall back to `center`. |

`veil daemon --help` (or `man veil-daemon`) lists every option.

### Images and videos

Whether a file is a picture or a video is read from its content, not from its name:

| The file is | It is shown as |
|---|---|
| A picture (PNG, JPEG, WebP, BMP, TIFF and the other formats of the `image` crate) | An image, whatever its extension. |
| A GIF with one frame | An image. |
| A GIF with several frames | A video (looped). GStreamer needs `gst-libav` for it. |
| An animated WebP or PNG | An image: its first frame. For WebP a warning is logged. |
| A video container (Matroska, WebM, MP4, QuickTime, 3GP, AVI, Ogg, FLV, ASF, MPEG program or transport stream) | A video, whatever its extension. Whether it can be played depends on the installed GStreamer plugins. |
| Something else, with a video extension (`mp4`, `m4v`, `mov`, `mkv`, `webm`, `avi`, `ogv`, `mpg`, `mpeg`, `ts`, `m2ts`, `mts`, `wmv`, `asf`, `flv`, `3gp`, `vob`) | Handed to GStreamer, which knows more containers; its answer is the result. |
| Something else, with an image extension (for example `tga`, a format without a signature) | Handed to the image decoder. |
| A HEIC/HEIF or JPEG XL picture | Refused: `'photo.heic' is a picture in the HEIF format, which veil cannot decode`. (An AVIF picture is refused by the image decoder: `The image format Avif is not supported`.) |
| Anything else | Refused: `'notes.txt' is neither an image nor a video that veil recognises, by its content or by its extension`. |

A path that does not exist, a directory, an empty file and a file that cannot be read are refused with exactly that reason. A video with a single frame is shown as a still picture and not looped.

### Change the wallpaper

```bash
veil set --input ~/Videos/waves.mp4                    # every monitor
veil set --input ~/Pictures/photo.jpg --output DP-3    # one monitor; the others keep theirs
veil set --input ~/Pictures/pattern.png --mode tile    # another mode than the daemon's
veil set --input ~/Pictures/preview.png --no-persist   # not saved: a restart shows what was saved before
```

The daemon loads the file in the background, and `veil set` returns once the wallpaper is on screen (for a video, its first frame) on every monitor it is for. If the file is missing or cannot be shown, `veil set` prints why and exits with status 1, and the old wallpaper stays. A relative path is fine: `veil set` makes it absolute. If several `veil set` come faster than files load, the latest wins on each monitor; one that is replaced before it was shown says so and exits with status 1. Without `--output`, the wallpaper also becomes the default for monitors connected later (a video once it has been shown).

### Control a running daemon

```bash
veil clear                     # only the background colour (saved like `veil set`)
veil pause                     # pause videos until `veil resume`, whatever is on screen
veil resume --output DP-3
veil toggle                    # prints `paused` or `playing`
veil status                    # what each monitor shows (--json for scripts)
veil outputs                   # the compositor's monitors, and which ones veil draws on
veil quit                      # stop the daemon; returns once it has ended (alias: veil stop)
veil ping                      # prints `pong` if a daemon runs, else exits with status 3
```

`veil status` prints a line per monitor:

```text
veil 0.1.0 (pid 512958), running for 7s; videos pause on: fullscreen
default: /home/me/Videos/waves.mp4 (fill, #000000)
DP-3: paused (by veil pause) /home/me/Videos/waves.mp4 (fill, #000000); 1920x1080 at scale 1; video 1280x720, 30 fps, 0 frames shown, 0 dropped
HDMI-A-1: showing /home/me/Pictures/wallpaper.png (fit, #000000); 2048x1152 at scale 1.25
```

Every command takes `--help`, and has a man page (`man veil-set`, ...). What `veil pause` holds lasts until `veil resume` or the end of the daemon; it is not saved.

### Automatic pause

Under Hyprland and sway, veil follows the compositor's IPC and pauses a video nobody can see: it keeps its frame on screen and decodes nothing, and plays on from the same frame when it can be seen again. `--pause-on` (or `pause_on` in the configuration file) says when:

| `--pause-on` | A video is paused while its monitor shows |
|---|---|
| `never` | Nothing pauses it. |
| `fullscreen` (default) | A fullscreen window. A maximised window does not count. |
| `covered` | A fullscreen window, or windows that cover the whole monitor (gaps or a bar leave it uncovered). A translucent window counts as covering. |

A video is paused once it has been hidden for a second, so switching workspaces back and forth does not stop it every time, and plays again at once. Under other compositors nothing is paused this way.

On any compositor, a video whose frames the compositor has not taken for `--release-unseen-after` seconds (default 2) is stopped and its pipeline freed: the screen is locked, the monitor is off, or (in sway) a window covers it. It starts again from its beginning when the monitor shows it again. `0` turns this off.

### Configuration file

`~/.config/veil/config.toml` (or `$XDG_CONFIG_HOME/veil/config.toml`, or `veil daemon --config FILE`) holds the daemon's options and the wallpaper of each monitor:

```toml
[defaults]
mode = "fill"           # --mode
bg_color = "#000000"    # --bg-color
pause_on = "covered"    # --pause-on

[output."DP-3"]
input = "~/Videos/waves.mp4"
mode = "fit"

[output."*"]            # every monitor without a section of its own
input = "~/Pictures/wallpaper.png"
```

A flag on the command line wins over the file, and the file over the built-in default. A monitor with a wallpaper in the file shows it at every start; `veil set` changes it until the daemon stops. The file is read only when the daemon starts. `veil config check` reads it as the daemon would, checks its wallpapers and prints what the daemon would start with; a wrong key or value is reported with its line and column. Every key is in [`docs/configuration.md`](docs/configuration.md), and [`contrib/config.example.toml`](contrib/config.example.toml) is an annotated file.

### Saved wallpapers

Every `veil set` and `veil clear` that is shown is saved, per monitor, in `$XDG_STATE_HOME/veil/state.json` (`~/.local/state/veil/state.json`), with its mode and colour. A daemon started without `--input` shows each monitor's saved wallpaper again, unless the configuration file gives that monitor one; started with `--input`, it shows that everywhere and saves it. `--no-persist` shows a wallpaper without saving it. A saved file that has gone missing is logged, and its monitor shows the background colour. The file is shared by all daemons of a user: a daemon you start for a test in a nested compositor saves there too, unless you give it another `XDG_STATE_HOME`.

### Run as a systemd user service

The unit ([`contrib/systemd/veil.service`](contrib/systemd/veil.service)) runs `veil daemon` without options, so it shows the wallpapers of the configuration file and the saved ones. Installed as above (or by the Nix package):

```bash
systemctl --user daemon-reload
systemctl --user enable --now veil
```

- `systemctl --user start veil` returns once every monitor shows its wallpaper, and `systemctl --user status veil` says how start-up went.
- After a crash (such as the one on monitor changes, [known issue 1](docs/known-issues.md)) it is restarted after 2 seconds, with the wallpapers it had. It is not restarted after a wrong configuration file (fix it, then `systemctl --user restart veil`), a second daemon, or the end of the session.
- A watchdog restarts a daemon whose event loop is stuck, and `MALLOC_ARENA_MAX=1` keeps memory flat over video switches.
- The log is in the journal: `journalctl --user -u veil`.

The unit starts with `graphical-session.target` and needs `WAYLAND_DISPLAY` in the user manager's environment (and `HYPRLAND_INSTANCE_SIGNATURE` or `SWAYSOCK` for the automatic pause). Hyprland imports these by itself. A session without `graphical-session.target` starts it from its own configuration instead of enabling it:

```bash
# Hyprland (hyprland.conf)
exec-once = systemctl --user start veil
# sway (config)
exec systemctl --user import-environment WAYLAND_DISPLAY SWAYSOCK && systemctl --user start veil
```

Daemon options go into the configuration file, or into `ExecStart` with `systemctl --user edit veil`. Use `systemctl --user stop veil` rather than `veil quit` to stop the service.

### Shell completions and man pages

The Nix package installs both. Otherwise the binary prints the completion script of a shell, and writes the man pages:

```bash
veil completions bash > ~/.local/share/bash-completion/completions/veil
veil completions zsh > ~/.zfunc/_veil            # with fpath+=(~/.zfunc) before compinit in ~/.zshrc
veil completions fish > ~/.config/fish/completions/veil.fish
veil man-pages ~/.local/share/man/man1           # veil.1, veil-set.1, ...
```

In nushell: `mkdir ($nu.user-autoload-dirs | first); veil completions nushell | save -f ($nu.user-autoload-dirs | first | path join veil.nu)`. Create the directories first if they do not exist. Monitor names are not completed.

### Scripts and status bars

The `veil` commands talk to the daemon through a Unix socket, `$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock`, that other programs can use too: one JSON request and one JSON response per connection, only for processes of your own user. `veil status --json` and `veil outputs --json` print the same answers. Fields, commands and error codes are only ever added, never renamed or removed, and the exit statuses of `veil` are stable (for example 3 when no daemon runs). The reference of the protocol, the JSON fields and the exit statuses is [`docs/ipc.md`](docs/ipc.md); the exit statuses are also in `man veil`.

## Known limitations

The details, the measurements and the options considered are in [`docs/known-issues.md`](docs/known-issues.md).

- **A monitor change while a video plays crashes the daemon** (entry 1). The bug is in GStreamer's `waylandsink` (1.28.7, unchanged upstream): it segfaults when the compositor reports a change of any monitor (scale, resolution, refresh rate, rotation, position, a monitor powered on again) and, with several monitors, one being added or removed. Pictures are not affected. Run veil under its systemd unit: it is back with its wallpapers within about three seconds.
- **A video costs CPU on NVIDIA** (entry 7): about 20 % of a core for 1080p at 30 fps (see [Performance](#performance)).
- **Memory grows with video switches without one allocator arena** (entry 3). The Nix package and the systemd unit set `MALLOC_ARENA_MAX=1`; set it yourself when you start a binary built from source by hand.
- **A stopped video starts from its beginning.** A video stopped while the screen was locked or the monitor was off plays again from its start, and each lock is one more pipeline built. A monitor switched off and on again within 2 seconds still hits the crash above.
- **The automatic pause knows only rectangles.** With `--pause-on covered` a translucent window counts as covering; a floating window that is moved is noticed at the next workspace or window event. It works under Hyprland (0.40 or newer) and sway only.
- **A picture is scaled again on the event loop** when its monitor is resized or rescaled, once per change. A file that never finishes loading (a network mount that hangs) holds back later `veil set`, which give up after 30 seconds; the daemon keeps running.
- **A video that hangs without an error is not noticed,** and one whose frames are damaged while its container stays readable is played as it is.
- **Not checked yet:** a real Hyprland session (the checks before the release), Intel and AMD cards, other compositors, runs of more than ten minutes.

## Troubleshooting

Start with `veil status` (what each monitor shows, and whether a video plays, waits, or stopped after an error) and the log (`journalctl --user -u veil`, or the daemon's stderr): its first line names the versions of veil and GStreamer, and every video names its decoder. `RUST_LOG=veil=debug` shows more. [`docs/troubleshooting.md`](docs/troubleshooting.md) goes through the common failures with the messages they give (a black screen, a missing GStreamer element, a stale GStreamer cache, no hardware decoding, an output name that is not found), has a cheat-sheet of debug variables, and the table of compositors and graphics cards veil has been tried on.

## Documentation

| Document | What it has |
|---|---|
| [`docs/configuration.md`](docs/configuration.md) | Every key of the configuration file, and how it combines with `veil set` |
| [`docs/ipc.md`](docs/ipc.md) | The control socket, every command, field, error code and exit status |
| [`docs/troubleshooting.md`](docs/troubleshooting.md) | Common failures, debug cheat-sheet, compatibility table |
| [`docs/known-issues.md`](docs/known-issues.md) | What veil cannot fix today, and why |
| [`docs/roadmap.md`](docs/roadmap.md) | What is left before 1.0, and what comes after it |
| [`docs/measurements.md`](docs/measurements.md) | How veil is measured, and every number so far |
| `man veil`, `veil <command> --help` | Every command and option |
| [`CHANGELOG.md`](CHANGELOG.md) | What changes for users, per release |

## Roadmap

[`docs/roadmap.md`](docs/roadmap.md) has the whole list. What is left before 1.0: the rest of the checks in a real Hyprland session, a 24-hour soak, a security review of the control socket, and other compositors and graphics cards. The [planned features](#features) come after 1.0.

## Reporting problems

veil does not take pull requests or issues ([`CONTRIBUTING.md`](CONTRIBUTING.md)). Reports of what worked or failed on your machine are welcome in [Discussions, category "Reports"](https://github.com/ordreth/veil/discussions/categories/reports); [`docs/troubleshooting.md`](docs/troubleshooting.md#reporting-a-problem) says what to include. Nothing is promised in return (no answer, no fix, no date). A security problem is reported privately, as [`SECURITY.md`](SECURITY.md) says. Everyone who takes part follows the [code of conduct](CODE_OF_CONDUCT.md).

## License

veil is **free to use and source-available, not open source**. It is licensed under the [PolyForm Strict License 1.0.0](LICENSE): you may read the code and use veil for any noncommercial purpose, but not distribute it, change it, make new works from it or use it commercially. [`ADDITIONAL-PERMISSIONS.md`](ADDITIONAL-PERMISSIONS.md) adds to that: you may redistribute the unchanged release tarballs, build veil from source for your own use, and quote short excerpts. The name and the icon are reserved there too. The licences of the Rust crates veil links are in [`THIRD-PARTY-NOTICES`](THIRD-PARTY-NOTICES). The terms of a version stay as they were released if the license changes later.

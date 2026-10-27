# Troubleshooting

What to do when veil does not show what it should: where to look first, the failures that are common enough to have a name, a cheat-sheet of the commands that tell you more, and a table of the compositors and graphics cards veil has been tried on.

The failures below were produced on purpose in a headless sway 1.12 with GStreamer 1.28.7, and the messages are the ones the daemon gave (2026-10-19; how, in [`manual-tests.md`](manual-tests.md), "Troubleshooting"); the table of other messages quotes the code. Log lines are shown without their time stamp. The limitations veil knows about and cannot fix today, such as the crash on a monitor change while a video plays, are in [`known-issues.md`](known-issues.md).

## Where to look first

1. **Is a daemon running?** `veil ping` prints `pong`, or exits with status 3 when no daemon runs for this compositor. Under systemd: `systemctl --user status veil` (it also says how start-up went).
2. **What does it think it shows?** `veil status` has one line per output:

   ```text
   veil 1.0.0-beta.1 (pid 295816), running for 4s; videos pause on: fullscreen
   default: /home/me/walls/sea.mp4 (fill, #000000)
   HEADLESS-1: playing /home/me/walls/sea.mp4 (fill, #000000); 1920x1080 at scale 1; video 1920x1080, 30 fps, 71 frames shown, 0 dropped
   ```

   The word after the output's name is its state:

   | State | Meaning |
   |---|---|
   | `showing` | An image, or the background colour, is on screen. |
   | `starting` | A video is loaded and waits for its first frame (at most 10 seconds). |
   | `playing` | A video plays. The frame counts are the video sink's own; they start again at every loop. |
   | `paused (by veil pause)`, `paused (hidden by a window)` | A video is paused, by hand or because a fullscreen window (or, with `--pause-on covered`, the windows) hides it. |
   | `stopped while the compositor does not show it` | The compositor took none of the video's frames for 2 seconds (a locked screen, a monitor that is off, in sway a window over all of it), so veil let go of the video. It starts again from its beginning when the compositor draws the output again. |
   | `stopped after an error (see the log)` | The video could not be started, or failed and could not be restarted. The surface shows the video's background colour. The reason is in the log. |
   | `not connected` | The output is unplugged or disabled. Its wallpaper comes back with it. |

   An output that is missing from the list is one the daemon does not draw on (see [An output name is not found](#an-output-name-is-not-found)).
3. **What does the log say?** `journalctl --user -u veil` under systemd, otherwise the daemon's stderr. The first line names the versions (`veil 1.0.0-beta.1 (GStreamer 1.28.7)`), and every video logs the decoder GStreamer picked for it (`video decoder: nvh264dec (hardware)`).

## The screen is black, or shows only a colour

`veil status` tells the cases apart:

| `veil status` says | Why | What to do |
|---|---|---|
| `veil ping` exits with status 3 | No daemon runs. | Start it (`veil daemon`, or `systemctl --user start veil`). Under systemd, `systemctl --user status veil` and the journal say why it stopped. |
| The output is not listed | The daemon does not draw on it: `--output` (or `outputs` in the configuration file) names other outputs, or a name is misspelt. What you see is the compositor's own background. | [An output name is not found](#an-output-name-is-not-found). |
| `showing the background colour #000000` | Nothing was set: no `--input`, nothing in the configuration file, nothing saved, or `veil clear`. Black is the default `--bg-color`. | `veil set --input FILE`. |
| `starting ...` | A video waits for its first frame. A slow disk or a decoder that is being loaded can take a moment; after 10 seconds it counts as failed. | Wait; then see the log. |
| `stopped after an error (see the log)` | The video cannot be played. | The log line `Failed to show the wallpaper on ...` says why; the most common reasons are below ([A video does not play](#a-video-does-not-play-a-gstreamer-element-is-missing)). |
| `stopped while the compositor does not show it` | Nobody is shown the video right now. | Nothing: it plays again when the output is drawn again. `--release-unseen-after 0` turns this off. |
| `showing` or `playing`, and the frame counts go up, but something else is on screen | Another program draws on the same layer: another wallpaper daemon (`swaybg`, `hyprpaper`, `swww`, `mpvpaper`, ...). In sway, the one started last is on top, and a later `veil set` does not change that (seen with `swaybg` 1.2.2). | Stop the other program, or remove it from the compositor's configuration (sway starts `swaybg` for an `output * bg ...` line). |

**For developers: a video needs a buffer under it.** `waylandsink` draws the video into a subsurface of veil's surface, and a subsurface is shown only when its parent surface is mapped, that is, has a buffer attached and committed. veil puts the video's background colour under every video (`RenderTarget::fill`). A build that leaves the buffer out shows the compositor's background where the video should be: tried with the three `fill` calls of `engine/video.rs` removed, the screen showed sway's own background colour, nothing moved, the sink counted every frame as dropped, and two seconds later `veil status` said `stopped while the compositor does not show it`.

## A video does not play: a GStreamer element is missing

veil builds every video pipeline from GStreamer elements found at run time. If one is missing, the video cannot be shown:

| The log or `veil set` says | Missing | In the plugin set |
|---|---|---|
| `Internal GLib/GStreamer error: no element "waylandsink"` | the video sink | `gst-plugins-bad` |
| `Internal GLib/GStreamer error: no element "videoconvert"` | the converter | `gst-plugins-base` (or [a stale registry](#a-stale-gstreamer-registry), or [plugins of another GStreamer](#plugins-of-another-gstreamer-version)) |
| `Internal GLib/GStreamer error: no element "videocrop"` | the crop of `fill` | `gst-plugins-good` |
| `GStreamer pipeline error: the video cannot be played: Your GStreamer installation is missing a plug-in.`, after a warning `No decoder available for type 'video/webm'` | a demuxer or a decoder for this file | see below |

The type in the warning says which: `video/webm` and `video/x-matroska` need `matroskademux`, `video/quicktime` (MP4, MOV) `qtdemux`, both from `gst-plugins-good`; VP8 and VP9 are decoded by `gst-plugins-good` (`vp8dec`, `vp9dec`), H.264 and HEVC in software by `gst-libav` (`avdec_h264`, `avdec_h265`); `image/gif` (an animated GIF) needs `gst-libav` (`avdemux_gif`, `avdec_gif`). Hardware decoders come from `gst-plugins-bad` (`va`, `nvcodec`) and need a driver (see [No hardware decoding](#no-hardware-decoding)). The package names on Arch, Debian, Ubuntu and Fedora are in the README ([From source](../README.md#from-source)); Fedora's own `gst-libav` has no H.264 or HEVC decoder (`openh264dec` decodes H.264 there).

What happens then:

* `veil set --input FILE`: it prints the error and exits with status 1; the wallpaper on screen stays.
* `veil daemon --input FILE`: a missing `waylandsink`, `videoconvert` or `videocrop` stops the daemon (status 1, with a desktop notification). A missing demuxer or decoder does not: the daemon runs, the output shows the background colour and `veil status` says `stopped after an error`.
* Images do not go through GStreamer: a daemon without `gst-plugins-bad` showed a picture as usual.

**With the Nix package** all of these come with it (`base`, `good`, `bad`, `libav`; the package's `plugins` check looks up every element veil uses). A missing element there means a stale registry or plugins of another GStreamer on `GST_PLUGIN_PATH` (the next two sections).

**On other distributions**, install the plugin sets. Their packages are usually named after them: `gst-plugins-bad` (Arch), `gstreamer1.0-plugins-bad` (Debian, Ubuntu), `gstreamer1-plugins-bad-free` (Fedora), and the same for `base`, `good` and `libav` (not checked on those distributions). Check with GStreamer's own tool, which sees the same plugins as veil:

```bash
gst-inspect-1.0 waylandsink videoconvert videocrop > /dev/null && echo "all there"
gst-inspect-1.0 --exists avdec_h264 || echo "no software H.264 decoder"
```

Inside veil's Nix dev shell, `gst-inspect-1.0` may be the system's and list the system's plugins too; it is no check of the package.

## An animated GIF stands still, does not start, or keeps a core busy

With GStreamer before 1.26, GStreamer's GIF demuxer fails (known issue 8; seen with the release tarball): on Ubuntu 22.04 and Debian 12 the GIF stands still on one of its first frames while `veil status` says `playing`, and on Debian 12 the daemon also uses a whole CPU core; on Ubuntu 24.04 `veil set` fails with `the video did not show a frame within 10s`. Images and other videos are not affected, and neither is the Nix package, which brings GStreamer 1.28. Convert the GIF to a video, which plays on all of them:

```bash
ffmpeg -i anim.gif -c:v libvpx-vp9 -pix_fmt yuv420p -vf 'scale=trunc(iw/2)*2:trunc(ih/2)*2' anim.webm
veil set --input anim.webm
```

(`scale` makes the width and height even, which the colour format needs.)

## A stale GStreamer registry

GStreamer keeps a cache of the plugins it found, the *registry*. A plugin that failed to load once is recorded as failed, and left out for as long as its file keeps its path, modification time and size, even if the reason has gone. In the Nix store files never change, so it is left out for good. Since `nvcodec` declares no dependency on the driver, the same holds for the NVIDIA decoders: a registry written while the driver could not be reached keeps them out after the driver works again (tried: written with `CUDA_VISIBLE_DEVICES=` set, the next start without it still decoded in software). The `va` plugin is scanned again when the render devices, `LIBVA_DRIVER_NAME` or the driver directory change.

**Symptoms:** `no element "videoconvert"` (or another element) although the plugins are installed; a hardware decoder that GStreamer used before is not used any more.

**Fix:** delete the registry; the next start writes a new one (that start takes a second or two longer).

```bash
rm ~/.cache/veil/gstreamer-registry.bin          # the Nix package's (or under $XDG_CACHE_HOME)
rm ~/.cache/gstreamer-1.0/registry.*.bin         # GStreamer's shared one: every other build of veil
```

`GST_REGISTRY` (or `GST_REGISTRY_1_0`), if set, names the file instead. To see what the registry has left out, ask `gst-inspect-1.0` with the same environment: `gst-inspect-1.0 -b` lists the plugins it has blacklisted. The Nix package keeps a registry of its own for this reason: the shared one is rewritten by every GStreamer program with its own plugins (it once lost the base plugins on the developer's machine, 2026-10-15).

Reproduced: a copy of `libgstvideoconvertscale.so` damaged without changing its size or time stamp made the daemon fail with `no element "videoconvert"` (the log had `Failed to load plugin ...: invalid ELF header`); repaired byte for byte, it still failed, and `gst-inspect-1.0 -b` listed it; with the registry deleted the video played.

## Plugins of another GStreamer version

`GST_PLUGIN_PATH` is searched before the plugin set veil comes with (also before the Nix package's). Plugins there that belong to another GStreamer version are loaded in place of veil's, and they break the ones that are left: with the plugins of GStreamer 1.26 on `GST_PLUGIN_PATH`, the 1.28 `videoconvert` could not be loaded (`Failed to load plugin '.../libgstvideoconvertscale.so': ... undefined symbol: gst_video_meta_transform_matrix_init`) and the daemon stopped with `no element "videoconvert"`. Unsetting the variable did not help: the registry kept the failure until it was deleted.

**Fix:** start veil without `GST_PLUGIN_PATH` (under systemd: `systemctl --user edit veil`, then `UnsetEnvironment=GST_PLUGIN_PATH` under `[Service]`), and delete the registry as above. `GST_DEBUG=GST_PLUGIN_LOADING:4` logs the file every plugin is loaded from.

## No hardware decoding

The log names the decoder of every video, and whether it runs on the GPU:

```text
INFO veil::engine::video: video decoder: nvh264dec (hardware)
INFO veil::engine::video: video decoder: avdec_h264 (software)
```

A video decoded in software still plays; it costs more CPU. What GStreamer can use depends on the system:

* **Intel and AMD:** the `va` decoders (`vah264dec`, `vah265dec`, `vavp9dec`, `vaav1dec`, ...) need a VA-API driver (`intel-media-driver` or Mesa's `radeonsi`). `vainfo` (from `libva-utils`) shows whether one works and which codecs it decodes. On NixOS the drivers come from `hardware.graphics` (`hardware.graphics.enable = true`, plus `extraPackages` for the Intel driver).
* **NVIDIA:** the `nvcodec` decoders (`nvh264dec`, `nvh265dec`, `nvvp9dec`, `nvav1dec`, ...) need the proprietary driver's CUDA and video decoding libraries; on NixOS they come with `hardware.nvidia`. The NVIDIA VA-API driver is used only with `GST_VA_ALL_DRIVERS=1`, and is no better here (known issue 7).
* **The codec:** a GPU decodes only the codecs it has hardware for (AV1, for instance, only on recent cards). For the others GStreamer picks a software decoder.
* **Nix on another distribution:** the Nix-built libraries look for drivers in `/run/opengl-driver` and in the usual system directories, but a driver built for the distribution may not load into them. Not tested.
* **A registry written without the driver** keeps the NVIDIA decoders out ([A stale GStreamer registry](#a-stale-gstreamer-registry)).

Hardware decoding does not make a video free: on NVIDIA every frame is still converted to RGB on the CPU (about 18 % of a core for 1080p at 30 fps). On Intel and AMD the frames can go to the compositor without that (as dmabufs), which has not been verified ([`known-issues.md`](known-issues.md), entry 7).

### Forcing software decoding

When a hardware decoder misbehaves (a driver bug, a stream it shows wrongly), take it out of GStreamer's choice by giving it the rank 0 (`NONE`): GStreamer then picks the next decoder for that format. The name is the one in the log:

```bash
GST_PLUGIN_FEATURE_RANK=nvh264dec:0 veil daemon
# several, comma-separated
GST_PLUGIN_FEATURE_RANK=nvh264dec:0,nvh265dec:0,nvvp9dec:0,nvav1dec:0 veil daemon
```

Measured: `nvh264dec:0` (or `nvh264dec:NONE`) and the H.264 clip was decoded by `avdec_h264 (software)`; `nvvp9dec:0` and the VP9 clip by `vp9dec (software)`. Under systemd, `systemctl --user edit veil` and add `Environment=GST_PLUGIN_FEATURE_RANK=nvh264dec:0` under `[Service]`. The variable is read when the daemon starts; it does not change the registry. (A decoder option of veil's own is planned after 1.0, see [`roadmap.md`](roadmap.md).)

## A video opens in a window of its own

**Symptom:** a normal window (in sway: tiled, with a title bar, its `app_id` the program's name) plays the video, and the wallpaper itself shows only its background colour.

**Cause:** `waylandsink` did not get veil's Wayland connection, so it opened its own and made a window there. veil hands the connection to it in a GStreamer context of the type `GstWaylandDisplayHandleContextType` (`engine::video::display_context`), with the display pointer in the field `handle`. GStreamer 1.28.7 reads the field `display` first and `handle` "for backward compatibility" (`gst-libs/gst/wayland/gstwlcontext.c`); in a test both names worked, and a field of another name gave exactly this window. veil keeps `handle`, the field GStreamer keeps reading for compatibility (which field versions before 1.28 read was not checked).

This does not happen with veil as it is released. If you see it, report it with the GStreamer version (the first line of the log) and a log with `GST_DEBUG=waylandsink:5`.

## An output name is not found

Output names are the compositor's (`DP-3`, `HDMI-A-1`, `eDP-1`, ...), and they are case-sensitive. `veil outputs` lists the outputs the compositor has and which ones the daemon draws on; `hyprctl monitors` and `swaymsg -t get_outputs` give the same names.

| Where | What happens |
|---|---|
| `veil daemon --output DP-9`, and no output has that name | The daemon does not start (status 1): `Output 'DP-9' was not found; available outputs: HEADLESS-1`. The same for `--output headless-1` when the output is `HEADLESS-1`. |
| `--output DP-9 --output HDMI-A-1`, and only `HDMI-A-1` is there | The daemon starts and waits: `WARN veil::wayland: output DP-9 not connected; waiting for it (connected: HDMI-A-1)`. `DP-9` gets its wallpaper when it is connected. |
| `veil set --output DP-9` (also `clear`, `pause`, `resume`, `toggle`) | Refused at once, status 1: `output 'DP-9' is not driven by this daemon; it draws on HEADLESS-1`. |
| `[output."DP-9"]` in the configuration file | Used when an output of that name is connected; `veil config check` does not know which outputs exist and does not complain. |

An output that has no name (rare: a compositor that does not send `wl_output.name`) gets the default wallpaper and cannot be named.

## Other messages

| Message | Meaning |
|---|---|
| `GStreamer-Wayland-WARNING **: Could not bind to zwp_linux_dmabuf_v1` | The compositor offers no dmabufs (a headless sway that renders on the CPU, for instance). Frames go through shared memory; nothing else changes. |
| `XDG_RUNTIME_DIR is not set to an absolute path; veil keeps its control socket there` | Not in a normal desktop session. Start veil from the session (or its systemd user manager). |
| `veil daemon` exits with status 4 | A daemon already runs for this compositor. `veil quit` stops it. |
| `Lost the connection to the Wayland compositor`, status 5 | The compositor exited or crashed. veil does not reconnect; the next session starts it again. |
| The daemon dies with SIGSEGV (status 139) when a monitor is changed, plugged in or powered on while a video plays | Known issue 1, a bug in GStreamer's `waylandsink`. Run veil under its systemd unit: it is back within about three seconds with its wallpapers. |
| A configuration file error, status 2 | The message has the file, line and column. `veil config check` says the same without a daemon. |

## Debug cheat-sheet

| What you want | Command |
|---|---|
| Is a daemon running? | `veil ping` (status 3: none) |
| What each output shows, its state, size and scale, the video's frame counts | `veil status` (`--json` for scripts) |
| The outputs and their names | `veil outputs` |
| What the configuration file amounts to, and whether its wallpapers can be shown | `veil config check` |
| The log under systemd | `journalctl --user -u veil -f` |
| More from veil: each load, switch, pause and what the compositor reports | `RUST_LOG=veil=debug veil daemon` |
| Everything, also the libraries veil uses | `RUST_LOG=debug veil daemon` |
| The video sink: buffers, the window, the render rectangle | `GST_DEBUG=waylandsink:5 veil daemon` |
| Which file every GStreamer plugin is loaded from, and which fail | `GST_DEBUG=GST_PLUGIN_LOADING:4 veil daemon` |
| Whether `videocrop` crops without a copy (`in-place transform using crop meta`) | `GST_DEBUG=videocrop:5 veil daemon --mode fill` |
| A GStreamer log in a file of its own | `GST_DEBUG_FILE=/tmp/gst.log` with any of the `GST_DEBUG` lines |
| Every Wayland request and event (`{GStreamer display queue}` lines are the video sink's) | `WAYLAND_DEBUG=1 veil daemon` |
| Take a hardware decoder out | `GST_PLUGIN_FEATURE_RANK=nvh264dec:0 veil daemon` |
| A fresh GStreamer registry, to rule out a stale one | `GST_REGISTRY=/tmp/veil-registry.bin veil daemon` |
| Under systemd | `systemctl --user edit veil`, then `Environment=RUST_LOG=veil=debug` (or any variable above) under `[Service]`, and `systemctl --user restart veil` |

## Reporting a problem

Reports go to [Discussions, category "Reports"](https://github.com/ordreth/veil/discussions/categories/reports) (veil takes no issues or pull requests; nothing is promised in return). A report is most useful with the first line of the log (veil's and GStreamer's versions), `veil status`, the compositor and its version, the graphics card and its driver, and the `video decoder:` line for a video. Say what you did and what you expected, and what happened. A security problem is not reported there: see [`SECURITY.md`](../SECURITY.md).

## Support matrix

What was run, where, and by whom. The labels: **Verified by owner** (the project owner ran it), **Reported working** (run once, by the project's tooling or a user, not yet confirmed by the owner), **Untested**, **Unsupported**. The details of each row are in the [compatibility table](#compatibility) below, which is the source of this one.

| | Status | What, when |
|---|---|---|
| **Compositors** | | |
| sway (headless, 1.12) | Verified by owner | Every check of `tests/headless.rs` and the measurements; 2026-10-19. |
| Hyprland 0.56.2 | Reported working | Images, H.264 video on two monitors and the automatic pause, run on the owner's machine with an agent's help, 2026-10-20. Smoothness, lock, DPMS, logout and crash recovery are **Untested**. |
| river, niri, Wayfire, labwc, other `wlr-layer-shell` compositors | Untested | Should work. No automatic pause (every video counts as visible), only the release of a video nobody is shown. |
| KDE Plasma (KWin) | Untested | Has `wlr-layer-shell`; nobody has tried. |
| GNOME (Mutter) | Unsupported | No `wlr-layer-shell`. |
| **Graphics** | | |
| NVIDIA (NVDEC) | Reported working | RTX 3060 Ti, driver 595.91.07, sway and Hyprland. Every video frame is converted on the CPU ([known issue 7](known-issues.md)). |
| Intel, AMD (VA-API) | Untested | The zero-copy path is not verified on any hardware. |
| Software decoding | Reported working | In the tests and by taking the hardware decoders out ([troubleshooting](#no-hardware-decoding)). |
| **Architectures** | | |
| x86_64 | Reported working | The tarball in a container of six distributions, 2026-10-19. |
| aarch64 | Untested | The release workflow builds it and runs the tests there; it was never run with a compositor. |
| **Distributions** (the prebuilt tarball, in a headless sway) | | |
| Debian 12 and 13, Ubuntu 22.04 and 24.04, Fedora 44, Arch Linux | Reported working | 2026-10-19. Animated GIFs need GStreamer 1.26 or newer, and Ubuntu 24.04 drops frames of 1080p videos ([known issue 8](known-issues.md)). |
| NixOS (the flake) | Verified by owner | The owner's machine, 2026-10-20. |

What a user should know before using veil with several monitors and a video: a monitor that is changed, plugged in or switched on while a video plays crashes the daemon ([known issue 1](known-issues.md)); under the systemd unit it comes back within seconds.

## Compatibility

What veil has been run on, and what worked. **Testers: please report a row** in [Discussions, category "Reports"](https://github.com/ordreth/veil/discussions/categories/reports): the compositor and its version, the graphics card and driver, the veil and GStreamer versions (the first line of the log), and for each column whether it worked, with what was tried. "Not yet" means nobody has reported it, not that it fails.

| Compositor | GPU, driver | Images | Video | Decoder (from the log) | Automatic pause | Notes | Checked |
|---|---|---|---|---|---|---|---|
| sway 1.12, headless, CPU rendering (`pixman`) | NVIDIA RTX 3060 Ti, 595.91.07 | yes | yes: H.264, HEVC, VP8, VP9, AV1 | `nvh264dec`, `nvh265dec`, `nvav1dec`, `nvvp8dec`, `nvvp9dec` (hardware); software ones when the hardware ones are taken out | yes (sway's IPC) | Where every check of `tests/headless.rs` and the measurements run. Known issue 1: a monitor change while a video plays kills the daemon. Frames are converted on the CPU (known issue 7). | veil 0.1.0, GStreamer 1.28.7, 2026-10-19 |
| sway 1.12, headless, GPU rendering (`gles2`) | NVIDIA RTX 3060 Ti, 595.91.07 | not yet | not yet | | not yet | The sink pipeline of veil was run here with `gst-launch-1.0` (veil itself was not run): sway offers dmabufs, and takes NV12 ones only with NVIDIA's tiled modifiers. | |
| Hyprland 0.56.2 | NVIDIA RTX 3060 Ti (the machine of the first row) | yes (the owner's daily wallpaper, through the systemd unit) | yes: H.264 1080p60 on two 1080p monitors at once | `nvh264dec` (hardware) | yes: a fullscreen window paused its output ("cannot be seen; pausing it"), closing it played again; the other output kept playing | About 90 to 99 % of one core for two 1080p60 streams (known issue 7), about 41 % with one output paused. Smoothness without lag (known issue 6), lock, DPMS, logout, crash recovery and a monitor change were not checked on Hyprland. The state veil reads from its IPC matches `hyprctl monitors`. | veil 0.1.0 (a release build of the tree), GStreamer 1.28.7, 2026-10-20 |
| sway, on hardware | any | not yet | not yet | | | | |
| Hyprland | Intel or AMD (VA-API) | not yet | not yet | | | Would show whether the zero-copy path works (known issue 7, option A). | |
| river, niri, Wayfire, labwc, KDE Plasma (wlr-layer-shell) | any | not yet | not yet | | no (pause only behind a locked screen or a monitor that is off) | Should work: veil needs `wlr-layer-shell`; `wp_viewporter` and `wp_fractional_scale_v1` are used when they are there. | |
| GNOME (Mutter) | any | no | no | | | Has no `wlr-layer-shell`, which veil needs. | |

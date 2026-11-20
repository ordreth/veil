# Known issues and the decisions behind them

This file records problems that veil cannot (or deliberately does not) solve today, what was measured, which options were considered, and what was decided. It exists so that whoever comes back to one of these later does not have to rediscover it.

Each entry has the date of the last check. Re-check before relying on it: GStreamer and compositors change.

---

## 1. A monitor change while a video plays crashes the daemon (GStreamer `waylandsink` bug)

**Status:** fixed after 1.0.0-beta.1 (2026-11-09): veil no longer uses `waylandsink` (option E, see "Fixed: veil's own sink" at the end of this entry). The bug itself is still in GStreamer and was not reported upstream.
**Affected:** video wallpapers of 1.0.0-beta.1. Static images and the solid background never were.
**Last checked:** 2026-11-09, headless sway 1.12, GStreamer 1.28.7: none of the changes of the tables below ends the daemon any more (`output_changes_while_videos_play_leave_them_playing` in `tests/headless.rs`). Not checked on Hyprland yet. What follows up to "Fixed" is the history.

### Symptom

The daemon dies with SIGSEGV (exit status 139 from a shell). There is no clean shutdown: no log line from veil, and the IPC socket file is left behind (harmless: the next daemon replaces it, and `veil set` reports that no daemon is running). The core dump shows the crash on the thread named `GstWlDisplay`:

```text
#0 g_mutex_lock            (libglib-2.0.so.0)
#1 output_done             (libgstwayland-1.0.so.0)
#2 ffi_call_unix64 ...
#5 wl_closure_invoke       (libwayland-client.so.0)
#7 wl_display_dispatch_queue_pending
#8 gst_wl_display_thread_run (libgstwayland-1.0.so.0)
```

### What triggers it

Measured in a headless sway 1.12 with a video playing:

| Setup | Action | Result |
|---|---|---|
| One output | Change its scale | **crash** |
| One output | Disable it, enable it again (10 times) | works: the video restarts each time |
| One output | The compositor exits or is killed | clean exit, status 5 (`EXIT_COMPOSITOR_GONE`; not this bug) |
| Two outputs, video on A | Disable B | **crash** |
| Two outputs, video on A | Change the scale of B | **crash** |
| Two outputs, video on A | Move B (change its position) | **crash** |
| Two outputs, video on A | Plug in a third output | **crash** |
| Two outputs, video on A | Disable A itself | **crash** |
| Two outputs | Nothing changes | works |

In short: with a single monitor, unplugging and replugging it is fine. With more than one monitor, any change to the monitor layout kills a playing video, because the compositor then re-sends the state of the monitors that stay.

Measured again with multi-monitor support (2026-10-14, headless sway 1.12, two outputs, every output with a surface of its own):

| Setup | Action | Result |
|---|---|---|
| Images on both outputs | Add a third output, disable one, enable it, unplug one | works: the new output gets the wallpaper, the others are unchanged (0 differing pixels) |
| Video on both outputs | Add a third output | **crash** |
| Video on both outputs | Disable the second output | **crash** |
| Video on the first, image on the second | Disable the second | **crash** |
| Image on the first, video on the second | Disable the first | **crash** |
| Video on the first, second disabled before | Enable the second | **crash** |

Same core dump (`output_done` on the thread `GstWlDisplay`). Nothing changed compared with one surface: a single pipeline was already killed by a change of any other output, because every sink binds every output. What did change is how often it is met, since the daemon now draws on every output by default: a video wallpaper on a multi-monitor setup plays on each monitor, and the first change of the layout ends all of them.

Measured when unseen videos were released (2026-10-15, headless sway 1.12, one output, `swaymsg output HEADLESS-1 power off` and `power on`):

| Setup | Action | Result |
|---|---|---|
| Video, without the release of unseen videos | Power the output off, then on | off: works (the video goes on decoding); on: **crash** |
| Video, with the release | Power off for 4 s, then on | works: the video was stopped after 2 s (it was not shown), and starts again at power on |
| Video, with the release | Power off and on again within 1 s | **crash** (the video was not stopped yet) |
| Video | Lock the session (`swaylock`), unlock | works |

At power on sway re-sends the geometry, scale and `done` of the output, to every client, and the sink's own `wl_output` takes the second `done`. A monitor that is put to sleep by an idle daemon (DPMS) and woken up again is therefore a crash in sway, whenever a video is still alive at that moment.

It was reproduced in sway only. From the cause below it should happen on every compositor, including Hyprland, whenever the compositor sends a second `wl_output.done` for an output. Which actions make Hyprland do that has **not** been tested (the developer's session was deliberately left alone).

### Cause

`waylandsink` shares veil's Wayland connection. It creates its own helper object (`GstWlDisplay`), which binds every `wl_output` the compositor announces and handles their events on its own thread.

In `gst-libs/gst/wayland/gstwldisplay.c`:

- `registry_handle_global` stores a back pointer on each output object: `g_object_set_data (G_OBJECT (output), "display", self);`
- `output_done` reads it with `g_object_steal_data (G_OBJECT (output), "display")`, which *removes* it, and then uses it without a `NULL` check (`gst_wl_display_get_instance_private (self)`, then `g_mutex_lock (&priv->outputs_mutex)`).

So the code assumes exactly one `done` event per output. The compositor sends `done` again after every change of the output (`wl_output` protocol: "sent after all other properties have been sent"), and the second one dereferences `NULL`.

A fix has to make `output_done` safe to call more than once per output. Replacing `g_object_steal_data` with `g_object_get_data` alone is not obviously enough, because the handler also (re)inserts the output into a hash table that owns a reference; check that before writing a patch.

### Why veil cannot prevent it

- The events go straight from libwayland to GStreamer's thread. No veil code runs in between, so there is nothing to intercept, and veil learns about the change at the same moment.
- `waylandsink` binds the outputs itself. There is no property to turn that off, and the sink must use veil's connection (the video is a subsurface of veil's surface), so it cannot be given a filtered one.
- The helper object lives as long as the sink does, even in the `Null` state. Only destroying the sink stops it.

### What veil does about it

- The video pipeline (and with it the sink and its thread) exists only while there is a surface to play on. When the wallpaper's output goes away, `VideoEngine::detach` drops the pipeline before the surface is destroyed, and a new pipeline is built when the output returns. Nothing can crash while the output is away.
- A video whose frames the compositor has refused for 2 seconds (`--release-unseen-after`) is stopped and its pipeline dropped, and built again when the output is drawn again. A monitor that has been off for longer than that has no sink to kill when it comes back. This is not a fix: it does not cover a monitor switched off and on within 2 seconds, a video paused by the automatic pause (not watched), `--release-unseen-after 0`, or any other output change.
- The wallpaper of each output is saved (`$XDG_STATE_HOME/veil/state.json`), and a daemon started again without `--input` shows the same wallpapers. Checked in a headless sway on 2026-10-16: a scale change of the output a video played on killed the daemon (status 139), and the daemon started again by hand showed the image and the video it had.
- The systemd user unit (`contrib/systemd/veil.service`, `Restart=on-failure`, `RestartSec=2`) restarts the daemon after the crash, and it comes back with those wallpapers: option D is done. Checked in a headless sway under the developer's systemd user manager: the scale change of the output a video played on killed the daemon (`code=dumped, status=11/SEGV`), and 2.9 s later the unit was active again with the video playing. This needs the daemon to run under the unit; started by hand, it still has to be started again by hand. A test of it is still to be written ([`testing.md`](testing.md), "Checks to be turned into tests").
- The limitation is stated in the README ("Known limitations"), in the module docs of `src/engine/video.rs`, and here.

### Options considered

| Option | What it gives | Why it was not done (yet) |
|---|---|---|
| **A. Accept and document** | Nothing breaks that was not already broken. Images are fully robust. | *Chosen.* |
| **B. Patch `gst-plugins-bad` in the Nix flake** | The crash disappears for Nix builds (dev shell and package). | GStreamer's plugin set is then compiled from source: a long first build locally and on every CI run without a binary cache. Other distributions still crash. Could be combined with the Nix wrapper. |
| **C. Report the bug upstream** | A fix for everyone, eventually. | Declined for now by the maintainer (2026-10-10). No existing upstream report was found at that time. |
| **D. Restart after the crash** | The wallpaper comes back by itself within a second or two. | Needs a supervisor (the systemd unit with `Restart=on-failure`) and persistence of the last wallpaper, otherwise the restarted daemon shows the startup wallpaper. Both exist; together they are the cheapest real mitigation. A supervisor built into veil (a parent process that restarts a worker) would do the same without systemd, at the cost of process-management code. |
| **E. Replace `waylandsink`** | Removes the bug and this whole class of dependency on `waylandsink` (see also issue 2). | Large: veil would attach video buffers itself (for example `appsink` plus DMABUF/SHM buffers on its own subsurface, or a GL path). It overlaps with the zero-copy investigation (entry 7), which is the natural place to decide it. |
| **F. Work around it inside the process** | (nothing clean) | The only ways are reaching into GStreamer's private objects or interposing GLib symbols. Rejected as too fragile. |

### Decision (2026-10-10)

Option A. Do everything that is possible on veil's side, document the rest, and do not report it upstream for now. Replacing `waylandsink` (option E) may be reconsidered later, most likely together with the zero-copy question (entry 7). Option D is done (2026-10-16): the systemd unit and the saved wallpapers.

### When to revisit

- A GStreamer release changes `output_done` in `gstwldisplay.c` (check whether the `g_object_steal_data` line is still there).
- ~~The zero-copy investigation~~: decided on 2026-10-17: veil keeps `waylandsink` for 1.0. The measurements did not make replacing it necessary: what a video costs is the colour conversion, which any sink that shows NVIDIA's frames through shared memory needs (entry 7). Option E stays after 1.0; for 1.0 the answer to this issue is option D (the systemd unit and the saved wallpapers), still to be checked on Hyprland by hand.
- ~~Multi-monitor support~~: done; measured again (see the second table above). As expected, the bug is the same and meets more setups, because every monitor now plays the video. It is the strongest argument for option B, D or E so far.
- The saved wallpapers and the systemd unit are done (2026-10-16, see "What veil does about it"): under the systemd unit a crash recovers by itself. A test of it is still to be written ([`testing.md`](testing.md), "Checks to be turned into tests").

### Fixed: veil's own sink (2026-11-09)

Option E. veil shows a video's frames itself: `PlaneSink` (`src/engine/video/sink.rs`), a GStreamer video sink of veil's own, converts every frame into a `wl_shm` buffer of a subsurface that veil's Wayland code makes (`src/wayland/plane.rs`). The subsurface's objects are made on veil's own event queue, so their events are handled by the event loop, and nothing binds a `wl_output` but veil's own code. With that the crash is gone, and so is the reason for the restart under the systemd unit and for most of the release of unseen videos (both stay: the unit for any other crash, the release because it saves the CPU of a video nobody sees).

What it took that `waylandsink` did by itself: the frame-callback pacing (a frame that comes while the compositor has not taken the one before is dropped and counted, so `wayland::unseen` still works), the preroll frame (shown in `Paused`, and a preroll must never report a drop, or `basesink` never finishes the state change: found when a video paused behind a fullscreen window never played again), and the position of the subsurface (state of its parent, committed by the event loop when the sink reports a move).

What was gained besides: the conversion and the crop of `fill` and `center` happen in one pass, straight into the compositor's memory, and no pixel is converted that is cropped away (entry 7); the event loop wakes about half as often while a video plays (`measurements.md`, "veil's own sink"). Scaling down on the CPU a frame larger than its place was tried and dropped: GStreamer's converter is slower when it scales than when it converts at the frame's own size (`src/engine/video/sink.rs`). What it costs: memory, in a compositor that holds the frame it shows until the next one replaces it (a headless sway): the sink then grows from two buffers to three, about 100 MB more than `waylandsink` for a 4K video. What was lost: frames in dmabufs. The sink takes system memory only; on NVIDIA that was all there was (entry 7), on Intel and AMD with VA-API a decoder that could hand the compositor dmabufs now hands veil system memory, which was never measured either way (no such hardware).

The test that expected the crash became `output_changes_while_videos_play_leave_them_playing`: two outputs playing the same clip, a scale change, a move, a disable and an enable, a power off and on within a second, and a new output; the daemon runs and every video moves afterwards.

### How to reproduce (1.0.0-beta.1)

This is the procedure used to find the bug, written out by hand (sway and veil's GStreamer plugins must be available, for example `nix shell nixpkgs#sway` inside `nix develop`):

```bash
# A private headless compositor, so the real session is not touched.
# XDG_RUNTIME_DIR must be a short path (Unix socket path limit).
export XDG_RUNTIME_DIR=/run/user/$UID/veil-test && mkdir -p -m 700 "$XDG_RUNTIME_DIR"
env -u WAYLAND_DISPLAY WLR_BACKENDS=headless WLR_RENDERER=pixman \
    WLR_LIBINPUT_NO_DEVICES=1 LIBSEAT_BACKEND=noop sway &
export WAYLAND_DISPLAY=wayland-1 SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*)

./target/release/veil daemon --input some-video.webm &
sleep 3
swaymsg output HEADLESS-1 scale 2      # the daemon segfaults here
coredumpctl info veil                  # shows output_done on thread GstWlDisplay
```

---

## 2. A video pipeline cannot move to another surface

**Status:** no longer forced since veil shows its videos itself (2026-11-09, entry 1); the design below is kept. `PlaneSink` can be given another plane, so a pipeline could be kept while its output is away and moved to the output's new surface, and a video would then go on where it was instead of starting again. Not done: a pipeline kept for an output that is gone holds its memory and its threads.
**Last checked:** 2026-10-10, GStreamer 1.28.7.

`waylandsink` cannot change its window handle: a second `set_window_handle` only logs "changing window handle is not supported". It also keeps its window and display objects until the element is finalized, even in the `Null` state.

Consequences in veil:

- A pipeline serves exactly one surface (`Player` in `src/engine/video.rs`). When the surface is lost, the pipeline is dropped and rebuilt on the next `attach`. The video then starts from the beginning.
- This does not conflict with the rule "never recreate the pipeline on video loops": loops still seek. A rebuild happens only when the surface itself is gone.

---

## 3. Memory is not given back after video switches (glibc allocator arenas)

**Status:** mitigated where veil is packaged (option C): with one allocator arena, which the Nix package and the systemd unit set (decided 2026-10-16, measured 2026-10-17), the heap does not rise over 500 switches, and playback costs the same. Open for a binary started without the variable, which gets glibc's default and the growth below. B is not needed. See "Measured (2026-10-17)".
**Affects:** a daemon that has replaced one video by another, or whose output was unplugged and replugged while a video played. Also every time a video is stopped because nobody is shown it (a locked screen, a monitor that is off) and started again: one pipeline built per lock or monitor-off, per output. A daemon that only shows images is not affected. A video that only loops rebuilds nothing, and kept nothing over ten minutes of loops; days are left to the 24-hour soak of the release checklist ([`roadmap.md`](roadmap.md)).
**Last checked:** 2026-10-17, glibc 2.44, GStreamer 1.28.7, release build, 12 CPU threads, headless sway. First measured on 2026-10-13; seen again after the atomic engine switching was added (200 video switches: 191 MB to 306 MB).

This entry used to describe the thread leak of replaced pipelines. That leak is fixed: the bus thread of a pipeline is now joined when the pipeline is dropped. Threads and file descriptors are flat over 600 video switches. What is described here was found while checking that fix, and exists with and without it.

### Symptom

The resident memory (RSS) of the daemon rises with the first video switches and does not come back, also not when an image replaces the video. It keeps rising slowly afterwards.

Measured in a headless sway 1.12 (1920x1080), alternating `veil set` between a 1280x960 and a 2560x1080 VP8 clip:

| State | RSS |
|---|---|
| Daemon started with an image, no video played yet | 18.6 MB |
| Image again after 1 video | 99.8 MB (64 MB are the GStreamer and codec libraries, which stay mapped) |
| Image again after 11 video switches | 183 MB |
| Image again after 111 | 244 MB |
| Image again after 301 / 511 / 601 (two runs) | 261 / 263 / 330 MB |
| A video playing, at the start | 190 MB |
| A video playing, after 200 switches | 336 MB |
| A video playing, after 10 unplug/replug cycles of its output | 238 MB, from 175 MB |

### Cause

It is free memory that glibc's allocator holds, not memory that anything still uses:

- GStreamer's own leaks tracer finds no object that outlives a switch (the same 224 registry objects are alive after 1, 11 and 111 switches, with an image on screen).
- With one allocator arena (`MALLOC_ARENA_MAX=1`) the same run is flat: 116, 124, 126 and 121 MB after 11, 111, 311 and 511 switches.
- `malloc_trim(0)`, called through a debugger after 100 switches with an image on screen, took the daemon from 226 MB to 118 MB.

A video pipeline runs about seven streaming threads, and every switch starts them anew. glibc gives threads arenas of their own (up to eight per CPU thread), and the large blocks a decoder frees in one of them go back to the system only when they happen to sit at the end of that arena. Over many switches more arenas come into use and each keeps what it once needed.

### Options considered

| Option | What it gives | What it costs |
|---|---|---|
| **A. Accept and document** | Nothing changes. The memory is bounded in theory (a fixed number of arenas), but no bound was reached in 601 switches. | The goal "no memory growth" is then not met for anyone who changes video wallpapers often (a playlist, planned after 1.0). The release checklist item "flat RSS over 500 wallpaper switches" cannot pass. |
| **B. Call `malloc_trim(0)` after a pipeline is torn down** | Gives back what is free: 108 of 226 MB in the measurement. | One `unsafe` call into glibc, which exists only there (it must be left out on musl). veil allows `unsafe` only for passing raw Wayland pointers, in one module (`engine::video`), so that rule would have to change. How long the call takes was not measured; it should not run on the event loop. A replaced video is shut down on a thread of its own (`veil-teardown`, `src/wayland/retire.rs`), which is the natural place for it. |
| **C. Fewer arenas** | With `MALLOC_ARENA_MAX=2`: 235 MB instead of 337 MB after 300 switches with a video playing. With `1`: flat, see above. | As an environment variable it only helps where veil is started through something that sets it (the systemd unit, a wrapper). Setting it from inside needs `mallopt`, an `unsafe` call like B. Decoder threads then share allocator locks; the effect on playback was not measured. It slows the growth and does not give memory back. |
| **D. Another allocator for the Rust code** (`#[global_allocator]`) | Nothing: the memory in question is allocated by GStreamer and the decoders through the C library, not by Rust code. | |
| **E. Keep one pipeline and change its file** | Fewer objects are rebuilt for a video-to-video switch. | Not measured, and the streaming threads are still started anew for every file, so the arenas may fill the same way. It was not tried. |
| **F. A fixed mmap threshold** (`MALLOC_MMAP_THRESHOLD_`) | Not measured. glibc raises its threshold for `mmap` each time a large block is freed, so after the first frames a decoder's buffers come from the arenas; a fixed threshold turns that off, and blocks above it are given back when they are freed. | An environment variable like C, with the same reach. Every large allocation becomes a system call. |

Every locked session and every monitor that is switched off rebuilds a pipeline per output when it comes back, so the memory grows with daily use and not only with manual switches. The decision was therefore taken before the soak tests.

### Decision (2026-10-16, the maintainer)

**C now, B only if the soak tests show that C is not enough.**

- `MALLOC_ARENA_MAX=2` is set where veil is started: the Nix wrapper sets it unless the variable is already set (`--set-default` in `nix/package.nix`; the flake check `plugins` checks both cases), and the systemd unit has `Environment=MALLOC_ARENA_MAX=2` (`tests/systemd.rs` checks it). No code changed, and `unsafe` stays where it was. A binary started some other way (`cargo build`, the headless tests, `scripts/bench/` without the variable) runs with glibc's default.
- The switching soak measures the daemon with `2` (what ships), `1`, and glibc's default, and F if there is time, together with what each costs playback (frames shown, CPU). If none of them is flat, B comes in: `malloc_trim(0)` on the `veil-teardown` thread, and a second place that is allowed `unsafe`, for that one call.
- Why not B at once: it widens the `unsafe` rule for something an environment variable may settle, and with `1` the run was flat. Why `2` and not `1`: `1` makes the seven or so decoder threads share one allocator lock, and what that costs playback was not measured; the soak measures it and may change the number.
- What C does not give: memory that glibc already holds is not given back, so with `2` the growth is slower, not gone (235 MB instead of 337 MB after 300 switches). Users who build veil themselves and do not use the unit get none of it.

### Measured (2026-10-17): the number became 1

The switching soak (`scripts/bench/soak.sh switch`: 500 `veil set`, images and videos in turn, H.264 1280x960 and VP9 2560x1080 through the NVIDIA decoders) and `bench.sh` for what each setting costs playback; the numbers are in [`measurements.md`](measurements.md), "Soak tests and the allocator". The heap (`RssAnon`) with an image on screen:

| `MALLOC_ARENA_MAX` | After 1 video | After 250 videos | Over switches 100 to 500 |
|---|---|---|---|
| 1 | 42.5 MiB | 42.2 MiB | flat |
| 2 | 41.2 MiB | 73.0 MiB | flat from the tenth switch on |
| glibc's default (96 arenas) | 41.4 MiB | 129.1 MiB | rising (+6.25 KiB per switch) |
| default with `MALLOC_MMAP_THRESHOLD_=131072` (F) | 22.4 MiB | 107.9 MiB | rising (+0.86 KiB per switch) |

Playback cost the same with all three numbers, within the noise of a run: CPU and frames per second of a 1080p60 and a 2160p60 H.264 clip with the NVIDIA decoder, of the 2160p60 clip in software (150 to 152 % of a core, 60 frames per second), and of two videos at once on two outputs (two 4K streams in software: 363.5, 361.9 and 357.6 % with the default, 2 and 1, 120 frames per second each). `veil set` took as long (110 ms to an image, 176 ms to a video, the same CPU per switch).

So both conditions the decision named are met: C is enough (B is not needed), and `1` is flat and costs playback nothing measurable. **veil ships with `MALLOC_ARENA_MAX=1`** (the Nix wrapper, `--set-default`, and `Environment=MALLOC_ARENA_MAX=1` in the systemd unit; the flake check `plugins` and `tests/systemd.rs` check them). A user can still set another number. F alone is not enough and costs a little time per switch; it was not combined with an arena limit, which needs nothing more.

What is left: a binary started without the variable (`target/release/veil`, a distribution package that does not set it) gets glibc's default and the growth in the table; it is bounded by the number of arenas, but no bound was reached in 600 switches. And the soak ran in a headless sway, with the frames in shared memory: a real session with dmabuf buffers may allocate differently (the 24-hour soak of the release checklist, [`roadmap.md`](roadmap.md)).

### How to reproduce

`scripts/bench/soak.sh switch --arenas N` (with `default` for glibc's own), or by hand in a headless sway as in entry 1: start `veil daemon --input image.png`, then alternate `veil set` between two video files a few hundred times, set the image again, and read `VmRSS`, `RssAnon` and `Threads` from `/proc/<pid>/status`. Repeat with `MALLOC_ARENA_MAX=1` in the daemon's environment. A daemon started from `nix build` or by the systemd unit has `MALLOC_ARENA_MAX=1` already; to see glibc's default, set the variable to `8` times the number of CPU threads, or start `target/release/veil` directly. For the leaks tracer, start the daemon with `GST_TRACERS=leaks GST_LEAKS_TRACER_SIG=1 GST_DEBUG=GST_TRACER:7` and send it SIGUSR1 (only after the first video: before that GStreamer is not initialised, and the signal ends the daemon).

---

## 4. A wallpaper stays with its output and does not move

**Status:** by design. The rule is kept per output since multi-monitor support.
**Decided:** 2026-10-10; adapted 2026-10-14 for multi-monitor support.

Every output the daemon draws on has a wallpaper of its own, remembered **by the output's name** (`OutputKey` in `src/wayland/binding.rs`, the `Screen` in `src/wayland/screen.rs`). When an output goes away its surface is released and its wallpaper is kept (detached, holding no pixels and no pipeline); when an output with the same name is announced again, the wallpaper is put back on it. The other outputs are not touched.

Considered and rejected: moving a wallpaper to another connected output when its own disappears. It would make wallpapers hop between monitors (and not hop back). Before multi-monitor support the daemon drove a single output, and the price of this rule was that a laptop started docked showed nothing once undocked. With it that question is gone: without `--output` every output, also one connected later, gets a surface. An output the daemon has not drawn on before gets the default wallpaper: that of the latest `veil set` for every output whose file could be loaded, or else `--input`. It shows `--bg-color` while that is loaded off the event loop.

With `--output` (repeatable) only the outputs named there get a surface. A `veil set --output X` for one of those is accepted before X is connected: the wallpaper is kept for it.

If the compositor reports no output name, the output cannot be recognised when it returns: its wallpaper ends with its surface, and it is treated as a new output when it is announced again. It cannot be named in `veil set --output`; a `veil set` for every output includes it.

`--output` names that do not exist are an error **at startup** only, and only if none of them exists (most likely a typo). If some exist, the missing ones are logged and waited for. After an output has been bound once, its absence is never an error.

---

## 5. A video that covers the whole surface stands still for a moment at every loop

**Status:** not reproduced since 2026-10-16, with any build, also the one it was found with; kept open as a record. No test looks for it, and Hyprland, where it would matter most, is not covered by tests ([`testing.md`](testing.md), "Not covered by any test"); it is reopened if it is seen in use. Found on 2026-10-13, and older than that: the build before the error recovery was added behaved the same then.
**Affects:** videos in `fill` (the default) and `stretch`, and any mode in which the video happens to cover the surface completely. Not `fit` with bars or `center` with a border.
**Last checked:** 2026-10-16, headless sway 1.12 (pixman renderer, frames through `wl_shm`), GStreamer 1.28.7: not there. **Not checked on Hyprland or on real hardware.**

### Symptom

Each time the video starts again from its beginning, the picture does not move for about 0.7 seconds. With a short loop this is plain to see.

Measured with 150 screenshots taken back to back over 20 seconds, of a 3 s clip whose content moves from its very first frame (its decoded frames all differ):

| Mode | Stretches with an unchanged picture |
|---|---|
| `fill` (the video covers the surface) | six, one at each loop: 0.67 to 0.82 s each |
| `fill`, an earlier build | six, 0.67 to 0.81 s each |
| `fit` (bars of the background colour left and right) | none |
| `center` with a smaller clip | none |

### What is known

- The pipeline notices nothing. It is back in `Playing` 5 ms after the seek, the end of the run comes exactly one clip length later, and the sink counts every frame as rendered and none as dropped.
- So the frames are lost between the sink and the screen. A mechanism suggested earlier, that the sink skips a frame while it still waits for the compositor's frame callback for the one before and counts it as rendered, is unlikely: in the 1.28.7 source a frame replaced that way is reported as dropped (`gst_wl_window_render_hdr` returns false, the sink returns `GST_BASE_SINK_FLOW_DROPPED`), and `dropped` was seen to grow by 30 a second while a locked session kept the compositor from answering. Here it stays at zero.
- sway sends no frame callbacks to veil's surface while an opaque video subsurface covers all of it (measured twice: none in 3 s, while the sink got 180 for its own subsurface). veil's own loop of that time (removed for a day, see entry 6), which committed that surface once per frame callback, therefore stood still in exactly the modes that showed this problem, and ran in the modes that did not.
- What has not been established: why this hurts only after the flushing seek and not during normal playback, and what ends the wait after 0.7 s.

### Not reproduced (2026-10-16)

It was measured again before anything was changed, with screenshots taken back to back (26 a second as PPM, 8 a second as PNG as in the first measurement) over 8 to 20 seconds, that is three to six loops per run, and found no stretch with an unchanged picture longer than 100 ms in any of these:

- an early measurement build and the build it was first found with (commit `9b41671`, built again for this);
- the sway of the flake's `headless` shell and the one of `nix shell nixpkgs#sway` (both 1.12, different builds), and the host's `grim` as well as the shell's;
- the decoders `nvvp8dec` (the default on the reference machine), `vp8dec` and `avdec_vp8` for VP8, `nvh264dec`, `nvh265dec` and `nvvp9dec` for the reference clips of `scripts/bench/clips.sh`;
- `fill` with clips of the output's size and of other sizes (scaled up, cropped), `fit` and `center`, at output scales 1, 1.5 and 2;
- a 1.9 MB SMPTE clip like the one of the first measurement, and the moving ball.

What differs from 2026-10-13 cannot be established now: the sway of the first measurement came from the registry's `nixpkgs`, which has moved on since, and the GStreamer registry the daemon used then was the shared one that was later found broken (2026-10-15), so the decoder may have been another. It was also tried without the commit of veil's surface on every frame, the suspected link: no stall either, in `fill`, `fit` and `center`. (That change was undone because Hyprland needs the commits: entry 6.)

### Options

If it comes back: measure it first (the procedure below), then compare `fill` with `fit`: veil's commit on every frame (entry 6) runs in `fit` and stands still in `fill` on sway, which is the one mechanism known to differ between the modes. A workaround for a user who sees it: `--mode fit` on a video whose aspect ratio differs from the monitor's.

### How to reproduce

In a headless sway as in entry 1, start `veil daemon --input clip.webm` with a short clip of something that moves all the time (`videotestsrc pattern=ball` gives one), take screenshots as fast as `grim` allows for a few loops, and compare each with the one before (`magick compare -metric AE`). Repeat with `--mode fit` and a clip that is narrower than the output.

---

## 6. Hyprland needs veil's surface committed on every frame while a video plays

**Status:** worked around (the commit loop stays); the cause is not known. **Found:** 2026-10-16, by the maintainer, in their Hyprland session, with a build that no longer committed the surface on every frame. **Affects:** any change to the frame-callback loop.

### Symptom

A change stopped committing veil's own surface on every frame callback while a video plays, because `waylandsink` commits its own subsurfaces (they are desynchronized, GStreamer 1.28.7 `gstwlwindow.c`) and a headless sway showed the video correctly without those commits: the picture moved at 30 frames per second, placed as before, in `fill`, `fit` and `center`. On Hyprland the same build made the video "extremely laggy", and it looked "cut with multiple layers": parts of older frames on top of each other. It did not happen before the change. The loop was restored the same day, exactly as it was, and with it the video played smoothly again on Hyprland (checked by the maintainer, 2026-10-16).

### What is known

- The loop commits veil's surface (the parent of the sink's subsurfaces) once per frame callback, that is once per refresh of its output, with no new buffer and no damage.
- The sink's subsurfaces are desynchronized: by the protocol their commits take effect at once, without a commit of the parent.
- What Hyprland does differently was not examined (its source, or `WAYLAND_DEBUG` and `HYPRLAND_TRACE` in its session). A guess, not checked: it repaints only the damage of the subsurface, at a wrong place or time, unless the parent's commit makes it repaint the whole surface.

### Decision (2026-10-16)

Keep the loop. Its cost is wake-ups, not CPU: in a headless sway, 160 to 175 wake-ups per second of veil's event loop in `fit` against 75 to 85 without it, and no measurable difference in CPU, in veil or in the compositor (`docs/measurements.md`).

### When to revisit

When the cause is known, or if a later change alters how frames reach the screen. Replacing `waylandsink` (2026-11-09, entry 1) kept everything this entry depends on: the video's subsurface is desynchronized and committed by the sink, as `waylandsink`'s was, and the loop commits veil's surface as before. No test covers this: only Hyprland showed the problem, and Hyprland's rendering is not covered by tests ([`testing.md`](testing.md), "Not covered by any test"). A change to the loop passes the headless tests either way, so it is left alone until the cause is known; a regression would show in daily use on Hyprland.

Not tried: committing at the video's frame rate instead of on every frame callback (the fallback considered when the loop was measured). That work was closed on 2026-10-16 without it, by the maintainer's decision, with its goal (wake-ups close to the video's frame rate) not met. It is after 1.0.

## 7. A video costs CPU: its frames are converted to RGB on the CPU

**Status:** open, accepted for 1.0 (2026-10-17); solved for VA-API decoders whose dmabufs the compositor takes (2026-11-20, below). **Affects:** every video wallpaper whose decoder gives its frames in system memory, which on the reference machine (NVIDIA) is every decoder. The goal of "below 1 % CPU" for videos is not met; images are not affected (0 %).

### Symptom

With the NVIDIA hardware decoder the daemon uses about 18 % of a core for a 1080p video at 30 fps, 37 % at 60 fps and 135 % for 2160p at 60 fps (`docs/measurements.md`, "Where a video's CPU goes"). The cost follows the pixels per second, not the codec.

### Cause

Decoding costs about 1 % of a core. The rest is `videoconvert` in veil's sink bin turning every NV12 frame into RGB. It has to: no compositor here takes NV12 in shared memory (the headless sway's and Hyprland 0.56.2's `wl_shm` take only RGB formats). A compositor can take an NV12 frame as it is only as a dmabuf, and on NVIDIA none of the decoders GStreamer has gives one. The `nvcodec` decoders give system, CUDA or GL memory; the VA-API decoders of the NVIDIA VA driver give system memory only.

### What veil does about it

- No converter of `playbin`'s own any more (`native-video`), so the one in the sink bin is the only one, and it passes through frames the sink takes as they are (`video/x-raw(ANY)`, dmabufs included).
- The conversion runs on two threads, so 4K at 60 fps is shown in full (it lost 15 to 25 % of its frames on one thread), for 1 to 4 more points of a core at 1080p.
- The decoder is logged when a video starts (`video decoder: nvh264dec (hardware)`), so a report says which path it took.

### Options considered

| Option | What it gives | Why it was not done (yet) |
|---|---|---|
| **A. VA-API decoders with DMABuf output** | Zero-copy: the decoder's frames go to the compositor as they are. | Done (2026-11-20, below). Needs an Intel or AMD VA-API driver; on NVIDIA the VA driver gives no dmabufs to GStreamer. |
| **B. Convert on the GPU** (`cudaconvertscale`, `glcolorconvert`, Vulkan) | The CPU only copies the RGB frame. | `cudaconvertscale` is not in this build of `nvcodec`; the GL path did not negotiate here and would bring a GL context into the daemon, with an element chain that only fits NVIDIA. Not pursued. Checked again on 2026-11-20 with GStreamer 1.28.8: `nvcodec` registers `cudaconvert`, `cudascale` and `cudaconvertscale` only where NVIDIA's runtime compiler (NVRTC, from the CUDA toolkit, not the driver) loads, so most systems would not have them either; `nvh264dec ! glcolorconvert ! gldownload` still did not negotiate, without a display and with surfaceless EGL; there is no `vulkancolorconvert`. What is left is option D, a GPU context of veil's own. |
| **C. DRM dumb buffers** (`waylandsink drm-device=...`) | NV12 copied into a dmabuf, no conversion. | sway takes NV12 dmabufs only with NVIDIA's tiled modifiers, not as linear dumb buffers: it still converted (21.5 %). |
| **D. Replace `waylandsink`** (entry 1, option E) | A sink of veil's own could take CUDA or GL frames, export them as dmabufs, and would also remove entries 1, 2 and the wake-ups of entry 6. | Large, and after 1.0. |

### Since 2026-11-09

veil's own sink replaced `waylandsink` (entry 1). The conversion is the same code (`GstVideoConverter`, two threads), now inside the sink, straight into the `wl_shm` buffer, and it skips what `fill` and `center` crop away. The cost of a video is the same within the noise (`measurements.md`, "veil's own sink"). The way to a cheaper video is unchanged: dmabufs (VA-API on Intel and AMD, which the sink does not take yet) or a conversion on the GPU.

### Since 2026-11-20: frames in dmabufs

The sink takes a decoder's frames in dmabufs (`video/x-raw(memory:DMABuf)`, `DMA_DRM`) when the compositor takes their format and modifier (`zwp_linux_dmabuf_v1`), and shows them as they are: no conversion, no copy, the crop is the viewport's. On an Intel GPU (Raptor Lake, `vah264dec`) in a headless sway rendering on that GPU, H.264 at 1080p30 costs about 2 % of a core instead of 45 %, and 4K at 60 fps plays in full for about 7 % instead of losing a third of its frames at 175 % (`measurements.md`, "Frames in dmabufs").

What is left:

- **NVIDIA:** unchanged. Its decoders give no dmabufs (option B or D would be needed).
- **The colours need the compositor to know how they are coded.** The compositor turns the NV12 frame into RGB itself, and is told the coding (BT.709, BT.601, BT.2020, the range) through `wp_color_representation_v1`. One that cannot be told uses BT.601 with the limited range, the default of EGL's dmabuf import, and would show a BT.709 video with wrong colours (measured: red 231 instead of 251, cyan with red in it). veil hands it only videos coded as BT.601 then, and converts the others on the CPU, as before. sway 1.12 rendering with `gles2` advertises the protocol with no coding at all; with `vulkan` it advertises BT.601, BT.709 and BT.2020 in both ranges. Most HD video is BT.709 (and GStreamer takes an untagged HD video for BT.709), so on a compositor that cannot be told, most videos are still converted.
- **Hyprland up to 0.57 takes no YUV dmabufs.** Its renderer leaves every YUV format (NV12, P010, ...) out of the formats it lists (`Do not advertise YUV until the renderer supports it`, `src/render/OpenGL.cpp`), and it has no `wp_color_representation_v1`, so every video is converted on the CPU there, BT.601 ones too; the log says so at start (`compositor takes dmabufs in no YUV format`). Seen on 0.57.2 (2026-11-20). Hyprland's main branch (November 2026) lists YUV formats, sampled as external textures, and has `wp_color_representation_v1`: the release after 0.57 should take veil's frames with no change in veil. Not tested yet.
- **A decoder does not negotiate again while it plays.** When the compositor refuses a dmabuf, or cannot be told a video's coding, the sink offers system memory only and the pipeline is started once more (VA-API decoders ignore a request to negotiate again); the previous wallpaper stays on screen meanwhile.
- **Laptops with two GPUs:** GStreamer ranks NVIDIA's decoders above the VA-API ones, so a laptop with both decodes H.264, HEVC and AV1 on the NVIDIA card, in system memory, even when the compositor runs on the Intel GPU and would take its dmabufs (VP9 went to `vavp9dec` on the machine measured). `GST_PLUGIN_FEATURE_RANK=nvh264dec:0,nvh265dec:0,nvav1dec:0` makes GStreamer pick the VA-API decoders (`troubleshooting.md`). veil does not choose the decoder by itself.

### Decision (2026-10-17)

Keep `waylandsink` for 1.0, with the cheapest correct pipeline (`native-video`, two conversion threads), and record the cost as it was measured. The conversion is not `waylandsink`'s doing: any sink that shows NVIDIA's frames through shared memory needs it.

### When to revisit

When `nvcodec` or the NVIDIA VA driver can hand out dmabufs, when a conversion on the GPU is taken up (option B), when Hyprland's support of `wp_color_representation_v1` is known (it decides whether BT.709 video takes the dmabuf path there), and when a tester reports a video on AMD hardware.

## 8. Older GStreamer versions: animated GIFs, and frames dropped on Ubuntu 24.04

**Status:** open, documented (2026-10-19). Upstream, in the distributions' GStreamer and FFmpeg; not in veil. **Affects:** the release tarball (which uses the system's GStreamer) on Debian 12, Ubuntu 22.04 and Ubuntu 24.04. The Nix package brings GStreamer 1.28 and is not affected, nor are Debian 13, Fedora 44 and Arch.
**Last checked:** 2026-10-19, with the x86_64 tarball of 0.1.0 in a container of each distribution, against a headless sway on the host, by hand (not a test: [`testing.md`](testing.md), "Checks to be turned into tests").

### Symptom

| Distribution | GStreamer, FFmpeg | Animated GIF | Videos |
|---|---|---|---|
| Ubuntu 22.04 | 1.20.3, 4.4 | Shows its first frames, then stands still (`veil status` says `playing`; 4 CPU ticks in 5 s) | play |
| Debian 12 | 1.22.0, 5.1 | Shows its first frames, then stands still, and the daemon uses a whole CPU core for as long as it is the wallpaper (499 CPU ticks in 5 s, all in GStreamer's `avdemux_gif0:sink` thread) | play |
| Ubuntu 24.04 | 1.24.2 (`gst-libav` 1.24.1), 6.1 | Never starts: `veil set` fails with `the video did not show a frame within 10s`, and the previous wallpaper stays | 1080p videos showed a half to a quarter of their frames (`veil status`: H.264 at 30 fps 133 shown and 148 dropped, VP9 at 60 fps 51 and 167); a 320x180 one all of them in `center`, nearly all in `fill` |
| Debian 13, Fedora 44, Arch | 1.26.2, 1.28.8 | plays | play |

### Cause

The GIF failures are in GStreamer's FFmpeg demuxer (`avdemux_gif`), without veil: `gst-launch-1.0 filesrc location=anim.gif ! decodebin ! fakesink sync=true` gives every frame the time stamp 0 and repeats them without end on Debian 12 (so the stream never reaches its end and the demuxer's thread never rests), and never prerolls on Ubuntu 24.04. Ubuntu 22.04 was only seen through veil. The same animation converted to VP9 in WebM plays on all three (three different screenshots out of three).

The frames dropped on Ubuntu 24.04 are counted by its `waylandsink` (the decoder and the conversion used a few percent of a core, so the CPU is not the limit). The cause was not established. A control that rules veil out could not be made: plain `gst-launch-1.0 ... ! waylandsink` in a window of its own drops about half the frames of the same file in the headless sway on Debian 13 too, where veil drops none. Whether it happens with a compositor on real hardware was not checked.

### Options

| Option | Why not (for now) |
|---|---|
| **A.** Require GStreamer 1.26 for the tarball | Leaves out Debian 12 and both Ubuntu LTS releases, where pictures and videos work. |
| **B.** Refuse animated GIFs when the runtime GStreamer is older than 1.26 | It is not known which of GStreamer and FFmpeg the bug follows, so a version check would guess. |
| **C.** Document it, with the workaround: convert the GIF to a video (`troubleshooting.md`) | Chosen. |

### Decision (2026-10-19)

C. The README lists the distributions the tarball was run on and points here; `troubleshooting.md` says how to turn a GIF into a WebM that plays everywhere. For the maintainer to confirm or change.

### When to revisit

When a tester reports the tarball on one of these distributions with a compositor on real hardware (especially Ubuntu 24.04 and its frame drops), or when the oldest distributions the tarball is for change.

---

## 9. Washed-out colours from VA-API decoders in system memory

**Status:** fixed after 1.0.0-beta.1 (2026-11-20, see "Fixed" at the end of this entry). **Affected:** videos decoded by a VA-API decoder (Intel, AMD) whose frames veil converts on the CPU: every such video in 1.0.0-beta.1, and later the ones that cannot go to the compositor in dmabufs (entry 7), which on Hyprland up to 0.57 is every video.
**Last checked:** 2026-11-20, `vah264dec` (GStreamer 1.28.8) on an Intel GPU, a headless sway rendering with `pixman` (frames converted on the CPU).

### Symptom

The colours are paler and darker than they are: on clips of colour bars coded as BT.709 and as BT.601, both limited range, pure red is shown as about (237, 13, 15) instead of (251, 0, 0) (ffmpeg's decode), white is grey. The same clips in dmabufs, converted by the compositor, are right.

### Cause

The decoder's caps in system memory say `colorimetry=0:3:0:0`: the matrix (BT.709), but an unknown range. GStreamer's converter takes an unknown range for the full one and does not stretch the 16 to 235 of a video to 0 to 255. veil's dmabuf path takes an unknown range for the limited one, as GStreamer itself does when it fills in a colorimetry.

### What would fix it

Taking an unknown range of a YCbCr stream for the limited one before the converter is made (`sink.rs`, `new_converter`). Left for its own change: it changes the colours of every video that comes this way, which is to be checked on its own. Whether NVIDIA's decoders say the range was not checked.

### Fixed (2026-11-20)

Before the converter is made, what the stream's colorimetry leaves open is filled in as the dmabuf path does: the limited range, and the matrix GStreamer assumes for the frame's height (`with_known_coding` in `sink.rs`). A range or matrix the stream does say is kept. On the same clips of colour bars, converted on the CPU, the colours are now ffmpeg's within 3 (red (251, 0, 0) for BT.709, (253, 0, 0) for BT.601), as in dmabufs. The cost of a video is the same: H.264 1080p30 with `vah264dec`, 40.9 to 41.2 % of a core against 40.7 to 41.0 % before, every frame shown (four alternating 30 s runs each; the first run of the old build, 35.2 %, was off from all the others). Whether NVIDIA's decoders say the range is still not checked; they get the same treatment if they do not.


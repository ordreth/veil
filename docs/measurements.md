# Measurements

How much the daemon costs while it shows a wallpaper, measured with `scripts/bench/`. The README's performance goals are checked against these numbers (sub-1% CPU for video, 0% for still images, no growth); where they are not met, this file says so.

## Running it

Inside `nix develop .#headless` (sway, grim, jq, ffmpeg and the GStreamer plugins), with a release build:

```bash
cargo build --release
scripts/bench/clips.sh                                      # the reference clips, in target/bench/clips
scripts/bench/bench.sh target/bench/clips/h264-1080p30.mp4  # 5 s warm-up, then 60 s of samples
```

`bench.sh FILE` starts a headless sway of its own (pixman rendering, one output of 1920x1080; `--size` changes it), runs `veil daemon --input FILE` in it with directories of its own (your saved wallpapers and configuration file are not read or written), waits until the wallpaper is shown, waits `--warmup` seconds, and then samples the daemon once per `--interval` (1 s) for `--duration` (60 s). The results go to `target/bench/<date>-<file>/` (`--out` to choose): `samples.csv`, `report.txt` (also printed), and the logs of the daemon, of sway, and of the GStreamer elements the pipeline created. `scripts/bench/summary.sh samples.csv` prints the summary of a run again.

Other options: `--mode` (veil's `--mode`), `--veil PATH` (another binary, for example `./result/bin/veil` of `nix build`, to compare builds), and `--outputs N` (N headless outputs side by side, each playing the file in a pipeline of its own; the frame counts are summed), and `--session`, which runs the daemon in the Wayland session the script is started from instead of a headless sway (stop your own veil daemon first: a second one refuses to start; `--output NAME` limits it to one monitor). Environment variables reach the daemon: `GST_PLUGIN_FEATURE_RANK=nvh264dec:0` takes a decoder out of the way, `MALLOC_ARENA_MAX=1` changes the allocator (`target/release/veil` has glibc's default unless it is set; the report names it).

### The columns of `samples.csv`

| Column | What it is |
|---|---|
| `t_s` | seconds since the first sample |
| `cpu_pct` | CPU time of the daemon (all its threads, user and system, from `/proc/<pid>/stat`) over the interval, in percent of one core |
| `rss_kib` | resident memory (`VmRSS`) |
| `threads`, `fds` | entries of `/proc/<pid>/task` and `/proc/<pid>/fd` |
| `sink_rendered`, `sink_dropped` | the video sink's own frame counts, summed over the outputs, from `veil status --json`. They start again at 0 at every loop of the video, so the summary counts the frame rate over the intervals without a loop. A frame the sink never received (dropped earlier in the pipeline, by a decoder that falls behind) is in neither column. |
| `gpu_pct`, `gpu_decoder_pct` | with `nvidia-smi`: the utilisation of the whole GPU and of its video decoder. **System-wide**: every process counts, also the desktop the script runs in. Empty without `nvidia-smi`. |
| `wakeups_per_s` | context switches of the daemon's event loop thread (the main thread, `/proc/<pid>/task/<pid>/status`) per second over the interval: how often it was woken. The `veil status --json` the script sends once per sample counts too (about 2 per second; a still picture shows nothing else). It is the last column, added after the others, so files of older runs still read. |

`report.txt` adds the clip (size, rate, codec through `ffprobe`), the decoder GStreamer created (from `GST_DEBUG=GST_ELEMENT_FACTORY:4`; if it tried several, all are named), the veil build (version and commit), the compositor, the CPU, the GPU and the kernel.

Notes: frame drops come from the sink's own counters (the same as `veil status`) and not from GStreamer QoS messages; the GPU column supports NVIDIA only (`intel_gpu_top` and `radeontop` need hardware the reference machine does not have, and are not covered).

## Reference machine and clips

| | |
|---|---|
| CPU | Intel Core i5-11400F (6 cores, 12 threads, 2.6 GHz), no integrated GPU |
| GPU | NVIDIA GeForce RTX 3060 Ti, NixOS driver; no VA-API driver, so GStreamer's `va` plugin offers nothing and the `nvcodec` decoders are used (`nvh264dec` and the others rank above `avdec_h264`) |
| Memory | 32 GB |
| System | NixOS, Linux 7.2.0, GStreamer 1.28.7 (the package's plugin set), sway 1.12 headless (pixman), output 1920x1080 at scale 1 |

The clips come from `scripts/bench/clips.sh`: ffmpeg's `testsrc2` (every frame differs), 10 seconds each, so a 60 s run loops six times.

| File | Codec | Size, rate |
|---|---|---|
| `h264-1080p30.mp4` | H.264 High (libx264, CRF 23) | 1920x1080, 30 fps |
| `h264-1080p60.mp4` | H.264 High | 1920x1080, 60 fps |
| `h264-2160p60.mp4` | H.264 High | 3840x2160, 60 fps |
| `hevc-2160p60.mp4` | HEVC Main (libx265, CRF 26) | 3840x2160, 60 fps |
| `vp9-1080p60.webm` | VP9 Profile 0 (libvpx, 4 Mbit/s) | 1920x1080, 60 fps |
| `av1-1080p60.mkv` | AV1 Main (SVT-AV1, CRF 35) | 1920x1080, 60 fps |
| `still-1080p.png` | PNG | 1920x1080, one frame |

## Results (2026-10-16, headless sway)

Each row is one run of 60 samples after a 5 s warm-up, `--mode fill` unless said otherwise, built from the working tree of that day. CPU is the mean (the median was within 0.6 points of it in every run); memory is the resident set at the end of the run.

| Wallpaper | Decoder | CPU (one core) | Frames shown per s | Dropped by the sink | RSS | Threads | Descriptors |
|---|---|---|---|---|---|---|---|
| `still-1080p.png` | none | **0.00 %** (0 ticks in 60 s) | none | none | 19.7 MiB | 1 | 15 |
| `h264-1080p30.mp4` | `nvh264dec` | 17.2 % | 30.0 | 0 | 202.7 MiB | 14 to 15 | 65 |
| `h264-1080p30.mp4`, `--mode fit` | `nvh264dec` | 17.0 % | 30.1 | 0 | 202.8 MiB | 14 to 15 | 65 |
| `h264-1080p60.mp4` | `nvh264dec` | 34.1 % | 60.1 | 0 | 201.3 MiB | 14 to 15 | 65 |
| `vp9-1080p60.webm` | `nvvp9dec` | 34.0 % | 60.1 | 0 | 248.9 MiB | 14 to 15 | 65 |
| `av1-1080p60.mkv` | `nvav1dec` | 33.9 % | 60.1 | 0 | 201.5 MiB | 14 to 15 | 65 |
| `h264-2160p60.mp4` | `nvh264dec` | 108.0 % | **51.1** | 0 | 296.2 MiB | 14 to 15 | 65 |
| `hevc-2160p60.mp4` | `nvh265dec` | 108.0 % | **51.3** | 0 | 295.4 MiB | 14 to 15 | 65 |
| `h264-1080p30.mp4`, software | `avdec_h264` | 22.1 % | 30.0 | 0 | 161.7 MiB | 21 to 22 | 27 |
| `h264-2160p60.mp4`, software | `avdec_h264` | 153.3 % | 60.0 | 0 | 456.5 MiB | 21 to 22 | 27 |

"Software" is `GST_PLUGIN_FEATURE_RANK=nvh264dec:0,nvh265dec:0,nvav1dec:0`. The GPU columns are left out of the table: they are system-wide, and the desktop the runs were started from used 1 to 20 % of the GPU by itself (the still picture, which uses no GPU at all, read 8.9 % on average). The decoder utilisation was 4 % for 1080p30 H.264, 10 % for 1080p60 and 35 % for 2160p60 H.264 (15 % for HEVC), and 0 with software decoding.

### What the numbers say

- **A still picture costs nothing**: no CPU time at all over 60 s, one thread, 15 descriptors, 20 MiB. The goal of 0.0 % CPU is met here (headless sway; not yet measured in a real session).
- **A video is far from the 1 % goal.** With the hardware decoder the daemon still uses about 17 % of a core for 1080p at 30 fps, and the cost grows with the number of pixels per second (34 % at 60 fps, 108 % for four times the pixels), not with the codec: H.264, VP9 and AV1 at 1080p60 cost the same. So the decoding is not what costs: the frames are copied on the CPU after the GPU has decoded them, which is examined in "Where a video's CPU goes", below. In this setup some copy is unavoidable: the headless sway with pixman offers no `zwp_linux_dmabuf_v1`, so `waylandsink` has to put every frame into shared memory. A real session (`--session`) is the measurement that tells how much of it is veil's pipeline. *That investigation found that it is the colour conversion of `videoconvert`, which happens in a real session on NVIDIA too ("Where a video's CPU goes", below).*
- **The hardware path cannot keep up with 4K at 60 fps here**: 51 frames per second reach the sink and the daemon uses more than one core. The sink counts nothing as dropped, so the missing frames are lost before it (by a part of the pipeline that falls behind; which one was not established). Software decoding shows all 60 at 153 %. A likely cause is the same copy, done in one streaming thread; "Where a video's CPU goes" confirms it. *Confirmed there: with two conversion threads all 60 frames are shown.*
- **The pipeline has more elements than veil's sink bin**: besides `videoconvert ! videocrop ! waylandsink`, `playbin` creates a `videoconvert`, a `videoscale` and a `videobalance` of its own (seen in the element log of every video run). Where they sit and whether they copy frames was looked at in "Where a video's CPU goes". *Removed since (`native-video`).*
- **Fill or fit makes no difference to the CPU** (17.2 % against 17.0 %), although veil's own surface gets frame callbacks in `fit` and none in `fill`: the commit loop is not where the time goes (measured again in "The frame-callback loop", below).
- **Memory and descriptors are flat while a video loops** (six loops per run): the slope of RSS was between +33 and +144 KiB per minute with the hardware decoder, within the noise of a 60 s run, and threads and descriptors did not change. With software decoding RSS grew by 6 to 12 MiB during the run, which may be the decoder's pools filling after the warm-up; a 60 s run cannot tell that from a leak. The long run is in "Soak tests and the allocator", below.
- **The sink loses no frame at a loop**: 30.0 and 60.1 frames per second over the intervals without a loop. The stall of known issue 5 (the picture standing still at every loop in `fill`) is therefore not visible in the sink's counts, as entry 5 already says; it has to be measured on the screen.

## The frame-callback loop (2026-10-16, headless sway)

The daemon commits its surface at every frame callback while a video plays, so that the compositor presents the video's subsurface. `waylandsink`'s subsurfaces are desynchronized and committed by the sink itself, so it was tried without these commits and both were measured. **The change was undone the same day**: on Hyprland the video lagged and showed parts of older frames without the commits (`known-issues.md`, entry 6). The numbers below are what the loop costs; "after" is the build without it, which is not what veil does. Measured with `bench.sh --size 2560x1080 --duration 30` (so that `fit` leaves bars and veil's surface is visible, the case where the loop ran), `h264-1080p30.mp4`, `nvh264dec`, release builds of the working tree before and after the change:

| Mode | Build | Wake-ups of the event loop per s | CPU (one core) | Frames shown per s |
|---|---|---|---|---|
| `fit` | before | 158.4 | 18.3 % | 30.0 |
| `fit` | after | **85.4** | 18.2 % | 30.1 |
| `fill` | before | 88.7 | 18.0 % | 30.0 |
| `fill` | after | 84.8 | 17.8 % | 30.0 |
| `still-1080p.png` | after | 1.8 | 0.00 % | none |
| `h264-1080p60.mp4`, `fill`, 1920x1080 | after | 200.9 | (not compared) | 60.1 |

Measured by hand in the same setup (a 1280x1080 VP8 clip on a 1920x1080 output, 10 to 20 s per run, two rounds alternating the builds): frame callbacks on veil's surface went from 58 per second to none in `fit` and `center` (none in `fill` before either); the CPU of the event loop thread stayed at the resolution of the counter (0.15 to 0.4 %), and so did sway's (7.5 % in `fit`, 0.3 % in `fill`, unchanged).

What it says:

- **The loop costs wake-ups, not CPU.** One wake-up per display refresh and output, about 70 a second here, and nothing measurable in CPU, in veil or in sway. It stays, because Hyprland needs it.
- **The event loop still wakes about three times per video frame** (85 at 30 fps, 201 at 60 fps), with or without the loop: these are `waylandsink`'s own events (the answer to its sync request, its frame callback and the release of the buffer before) on the Wayland connection veil shares with it, which wake veil's poll of that connection too. A still picture does not wake it at all (the 1.8 per second are the script's own `veil status`). The sink's share goes only with replacing `waylandsink` (after 1.0), because the sink's subsurface must be made on veil's connection.

## Soak tests and the allocator (2026-10-17, headless sway)

`scripts/bench/soak.sh` runs the daemon for a long time in the same setup as `bench.sh` and fails when its memory, threads or file descriptors rise:

```bash
scripts/bench/soak.sh loop     # a 1 s H.264 clip, so it loops once a second; 10 min, sampled every second
scripts/bench/soak.sh switch   # 500 `veil set`: video A, image B, video B, image A, ...; sampled after each
```

The media are made once, in `target/bench/soak-media` (`--clip FILE` for another loop): two 1920x1080 PNGs, video A H.264 1280x960 and video B VP9 2560x1080, 3 s each at 30 fps, as in the memory measurements of `known-issues.md`, entry 3. Each run writes `samples.csv`, `report.txt` (also printed) and `graph.svg` (every column against time or switch, the stable window marked, its least-squares lines dashed) to `target/bench/soak-<test>-<date>/`. The daemon gets `MALLOC_ARENA_MAX=1`, as the Nix package and the systemd unit give it (`--arenas N|default` to change it; `default` leaves it to glibc, which allows 8 arenas per CPU thread); `--mmap-threshold BYTES` sets `MALLOC_MMAP_THRESHOLD_`.

**What is tested.** The heap (`RssAnon`: what the allocator holds, without the libraries and without the `wl_shm` buffers of the surface) over the stable window, which leaves out the first fifth of the run (`--settle`). In `switch` only the samples with an image on screen count: no pipeline exists then, so what is left is what the allocator kept. The test is the rise of the window's bounds (`scripts/bench/trend.sh`): the lowest sample of the window's last quarter against the lowest of its first quarter, the same for the highest, and of the two the one nearer to zero. A least-squares line does not work here: the heap jumps between levels some MiB apart (after a switch by what the allocator happened to keep, 9 MiB more after image B than after image A with one arena; while a software decoder plays by whether its buffers are taken at the moment of the sample, about 20 MiB), which tilts the line by ±20 KiB per switch over a window of 100 switches. Memory that is kept for good raises both bounds; that noise moves one. Limits: 8 KiB per minute for `loop` (11 MiB a day), 2 KiB per switch for `switch` (1 MiB over 500 switches; `--max-rise`). Threads and descriptors may rise by 0.05 per minute and 0.01 per switch (least squares; a leak was one of each per switch before the bus-thread leak was fixed). `loop` also fails when the video shows less than 90 % of its frame rate or does not loop, and `switch` when a `veil set` fails. Loops and frames come from the bus thread's `debug` line at every loop: the sink's own counters start again at every loop, so a 1 s clip cannot be counted by sampling them once a second.

CI runs a short version of both (`soak` job: `loop --duration 180`, `switch --switches 200`, software decoding on GitHub's runners) and keeps the reports and graphs as an artifact. Rehearsed here with software decoding: both pass (the heap's bounds did not rise in the loop, +0.23 KiB per switch).

### A looping video

| Clip, decoder | `MALLOC_ARENA_MAX` | Over 10 minutes | Heap rise | Threads, descriptors | Frames per s, CPU |
|---|---|---|---|---|---|
| 1 s 1080p30 H.264, `nvh264dec` | 2 | RSS 202.1 MiB, the same to the KiB from the 15th second to the end | 0 | 15, 64 | 29.7, 16.8 % |
| `h264-2160p60.mp4`, `avdec_h264` | 2 | heap 370 to 432 MiB; its top 432,428 KiB from the third minute on | none | 22, 26 | 59.6, 152 % |
| 1 s 1080p30 H.264, `avdec_h264` | 2 | heap 102 or 122 MiB; the top crept from 122,380 to 122,876 KiB over the first 8 minutes | +32 KiB/min from 120 s (fails), 0 from 300 s | 22, 26 to 27 | 29.6, 25.4 % |

608 loops of the hardware-decoded clip changed nothing at all, so the loop itself (a flushing seek on the same pipeline) keeps nothing. The software decoder's 1 s clip kept about 500 KiB more over its first 500 loops and then nothing more for the last five minutes, which looks like the allocator settling and not a leak; whether it stays flat for hours is left to the 24-hour soak of the release checklist ([`roadmap.md`](roadmap.md)). Every row ran with 2 arenas, before the shipped number became 1; the CI rehearsal with 1 and software decoding passed.

### Wallpaper switches

500 switches each (250 videos), `nvh264dec` and `nvvp9dec`, about 77 s per run. "Image after" is the sample after the image that replaced a video; "rise" is the test above over switches 100 to 500.

| `MALLOC_ARENA_MAX` | Heap after 1 video | Heap after 250 videos (RSS) | Heap rise | Threads, descriptors with an image | `veil set` to an image / a video | CPU per switch |
|---|---|---|---|---|---|---|
| 1 | 42.5 MiB | **42.2 MiB** (117.1 MiB) | 0.00 KiB/switch | 7, 29 | 110 / 176 ms | 0.15 s |
| 2 | 41.2 MiB | 73.0 MiB (147.8 MiB), level from the tenth switch on | 0.00 KiB/switch | 7, 29 | 111 / 177 ms | 0.15 s |
| glibc's default (96 arenas here) | 41.4 MiB | 129.1 MiB (203.8 MiB), still rising | **+6.25 KiB/switch** (would fail) | 7, 29 | 112 / 176 ms | 0.15 s |
| default, `MALLOC_MMAP_THRESHOLD_=131072` (option F) | 22.4 MiB | 107.9 MiB (182.7 MiB), still rising | +0.86 KiB/switch | 7, 29 | 116 / 181 ms | 0.16 s |

Threads and descriptors were the same after every image of every run, and 20 s after the last video the daemon was down to 3 threads (the main thread, GLib's `pool-spawner` and a thread of the NVIDIA library). No `veil set` failed.

### What the allocator settings cost playback

`bench.sh`, 60 s per run, one output and two (`--outputs 2`, the same file on both, so two pipelines share the allocator). "Software" is `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`.

| Clip, decoder | Outputs | CPU: default / 2 / 1 | Frames per s: default / 2 / 1 |
|---|---|---|---|
| `h264-1080p60.mp4`, `nvh264dec` | 1 | 33.9 / 33.5 / 33.6 % | 59.7 / 59.6 / 59.6 |
| `h264-2160p60.mp4`, `nvh264dec` | 1 | 108.1 / 108.0 / 107.9 % | 51.3 / 51.2 / 51.5 |
| `h264-2160p60.mp4`, software | 1 | 150.4 / 152.0 / 151.9 % | 60.2 / 60.0 / 59.9 |
| `h264-1080p60.mp4`, `nvh264dec` | 2 | 73.3 / 73.3 / 72.8 % | 119.9 / 119.8 / 120.0 |
| `h264-2160p60.mp4`, software | 2 | 363.5 / 361.9 / 357.6 % | 120.4 / 120.0 / 120.2 |

No difference beyond the run-to-run noise, also with two 4K videos decoded in software (some 40 streaming threads sharing one arena). `veil set` took as long with each setting (table above).

### What the numbers say

- **Threads and file descriptors do not leak**, over 500 switches and over 600 loops.
- **A looping video keeps nothing.** With the hardware decoder the resident memory did not change by a single page in ten minutes of loops.
- **The memory kept after switches is the allocator's, and one arena keeps none of it.** With glibc's default the heap after 250 videos is three times what it is after one, and still rising; with 2 arenas it rises for ten switches and then stays; with 1 it does not rise at all. Playback costs the same with each. So veil ships with `MALLOC_ARENA_MAX=1` (it was 2, chosen before this was measured), and `malloc_trim` (option B of `known-issues.md`, entry 3) is not needed.
- **A fixed mmap threshold (option F) is not enough** on its own: it keeps less than the default at first, rises all the same, and costs a little time per switch.
- Software decoding needs more heap than the hardware decoder (72 against 35 MiB with a video playing, one arena) and switches faster here (53 against 177 ms per video; the NVIDIA decoder's setup is the difference).

## Where a video's CPU goes (2026-10-17)

The first results found a cost that follows the pixels per second and not the codec. The pipeline was then taken apart to find out why. First with `gst-launch-1.0` in the same headless sway (`h264-1080p30.mp4`, `nvh264dec`, 6 s per run), then with the daemon itself (`bench.sh`, 30 s per run).

### The pieces, one at a time

| Pipeline after `nvh264dec` | Compositor | CPU (one core) |
|---|---|---|
| `fakesink sync=true` (decoding only, frames downloaded to system memory) | none | 0.8 to 1.0 % |
| `videoconvert ! video/x-raw,format=BGRx ! fakesink` (decoding and conversion, no compositor) | none | 17.7 % |
| `videoconvert ! waylandsink` | sway, pixman | 18.8 % |
| `waylandsink` without `videoconvert` | sway, pixman | not negotiated |
| `videoconvert ! waylandsink` | sway, `gles2` on the NVIDIA GPU (offers `zwp_linux_dmabuf_v1`) | 18.3 % |
| `waylandsink` without `videoconvert` | sway, `gles2` | not negotiated |
| `videoconvert ! videocrop ! waylandsink drm-device=/dev/dri/card1` (NV12 into DRM dumb buffers) | sway, `gles2` | 21.5 % (still converts: sway takes NV12 dmabufs only with NVIDIA's tiled modifiers) |
| `playbin` as veil built it before this investigation (`flags=video`) | sway, pixman | 19.8 % |
| `playbin` with `flags=video+native-video` | sway, pixman | 18.5 % |

What it says:

- **Decoding costs nothing; converting costs everything.** NVDEC decodes 1080p30 for about 1 % of a core. The other 17 % are `videoconvert` turning every NV12 frame into RGB on one CPU thread. Writing the result into the shared-memory buffer costs almost nothing.
- **The conversion cannot be skipped with this hardware.** The sink cannot take NV12 in shared memory: the headless sway's `wl_shm` takes only RGB formats, and so does Hyprland's (0.56.2, the owner's session, read with `wayland-info`: `AR24`, `XR24`, `AB24`, `XB24` and their 10-bit forms; NV12 only as a dmabuf). An NV12 frame only reaches a compositor without conversion as a **dmabuf**, and none of the decoders here produces one. The `nvcodec` decoders give system, CUDA or GL memory. The VA-API decoders exist through the NVIDIA VA driver (`nvidia_drv_video.so`, with `GST_VA_ALL_DRIVERS=1 LIBVA_DRIVER_NAME=nvidia NVD_BACKEND=direct`: `vah264dec`, `vah265dec`, `vavp9dec`, `vaav1dec`, ...), but they also give only system memory (their source caps have no `memory:DMABuf`). `cudaconvertscale`, which would convert on the GPU, is not in this build of `nvcodec`. A GL conversion (`glcolorconvert ! gldownload`) did not negotiate here and was not pursued.
- **The headless numbers stand for a real session on NVIDIA.** The conversion happens in veil's process whatever the compositor, because no compositor takes NV12 in shared memory. What a real session adds or saves on the compositor's side is not in these numbers.
- **The path that would be zero-copy is open on other GPUs, but not verified.** With an Intel or AMD VA-API driver, `vah264dec` gives DMABuf frames. `videoconvert` and `videocrop` accept `video/x-raw(ANY)`, `DMA_DRM` included, and pass such frames through untouched, and `waylandsink` takes dmabufs. No such hardware here.
- **`playbin` added a `videoconvert`, a `videoscale`, a `videobalance` and an `identity` of its own** in front of veil's sink bin (seen in the element log of the first runs). It does that unless `native-video` is among its flags. They cost about 1 point here, because only one of the two converters converts. They are gone now.

### The daemon before and after (headless sway, pixman)

`bench.sh --duration 30`, release builds of the working tree. "Before" is the build before the change; "native-video" adds the flag; "2 threads" also gives the sink bin's `videoconvert` `n-threads=2`, which is what veil does now. Two runs of a cell are both given.

| Clip, decoder | Before | native-video | native-video, 2 threads (shipped) |
|---|---|---|---|
| `h264-1080p30.mp4`, `nvh264dec` | 19.3 %, 30.0 fps | 18.6 / 17.6 %, 30.0 fps | 20.0 / 18.7 %, 30.0 fps |
| `h264-1080p60.mp4`, `nvh264dec` | (first run: 34.1 %, 60.1 fps) | 34.8 %, 60.0 fps | 38.9 / 36.2 %, 60.0 fps |
| `vp9-1080p60.webm`, `nvvp9dec` | (first run: 34.0 %, 60.1 fps) | | 38.5 %, 60.1 fps |
| `h264-2160p60.mp4`, `nvh264dec` | 108.8 %, **44.8 fps** (first run: 51.1) | 109.0 %, **50.7 fps** | 134.7 %, **59.9 fps** |
| `h264-2160p60.mp4`, `avdec_h264` | (soak: 150 to 152 %, 60 fps) | | 168.3 %, 59.9 fps |

What it says:

- **4K at 60 fps plays in full now.** With one conversion thread the hardware path lost 15 to 25 % of the frames: that thread also waits for the download from the GPU, and more than a core of work does not fit in one thread. Two threads show all 60 frames per second, at 135 % of a core.
- **The second thread costs a little everywhere else:** 1 to 4 points at 1080p, and 15 points for 4K decoded in software, which kept up with one thread too. A smooth 4K video was chosen over those points. A setting that picks the thread count by the frame size was not tried: GStreamer's choice of decoder, not the frame size, decides whether one thread is enough.
- **The goal of 1 % is out of reach with these decoders and `waylandsink`.** What a video costs on the reference machine: about 18 % of a core at 1080p30, 37 % at 1080p60, 135 % at 2160p60. It is all conversion, so a cheaper path needs the decoder's frames as dmabufs (VA-API on Intel or AMD, which `waylandsink` takes as they are), or a conversion on the GPU (CUDA, GL or Vulkan), which `waylandsink` cannot take from `nvcodec`. A sink of veil's own could take them (option E of `known-issues.md`, entry 1), after 1.0.
- The daemon logs the decoder it uses when a video starts: `video decoder: nvh264dec (hardware)`.

## The README's numbers (2026-10-19, headless sway)

The table in the README's "Performance" section, measured again on the build of that day (the video path as the investigation above left it): `bench.sh` with the defaults (60 s after a 5 s warm-up, `fill`, one 1920x1080 output), `MALLOC_ARENA_MAX=1` as the package and the unit start the daemon, release build of the working tree. "Software" is `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`.

| Wallpaper | Decoder | CPU (one core) | Frames shown per s | Dropped by the sink | RSS | Threads | Descriptors |
|---|---|---|---|---|---|---|---|
| `still-1080p.png` | none | 0.02 % (one tick in 64 s; the median 0) | none | none | 19.8 MiB | 1 | 15 |
| `h264-1080p30.mp4` | `nvh264dec` | 19.5 % | 30.0 | 0 | 203.4 MiB | 15 to 16 | 65 |
| `h264-1080p60.mp4` | `nvh264dec` | 39.7 % | 59.8 | 0 | 203.3 MiB | 15 to 16 | 65 |
| `vp9-1080p60.webm` | `nvvp9dec` | 37.9 % | 59.9 | 0 | 250.7 MiB | 15 to 16 | 65 |
| `h264-2160p60.mp4` | `nvh264dec` | 136.3 % | 60.2 | 0 | 297.1 MiB | 15 to 16 | 65 |
| `h264-1080p30.mp4`, software | `avdec_h264` | 24.5 % | 30.0 | 0 | 160.7 MiB | 22 to 23 | 27 |
| `h264-2160p60.mp4`, software | `avdec_h264` | 173.6 % | 60.1 | 0 | 458.4 MiB | 22 to 23 | 27 |

They agree with the table above (shipped column: 18.7 to 20.0 %, 36.2 to 38.9 %, 38.5 %, 134.7 %, 168.3 %). Software decoding at 1080p30 costs more than the first results' 22.1 %, which was measured with one conversion thread, before the second one. As in the first results, resident memory with software decoding grew during the 60 s (by 4 and 10 MiB a minute), which the ten-minute soaks above show settling.

**A first session the same day was thrown out.** The desktop's browser used 9 to 11 of the 12 CPU threads during it (load 12.8, found with `top` after the runs). Its 1080p runs agreed with the table above (18.2, 35.5 and 38.6 %), but the runs that need several cores did not: hardware 4K at 153.6 %, software 1080p at 27.5 %, software 4K at 221.4 %. The table is from a second session with the machine 92 % idle (the desktop running, as in the earlier runs). A bench run says nothing about the machine's load; check `uptime` before and after a run.

## Not measured yet

- A real session (Hyprland with its dmabuf path): `--session`, by the project owner. The conversion is known to happen there too on NVIDIA (above); what is left is the compositor's own share.
- Intel and AMD GPUs, and VA-API: no such hardware here.
- A paused video and a released one: the headless tests check them (CPU at most a quarter of playing, and at most 2 ticks in 2 s), but there is no bench run of them.
- Hours and days: the 24-hour soak of the release checklist ([`roadmap.md`](roadmap.md)), on this machine, also with the software decoder (the slow start of the 1 s clip above).

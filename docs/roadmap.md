# Roadmap

What is left before veil 1.0, and what is deliberately left for after it. Everything in the feature table of the [README](../README.md) is implemented and tested; what remains before the release is turning the last checks made by hand into tests, and publishing. veil is checked by automated tests only ([`testing.md`](testing.md)).

**What 1.0 is:** a wallpaper daemon that the user changes by hand (`veil set` and the other commands, a configuration file), that survives restarts and recovers from a crash by itself, whose resource claims are measured ([`measurements.md`](measurements.md)), and that is packaged for Nix and as release binaries. Video plays without audio.

## The beta (1.0.0-beta.1)

The first public release, 1.0.0-beta.1 (published on 2026-11-04), is a beta, and it relaxes the 1.0 checklist below. **Required, and done:** the release workflow run on the release tag itself. **Not required, and said to be untested in the [support matrix](troubleshooting.md#support-matrix) instead:** the 24-hour soak, a run on ARM, Intel and AMD graphics, other distributions and compositors. The beta is free and source-available ([`LICENSE`](../LICENSE)), takes no issues or pull requests (reports go to GitHub Discussions), and is distributed as the release tarballs, the Nix flake and, after the release, an AUR `veil-bin` package.

## Before 1.0

### Tests still to write

None: since 2026-11-20 every check that was made by hand when its feature was added is a test or a script ([`testing.md`](testing.md)). Three of them had not run in CI when they were written, because they need Nix or containers: the flake checks `video` and `completions`, and the workflow `Installation on distributions`; their first green run completes this. What no test can reach (Hyprland's own rendering, graphics cards and architectures CI does not have, systemd acting on what the daemon tells it) is listed in [`testing.md`](testing.md), "Not covered by any test", and is not a condition of the release.

### The release workflow

`.github/workflows/release.yml` ran on GitHub for the tag `v1.0.0-beta.1` (2026-11-04) and published the release as a pre-release with the x86_64 and aarch64 tarballs and `SHA256SUMS`. Before 1.0:

- Confirm or change the decision of known issue 8 (animated GIFs and frame drops with the GStreamer of Debian 12 and Ubuntu 22.04 and 24.04: documented, not worked around).

### Release checklist

All of these must be true for 1.0:

- CI green on `main`: `cargo fmt --check`, clippy with `-D warnings`, `cargo test`, `nix build`, `cargo deny check`, the MSRV build, and the release build.
- Every check once made by hand is a test (done 2026-11-20, [`testing.md`](testing.md)), green in CI, the flake checks and the workflow `Installation on distributions` included. Of a crash that recovers by itself, the daemon's part is tested (it comes back with its saved wallpapers after `kill -9`, and tells systemd what the unit relies on); systemd's restart itself cannot run where the tests do.
- Soak: flat resident memory, threads and descriptors over 24 hours of a looping video and over 500 wallpaper switches (`scripts/bench/soak.sh loop --duration 86400`, once with the hardware decoder and once with `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`, which crept for its first minutes; `scripts/bench/soak.sh switch`; both with the shipped `MALLOC_ARENA_MAX=1`).
- Idle: 0.0 % CPU for static images and for fully covered or paused video, as the headless tests measure it.
- Every example in the documents is run by a test, as those of [`ipc.md`](ipc.md) are.
- No `unwrap()` in production paths and every `unsafe` documented (the lints in `Cargo.toml` enforce it; confirm that nothing has silenced them).
- A security review of the control socket ([`SECURITY.md`](../SECURITY.md)).
- `CODE_OF_CONDUCT.md` names contact@ordreth.com as its contact, private vulnerability reporting is turned on for the repository, and Discussions has its "Reports" category (settings on GitHub).
- `CHANGELOG.md` and the version in `Cargo.toml` and `flake.nix` are bumped.

### Open decisions

- **Known issue 1:** fixed in veil by its own video sink (2026-11-09). Whether to report the `waylandsink` crash upstream after all is left open; it no longer affects veil.
- **Other distributions and compositors:** veil is developed on NixOS and Hyprland, at our own pace. Another system is looked at when people who use it show interest, not ahead of time ([Discussions, "Reports"](https://github.com/ordreth/veil/discussions/categories/reports)).
- **The crate name:** `veil` is taken on crates.io; decided on 2026-10-19 not to publish there for 1.0.

## After 1.0

Not started before the release. None of these is needed to change wallpapers by hand, and waiting locks nothing in: the state file has a version, the configuration file has no keys for features that do not exist, and new IPC commands are additive.

- **Playback options:** audio (muted today; with one pipeline per output it first needs a decision on which output plays the sound), a frame-rate cap (`videorate` drops frames after they are decoded, so whether it saves anything is a question for the measurements), playback speed, loop points, a decoder preference. Forcing software decoding already works through `GST_PLUGIN_FEATURE_RANK` ([`troubleshooting.md`](troubleshooting.md)).
- **Playlists and rotation:** `veil set <dir> --interval 30m [--shuffle]`, `next`/`prev`, on a `calloop` timer and not a thread. Rotation can be scripted with `veil set` meanwhile.
- **Transitions:** an optional fade between images. The two-engine hand-over of `RenderState` is what a fade would build on.
- **More compositor backends and triggers:** the niri and river backends (sway and Hyprland exist), and pausing when the session is idle or on battery.
- **After replacing `waylandsink`** (done on 2026-11-09, known issue 1): dmabuf frames through `zwp_linux_dmabuf_v1` (VA-API on Intel and AMD, without a conversion on the CPU: done on 2026-11-20), a conversion on the GPU for NVIDIA's decoders (known issue 7), and a pipeline that moves to its output's new surface instead of starting again (known issue 2).
- **A frame-callback loop at the video's frame rate** instead of the display's (known issue 6): not tried.
- **Packaging:** NixOS and home-manager modules (`programs.veil.enable`, `settings` rendered to TOML), AUR, `.deb` and `.rpm` packages. Flatpak is out of scope: it cannot reach the compositor's IPC socket by default.
- **Dependency hygiene:** fewer `notify-rust` features or a direct call to `org.freedesktop.Notifications`; upgrades of `gstreamer` (0.22), `thiserror` (1 to 2) and `smithay-client-toolkit`, which remove the exceptions in `deny.toml`; the `image` features veil does not use (the default features build an AV1 encoder and the OpenEXR codec); Dependabot or Renovate.
- **Tests and documentation:** fuzzing (`cargo-fuzz` for the IPC request parser, the configuration parser, the Hyprland event parser and `engine::media::sniff`; `proptest` for the scaling geometry), a comparison with `mpvpaper` and `swww` on the same clips and hardware (`docs/benchmarks.md`), rustdoc for the public traits and `#![warn(missing_docs)]`.
- **Diagnostics:** `--log-level`, `--log-format {pretty,json}`, and `veil doctor` (Wayland globals, GStreamer plugins, VA-API, socket permissions, configuration).
- **After the release:** a label scheme for bug triage, a release cadence (patch releases for fixes), a deprecation policy for the `--daemon` alias, and the compatibility reports of users.

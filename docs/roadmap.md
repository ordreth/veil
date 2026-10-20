# Roadmap

What is left before veil 1.0, and what is deliberately left for after it. Everything in the feature table of the [README](../README.md) is implemented and tested; what remains before the release is checking by hand where no test can, and publishing.

**What 1.0 is:** a wallpaper daemon that the user changes by hand (`veil set` and the other commands, a configuration file), that survives restarts and recovers from the GStreamer crash of [known issue 1](known-issues.md) by itself, whose resource claims are measured ([`measurements.md`](measurements.md)), and that is packaged for Nix and as release binaries. Video plays without audio.

## The beta (1.0.0-beta.1)

The first public release is a beta, and it relaxes the 1.0 checklist below. **Required before the tag:** CI green on the commit that becomes the public one; the checklist in [`manual-tests.md`](manual-tests.md), "In a real Hyprland session", done by the project owner, with what it finds fixed or recorded in [`known-issues.md`](known-issues.md) (on 2026-10-20 an agent ran the pause behind a fullscreen window, images and video on the owner's Hyprland; the rest is open); and the release workflow run once, on a pre-release tag. **Not required, and said to be untested in the [support matrix](troubleshooting.md#support-matrix) instead:** the 24-hour soak, a run on ARM, Intel and AMD graphics, other distributions and compositors. The beta is free and source-available ([`LICENSE`](../LICENSE)), takes no issues or pull requests (reports go to GitHub Discussions), and is distributed as the release tarballs, the Nix flake and, after the release, an AUR `veil-bin` package.

## Before 1.0

### Checks in a real Hyprland session

Almost everything was checked in a headless sway; on Hyprland 0.56.2 only images, a video on two monitors and the pause behind a fullscreen window were run (2026-10-20, by an agent on the owner's machine). The checklist is in [`manual-tests.md`](manual-tests.md), "In a real Hyprland session": the pause behind fullscreen windows, the lock screen and DPMS, whether a video in `fill` stands still at every loop (known issue 5), whether it plays smoothly (known issue 6), the systemd unit, crash recovery, logout, completions, and the Hyprland row of the compatibility table in [`troubleshooting.md`](troubleshooting.md#compatibility). A problem found there is fixed, or recorded as a known issue, before the release.

### The release workflow

`.github/workflows/release.yml` has not run on GitHub yet (see [`manual-tests.md`](manual-tests.md), "The release tarball", for what was checked locally). Before 1.0:

- Push a pre-release tag (`v1.0.0-rc.1`), and check that it publishes both tarballs, `SHA256SUMS` and the attestations.
- Run the aarch64 tarball on an ARM machine with a compositor: nothing has shown a picture or a video with it yet.
- Confirm or change the decision of known issue 8 (animated GIFs and frame drops with the GStreamer of Debian 12 and Ubuntu 22.04 and 24.04: documented, not worked around).

### Release checklist

All of these must be true for 1.0:

- CI green on `main`: `cargo fmt --check`, clippy with `-D warnings`, `cargo test`, `nix build`, `cargo deny check`, the MSRV build, and the release build.
- A crash recovers by itself: under the systemd unit the daemon comes back with its saved wallpaper (known issue 1, option D).
- Soak: flat resident memory, threads and descriptors over 24 hours of a looping video and over 500 wallpaper switches (`scripts/bench/soak.sh loop --duration 86400`, once with the hardware decoder and once with `GST_PLUGIN_FEATURE_RANK=nvh264dec:0`, which crept for its first minutes; `scripts/bench/soak.sh switch`; both with the shipped `MALLOC_ARENA_MAX=1`).
- Idle: 0.0 % CPU for static images and for fully covered or paused video.
- Verified on at least Hyprland and one other wlroots compositor, on Intel, AMD and NVIDIA graphics (or the unsupported ones are documented).
- Every example in the documents executes as written.
- No `unwrap()` in production paths and every `unsafe` documented (the lints in `Cargo.toml` enforce it; confirm that nothing has silenced them).
- A security review of the control socket ([`SECURITY.md`](../SECURITY.md)).
- `CODE_OF_CONDUCT.md` names the GitHub account of the maintainer as its contact, private vulnerability reporting is turned on for the repository, and Discussions has its "Reports" category (see [`manual-tests.md`](manual-tests.md), "Governance files").
- `CHANGELOG.md` and the version in `Cargo.toml` and `flake.nix` are bumped.

### Open decisions

- **Known issue 1:** whether to report the `waylandsink` crash upstream after all. Not blocking.
- **The crate name:** `veil` is taken on crates.io; decided on 2026-10-19 not to publish there for 1.0.

## After 1.0

Not started before the release. None of these is needed to change wallpapers by hand, and waiting locks nothing in: the state file has a version, the configuration file has no keys for features that do not exist, and new IPC commands are additive.

- **Playback options:** audio (muted today; with one pipeline per output it first needs a decision on which output plays the sound), a frame-rate cap (`videorate` drops frames after they are decoded, so whether it saves anything is a question for the measurements), playback speed, loop points, a decoder preference. Forcing software decoding already works through `GST_PLUGIN_FEATURE_RANK` ([`troubleshooting.md`](troubleshooting.md)).
- **Playlists and rotation:** `veil set <dir> --interval 30m [--shuffle]`, `next`/`prev`, on a `calloop` timer and not a thread. Rotation can be scripted with `veil set` meanwhile.
- **Transitions:** an optional fade between images. The two-engine hand-over of `RenderState` is what a fade would build on, and it would have to be redone if `waylandsink` is replaced.
- **More compositor backends and triggers:** the niri and river backends (sway and Hyprland exist), and pausing when the session is idle or on battery.
- **Replacing `waylandsink`:** a sink of veil's own would remove known issues 1 and 2, the conversion cost of known issue 7 on NVIDIA, and the wake-ups measured in [`measurements.md`](measurements.md). Large.
- **A frame-callback loop at the video's frame rate** instead of the display's (known issue 6): not tried.
- **Packaging:** NixOS and home-manager modules (`programs.veil.enable`, `settings` rendered to TOML), AUR, `.deb` and `.rpm` packages. Flatpak is out of scope: it cannot reach the compositor's IPC socket by default.
- **Dependency hygiene:** fewer `notify-rust` features or a direct call to `org.freedesktop.Notifications`; upgrades of `gstreamer` (0.22), `thiserror` (1 to 2) and `smithay-client-toolkit`, which remove the exceptions in `deny.toml`; the `image` features veil does not use (the default features build an AV1 encoder and the OpenEXR codec); Dependabot or Renovate.
- **Tests and documentation:** fuzzing (`cargo-fuzz` for the IPC request parser, the configuration parser, the Hyprland event parser and `engine::media::sniff`; `proptest` for the scaling geometry), a comparison with `mpvpaper` and `swww` on the same clips and hardware (`docs/benchmarks.md`), rustdoc for the public traits and `#![warn(missing_docs)]`.
- **Diagnostics:** `--log-level`, `--log-format {pretty,json}`, and `veil doctor` (Wayland globals, GStreamer plugins, VA-API, socket permissions, configuration).
- **After the release:** a label scheme for bug triage, a release cadence (patch releases for fixes), a deprecation policy for the `--daemon` alias, and the compatibility reports of users.

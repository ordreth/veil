# Changelog

Every change a user of veil can notice is recorded here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and veil uses [semantic versioning](https://semver.org/spec/v2.0.0.html) from 1.0 on. What counts as veil's interface for the version number: the command line (commands, flags, values, what `--json` prints), the exit statuses, the control-socket protocol ([`docs/ipc.md`](docs/ipc.md)), the keys of the configuration file ([`docs/configuration.md`](docs/configuration.md)) and the state file.

veil 1.0.0-beta.1 is the first release, a beta. Everything below is in it. The limitations that are known and left open on purpose are in [`docs/known-issues.md`](docs/known-issues.md); the most important ones are listed at the end of the section.

## [Unreleased]

## [1.0.0-beta.1] - 2026-10-20

### Added

- veil is source-available under the PolyForm Strict License 1.0.0 with additional permissions (`ADDITIONAL-PERMISSIONS.md`), free to use for noncommercial purposes; it is not open source. The release tarballs and the Nix package carry `LICENSE`, `ADDITIONAL-PERMISSIONS.md` and `THIRD-PARTY-NOTICES`, the licences of the Rust crates veil links.
- `veil daemon`: a wallpaper on every output of a Wayland compositor with `wlr-layer-shell` (Hyprland, sway and other wlroots compositors), also on outputs connected later, or only on the outputs named with `--output`. An output that is unplugged gets its wallpaper back when it returns.
- Images in the formats of the `image` crate (PNG, JPEG, WebP, BMP, TIFF, GIF, ...), decoded once and composited at the output's resolution; pictures over 16384 pixels per side, or that would need more than 512 MiB, are refused before they are decoded.
- Video wallpapers through GStreamer (`waylandsink`), without audio, looped without rebuilding the pipeline, decoded in hardware when GStreamer has a decoder for the card. The log names the decoder.
- Images and videos are told apart by the content of the file. Animated GIFs play as videos. Files that cannot be a wallpaper (a directory, an empty file, HEIF, JPEG XL, something unknown) are refused with the reason.
- Scaling modes `fill`, `fit`, `stretch`, `center` and `tile` (`--mode`), and a background colour for the bars (`--bg-color`), for images and videos.
- HiDPI and fractional scales (`wp_viewporter`, `wp_fractional_scale_v1`): pictures are drawn at the output's physical resolution.
- A new wallpaper replaces the old one only once it has something to show (for a video, its first frame); one that fails leaves the old one on screen.
- A video that fails while it plays is restarted (four times in a row at most), then given up with one desktop notification. Notifications are throttled.
- Automatic pause of a video behind a fullscreen window, or, with `--pause-on covered`, behind windows that cover the whole output, read from the IPC of Hyprland or sway, per output.
- A video nobody is shown (a locked screen, a monitor that is off) is stopped after two seconds and started again when the output is drawn again, on any compositor (`--release-unseen-after`).
- Commands for a running daemon: `veil set`, `clear`, `pause`, `resume` and `toggle`, for every output or one (`--output`), `status` and `outputs` (both with `--json`), `quit` (alias `stop`) and `ping`, with stable exit statuses.
- A control socket in `XDG_RUNTIME_DIR` for other programs (status bars, scripts), one JSON object per line, versioned, and only for processes of the same user. Reference: [`docs/ipc.md`](docs/ipc.md).
- The wallpaper of each output is saved (`$XDG_STATE_HOME/veil/state.json`) and shown again after a restart; `veil set --no-persist` skips that.
- A configuration file, `~/.config/veil/config.toml`: the daemon's options and each output's wallpaper, checked with `veil config check`. Reference: [`docs/configuration.md`](docs/configuration.md), example: `contrib/config.example.toml`.
- A systemd user unit (`contrib/systemd/veil.service`): active once every output shows its wallpaper, restarted after a crash with the wallpapers it had, a watchdog on the event loop.
- One daemon per compositor; a second one exits with status 4.
- Shell completions for bash, zsh, fish and nushell (`veil completions`) and a man page per command.
- A Nix flake: the package (wrapped with its GStreamer plugins and a GStreamer registry of its own), `nix run`, an overlay, and dev shells.
- Prebuilt binaries for x86_64 and aarch64 Linux with every release: a tarball with the binary, the systemd unit, the man pages, the shell completions, the documents and `install.sh` (into `/usr/local`, or `--prefix ~/.local`; `--uninstall`), with `SHA256SUMS` and a build-provenance attestation per tarball. They use the system's GStreamer and need glibc 2.34 or newer; run on Debian 12 and 13, Ubuntu 22.04 and 24.04, Fedora 44 and Arch Linux.
- Documentation: a README with a quick start, the features and their status, the measured numbers, and the installation from source on Arch, Debian, Ubuntu and Fedora; [troubleshooting and a compatibility table](docs/troubleshooting.md), [what was measured](docs/measurements.md), [known issues](docs/known-issues.md).

### Deprecated

- `veil --daemon ...`, the command line of the first prototype. It still starts the daemon, with a warning; use `veil daemon ...`.

### Known issues

- A monitor that is changed, plugged in or powered on while a video plays crashes the daemon, inside GStreamer's `waylandsink` (known issue 1). Under the systemd unit it is back within seconds with its wallpapers.
- On NVIDIA every video frame is converted to RGB on the CPU: about 18 % of a core for 1080p at 30 fps (known issue 7).
- Without `MALLOC_ARENA_MAX=1`, which the Nix package and the systemd unit set, memory grows with every video switch (known issue 3).
- With the prebuilt binary on a GStreamer older than 1.26 (Debian 12, Ubuntu 22.04 and 24.04), animated GIFs stand still or do not start, because of GStreamer's GIF demuxer (on Debian 12 it also keeps a CPU core busy); converted to a video they play. On Ubuntu 24.04, 1080p videos showed only part of their frames in a headless sway (known issue 8).
- Everything so far was checked in a headless sway on an NVIDIA card; the checks in a real Hyprland session, and on Intel and AMD cards, are still to be done.

[Unreleased]: https://github.com/ordreth/veil/compare/v1.0.0-beta.1...HEAD
[1.0.0-beta.1]: https://github.com/ordreth/veil/releases/tag/v1.0.0-beta.1

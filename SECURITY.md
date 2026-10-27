# Security policy

## Reporting a vulnerability

Please report a vulnerability privately, through GitHub: on the repository's **Security** tab, **Report a vulnerability** ([direct link](https://github.com/ordreth/veil/security/advisories/new)). Do not post it in a public discussion. If you cannot use GitHub, write to contact@ordreth.com with "veil security" in the subject; do not put the details of an unfixed problem in a public place.

A useful report says:

- the version (the first line of the daemon's log: `veil 1.0.0-beta.1 (GStreamer 1.28.7)`) or the commit,
- what an attacker needs: a process of another user, a file that someone shows as a wallpaper, a compositor, ...,
- what they can do with it, and how to reproduce it (a file that triggers it, if there is one).

You will be told when the report has been read and what is planned. A fix is published together with a GitHub security advisory, which credits you if you want it to. There is no fixed time frame.

## Supported versions

veil is in beta (1.0.0-beta.1). Fixes go to `main` and into a new release; only the latest release gets them.

## Verifying a release

Release tarballs are built only by the release workflow (`.github/workflows/release.yml`), on GitHub's machines, from the tagged commit, in a Debian 12 container pinned by its digest (`scripts/release/build.sh`). Each release lists the SHA-256 sum of every tarball in `SHA256SUMS`, and has a signed build-provenance attestation per tarball that names the repository, the workflow and the commit: `gh attestation verify veil-<version>-<target>.tar.gz --repo ordreth/veil`. The build is not bit-for-bit reproducible yet (two builds of the same commit differ in a few hundred bytes of machine code), so a tarball cannot be checked by building it again; the attestation is the check. veil is not published on crates.io.

## What veil trusts

veil is a user's own wallpaper daemon. It runs with the user's rights and needs no others, opens no network connection, and listens on nothing but its Unix control socket. What it trusts, and what it does not:

**The control socket.** It is `$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock`, with mode `0600`, in a directory that is the user's own; there is no fallback to `/tmp` or another shared directory. The daemon also checks the user id of every client (`SO_PEERCRED`) and answers a process of another user `forbidden` without reading its request; `veil` does not send requests to a socket that another user listens on. A client of the same user is trusted with everything the commands do: it can make the daemon show any file the user can read, pause it, stop it, and read the paths of its wallpapers and the names of the outputs (`veil status`). It gets no file content back. So the socket gives nothing beyond the user's own rights, **but a sandboxed program that is given access to the socket can make veil open files outside its sandbox** and show them on screen.

A client cannot block the daemon or make it exit: requests have a time limit (2 s) and a size limit (64 KiB), at most 16 connections are served at once, the daemon never waits for a client, and a malformed request is answered with an error. The limits are in [`docs/ipc.md`](docs/ipc.md#limits).

**Media files.** A wallpaper is untrusted input, and the code that reads it is most of veil's attack surface.

- Pictures are decoded by the `image` crate (Rust). Before anything is decoded, a picture over 16384 pixels per side, or one that would need more than 512 MiB, is refused.
- Videos are read and decoded by GStreamer and its plugins (C, and FFmpeg through `gst-libav`), inside the daemon's process and with the user's rights. A file made to exploit a bug in a demuxer or a decoder can therefore run code as the user, as it can in any media player. Bugs in those libraries belong to [GStreamer](https://gstreamer.freedesktop.org/security/) or [FFmpeg](https://ffmpeg.org/security.html); tell us too if veil should do something about them.
- veil decides between image and video by the first 512 bytes of the file, and refuses directories, devices and pipes before a decoder sees them.

**The compositor.** The Wayland compositor is trusted: it has veil's surface anyway. Hyprland's and sway's IPC sockets (in `$XDG_RUNTIME_DIR/hypr/` and at `$SWAYSOCK`) are read without blocking and with time limits, and an answer that cannot be parsed never ends the daemon.

**The environment.** Whoever sets the daemon's environment can run code as the user (GStreamer loads plugins from `GST_PLUGIN_PATH`, for instance), so it is trusted, as for any program. The same goes for the configuration file, which is read once at start-up.

**What veil writes.** The state file (`$XDG_STATE_HOME/veil/state.json`) is written with mode `0600` in directories with mode `0700`, to a temporary file that is renamed over it. The control socket and its lock file have mode `0600`. Desktop notifications go over the session bus, and may contain the paths of wallpapers.

**The code.** `unsafe` is allowed in one module, `src/engine/video.rs`, to hand the raw Wayland pointers to GStreamer, with the reason for each use next to it; the compiler refuses it everywhere else. A panic ends the daemon (the release build aborts) instead of leaving it in a broken state.

## What is in scope

Report privately anything that lets someone do what the user did not allow: a process of another user getting the daemon to do anything, a file veil writes being readable by another user or written somewhere else, a request or a compositor answer that corrupts memory or runs code, a picture that gets past the size limits.

Not a vulnerability: what the user's own processes can do through the socket (above), and the crash of known issue 1 (a monitor change while a video plays, [`docs/known-issues.md`](docs/known-issues.md)). A crash or hang from a malformed request or compositor answer that does none of the above is a bug: report it in [Discussions, category "Reports"](https://github.com/ordreth/veil/discussions/categories/reports).

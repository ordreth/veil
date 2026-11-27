# Privacy Policy

**veil** · applies to veil 1.0.0-beta.1 and later · last updated 2026-11-27

> Anything here that veil's code does not back up is a bug: tell us at contact@ordreth.com.

## In short

- veil is a wallpaper daemon that runs on your computer. It collects nothing and sends nothing: no account, no
  telemetry, no analytics, no crash upload, no update check. **It opens no network connection.**
- What it keeps, it keeps on your computer, in files you can read and delete.
- Ordreth does not receive, collect or hold any personal data about you through veil.

## Who this is from

veil is made and published by **Ordreth**, an independent software studio ("we", "us"). The one way to reach us
is **contact@ordreth.com**. This policy covers the veil program and what it does. How the Ordreth website and our
e-mail address handle data is described on the website's own privacy page, and GitHub, which hosts veil's
repository, releases and discussions, has its own.

## What veil reads

- **The files you set as wallpapers**, with `veil set`, `veil daemon --input` or the configuration file. veil reads
  them to show them; it does not change, move or copy them.
- **Its configuration file**, `~/.config/veil/config.toml`, if you wrote one.
- **What your compositor says about its monitors and windows.** Under Hyprland and sway, veil reads the
  compositor's IPC (which monitor shows a fullscreen window, which windows cover it, which workspace is shown) to
  pause a video nobody can see. It keeps none of it once the decision is made.

## What veil writes on your computer

| What | Where | What it holds |
|---|---|---|
| The saved wallpapers | `~/.local/state/veil/state.json` (`$XDG_STATE_HOME`) | Per monitor name: the path of its wallpaper, the scaling mode and the background colour. Readable by you only (mode `0600`). |
| The control socket and its lock file | `$XDG_RUNTIME_DIR` (`veil-<display>.sock`) | Nothing: the socket carries commands from your own programs and is gone when your session ends. |
| A GStreamer plugin cache, with the Nix package only | `~/.cache/veil/gstreamer-registry.bin` | The list of GStreamer plugins installed, so that start-up is faster. |
| The log | Standard error, which systemd puts in your journal (`journalctl --user -u veil`) | What the daemon did: veil's and GStreamer's versions, the paths of wallpapers, the decoder each video uses, errors. veil writes no log file of its own. |

**Desktop notifications.** When the daemon cannot start, cannot draw a wallpaper, or gives up on a video that keeps
failing, veil shows a notification over the session bus (at most one per error, throttled). Its text is the error,
which may contain the path of the wallpaper; your notification daemon may keep a history of it.

`veil set --no-persist` shows a wallpaper without saving it. Deleting `~/.local/state/veil/` and `~/.cache/veil/`
removes everything veil wrote; the log follows your journal's own retention.

## What leaves your computer

Nothing, from veil. It has no network code; GStreamer, which plays the videos, is given local files only.

What you do around veil is handled by others:

- **Downloads.** The release tarballs and the source are served by GitHub, and the Nix flake is fetched from
  GitHub. GitHub shows us how many times each release file was downloaded, and nothing about who.
- **Reports and discussions** you post on GitHub are public, under GitHub's terms. A security report goes through
  GitHub's private vulnerability reporting or to contact@ordreth.com; we use what it contains only to look into the
  problem.

## Your rights

veil does not process personal data on our behalf, so there is nothing about you for us to show, correct or
delete. If you think it does, or if you write to us, the address is contact@ordreth.com; Brazilian law (LGPD)
applies to what we receive there, and the law of your country where it gives you more.

## Changes

If veil ever starts doing something this page does not describe, the page changes first, with the date above,
and the change is recorded in [`CHANGELOG.md`](CHANGELOG.md).

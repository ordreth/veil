# Configuration file

`veil daemon` reads a TOML file at start-up. The file sets the daemon's options and gives each output its wallpaper, so a setup can be written down once, in dotfiles or in a Nix module, instead of on a command line. An annotated example is [`contrib/config.example.toml`](../contrib/config.example.toml). The Nix package installs it as `share/doc/veil/config.example.toml`.

## Where it is

| | |
|---|---|
| Default | `$XDG_CONFIG_HOME/veil/config.toml`, or `~/.config/veil/config.toml` when `XDG_CONFIG_HOME` is not set (or is not an absolute path) |
| Another file | `veil daemon --config FILE`. The file must exist. |
| No file | Not an error. The built-in defaults and the saved wallpapers apply. |

The daemon reads the file once, when it starts. **A change takes effect at the next start** (`systemctl --user restart veil`, or `veil quit` and `veil daemon`). There is no reload command.

## Checking it

```bash
veil config check                  # the default file
veil config check --config FILE
```

`veil config check` reads the file the way the daemon would. It does not need a running daemon. It opens every `input` as the daemon does at start-up: the file must exist and be a video or an image that can be decoded. It then prints what the daemon would start with:

```text
/home/me/.config/veil/config.toml: ok
outputs: every output
mode: fill
bg_color: #000000
pause_on: fullscreen
release_unseen_after: after 2 s
[output."DP-3"]: video /home/me/Videos/waves.mp4 (fit, #000000)
[output."*"]: image /home/me/Pictures/wallpaper.png (fill, #000000)
```

Exit statuses: `0` when the file is fine (or when there is no file), `2` when the file is wrong, `1` when the file is fine but a wallpaper in it cannot be shown. Errors go to stderr and name the file, line and column:

```text
/home/me/.config/veil/config.toml:7:1: unknown field `imput`, expected one of `input`, `mode`, `bg_color`
```

A daemon that finds the file wrong does not start. It logs the same error, raises a desktop notification and exits with status 2, which the systemd unit does not restart on. A wallpaper in the file that cannot be shown does not stop the daemon: it logs a warning, and that output shows the section's background colour.

## Keys

Every key is optional. **An unknown key is an error**, so a misspelt key is reported instead of being ignored. There are keys only for what veil can do today. Later versions add keys; they do not rename or remove them.

### `[defaults]`

These keys are the daemon's flags. A flag given on the command line wins over the file, and the file wins over the built-in default.

| Key | Type | Default | Flag | Meaning |
|---|---|---|---|---|
| `mode` | string | `"fill"` | `--mode` | How a wallpaper is fitted to its output: `fill` (cover, crop the overflow), `fit` (inside, bars in `bg_color`), `stretch` (ignore the aspect ratio), `center` (native size, centred), `tile` (native size, repeated; `center` for a video). Also the default of `veil set`. |
| `bg_color` | string `"#rrggbb"` | `"#000000"` | `--bg-color` | Colour of what the wallpaper does not cover, and of an output without a wallpaper. Also the default of `veil set` and `veil clear`. |
| `pause_on` | string | `"fullscreen"` | `--pause-on` | When a video is paused because it cannot be seen: `never`, `fullscreen` (a fullscreen window on its output), `covered` (windows cover the whole output, translucent ones too). Works with Hyprland and sway. |
| `release_unseen_after` | integer ≥ 0 | `2` | `--release-unseen-after` | Seconds a video may go without the compositor taking its frames (a locked screen, a monitor that is off) before it is stopped. It starts again from its beginning when it is shown again. `0`: never. |
| `outputs` | array of strings | `[]` | `--output` (repeatable) | The outputs to draw on, by name. Empty: every output, also those connected later. Given on the command line, `--output` replaces this list. |

### `[output."NAME"]`

The wallpaper of one output, by the name the compositor gives it (`veil outputs` lists them, for example `DP-3` or `HDMI-A-1`). Quotes are needed only for names that are not plain TOML keys, such as `"*"`.

| Key | Type | Default | Meaning |
|---|---|---|---|
| `input` | string | none | An image or a video. Content decides which, as for `veil set`. See "Paths" below. |
| `mode` | string | `[defaults]` `mode` | As in `[defaults]`. Only with an `input`. |
| `bg_color` | string `"#rrggbb"` | `[defaults]` `bg_color` | As in `[defaults]`. |

A section without `input` shows only its `bg_color`, as after `veil clear`. A `mode` in such a section is an error.

A section's own `mode` and `bg_color` win over the defaults, also over a `--mode` or `--bg-color` on the command line. Those flags set the defaults, and the section is more specific.

### `[output."*"]`

The same keys, for every output that has no section of its own. That includes outputs connected later. It is the only pattern: `*` stands alone, and names such as `DP-*` are not patterns.

## Paths

In `input`:

- `~` at the start, alone or before `/`, is your home directory (`$HOME`). `~user` is an error.
- `$NAME` and `${NAME}` are environment variables of the daemon's process. A variable that is not set is an error. Write `$$` for a literal `$`.
- A relative path is taken from the directory of the configuration file, as the file was named. A symbolic link is not followed first, so with a file linked from the Nix store, the directory is `~/.config/veil`.

Under systemd, the daemon's environment is the user manager's, not your shell's. `$HOME` and the XDG variables are there; variables your shell sets may not be.

## The configuration file, `veil set` and the saved wallpapers

veil keeps two things that decide what an output shows: this file, and the state file of `veil set` (`$XDG_STATE_HOME/veil/state.json`, see the README). At start-up:

1. `veil daemon --input FILE` shows `FILE` on every output. The file's sections are not used.
2. Otherwise, an output with a section of its own shows that section's wallpaper.
3. Otherwise, if there is an `[output."*"]`, the output shows it.
4. Otherwise, the output shows its saved wallpaper: the last `veil set` for it, or for every output.
5. Otherwise, it shows `bg_color`.

**The file wins at every start.** `veil set` changes an output's wallpaper while the daemon runs, and it is saved as usual. But a restarted daemon shows the file's wallpaper again on every output the file covers. To keep a wallpaper from `veil set` across restarts, leave that output out of the file: no section of its own, and no `[output."*"]`. The file's wallpapers are also saved when the daemon starts, as `--input` is, so a daemon started later without the file shows them as well.

An output connected while the daemon runs gets the same treatment, unless a `veil set` for every output has happened since the start. That `set` then decides, as without a file.

## Example

```toml
[defaults]
mode = "fill"
pause_on = "covered"

# The main monitor plays a video, letterboxed on dark grey.
[output."DP-3"]
input = "~/Videos/waves.mp4"
mode = "fit"
bg_color = "#202020"

# The laptop panel shows only a colour.
[output."eDP-1"]
bg_color = "#1e1e2e"

# Every other monitor, also ones plugged in later.
[output."*"]
input = "~/Pictures/wallpaper.png"
```

With home-manager, `xdg.configFile."veil/config.toml".text = ''...'';` writes the file. Restart the service after a change.

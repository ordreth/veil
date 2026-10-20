# The control socket and the `veil` commands

`veil daemon` listens on a Unix socket. Every other `veil` command that talks to the daemon (`veil set`, `veil status`, ...) is a client of that socket, and so can any other program: a status bar, a script, a key binding, a program in another language. This page is the reference of the protocol (the socket, the wire format, every command, every field, every answer and every error code) and of the command-line side (which command sends what, what it prints, its exit status).

<!--
tests/ipc_reference.rs reads this file and checks it against the protocol
types, so keep its conventions:
- A request is a `### \`cmd\`` heading under "Requests"; its table lists its
  fields, one row each, the field name in backticks in the first cell.
- A result type is a `### \`Type\`` heading under "Results", a list of values a
  `### \`field\`` heading under "Values", with the same kind of table.
- The tables under "Fields", "Error codes" and "Exit statuses" list exactly the
  envelope's fields, the error codes and the exit statuses.
- Every `echo '<request>' | socat ...` line in an `sh` block is followed by a
  `# <response>` line: the request must be one the daemon reads (unless the
  answer is `invalid_request`), and the response, and its `result`, must read
  back as the types say, field for field. A `json` block is a whole response.
-->

## The socket

The socket is `$XDG_RUNTIME_DIR/veil-$WAYLAND_DISPLAY.sock`:

* `XDG_RUNTIME_DIR` must be set to an absolute path. There is no fallback to a shared directory such as `/tmp`, where another user could create the socket first; without it every `veil` command fails (status 1).
* If `WAYLAND_DISPLAY` is a path (some compositors set the full path of their socket), only its last component is used. If it is unset, the name is `wayland-0`.

So one daemon runs per compositor, and a client reaches the daemon of the compositor it runs in. In a shell:

```sh
sock="$XDG_RUNTIME_DIR/veil-${WAYLAND_DISPLAY##*/}.sock"
```

Access:

* The socket file has mode `0600`, and the daemon also checks the user id of every client (`SO_PEERCRED`): a process of another user is answered `forbidden` and its request is not read. `veil` does the same in the other direction and does not send requests to a socket that a process of another user listens on.
* Next to the socket is its lock file, `veil-$WAYLAND_DISPLAY.sock.lock`. A daemon holds a lock on it for as long as it runs, which is how a second daemon knows that one is running (it exits with status 4). The lock file stays when the daemon exits; do not remove it while a daemon runs.
* The daemon removes the socket when it exits. A daemon that was killed leaves it behind: connecting is then refused (`ECONNREFUSED`), which `veil` reports as "no daemon is running" (status 3), and the next daemon removes the leftover.

## Connections

One connection carries one request and one response:

1. The client connects and sends the request: one JSON object, in UTF-8, ending with a newline, or with the end of the stream (the client shuts down its writing side). White space around the object is ignored, so a CRLF line ending is fine.
2. The daemon reads up to the first newline. Anything after it is ignored: a second request on the same connection is never read.
3. The daemon sends the response, one JSON object on one line ending with a newline, and closes the connection.

`socat` does all of this. (The examples on this page were sent to a running daemon; only the paths, the output names and numbers such as the pid were changed.)

```sh
echo '{"cmd":"ping"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

Most requests are answered at once. `set` and `clear` are answered when the wallpaper is on screen, which takes as long as loading the file (for a video, until its first frame is shown); meanwhile the daemon goes on serving other clients. A client should wait for the answer; `veil` waits up to 30 seconds. If the client goes away before the answer, the request is still carried out.

### Limits

| Limit | Value | When it is over |
|---|---|---|
| Time to send the request, from the connect (more data does not extend it) | 2 s | `timeout`, and the connection is closed |
| Length of the request line, without the newline | 64 KiB | `too_large` |
| Clients connected at once, counting those that wait for the answer to a `set` | 16 | `busy`, at once |
| How long `veil` waits for an answer | 30 s | `veil` gives up (status 1); the daemon may still carry out the request |

The answers `timeout`, `too_large`, `busy` and `forbidden` come from the daemon's transport, before the request is read or parsed; the connection is closed right after them. A client that is still sending when its request is refused may then see its connection reset after the answer (`socat` prints `Connection reset by peer` after a `too_large`).

## Requests

A request is a JSON object whose `cmd` names the command, with the command's fields next to it:

```json
{"cmd":"set","output":"DP-3","input":"/home/me/walls/sea.png","mode":"fit"}
```

* `cmd` is one of the commands below, in lower case.
* A field the command does not take is refused (`invalid_request`), also for commands that take no fields: a misspelt option is never silently ignored. Fields may come in any order.
* An optional field may be left out or be `null`; both mean the default.
* `output` names one output (monitor) as the compositor names it, such as `DP-3` or `HDMI-A-1` (`veil outputs` lists them). It must not be empty. Without it a command is for every output the daemon draws on, including outputs that are unplugged at the moment (the daemon keeps their wallpaper for when they are back). An output is one the daemon draws on if it has seen it since it started, or, when the daemon was started with `--output` (or `outputs` in the configuration file), if it is one of the outputs named there, connected or not. Any other name is answered `unknown_output`, with the names the daemon draws on in the message.
* A colour (`bg_color`) is a string `#rrggbb` of six hexadecimal digits; the `#` may be left out and the digits may be upper or lower case. Answers always write `#rrggbb` in lower case.
* A path (`input`) is absolute. The daemon runs in a working directory of its own, so it refuses a relative path, and `~` is not expanded. `veil set` turns a relative path into an absolute one before it sends it.

### `ping`

No fields. Answered `ok` at once: a daemon is running and its event loop is not stuck.

### `set`

Shows a picture or a video as the wallpaper of one output or of every output.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `input` | string, required | | Absolute path of the file. Whether it is a picture or a video is decided by its content (by its extension only when the content is not recognised); see the README for the formats. |
| `output` | string or `null` | every output | The output to change. |
| `mode` | [`mode`](#mode) | the daemon's `--mode` | How the wallpaper is fitted to the output. |
| `bg_color` | colour | the daemon's `--bg-color` | The colour of what the wallpaper does not cover (bars in `fit`, the area around `center`). |
| `persist` | boolean | `true` | `false`: show the wallpaper until the daemon stops, without saving it, so a restarted daemon shows what was saved before (`veil set --no-persist`). |

What happens:

* The file is checked and decoded off the daemon's event loop, once for all the outputs of the request. One request is loaded at a time; the others wait in order.
* **The latest request wins on each output.** A request takes its outputs from every earlier `set` or `clear` that has not been shown yet. An earlier request that has no output left is answered `superseded` at once and is never shown; one that keeps some outputs (a `set` for every output followed by one for a single output) is shown on the outputs it kept.
* The answer is `ok` once the wallpaper is on every output it is for: a picture when it is drawn, a video when its first frame is on screen, and in both cases not before a video it replaces is gone. An output that is unplugged, or whose video is stopped because the compositor does not show it (`released`), stores the wallpaper for later, and that part is done at once.
* Without `output`, a wallpaper that was shown becomes the default: outputs connected later show it too, unless they have one of their own.
* Unless `persist` is `false`, each output's wallpaper is saved in `$XDG_STATE_HOME/veil/state.json`, and a daemon started without `--input` shows it again.

Errors:

* The file itself (it does not exist, is not a regular file, is empty, cannot be read, is no picture or video veil knows, or cannot be decoded): nothing changes on any output, and the code says why: `not_found`, `unsupported_media`, or `failed` for the others.
* An output where the wallpaper cannot be shown (a video that fails to start there): that output keeps what it showed, the others show the new wallpaper, and the answer is `failed`. When the request was for several outputs, the message names the outputs that failed (`DP-3: <reason>; HDMI-A-1: <reason>`), unless they all failed for the same reason.
* No output to show it on (a daemon without outputs, a `set` without `output`): `failed`.

```sh
echo '{"cmd":"set","input":"/home/me/walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
echo '{"cmd":"set","output":"HDMI-A-1","input":"/home/me/walls/sea.png","mode":"fit","bg_color":"#202020"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
echo '{"cmd":"set","output":"DP-3","input":"/home/me/walls/waves.webm","persist":false}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
echo '{"cmd":"set","input":"/home/me/walls/missing.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"not_found","message":"Failed to resolve path '/home/me/walls/missing.png': No such file or directory (os error 2)"}
echo '{"cmd":"set","input":"/home/me/notes.txt"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"unsupported_media","message":"'/home/me/notes.txt' is neither an image nor a video that veil recognises, by its content or by its extension"}
echo '{"cmd":"set","output":"DP-9","input":"/home/me/walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"unknown_output","message":"output 'DP-9' is not driven by this daemon; it draws on DP-3, HDMI-A-1"}
echo '{"cmd":"set","input":"walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"input must be an absolute path, got 'walls/sea.png'"}
echo '{"cmd":"set","input":"/home/me/walls/sea.png","mode":"zoom"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown variant `zoom`, expected one of `fill`, `fit`, `stretch`, `center`, `tile`"}
```

A `set` that a newer one replaced, sent while a video was still loading:

```sh
echo '{"cmd":"set","input":"/home/me/walls/waves.webm"}' | socat - UNIX-CONNECT:"$sock" &
echo '{"cmd":"set","input":"/home/me/walls/sea.png"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"superseded","message":"replaced by a newer `veil set` before it was shown"}
# {"version":1,"status":"ok"}
```

### `clear`

Shows only a colour, and lets go of the wallpaper's file (`veil clear`). It is a `set` without a file: it goes through the same queue (the latest `set` or `clear` wins on each output), is answered when the colour is on screen, becomes the default without `output`, and is saved unless `persist` is `false`, so a restarted daemon shows the colour too.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `output` | string or `null` | every output | The output to clear. |
| `bg_color` | colour | the daemon's `--bg-color` | The colour to show. |
| `persist` | boolean | `true` | As for `set`. |

```sh
echo '{"cmd":"clear","output":"HDMI-A-1","bg_color":"#102030","persist":false}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### `pause`

Pauses the video wallpaper of one output or of every output until `resume`, whatever is on screen (`veil pause`). Answered `ok` at once.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `output` | string or `null` | every output | The output to pause. It must be one the daemon has a wallpaper for: an output named with the daemon's `--output` that was never connected is answered `unknown_output`. |

A paused video keeps its frame on screen and decodes nothing. The daemon also pauses a video by itself when the compositor hides it (`--pause-on`); the two add up, and a video plays only when neither holds it. A wallpaper set on a paused output starts paused. Pictures have nothing to pause, but are held all the same (`paused_by_user` in `status`). A pause is not saved, and an output connected later is not paused.

```sh
echo '{"cmd":"pause","output":"DP-3"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### `resume`

Ends a `pause` (`veil resume`). Answered `ok` at once. A video that the compositor hides stays paused until it is shown again.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `output` | string or `null` | every output | The output to resume, as for `pause`. |

```sh
echo '{"cmd":"resume"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### `toggle`

`resume` if every output it is for is paused by `pause`, otherwise `pause` (all of them, so a key binding turns a mixed state into paused). Answered at once, with a [`Toggled`](#toggled) result that says which it did.

| Field | Type | Default | Meaning |
|---|---|---|---|
| `output` | string or `null` | every output | The output to toggle, as for `pause`. |

```sh
echo '{"cmd":"toggle"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok","result":{"paused":true}}
echo '{"cmd":"toggle"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok","result":{"paused":false}}
```

### `status`

No fields. Answered at once, with a [`Status`](#status) result: the daemon, its default wallpaper, and every output it draws on, also those that are not connected. `veil status --json` prints this result.

```sh
echo '{"cmd":"status"}' | socat - UNIX-CONNECT:"$sock"
```

The answer, on one line (spread out here):

```json
{
  "version": 1,
  "status": "ok",
  "result": {
    "default": {"bg_color": "#000000", "input": "/home/me/walls/sea.png", "mode": "fill"},
    "outputs": [
      {
        "connected": true,
        "hidden": false,
        "kind": "video",
        "name": "DP-3",
        "next": null,
        "paused_by_user": false,
        "scale": 1.0,
        "size": {"height": 1080, "width": 1920},
        "state": "playing",
        "video": {"dropped": 0, "fps": 30.0, "rendered": 41, "size": {"height": 720, "width": 1280}},
        "wallpaper": {"bg_color": "#000000", "input": "/home/me/walls/waves.webm", "mode": "fill"}
      },
      {
        "connected": true,
        "hidden": false,
        "kind": "image",
        "name": "HDMI-A-1",
        "next": null,
        "paused_by_user": false,
        "scale": 1.0,
        "size": {"height": 1080, "width": 1920},
        "state": "shown",
        "video": null,
        "wallpaper": {"bg_color": "#202020", "input": "/home/me/walls/sea.png", "mode": "fit"}
      }
    ],
    "pause_on": "fullscreen",
    "pid": 4242,
    "uptime_secs": 7385,
    "version": "0.1.0"
  }
}
```

### `outputs`

No fields. Answered at once, with an [`OutputList`](#outputlist) result: the outputs the compositor has announced, whether the daemon draws on them, and then the outputs the daemon waits for. `veil outputs --json` prints this result.

```sh
echo '{"cmd":"outputs"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok","result":{"outputs":[{"connected":true,"description":"Dell Inc. DELL U2720Q 1A2B3C4D","name":"DP-3","wallpaper":true},{"connected":true,"description":"Samsung Electric Company C24F390 HTQH600000","name":"HDMI-A-1","wallpaper":true}]}}
```

### `quit`

No fields. Ends the daemon as `SIGTERM` does: it is answered `ok` first, then the daemon takes down its wallpapers and exits with status 0. The socket is gone once the daemon has ended; `veil quit` waits for that, so `veil quit && veil daemon` works. Under the systemd unit, stop the service instead (`systemctl --user stop veil`).

```sh
echo '{"cmd":"quit"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"ok"}
```

### Requests that are refused

```sh
echo '{"cmd":"explode"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown variant `explode`, expected one of `set`, `clear`, `pause`, `resume`, `toggle`, `status`, `outputs`, `quit`, `ping` at line 1 column 16"}
echo '{"cmd":"status","output":"DP-3"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown field `output`, there are no fields"}
echo '{"cmd":"set","input":"/home/me/walls/sea.png","colour":"red"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: unknown field `colour`, expected one of `output`, `input`, `mode`, `bg_color`, `persist`"}
echo '{"cmd":"clear","bg_color":"red"}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: invalid colour 'red': expected #rrggbb"}
echo '{"cmd":"pause","output":""}' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"output must not be empty"}
echo 'not json' | socat - UNIX-CONNECT:"$sock"
# {"version":1,"status":"error","code":"invalid_request","message":"invalid request: expected ident at line 1 column 2"}
```

## Responses

A response is `ok`, with a `result` for the commands that report something:

```json
{"version":1,"status":"ok","result":{"paused":true}}
```

or an error, with a code for programs and a message for people:

```json
{"version":1,"status":"error","code":"unknown_output","message":"output 'DP-9' is not driven by this daemon; it draws on DP-3, HDMI-A-1"}
```

### Fields

| Field | Type | Meaning |
|---|---|---|
| `version` | integer | The protocol version of the daemon, `1`. See [Compatibility](#compatibility). |
| `status` | `"ok"` or `"error"` | Whether the request was carried out. |
| `code` | string | With `error` only: what kind of error, one of the [error codes](#error-codes). |
| `message` | string | With `error` only: what went wrong, in English, for people. Its wording may change; programs look at `code`. |
| `result` | object | With `ok`, for `toggle` ([`Toggled`](#toggled)), `status` ([`Status`](#status)) and `outputs` ([`OutputList`](#outputlist)); left out for the other commands. |

The order of the keys means nothing (today the keys of a `result` come in alphabetical order).

### Error codes

| Code | Meaning | Sent for |
|---|---|---|
| `invalid_request` | Not valid JSON or UTF-8, not an object, an unknown command or field, a field of the wrong type, or a value that is not accepted (an empty or relative `input`, an empty `output`, an unknown `mode`, a colour that is not `#rrggbb`). | any request |
| `too_large` | The request line is longer than 64 KiB. | any request |
| `timeout` | No complete request arrived within 2 seconds of connecting. | any connection |
| `forbidden` | The client runs as another user. | any connection |
| `busy` | 16 clients are connected already. | any connection |
| `superseded` | Newer `set` or `clear` requests took every output of this one before it was shown. | `set`, `clear` |
| `unknown_output` | `output` names an output the daemon does not draw on; for `pause`, `resume` and `toggle` also one it has no wallpaper for yet, because it was never connected. | `set`, `clear`, `pause`, `resume`, `toggle` |
| `not_found` | The `input` does not exist. | `set` |
| `unsupported_media` | The `input` cannot be a wallpaper: not a regular file, empty, a picture format veil cannot decode (HEIF, JPEG XL), or neither a picture nor a video veil recognises. | `set` |
| `failed` | Anything else: the request was valid but could not be carried out (a file that cannot be read or decoded, a video that fails to start, no output to show it on); `message` says why. | `set`, `clear`, `pause`, `resume`, `toggle`; in principle any request the daemon cannot answer |

## Results

Every field of a result is always present, `null` when there is nothing to say, so a program does not have to tell a missing key from an empty one.

### `Status`

The result of `status`.

| Field | Type | Meaning |
|---|---|---|
| `version` | string | The daemon's version (`veil --version`). |
| `pid` | integer | The daemon's process id. |
| `uptime_secs` | integer | How long the daemon has run, in seconds. |
| `pause_on` | [`pause_on`](#pause_on) | When a video is paused because it is hidden (`--pause-on`). |
| `default` | [`WallpaperInfo`](#wallpaperinfo) or `null` | What an output connected later shows if it has no wallpaper of its own; `null`: the daemon's background colour. |
| `outputs` | array of [`OutputStatus`](#outputstatus) | Every output the daemon draws on, also those that are not connected. |

### `OutputStatus`

One output in a `Status`.

| Field | Type | Meaning |
|---|---|---|
| `name` | string or `null` | The output's name; `null` for an output the compositor gave no name. |
| `connected` | boolean | Whether the output is there. One that is not keeps its wallpaper for when it is back. |
| `wallpaper` | [`WallpaperInfo`](#wallpaperinfo) | What the output shows, or will show once it is connected. |
| `kind` | [`kind`](#kind) | What kind of wallpaper that is. |
| `state` | [`state`](#state) | Where the wallpaper is. |
| `paused_by_user` | boolean | Held by `pause` (until `resume`). |
| `hidden` | boolean | The compositor shows something over the output that pauses a video (`--pause-on`). |
| `size` | [`Size`](#size) or `null` | The size of the wallpaper's surface in logical pixels, while it has one. |
| `scale` | number or `null` | The surface's scale (`1.5` for 150 %), while it has one. Physical pixels are `size` times `scale`. |
| `video` | [`VideoInfo`](#videoinfo) or `null` | The video's stream, while a video is attached. |
| `next` | [`WallpaperInfo`](#wallpaperinfo) or `null` | A video that is starting on this output and replaces the current wallpaper once its first frame is there. A `set` that is still being loaded does not show here. |

### `WallpaperInfo`

A wallpaper: a file, or only a colour.

| Field | Type | Meaning |
|---|---|---|
| `input` | string or `null` | The absolute path of the file; `null` for a colour alone (`clear`). |
| `mode` | [`mode`](#mode) | How it is fitted to the output. |
| `bg_color` | string | The background colour, `#rrggbb`. |

### `VideoInfo`

A video's stream, and what the compositor did with its frames.

| Field | Type | Meaning |
|---|---|---|
| `size` | [`Size`](#size) or `null` | The size of the decoded frames, once known. |
| `fps` | number or `null` | Frames per second, once known, if the stream has a fixed rate. |
| `rendered` | integer or `null` | Frames the compositor took, counted by the video sink. The count starts again at every loop and every pause. |
| `dropped` | integer or `null` | Frames the compositor did not take in time, counted the same way. Many dropped and none rendered means the compositor does not show the video (a locked screen, a monitor that is off); the daemon then stops it (`released`). |

### `Size`

| Field | Type | Meaning |
|---|---|---|
| `width` | integer | In pixels. |
| `height` | integer | In pixels. |

### `OutputList`

The result of `outputs`.

| Field | Type | Meaning |
|---|---|---|
| `outputs` | array of [`OutputInfo`](#outputinfo) | The outputs the compositor has announced, then those the daemon waits for (named with `--output`, or unplugged with their wallpaper kept). |

### `OutputInfo`

| Field | Type | Meaning |
|---|---|---|
| `name` | string or `null` | `null` for an output the compositor gave no name. |
| `description` | string or `null` | What the compositor says the output is (make and model), if anything. |
| `connected` | boolean | Whether the output is there. |
| `wallpaper` | boolean | Whether the daemon draws a wallpaper on it: every output, or with `--output` the ones named. |

### `Toggled`

The result of `toggle`.

| Field | Type | Meaning |
|---|---|---|
| `paused` | boolean | `true`: the outputs are now held by `pause`; `false`: they were resumed. |

## Values

### `state`

Where the wallpaper of an output is (`OutputStatus.state`).

| Value | Meaning |
|---|---|
| `disconnected` | The output is not connected. |
| `starting` | The wallpaper is about to be shown: the surface is new, or a video waits for its first frame. |
| `shown` | A picture or a colour is on screen. |
| `playing` | A video plays. |
| `paused` | A video is paused (`paused_by_user`, `hidden`), its frame on screen. |
| `released` | A video was stopped because the compositor does not show it (a locked screen, a monitor that is off); it starts again from its beginning when it is shown again. |
| `stopped` | Nothing is shown after an error (the daemon's log says which); the next change of the output, or the next `set`, tries again. |

### `kind`

What an output shows (`OutputStatus.kind`).

| Value | Meaning |
|---|---|
| `color` | Only the background colour. |
| `image` | A picture. |
| `video` | A video (an animated GIF is one too). |

### `mode`

How a wallpaper is fitted to its output (`set`, `WallpaperInfo.mode`).

| Value | Meaning |
|---|---|
| `fill` | Covers the whole output, keeping the aspect ratio; what does not fit is cut off. The daemon's default. |
| `fit` | Shows the whole wallpaper, keeping the aspect ratio; bars of the background colour fill the rest. |
| `stretch` | Covers the whole output, distorted to its aspect ratio. |
| `center` | One pixel of the file per physical pixel, centred; cut off or surrounded by the background colour. |
| `tile` | One pixel per physical pixel, repeated over the output. For a video it acts as `center`. |

### `pause_on`

When the daemon pauses a video because the compositor hides it (`Status.pause_on`, the daemon's `--pause-on`). Needs a compositor whose IPC veil follows (Hyprland, sway); elsewhere videos always play.

| Value | Meaning |
|---|---|
| `never` | Never. |
| `fullscreen` | A fullscreen window is on the output. The default. |
| `covered` | Also when windows cover the whole output, translucent ones included. |

## Compatibility

* New commands, fields, result fields, error codes and values are added without changing `version`. Fields are never renamed or removed. `version` is raised only for a change that a peer of the previous version would misread.
* A client should ignore result fields it does not know, treat an error code it does not know like `failed`, and a `state` or `kind` it does not know as unknown. `veil` itself does so.
* A daemon older than a command answers it `invalid_request`, with `unknown variant` in the message; one older than a field, with `unknown field`. `veil` then says that the daemon is older than the client and should be restarted.
* A response without `version` comes from a daemon older than the field.

## The `veil` commands

The commands of `veil` are the clients above, plus a few that need no daemon. Every option is described in `veil <command> --help` and in the man pages (`man veil`, `man veil-set`, ...), which the binary generates from the same definitions.

### Commands

| Command | Request | Prints on stdout |
|---|---|---|
| `veil daemon` | none: it is the daemon | nothing; its log goes to stderr |
| `veil set --input FILE [--output NAME] [--mode MODE] [--bg-color COLOR] [--no-persist]` | `set`; a relative `FILE` is made absolute first, `--no-persist` sends `"persist":false` | nothing |
| `veil clear [--output NAME] [--bg-color COLOR] [--no-persist]` | `clear` | nothing |
| `veil pause [--output NAME]` | `pause` | nothing |
| `veil resume [--output NAME]` | `resume` | nothing |
| `veil toggle [--output NAME]` | `toggle` | `paused` or `playing` |
| `veil status [--json]` | `status` | a line per output; with `--json` the `result` on one line |
| `veil outputs [--json]` | `outputs` | a line per output; with `--json` the `result` on one line |
| `veil quit`, `veil stop` | `quit`, then it waits (up to 30 s) until the daemon has ended | nothing |
| `veil ping` | `ping` | `pong` |
| `veil config check [--config FILE]` | none: reads the configuration file as the daemon would | what the daemon would start with |
| `veil completions SHELL` | none | the completion script for `bash`, `zsh`, `fish` or `nushell` |

`veil status` without `--json`:

```text
veil 0.1.0 (pid 4242), running for 2h 03m; videos pause on: fullscreen
default: /home/me/walls/sea.png (fill, #000000)
DP-3: playing /home/me/walls/waves.webm (fill, #000000); 1920x1080 at scale 1; video 1280x720, 30 fps, 41 frames shown, 0 dropped
HDMI-A-1: showing /home/me/walls/sea.png (fit, #202020); 1920x1080 at scale 1
```

The text is for people and may change; scripts use `--json`, whose fields are described under [Results](#results). For example, with `jq`:

```sh
veil status --json | jq -r '.outputs[] | "\(.name): \(.state)"'
```

A command that fails prints its error on stderr. `veil set`, `clear`, `pause`, `resume`, `toggle` and `quit` also raise a desktop notification when they fail, since they are often run from a key binding where nobody sees stderr; `status`, `outputs` and `ping` never do (a status bar may ask every second while no daemon runs), and neither does a `set` or `clear` that was `superseded` (the newer one is on screen).

### Exit statuses

These are stable: scripts may rely on them.

| Status | Meaning |
|---|---|
| `0` | Success. For the daemon: it was stopped with `SIGINT`, `SIGTERM` or `quit`. |
| `1` | Any other error, among them every error the daemon answers (`superseded` included). The message is on stderr. |
| `2` | The command line could not be parsed, or the configuration file cannot be read or is wrong (`veil daemon`, `veil config check`). |
| `3` | A command for a running daemon: no daemon is running for this compositor (no socket, or a leftover one nobody listens on). |
| `4` | `veil daemon`: a daemon is already running for this compositor. Nothing was changed. |
| `5` | `veil daemon`: the compositor went away while the daemon ran. |

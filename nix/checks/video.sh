#!/usr/bin/env bash
# A video plays with the package's wrapper in an empty environment: the
# daemon and its client get nothing but what a desktop session gives every
# program (HOME, XDG_RUNTIME_DIR, WAYLAND_DISPLAY), so the GStreamer plugins,
# their registry and everything else must come from the wrapper itself.
#
# Usage: video.sh VEIL, where VEIL is the package's bin/veil. Run by the
# flake's check `video` (`nix flake check`); it needs sway, gst-launch-1.0
# and jq on PATH, and no GPU: sway renders on the CPU, and the clip (VP8) is
# decoded in software when there is no hardware decoder.
set -euo pipefail

veil=${1:?usage: video.sh VEIL}
work=$(mktemp -d)
run=$work/run
mkdir -m 700 "$run" "$work/home"
sway_pid=
daemon_pid=
cleanup() {
    [ -n "$daemon_pid" ] && kill "$daemon_pid" 2>/dev/null
    [ -n "$sway_pid" ] && kill "$sway_pid" 2>/dev/null
    wait 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT
fail() {
    echo "video check: $*" >&2
    for log in "$work"/*.log; do
        echo "--- $log" >&2
        tail -n 30 "$log" >&2
    done
    exit 1
}

# The clip, made with the package's own plugin set: the wrapper's script with
# gst-launch-1.0 in place of the binary.
sed 's|^exec .*|exec gst-launch-1.0 "$@"|' "$veil" > "$work/launch"
grep -q '^exec gst-launch-1.0' "$work/launch" || fail "the wrapper does not end in exec"
chmod +x "$work/launch"
clip=$work/ball.webm
env -i HOME="$work/home" PATH="$PATH" "$work/launch" -q \
    videotestsrc pattern=ball num-buffers=90 \
    ! video/x-raw,width=320,height=180,framerate=30/1 \
    ! vp8enc ! webmmux ! filesink location="$clip" > "$work/launch.log" 2>&1 \
    || fail "cannot make the clip"

# A headless sway with one output.
printf 'xwayland disable\noutput HEADLESS-1 resolution 640x360\n' > "$work/sway.conf"
env -i HOME="$work/home" PATH="$PATH" XDG_RUNTIME_DIR="$run" \
    WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1 \
    WLR_LIBINPUT_NO_DEVICES=1 LIBSEAT_BACKEND=noop \
    sway -c "$work/sway.conf" > "$work/sway.log" 2>&1 &
sway_pid=$!
display=
for _ in $(seq 100); do
    for socket in "$run"/wayland-[0-9]; do
        [ -S "$socket" ] && display=${socket##*/} && break
    done
    [ -n "$display" ] && break
    kill -0 "$sway_pid" 2>/dev/null || fail "sway exited"
    sleep 0.1
done
[ -n "$display" ] || fail "sway does not listen"

# The daemon and the client, with an empty environment but the session's.
session() {
    env -i HOME="$work/home" XDG_RUNTIME_DIR="$run" WAYLAND_DISPLAY="$display" "$veil" "$@"
}
session daemon --input "$clip" > "$work/daemon.log" 2>&1 &
daemon_pid=$!

# Playing, with frames the compositor took.
status=
for _ in $(seq 300); do
    kill -0 "$daemon_pid" 2>/dev/null || fail "the daemon exited"
    if status=$(session status --json 2>/dev/null) \
        && jq -e '.outputs[0] | .state == "playing" and .video.rendered >= 10' \
            <<< "$status" > /dev/null; then
        break
    fi
    status=
    sleep 0.1
done
[ -n "$status" ] || fail "no video played: $(session status --json 2>&1 || true)"
test -s "$work/home/.cache/veil/gstreamer-registry.bin" \
    || fail "the wrapper's registry is not in ~/.cache/veil"

session quit > "$work/quit.log" 2>&1 || fail "veil quit failed"
wait "$daemon_pid" || fail "the daemon exited with $?"
daemon_pid=
echo "video check: $(jq -c '.outputs[0] | {state, video}' <<< "$status")"

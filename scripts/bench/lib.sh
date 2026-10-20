# What bench.sh and soak.sh share: a daemon with directories of its own, the
# headless sway it runs in, and how its process is sampled. Sourced, not run.
#
# The caller sets `root` (the repository), `veil` (the binary) and `out` (the
# directory for logs and results) before it calls anything here, and leaves
# the cleanup to the EXIT trap that `bench_isolate` sets.

# Numbers with a decimal point, whatever the user's locale (bash's printf).
export LC_NUMERIC=C

sway_pid=
daemon_pid=
work=
compositor=

bench_cleanup() {
    if [ -n "$daemon_pid" ] && kill -0 "$daemon_pid" 2>/dev/null; then
        kill -TERM "$daemon_pid" 2>/dev/null || true
        wait "$daemon_pid" 2>/dev/null || true
    fi
    if [ -n "$sway_pid" ]; then
        kill "$sway_pid" 2>/dev/null || true
        wait "$sway_pid" 2>/dev/null || true
    fi
    [ -z "$work" ] || rm -rf "$work"
}

# The daemon's own directories: never the saved wallpapers or the
# configuration file of the user who runs this, and no desktop notifications.
bench_isolate() {
    work=$(mktemp -d /tmp/veil-bench.XXXXXX)
    trap bench_cleanup EXIT
    export XDG_STATE_HOME=$work/state XDG_CONFIG_HOME=$work/config
    export DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/veil-bench-bus
    # A GStreamer registry of veil's own (a stale shared one can hide plugins).
    if [ -z "${GST_REGISTRY-}${GST_REGISTRY_1_0-}" ]; then
        export GST_REGISTRY=${XDG_CACHE_HOME:-$HOME/.cache}/veil/bench-gst-registry.bin
        mkdir -p "$(dirname "$GST_REGISTRY")"
    fi
    unset NOTIFY_SOCKET WATCHDOG_USEC
}

bench_require() {
    local tool
    for tool in "$@"; do
        command -v "$tool" >/dev/null || { echo "$tool is missing: run inside nix develop .#headless" >&2; exit 2; }
    done
}

# A headless sway with outputs of the given size side by side (one unless a
# count is given; pixman rendering), in the runtime directory of
# `bench_isolate`. Sets SWAYSOCK and WAYLAND_DISPLAY.
bench_start_sway() {
    local size=$1 count=${2:-1} sock= display= i
    export XDG_RUNTIME_DIR=$work
    unset WAYLAND_DISPLAY WAYLAND_SOCKET SWAYSOCK HYPRLAND_INSTANCE_SIGNATURE DISPLAY
    echo 'xwayland disable' > "$work/sway.conf"
    for i in $(seq "$count"); do
        printf 'output HEADLESS-%d resolution %s position %d 0\n' "$i" "$size" $(((i - 1) * ${size%x*})) \
            >> "$work/sway.conf"
    done
    WLR_BACKENDS=headless WLR_HEADLESS_OUTPUTS=$count WLR_RENDERER=pixman WLR_LIBINPUT_NO_DEVICES=1 \
        LIBSEAT_BACKEND=noop sway -c "$work/sway.conf" > "$out/sway.log" 2>&1 &
    sway_pid=$!
    for _ in $(seq 100); do
        sock=$(ls "$work"/sway-ipc.*.sock 2>/dev/null || true)
        display=$(cd "$work" && ls wayland-? 2>/dev/null | head -n 1 || true)
        [ -n "$sock" ] && [ -n "$display" ] && break
        sleep 0.1
    done
    [ -n "$sock" ] && [ -n "$display" ] || { echo "sway did not start; see $out/sway.log" >&2; exit 1; }
    export SWAYSOCK=$sock WAYLAND_DISPLAY=$display
    if [ "$count" -eq 1 ]; then
        compositor="$(sway --version), headless, pixman, one output of $size"
    else
        compositor="$(sway --version), headless, pixman, $count outputs of $size"
    fi
}

# `veil daemon` with the given arguments, in the background, logging to
# daemon.log. Variables assigned in front of the call reach the daemon.
bench_start_daemon() {
    RUST_LOG=${RUST_LOG:-veil=info} "$veil" daemon "$@" > "$out/daemon.log" 2>&1 &
    daemon_pid=$!
}

bench_status_json() { "$veil" status --json 2>/dev/null; }

# Waits until every connected output shows its image or plays its video.
bench_wait_shown() {
    for _ in $(seq 300); do
        kill -0 "$daemon_pid" 2>/dev/null || { echo "the daemon exited; see $out/daemon.log" >&2; exit 1; }
        if bench_status_json | jq -e '[.outputs[] | select(.connected)] as $o
                | ($o | length) > 0 and all($o[]; .state == "playing" or .state == "shown")' >/dev/null 2>&1; then
            return
        fi
        sleep 0.1
    done
    echo "the wallpaper was not shown within 30 s; see $out/daemon.log" >&2
    exit 1
}

bench_alive() {
    kill -0 "$daemon_pid" 2>/dev/null || { echo "the daemon exited while it was measured; see $out/daemon.log" >&2; exit 1; }
}

# The daemon's CPU time in clock ticks (all threads, user and system).
cpu_ticks() { awk '{ sub(/^.*\) /, ""); print $12 + $13 }' "/proc/$daemon_pid/stat"; }
rss_kib() { awk '/^VmRSS:/ { print $2 }' "/proc/$daemon_pid/status"; }
# "VmRSS,RssAnon,RssShmem": everything resident, the heap and anonymous
# mappings (what the allocator keeps), and shared memory (the wl_shm buffers
# of the surfaces, which come and go with the wallpaper).
memory_kib() {
    awk '/^VmRSS:/ { r = $2 } /^RssAnon:/ { a = $2 } /^RssShmem:/ { s = $2 } END { print r "," a "," s }' \
        "/proc/$daemon_pid/status"
}
thread_count() { ls "/proc/$daemon_pid/task" | wc -l; }
fd_count() { ls "/proc/$daemon_pid/fd" | wc -l; }
# Context switches of the event loop thread (the main thread): how often it
# was woken, or had to wait.
loop_switches() {
    awk '/^(non)?voluntary_ctxt_switches:/ { n += $2 } END { print n }' "/proc/$daemon_pid/task/$daemon_pid/status"
}
now_ns() { date +%s%N; }

# The commit the binary was built from, as far as the working tree tells.
bench_commit() {
    local commit
    commit=$(git -C "$root" rev-parse --short HEAD 2>/dev/null || echo unknown)
    git -C "$root" diff --quiet HEAD 2>/dev/null || commit="$commit (with uncommitted changes)"
    echo "$commit"
}

bench_machine() {
    local gpu_name
    gpu_name=$(nvidia-smi --query-gpu=name --format=csv,noheader 2>/dev/null | head -n 1 || true)
    [ -n "$gpu_name" ] || gpu_name=$(lspci 2>/dev/null | awk -F': ' '/VGA|3D/ { print $2; exit }' || true)
    echo "CPU:         $(awk -F': ' '/^model name/ { print $2; exit }' /proc/cpuinfo), $(nproc) threads"
    echo "GPU:         ${gpu_name:-unknown}"
    echo "kernel:      $(uname -r), glibc $(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{ print $2 }')"
}

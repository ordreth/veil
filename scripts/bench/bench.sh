#!/usr/bin/env bash
# Measures what the veil daemon costs while it shows one file: CPU, memory,
# threads, file descriptors, the video sink's frame counts and, on NVIDIA, the
# GPU. One sample per interval goes to samples.csv; summary.sh turns them into
# report.txt, which is also printed.
#
#   scripts/bench/bench.sh [options] FILE
#
# By default the daemon runs in a headless sway of its own (pixman rendering,
# one output), which leaves the desktop alone and runs anywhere, also in CI.
# With --session it runs in the session this script is started from instead:
# stop your own veil daemon first (a second one refuses to start), and expect
# the wallpaper of that session to change while it runs.
#
# Run it inside `nix develop .#headless` (sway, grim, jq, the GStreamer
# plugins). `docs/measurements.md` explains the columns and has the reference
# numbers.

set -euo pipefail

usage() {
    cat <<'EOF'
usage: scripts/bench/bench.sh [options] FILE

  --duration SECONDS   how long to sample, after the warm-up (default 60)
  --warmup SECONDS     how long to wait after the wallpaper is shown (default 5)
  --interval SECONDS   time between samples, a whole number (default 1)
  --size WIDTHxHEIGHT  the headless output (default 1920x1080)
  --outputs N          how many headless outputs, side by side; the file
                       plays on each of them (default 1)
  --mode MODE          veil's --mode (default fill)
  --output NAME        with --session: draw on this output only
  --session            run in the current Wayland session, not a headless sway
  --veil PATH          the veil binary (default target/release/veil)
  --out DIR            where samples.csv and report.txt go
                       (default target/bench/<date>-<file name>)
EOF
}

duration=60
warmup=5
interval=1
size=1920x1080
outputs=1
mode=fill
output=
session=false
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
veil=$root/target/release/veil
out=
file=

while [ $# -gt 0 ]; do
    case "$1" in
        --duration) duration=$2; shift 2 ;;
        --warmup) warmup=$2; shift 2 ;;
        --interval) interval=$2; shift 2 ;;
        --size) size=$2; shift 2 ;;
        --outputs) outputs=$2; shift 2 ;;
        --mode) mode=$2; shift 2 ;;
        --output) output=$2; shift 2 ;;
        --session) session=true; shift ;;
        --veil) veil=$2; shift 2 ;;
        --out) out=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        -*) echo "unknown option $1" >&2; usage >&2; exit 2 ;;
        *) [ -z "$file" ] || { usage >&2; exit 2; }; file=$1; shift ;;
    esac
done
[ -n "$file" ] || { usage >&2; exit 2; }
[ -f "$file" ] || { echo "no such file: $file" >&2; exit 2; }
file=$(cd "$(dirname "$file")" && pwd)/$(basename "$file")
[ -x "$veil" ] || { echo "no veil binary at $veil (cargo build --release, or --veil PATH)" >&2; exit 2; }
# shellcheck source=lib.sh
. "$here/lib.sh"
bench_require jq getconf
$session || bench_require sway swaymsg
case "$interval" in ''|*[!0-9]*|0) echo "--interval takes a whole number of seconds" >&2; exit 2 ;; esac
case "$outputs" in ''|*[!0-9]*|0) echo "--outputs takes a whole number above 0" >&2; exit 2 ;; esac

[ -n "$out" ] || out=$root/target/bench/$(date +%Y%m%d-%H%M%S)-$(basename "$file")
mkdir -p "$out"
out=$(cd "$out" && pwd)

bench_isolate

if $session; then
    [ -n "${WAYLAND_DISPLAY-}" ] || { echo "--session needs WAYLAND_DISPLAY" >&2; exit 2; }
    if [ -n "${HYPRLAND_INSTANCE_SIGNATURE-}" ]; then
        compositor="Hyprland ($(hyprctl version -j 2>/dev/null | jq -r .tag 2>/dev/null || echo 'version unknown')), this session"
    elif [ -n "${SWAYSOCK-}" ]; then
        compositor="$(swaymsg -t get_version -r 2>/dev/null | jq -r .human_readable 2>/dev/null || echo sway), this session"
    else
        compositor="this session ($WAYLAND_DISPLAY)"
    fi
else
    bench_start_sway "$size" "$outputs"
fi

args=(--input "$file" --mode "$mode")
[ -n "$output" ] && args+=(--output "$output")
# GStreamer names every element it creates in this log: how the decoder is
# found (the one the video ends up with is the last decoder created).
GST_DEBUG=GST_ELEMENT_FACTORY:4 GST_DEBUG_NO_COLOR=1 GST_DEBUG_FILE="$out/gst-elements.log" \
    bench_start_daemon "${args[@]}"
bench_wait_shown
sleep "$warmup"

ticks_per_second=$(getconf CLK_TCK)
gpu() {
    # System-wide: every process on the GPU counts, not only the daemon.
    if command -v nvidia-smi >/dev/null 2>&1; then
        nvidia-smi --query-gpu=utilization.gpu,utilization.decoder --format=csv,noheader,nounits 2>/dev/null \
            | head -n 1 | tr -d ' '
    else
        echo ","
    fi
}

echo "t_s,cpu_pct,rss_kib,threads,fds,sink_rendered,sink_dropped,gpu_pct,gpu_decoder_pct,wakeups_per_s" > "$out/samples.csv"
start=$(now_ns)
last_ns=$start
last_ticks=$(cpu_ticks)
last_switches=$(loop_switches)
samples=$((duration / interval))
echo "sampling every ${interval} s for ${duration} s into $out/samples.csv" >&2
for _ in $(seq "$samples"); do
    sleep "$interval"
    bench_alive
    ns=$(now_ns)
    ticks=$(cpu_ticks)
    switches=$(loop_switches)
    # Ticks per elapsed time, as a percentage of one core.
    cpu=$(awk -v t=$((ticks - last_ticks)) -v hz="$ticks_per_second" -v ns=$((ns - last_ns)) \
        'BEGIN { printf "%.2f", t / hz / (ns / 1e9) * 100 }')
    wakeups=$(awk -v s=$((switches - last_switches)) -v ns=$((ns - last_ns)) 'BEGIN { printf "%.1f", s / (ns / 1e9) }')
    frames=$(bench_status_json | jq -r '[.outputs[] | .video // empty] |
        "\(map(.rendered // 0) | add // 0),\(map(.dropped // 0) | add // 0)"')
    t=$(awk -v ns=$((ns - start)) 'BEGIN { printf "%.1f", ns / 1e9 }')
    echo "$t,$cpu,$(rss_kib),$(thread_count),$(fd_count),$frames,$(gpu),$wakeups" \
        >> "$out/samples.csv"
    last_ns=$ns
    last_ticks=$ticks
    last_switches=$switches
done

stream=$(bench_status_json | jq -r '[.outputs[] | select(.video != null)][0].video // empty
    | "\(.size.width // "?")x\(.size.height // "?") at \(.fps // "?") fps"')
codec=
if [ -n "$stream" ] && command -v ffprobe >/dev/null 2>&1; then
    codec=$(ffprobe -v error -select_streams v:0 -show_entries stream=codec_name,profile,pix_fmt \
        -of default=noprint_wrappers=1:nokey=1 "$file" 2>/dev/null | paste -sd ' ' || true)
fi
decoders=$(grep -oE 'creating element "[^"]+"' "$out/gst-elements.log" 2>/dev/null \
    | sed 's/.*"\(.*\)"/\1/' | grep 'dec' | grep -v 'decodebin' | sort -u | paste -sd ' ' || true)
commit=$(bench_commit)

{
    echo "veil benchmark, $(date -Iseconds)"
    echo
    echo "file:        $file ($(du -h "$file" | cut -f1))"
    echo "stream:      ${stream:-not a video} ${codec:+($codec)}"
    echo "decoder:     ${decoders:-none (an image)}"
    echo "mode:        $mode"
    echo "veil:        $("$veil" --version) at commit $commit, $(head -n 1 "$out/daemon.log" | sed 's/.*(\(GStreamer [^)]*\)).*/\1/')"
    echo "binary:      $veil"
    echo "compositor:  $compositor"
    if [ -n "${MALLOC_ARENA_MAX-}" ]; then
        echo "allocator:   MALLOC_ARENA_MAX=$MALLOC_ARENA_MAX"
    else
        echo "allocator:   glibc's default (MALLOC_ARENA_MAX unset)"
    fi
    bench_machine
    echo "warm-up:     ${warmup} s, then ${samples} samples ${interval} s apart"
    echo
    "$here/summary.sh" "$out/samples.csv"
} > "$out/report.txt"
cat "$out/report.txt"

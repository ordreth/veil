#!/usr/bin/env bash
# Soak tests of the veil daemon, in a headless sway of its
# own, like bench.sh:
#
#   scripts/bench/soak.sh loop [options]     one short clip that loops
#   scripts/bench/soak.sh switch [options]   `veil set` again and again,
#                                            images and videos in turn
#
# Each run samples the daemon's memory (RSS and its heap, RssAnon), threads
# and file descriptors, and fails (exit status 1) when, over the stable
# window (after the start-up), the heap rises faster than the threshold (by
# the bounds of its samples; trend.sh says why not by a least-squares line),
# when threads or descriptors rise, when a `veil set` fails, or when the
# video stops playing. The results go to target/bench/soak-<test>-<date>/ (--out to choose):
# samples.csv, report.txt (also printed), graph.svg and the logs.
#
# Run it inside `nix develop .#headless`, with a release build. The media are
# made with ffmpeg on the first run (target/bench/soak-media). The daemon
# gets MALLOC_ARENA_MAX=1 unless --arenas says otherwise: that is what the
# Nix package and the systemd unit start it with (docs/known-issues.md,
# entry 3), and target/release/veil has none of it by itself.

set -euo pipefail

usage() {
    cat <<'EOF'
usage: scripts/bench/soak.sh loop|switch [options]

loop:
  --duration SECONDS     how long to sample (default 600)
  --interval SECONDS     time between samples, a whole number (default 1)
  --clip FILE            the clip (default a 1 s H.264 clip, made once)
  --max-rise KIB         allowed rise of the heap, in KiB per minute
                         (default 8)
switch:
  --switches N           how many `veil set` (default 500)
  --idle SECONDS         how long the last image is left alone before the
                         last sample, for GStreamer's idle threads (default 20)
  --max-rise KIB         allowed rise of the heap with an image on
                         screen, in KiB per switch (default 2)
both:
  --settle X             where the stable window starts: seconds (loop) or
                         switches (switch); default a fifth of the run
  --arenas N|default     MALLOC_ARENA_MAX of the daemon (default 1);
                         `default` leaves it to glibc (8 per CPU thread)
  --mmap-threshold BYTES MALLOC_MMAP_THRESHOLD_ of the daemon (default unset)
  --size WIDTHxHEIGHT    the headless output (default 1920x1080)
  --veil PATH            the veil binary (default target/release/veil)
  --media DIR            where the media are made (default target/bench/soak-media)
  --out DIR              where the results go
EOF
}

[ $# -ge 1 ] || { usage >&2; exit 2; }
test_name=$1
shift
case "$test_name" in
    loop|switch) ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
esac

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
veil=$root/target/release/veil
media=$root/target/bench/soak-media
duration=600
interval=1
switches=500
idle=20
settle=
max_rise=
arenas=1
mmap_threshold=
size=1920x1080
clip=
out=

while [ $# -gt 0 ]; do
    case "$1" in
        --duration) duration=$2; shift 2 ;;
        --interval) interval=$2; shift 2 ;;
        --clip) clip=$2; shift 2 ;;
        --switches) switches=$2; shift 2 ;;
        --idle) idle=$2; shift 2 ;;
        --settle) settle=$2; shift 2 ;;
        --max-rise) max_rise=$2; shift 2 ;;
        --arenas) arenas=$2; shift 2 ;;
        --mmap-threshold) mmap_threshold=$2; shift 2 ;;
        --size) size=$2; shift 2 ;;
        --veil) veil=$2; shift 2 ;;
        --media) media=$2; shift 2 ;;
        --out) out=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "unknown option $1" >&2; usage >&2; exit 2 ;;
    esac
done
for n in "$duration" "$interval" "$switches" "$idle"; do
    case "$n" in ''|*[!0-9]*) echo "durations and counts are whole numbers" >&2; exit 2 ;; esac
done
[ "$interval" -gt 0 ] || { echo "--interval must be above 0" >&2; exit 2; }
case "$arenas" in default|[1-9]|[1-9][0-9]*) ;; *) echo "--arenas takes a number or default" >&2; exit 2 ;; esac
[ -x "$veil" ] || { echo "no veil binary at $veil (cargo build --release, or --veil PATH)" >&2; exit 2; }

# shellcheck source=lib.sh
. "$here/lib.sh"
bench_require sway jq ffmpeg getconf

[ -n "$out" ] || out=$root/target/bench/soak-$test_name-$(date +%Y%m%d-%H%M%S)
mkdir -p "$out" "$media"
out=$(cd "$out" && pwd)
media=$(cd "$media" && pwd)

# The media: testsrc2 changes every frame, so a video is never a still.
make_media() {
    local name=$1
    shift
    [ -s "$media/$name" ] && return
    echo "making $media/$name" >&2
    ffmpeg -hide_banner -loglevel error -y "$@" -an "$media/.part-$name"
    mv "$media/.part-$name" "$media/$name"
}
if [ "$test_name" = loop ]; then
    if [ -z "$clip" ]; then
        # One second, so the video ends and is sought back once a second.
        make_media loop-1s.mp4 -f lavfi -i testsrc2=size=1920x1080:rate=30:duration=1 \
            -c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p
        clip=$media/loop-1s.mp4
    fi
    [ -f "$clip" ] || { echo "no such file: $clip" >&2; exit 2; }
    clip=$(cd "$(dirname "$clip")" && pwd)/$(basename "$clip")
else
    make_media image-a.png -f lavfi -i testsrc2=size=1920x1080:rate=1 -frames:v 1
    make_media image-b.png -f lavfi -i smptehdbars=size=1920x1080:rate=1 -frames:v 1
    # Two codecs and two sizes, as in the memory measurements of docs/known-issues.md (entry 3).
    make_media video-a.mp4 -f lavfi -i testsrc2=size=1280x960:rate=30:duration=3 \
        -c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p
    make_media video-b.webm -f lavfi -i testsrc2=size=2560x1080:rate=30:duration=3 \
        -c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 2M -pix_fmt yuv420p
fi

bench_isolate
bench_start_sway "$size"

# The allocator settings reach the daemon only, not this script's tools.
unset MALLOC_ARENA_MAX MALLOC_MMAP_THRESHOLD_
daemon_env=()
[ "$arenas" = default ] || daemon_env+=("MALLOC_ARENA_MAX=$arenas")
[ -z "$mmap_threshold" ] || daemon_env+=("MALLOC_MMAP_THRESHOLD_=$mmap_threshold")
allocator=$( [ ${#daemon_env[@]} -gt 0 ] && echo "${daemon_env[*]}" || echo "glibc's default (MALLOC_ARENA_MAX unset)")
start_daemon() {
    if [ ${#daemon_env[@]} -gt 0 ]; then
        export "${daemon_env[@]}"
    fi
    bench_start_daemon "$@"
    unset MALLOC_ARENA_MAX MALLOC_MMAP_THRESHOLD_
}

ticks_per_second=$(getconf CLK_TCK)
failures=()
fail() { failures+=("$1"); echo "FAIL: $1" >&2; }
# Whether a measured slope is within a threshold (awk compares as numbers).
within() { awk -v v="$1" -v max="$2" 'BEGIN { exit !(v <= max) }'; }

if [ "$test_name" = loop ]; then
    [ -n "$settle" ] || settle=$((duration / 5))
    [ -n "$max_rise" ] || max_rise=8
    # The bus thread logs every loop with the frames of the run that ended:
    # the sink's own counters start again at every loop, so a clip of one
    # second cannot be counted by sampling them once a second.
    RUST_LOG=veil=info,veil::engine::video=debug start_daemon --input "$clip" --mode fill
    bench_wait_shown
    loop_pattern='the video ended after [0-9]* frames'
    played() {
        grep -o "$loop_pattern" "$out/daemon.log" | awk '{ n++; f += $5 } END { printf "%d,%d", n, f }'
    }

    echo "t_s,cpu_pct,rss_kib,anon_kib,shmem_kib,threads,fds,loops,frames" > "$out/samples.csv"
    start=$(now_ns)
    last_ns=$start
    last_ticks=$(cpu_ticks)
    samples=$((duration / interval))
    echo "sampling every ${interval} s for ${duration} s into $out/samples.csv" >&2
    for _ in $(seq "$samples"); do
        sleep "$interval"
        bench_alive
        ns=$(now_ns)
        ticks=$(cpu_ticks)
        cpu=$(awk -v t=$((ticks - last_ticks)) -v hz="$ticks_per_second" -v ns=$((ns - last_ns)) \
            'BEGIN { printf "%.2f", t / hz / (ns / 1e9) * 100 }')
        t=$(awk -v ns=$((ns - start)) 'BEGIN { printf "%.1f", ns / 1e9 }')
        echo "$t,$cpu,$(memory_kib),$(thread_count),$(fd_count),$(played)" >> "$out/samples.csv"
        last_ns=$ns
        last_ticks=$ticks
    done
    fps=$(bench_status_json | jq -r '[.outputs[] | select(.video != null)][0].video.fps // empty')

    # Slopes per minute: the x axis is in seconds.
    # The test is on the heap (anon_kib): what the allocator keeps. The shared
    # memory of the surface's buffer is the same at every loop.
    read -r anon_slope anon_rise _ rss_n _ _ anon_min anon_max \
        < <("$here/trend.sh" "$out/samples.csv" t_s anon_kib "$settle")
    read -r rss_slope _ _ _ _ _ rss_min rss_max < <("$here/trend.sh" "$out/samples.csv" t_s rss_kib "$settle")
    read -r thr_slope _ _ _ _ _ thr_min thr_max < <("$here/trend.sh" "$out/samples.csv" t_s threads "$settle")
    read -r fd_slope _ _ _ _ _ fd_min fd_max < <("$here/trend.sh" "$out/samples.csv" t_s fds "$settle")
    rss_per_min=$(awk -v s="$rss_slope" 'BEGIN { printf "%.1f", s * 60 }')
    anon_per_min=$(awk -v s="$anon_slope" 'BEGIN { printf "%.1f", s * 60 }')
    rise_per_min=$(awk -v s="$anon_rise" 'BEGIN { printf "%.1f", s * 60 }')
    thr_per_min=$(awk -v s="$thr_slope" 'BEGIN { printf "%.3f", s * 60 }')
    fd_per_min=$(awk -v s="$fd_slope" 'BEGIN { printf "%.3f", s * 60 }')
    # Loops and frames in the window, from the counts of the log.
    read -r loops shown_per_s cpu_mean < <(awk -F, -v from="$settle" '
        NR == 1 { next }
        { n++; c += $2; t1 = $1; l1 = $8; f1 = $9 }
        $1 + 0 >= from + 0 && !have { have = 1; t0 = $1; l0 = $8; f0 = $9 }
        END { printf "%d %.1f %.2f\n", l1 - l0, (t1 > t0) ? (f1 - f0) / (t1 - t0) : 0, (n > 0) ? c / n : 0 }' "$out/samples.csv")

    within "$rise_per_min" "$max_rise" || fail "the heap rises by $rise_per_min KiB per minute (allowed: $max_rise)"
    within "$thr_per_min" 0.05 || fail "threads rise by $thr_per_min per minute"
    within "$fd_per_min" 0.05 || fail "file descriptors rise by $fd_per_min per minute"
    if [ -n "$fps" ]; then
        awk -v s="$shown_per_s" -v f="$fps" 'BEGIN { exit !(s >= 0.9 * f) }' \
            || fail "the video showed $shown_per_s frames per second of $fps"
    else
        fail "no video was playing at the end"
    fi
    [ "$loops" -gt 0 ] || fail "the video did not loop"

    {
        echo "veil soak test: a looping video, $(date -Iseconds)"
        echo
        echo "clip:        $clip (${fps:-?} fps)"
        echo "veil:        $("$veil" --version) at commit $(bench_commit)"
        echo "binary:      $veil"
        echo "compositor:  $compositor"
        echo "allocator:   $allocator"
        bench_machine
        echo "run:         $samples samples ${interval} s apart; stable window from ${settle} s ($rss_n samples)"
        echo
        printf 'memory:      RSS %.1f MiB at the first sample, %.1f MiB at the last; %.1f to %.1f MiB in the window\n' \
            "$(awk -F, 'NR == 2 { print $3 / 1024 }' "$out/samples.csv")" "$(tail -n 1 "$out/samples.csv" | awk -F, '{ print $3 / 1024 }')" \
            "$(awk -v v="$rss_min" 'BEGIN { print v / 1024 }')" "$(awk -v v="$rss_max" 'BEGIN { print v / 1024 }')"
        printf 'heap:        %.1f to %.1f MiB in the window (RssAnon)\n' \
            "$(awk -v v="$anon_min" 'BEGIN { print v / 1024 }')" "$(awk -v v="$anon_max" 'BEGIN { print v / 1024 }')"
        printf 'heap rise:   %+.1f KiB per minute (allowed: %s), %+.1f MiB per day at that rate\n' \
            "$rise_per_min" "$max_rise" "$(awk -v s="$rise_per_min" 'BEGIN { print s * 1440 / 1024 }')"
        printf '             least squares: heap %+.1f, RSS %+.1f KiB per minute\n' "$anon_per_min" "$rss_per_min"
        echo "threads:     $thr_min to $thr_max in the window, slope $thr_per_min per minute"
        echo "descriptors: $fd_min to $fd_max in the window, slope $fd_per_min per minute"
        echo "playback:    $loops loops in the window, $shown_per_s frames per second shown, CPU mean $cpu_mean % of one core"
    } > "$out/report.txt"
    "$here/plot.sh" --title "veil soak: looping video, $allocator" --from "$settle" \
        --label "rss_kib=RSS (KiB)" --label "anon_kib=heap, RssAnon (KiB)" \
        --label "threads=threads" --label "fds=file descriptors" \
        "$out/samples.csv" t_s rss_kib anon_kib threads fds > "$out/graph.svg"
else
    [ -n "$settle" ] || settle=$((switches / 5))
    [ -n "$max_rise" ] || max_rise=2
    sequence=("$media/video-a.mp4" "$media/image-b.png" "$media/video-b.webm" "$media/image-a.png")
    start_daemon --input "$media/image-a.png" --mode fill
    bench_wait_shown

    echo "switch,kind,set_ms,rss_kib,anon_kib,shmem_kib,threads,fds,cpu_ticks" > "$out/samples.csv"
    echo "0,start,0,$(memory_kib),$(thread_count),$(fd_count),$(cpu_ticks)" >> "$out/samples.csv"
    start=$(now_ns)
    set_failures=0
    echo "$switches switches into $out/samples.csv" >&2
    for i in $(seq "$switches"); do
        file=${sequence[$(((i - 1) % 4))]}
        case "$file" in *.png) kind=image ;; *) kind=video ;; esac
        t0=$(now_ns)
        if ! "$veil" set --input "$file" > /dev/null 2>> "$out/set.log"; then
            set_failures=$((set_failures + 1))
            echo "switch $i: veil set --input $file failed" >> "$out/set.log"
        fi
        t1=$(now_ns)
        bench_alive
        echo "$i,$kind,$(((t1 - t0) / 1000000)),$(memory_kib),$(thread_count),$(fd_count),$(cpu_ticks)" >> "$out/samples.csv"
        [ $((i % 50)) -ne 0 ] || echo "  $i switches" >&2
    done
    elapsed_ns=$(($(now_ns) - start))
    # The idle threads of GStreamer's pool leave about 15 s after the last video.
    last_kind=$(tail -n 1 "$out/samples.csv" | cut -d, -f2)
    if [ "$last_kind" = video ]; then
        "$veil" set --input "$media/image-a.png" > /dev/null 2>> "$out/set.log" || set_failures=$((set_failures + 1))
    fi
    sleep "$idle"
    bench_alive
    idle_row="$((switches + 1)),idle,0,$(memory_kib),$(thread_count),$(fd_count),$(cpu_ticks)"

    # The test is on the heap (anon_kib) with an image on screen: no pipeline
    # exists then, so what is left is what the allocator keeps. RSS also
    # counts the wl_shm buffer of the surface (8 MiB at 1920x1080), which is
    # resident or not depending on the moment of the sample.
    read -r anon_slope anon_rise _ rss_n _ _ anon_min anon_max \
        < <("$here/trend.sh" --where kind=image "$out/samples.csv" switch anon_kib "$settle")
    read -r rss_slope _ _ _ _ _ rss_min rss_max \
        < <("$here/trend.sh" --where kind=image "$out/samples.csv" switch rss_kib "$settle")
    read -r vid_slope vid_rise _ _ vid_first vid_last _ vid_max \
        < <("$here/trend.sh" --where kind=video "$out/samples.csv" switch anon_kib "$settle")
    read -r thr_slope _ _ _ _ _ thr_min thr_max \
        < <("$here/trend.sh" --where kind=image "$out/samples.csv" switch threads "$settle")
    read -r fd_slope _ _ _ _ _ fd_min fd_max \
        < <("$here/trend.sh" --where kind=image "$out/samples.csv" switch fds "$settle")
    echo "$idle_row" >> "$out/samples.csv"

    [ "$set_failures" -eq 0 ] || fail "$set_failures of the veil set calls failed (see $out/set.log)"
    within "$anon_rise" "$max_rise" || fail "the heap with an image on screen rises by $anon_rise KiB per switch (allowed: $max_rise)"
    within "$thr_slope" 0.01 || fail "threads rise by $thr_slope per switch"
    within "$fd_slope" 0.01 || fail "file descriptors rise by $fd_slope per switch"

    row() { awk -F, -v k="$1" '$1 == k { print; exit }' "$out/samples.csv"; }
    mib() { echo "$1" | awk -F, '{ printf "RSS %.1f MiB, heap %.1f MiB, %d threads, %d descriptors", $4 / 1024, $5 / 1024, $7, $8 }'; }
    latency() {
        awk -F, -v k="$1" '$2 == k { n++; v[n] = $3; s += $3 }
            END {
                for (i = 2; i <= n; i++) { x = v[i]; j = i - 1; while (j > 0 && v[j] > x) { v[j + 1] = v[j]; j-- } v[j + 1] = x }
                printf "mean %.0f ms, median %d ms, 95th percentile %d ms, max %d ms", s / n, v[int((n + 1) / 2)], v[int(n * 0.95 + 0.5)], v[n]
            }' "$out/samples.csv"
    }
    cpu_total=$(( $(echo "$idle_row" | cut -d, -f9) - $(row 0 | cut -d, -f9) ))
    {
        echo "veil soak test: $switches wallpaper switches, $(date -Iseconds)"
        echo
        echo "media:       $media (image-a.png, video-a.mp4 H.264 1280x960, image-b.png, video-b.webm VP9 2560x1080)"
        echo "veil:        $("$veil" --version) at commit $(bench_commit)"
        echo "binary:      $veil"
        echo "compositor:  $compositor"
        echo "allocator:   $allocator"
        bench_machine
        echo "run:         $switches switches in $((elapsed_ns / 1000000000)) s, images and videos in turn; stable window from switch $settle ($rss_n images)"
        echo
        echo "start:       $(mib "$(row 0)") (an image, no video played yet)"
        echo "first video: $(mib "$(row 1)") (playing)"
        echo "image after: $(mib "$(row 2)") (one video)"
        last_image=$(awk -F, '$2 == "image" { r = $1 } END { print r }' "$out/samples.csv")
        echo "image after: $(mib "$(row "$last_image")") ($((last_image / 2)) videos)"
        echo "idle:        $(mib "$idle_row") ($idle s later)"
        printf 'heap:        %.1f to %.1f MiB with an image on screen in the window (RssAnon)\n' \
            "$(awk -v v="$anon_min" 'BEGIN { print v / 1024 }')" "$(awk -v v="$anon_max" 'BEGIN { print v / 1024 }')"
        printf 'heap rise:   %+.2f KiB per switch with an image on screen (allowed: %s), %+.1f MiB over 500 switches at that rate\n' \
            "$anon_rise" "$max_rise" "$(awk -v s="$anon_rise" 'BEGIN { print s * 500 / 1024 }')"
        printf '             heap rise %+.2f KiB per switch with a video playing\n' "$vid_rise"
        printf '             least squares: heap %+.2f, RSS %+.2f KiB per switch with an image on screen; heap %+.2f with a video\n' \
            "$anon_slope" "$rss_slope" "$vid_slope"
        echo "threads:     $thr_min to $thr_max with an image on screen, slope $thr_slope per switch"
        echo "descriptors: $fd_min to $fd_max with an image on screen, slope $fd_slope per switch"
        echo "veil set:    to an image $(latency image); to a video $(latency video)"
        printf 'CPU:         %.1f s in all (%.2f s per switch)\n' \
            "$(awk -v t="$cpu_total" -v hz="$ticks_per_second" 'BEGIN { print t / hz }')" \
            "$(awk -v t="$cpu_total" -v hz="$ticks_per_second" -v n="$switches" 'BEGIN { print t / hz / n }')"
        echo "failed:      $set_failures veil set"
    } > "$out/report.txt"
    "$here/plot.sh" --title "veil soak: $switches switches, $allocator" --from "$settle" --series kind \
        --label "rss_kib=RSS (KiB)" --label "anon_kib=heap, RssAnon (KiB)" --label "threads=threads" \
        --label "fds=file descriptors" --label "set_ms=veil set (ms)" \
        <(grep -v ',idle,\|,start,' "$out/samples.csv") switch rss_kib anon_kib threads fds set_ms > "$out/graph.svg"
fi

{
    echo
    if [ ${#failures[@]} -eq 0 ]; then
        echo "result:      PASS"
    else
        echo "result:      FAIL"
        printf '             %s\n' "${failures[@]}"
    fi
} >> "$out/report.txt"
cat "$out/report.txt"
echo "graph: $out/graph.svg" >&2
[ ${#failures[@]} -eq 0 ]

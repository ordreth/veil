#!/usr/bin/env bash
# Summarises a samples.csv of bench.sh:
#
#   scripts/bench/summary.sh target/bench/<run>/samples.csv
#
# CPU is a percentage of one core. The memory slope is a least-squares line
# through the RSS samples. The frame counts of the video sink start again at
# zero at every loop of the video (a sample below the one before it): the
# frame rate is counted over the intervals without a loop in them.
# Wake-ups are the context switches of the daemon's event loop thread.

set -euo pipefail
[ $# -eq 1 ] || { echo "usage: $0 samples.csv" >&2; exit 2; }

awk -F, '
NR == 1 { next }
{
    n++
    t[n] = $1; cpu[n] = $2; rss[n] = $3; threads[n] = $4; fds[n] = $5
    rendered = $6; dropped = $7
    # An interval in which the video looped is left out: the frames of the
    # old run after the sample before it were never seen.
    if (n > 1 && rendered >= last_rendered && dropped >= last_dropped) {
        rendered_total += rendered - last_rendered
        dropped_total += dropped - last_dropped
        span += t[n] - t[n - 1]
    } else if (n > 1) {
        loops++
    }
    last_rendered = rendered; last_dropped = dropped
    if ($8 != "") { gpu_n++; gpu_sum += $8; if ($8 > gpu_max) gpu_max = $8 }
    if ($9 != "") { dec_n++; dec_sum += $9; if ($9 > dec_max) dec_max = $9 }
    if ($10 != "") { wake_n++; wake_sum += $10; if ($10 > wake_max) wake_max = $10 }
}
function min(a, k,   i, m) { m = a[1]; for (i = 2; i <= k; i++) if (a[i] < m) m = a[i]; return m }
function max(a, k,   i, m) { m = a[1]; for (i = 2; i <= k; i++) if (a[i] > m) m = a[i]; return m }
function mean(a, k,   i, s) { s = 0; for (i = 1; i <= k; i++) s += a[i]; return s / k }
END {
    if (n == 0) { print "no samples"; exit 1 }
    # Sorted CPU for the median.
    for (i = 1; i <= n; i++) sorted[i] = cpu[i]
    for (i = 2; i <= n; i++) {
        v = sorted[i]; j = i - 1
        while (j > 0 && sorted[j] > v) { sorted[j + 1] = sorted[j]; j-- }
        sorted[j + 1] = v
    }
    median = (n % 2) ? sorted[(n + 1) / 2] : (sorted[n / 2] + sorted[n / 2 + 1]) / 2

    # RSS slope in KiB per minute.
    tm = mean(t, n); rm = mean(rss, n); sxy = 0; sxx = 0
    for (i = 1; i <= n; i++) { sxy += (t[i] - tm) * (rss[i] - rm); sxx += (t[i] - tm) ^ 2 }
    slope = (sxx > 0) ? sxy / sxx * 60 : 0

    printf "samples:     %d over %.0f s\n", n, t[n]
    printf "CPU:         mean %.2f %%, median %.2f %%, max %.2f %% (of one core)\n", mean(cpu, n), median, max(cpu, n)
    printf "memory:      RSS %.1f MiB at the start, %.1f MiB at the end, %.1f MiB at most; slope %+.1f KiB/min\n", \
        rss[1] / 1024, rss[n] / 1024, max(rss, n) / 1024, slope
    printf "threads:     %d to %d\n", min(threads, n), max(threads, n)
    printf "descriptors: %d to %d\n", min(fds, n), max(fds, n)
    if (wake_n > 0) {
        printf "wake-ups:    mean %.1f, max %.1f per second (the event loop thread)\n", wake_sum / wake_n, wake_max
    }
    if (span > 0 && (rendered_total > 0 || dropped_total > 0)) {
        printf "frames:      %.1f per second shown, %d dropped (%.2f %%), over %.0f s without a loop (%d loops)\n", \
            rendered_total / span, dropped_total, 100 * dropped_total / (rendered_total + dropped_total), span, loops
    } else {
        print "frames:      none counted (an image, a paused video, or a video nobody is shown)"
    }
    if (gpu_n > 0) {
        printf "GPU:         mean %.1f %%, max %d %%; decoder mean %.1f %%, max %d %% (whole GPU, every process)\n", \
            gpu_sum / gpu_n, gpu_max, dec_sum / dec_n, dec_max
    } else {
        print "GPU:         not measured (nvidia-smi only)"
    }
}' "$1"

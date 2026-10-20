#!/usr/bin/env bash
# How one column of a CSV of bench.sh or soak.sh rises against another, over
# the rows from a given x on:
#
#   scripts/bench/trend.sh [--where COLUMN=VALUE] CSV X Y FROM
#
# prints "slope rise intercept rows first last min max":
#
#   slope      of the least-squares line, in Y per unit of X
#   rise       how fast the bounds of the samples rise, in Y per unit of X:
#              the lowest sample of the window's last quarter minus the
#              lowest of its first quarter, and the same for the highest,
#              each over the distance between the quarters' middles, and of
#              the two the one nearer to zero (zero if they go opposite
#              ways). The heap jumps between levels some MiB apart: after a
#              wallpaper switch by what the allocator happened to keep, while
#              a video plays by whether the decoder's buffers are taken at
#              the moment of the sample. Such noise tilts a least-squares
#              line over a short window, and moves one bound when the mix of
#              levels changes; memory that is kept for good raises both.
#              soak.sh tests this one.
#   intercept  of the least-squares line
#   first last the line's values at the window's ends
#   min max    of the samples in the window
#
# Columns are named by their header. With --where only the rows with that
# value in that column count (soak.sh: kind=image).

set -euo pipefail
where_col= where_val=
if [ "${1-}" = --where ]; then
    where_col=${2%%=*}; where_val=${2#*=}; shift 2
fi
[ $# -eq 4 ] || { echo "usage: $0 [--where COLUMN=VALUE] CSV X Y FROM" >&2; exit 2; }

awk -F, -v xname="$2" -v yname="$3" -v from="$4" -v wname="$where_col" -v wval="$where_val" '
NR == 1 {
    for (i = 1; i <= NF; i++) col[$i] = i
    if (!(xname in col) || !(yname in col) || (wname != "" && !(wname in col))) {
        print "no such column" > "/dev/stderr"; exit 2
    }
    xc = col[xname]; yc = col[yname]; wc = (wname != "") ? col[wname] : 0
    next
}
$xc + 0 >= from + 0 && (wc == 0 || $wc == wval) {
    n++; x[n] = $xc; y[n] = $yc; sx += $xc; sy += $yc
    if (n == 1 || $yc + 0 < ymin) ymin = $yc
    if (n == 1 || $yc + 0 > ymax) ymax = $yc
}
END {
    if (n < 8) { print "fewer than eight rows in the window" > "/dev/stderr"; exit 1 }
    xm = sx / n; ym = sy / n
    for (i = 1; i <= n; i++) { sxy += (x[i] - xm) * (y[i] - ym); sxx += (x[i] - xm) ^ 2 }
    slope = (sxx > 0) ? sxy / sxx : 0
    b = ym - slope * xm
    q = int(n / 4)
    lo1 = y[1]; lo4 = y[n - q + 1]; hi1 = lo1; hi4 = lo4; x1 = 0; x4 = 0
    for (i = 1; i <= q; i++) {
        if (y[i] + 0 < lo1 + 0) lo1 = y[i]
        if (y[i] + 0 > hi1 + 0) hi1 = y[i]
        if (y[n - q + i] + 0 < lo4 + 0) lo4 = y[n - q + i]
        if (y[n - q + i] + 0 > hi4 + 0) hi4 = y[n - q + i]
        x1 += x[i]; x4 += x[n - q + i]
    }
    d = (x4 - x1) / q
    floor = (d > 0) ? (lo4 - lo1) / d : 0
    ceiling = (d > 0) ? (hi4 - hi1) / d : 0
    if (floor * ceiling <= 0) rise = 0
    else rise = (floor > 0) ? ((floor < ceiling) ? floor : ceiling) : ((floor > ceiling) ? floor : ceiling)
    printf "%.6g %.6g %.6g %d %.6g %.6g %.6g %.6g\n", slope, rise, b, n, b + slope * x[1], b + slope * x[n], ymin, ymax
}' "$1"

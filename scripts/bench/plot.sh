#!/usr/bin/env bash
# Draws columns of a CSV of bench.sh or soak.sh as an SVG, one panel per
# column, against a column for the x axis:
#
#   scripts/bench/plot.sh [options] CSV X Y... > graph.svg
#
#   --title TEXT       the heading
#   --from X           marks where the stable window starts and draws the
#                      least-squares line through each panel from there
#   --series COLUMN    one colour per value of this column (soak.sh: kind),
#                      and with --from one line per value
#   --label Y=TEXT     the name a panel shows for column Y (repeatable)
#
# Needs only awk, so CI can keep the graph next to the numbers.

set -euo pipefail
title= from= series=
labels=()
while [ $# -gt 0 ]; do
    case "$1" in
        --title) title=$2; shift 2 ;;
        --from) from=$2; shift 2 ;;
        --series) series=$2; shift 2 ;;
        --label) labels+=("$2"); shift 2 ;;
        *) break ;;
    esac
done
[ $# -ge 3 ] || { echo "usage: $0 [--title T] [--from X] [--series COLUMN] [--label Y=TEXT]... CSV X Y..." >&2; exit 2; }
csv=$1 xname=$2
shift 2
ynames=$(IFS=,; echo "$*")
label_list=$(IFS=$'\x1f'; echo "${labels[*]-}")

awk -F, -v xname="$xname" -v ynames="$ynames" -v from="$from" -v sname="$series" \
    -v title="$title" -v label_list="$label_list" '
function esc(s) { gsub(/&/, "\\&amp;", s); gsub(/</, "\\&lt;", s); gsub(/>/, "\\&gt;", s); return s }
# A number for an axis label: few digits, no exponent.
function fmt(v,   a) {
    a = (v < 0) ? -v : v
    if (a >= 100000) return sprintf("%.0f", v)
    if (a >= 100) return sprintf("%.0f", v)
    if (a >= 20) return sprintf("%.0f", v)
    if (a >= 10) return sprintf("%.1f", v)
    return sprintf("%.2f", v)
}
NR == 1 {
    for (i = 1; i <= NF; i++) col[$i] = i
    if (!(xname in col)) { print "no column " xname > "/dev/stderr"; exit 2 }
    np = split(ynames, yn, ",")
    for (p = 1; p <= np; p++) {
        if (!(yn[p] in col)) { print "no column " yn[p] > "/dev/stderr"; exit 2 }
        label[p] = yn[p]
    }
    nl = split(label_list, ls, "\037")
    for (i = 1; i <= nl; i++) {
        k = substr(ls[i], 1, index(ls[i], "=") - 1); v = substr(ls[i], index(ls[i], "=") + 1)
        for (p = 1; p <= np; p++) if (yn[p] == k) label[p] = v
    }
    if (sname != "" && !(sname in col)) { print "no column " sname > "/dev/stderr"; exit 2 }
    next
}
{
    n++; x[n] = $col[xname]
    s[n] = (sname != "") ? $col[sname] : "all"
    if (!(s[n] in seen)) { seen[s[n]] = ++ns; sn[ns] = s[n] }
    for (p = 1; p <= np; p++) y[p, n] = $col[yn[p]]
    if (n == 1 || x[n] < xmin) xmin = x[n]
    if (n == 1 || x[n] > xmax) xmax = x[n]
}
END {
    if (n == 0) { print "no rows" > "/dev/stderr"; exit 1 }
    if (xmax == xmin) xmax = xmin + 1
    W = 900; L = 80; R = 24; T = 48; PH = 170; GAP = 52
    H = T + np * (PH + GAP) + 10
    split("#2563eb #d97706 #059669 #db2777", colour, " ")
    printf "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"%d\" height=\"%d\" viewBox=\"0 0 %d %d\" font-family=\"sans-serif\" font-size=\"12\">\n", W, H, W, H
    printf "<rect width=\"100%%\" height=\"100%%\" fill=\"#ffffff\"/>\n"
    printf "<text x=\"%d\" y=\"26\" font-size=\"15\" font-weight=\"bold\" fill=\"#111827\">%s</text>\n", L, esc(title)
    lx = W - R
    for (k = ns; k >= 1 && ns > 1; k--) {
        lx -= 8 + 7 * length(sn[k]) + 16
        printf "<rect x=\"%d\" y=\"16\" width=\"12\" height=\"12\" fill=\"%s\"/>", lx, colour[(k - 1) % 4 + 1]
        printf "<text x=\"%d\" y=\"26\" fill=\"#374151\">%s</text>\n", lx + 16, esc(sn[k])
    }
    pw = W - L - R
    for (p = 1; p <= np; p++) {
        top = T + (p - 1) * (PH + GAP) + 20
        ymin = y[p, 1]; ymax = y[p, 1]
        for (i = 2; i <= n; i++) { if (y[p, i] < ymin) ymin = y[p, i]; if (y[p, i] > ymax) ymax = y[p, i] }
        if (ymin > 0 && ymin < (ymax - ymin)) ymin = 0
        pad = (ymax - ymin) * 0.08; if (pad == 0) pad = (ymax != 0) ? ((ymax < 0 ? -ymax : ymax) * 0.05) : 1
        lo = ymin - pad; hi = ymax + pad; if (ymin >= 0 && lo < 0) lo = 0
        printf "<text x=\"%d\" y=\"%d\" font-weight=\"bold\" fill=\"#111827\">%s</text>\n", L, top - 8, esc(label[p])
        printf "<rect x=\"%d\" y=\"%d\" width=\"%d\" height=\"%d\" fill=\"none\" stroke=\"#9ca3af\"/>\n", L, top, pw, PH
        for (t = 0; t <= 4; t++) {
            v = lo + (hi - lo) * t / 4; py = top + PH - PH * t / 4
            printf "<line x1=\"%d\" x2=\"%d\" y1=\"%.1f\" y2=\"%.1f\" stroke=\"#e5e7eb\"/>", L, L + pw, py, py
            printf "<text x=\"%d\" y=\"%.1f\" text-anchor=\"end\" fill=\"#4b5563\">%s</text>\n", L - 6, py + 4, fmt(v)
            vx = xmin + (xmax - xmin) * t / 4; px = L + pw * t / 4
            printf "<text x=\"%.1f\" y=\"%d\" text-anchor=\"middle\" fill=\"#4b5563\">%s</text>\n", px, top + PH + 16, fmt(vx)
        }
        if (from != "" && from + 0 > xmin && from + 0 < xmax) {
            fx = L + pw * (from - xmin) / (xmax - xmin)
            printf "<line x1=\"%.1f\" x2=\"%.1f\" y1=\"%d\" y2=\"%d\" stroke=\"#6b7280\" stroke-dasharray=\"4 4\"/>\n", fx, fx, top, top + PH
        }
        for (k = 1; k <= ns; k++) {
            pts = ""; m = 0; sx = 0; sy = 0; delete wx; delete wy
            for (i = 1; i <= n; i++) {
                if (s[i] != sn[k]) continue
                px = L + pw * (x[i] - xmin) / (xmax - xmin); py = top + PH - PH * (y[p, i] - lo) / (hi - lo)
                pts = pts sprintf("%.1f,%.1f ", px, py)
                if (from == "" || x[i] + 0 >= from + 0) { m++; wx[m] = x[i]; wy[m] = y[p, i]; sx += x[i]; sy += y[p, i] }
            }
            printf "<polyline points=\"%s\" fill=\"none\" stroke=\"%s\" stroke-width=\"1.2\"/>\n", pts, colour[(k - 1) % 4 + 1]
            if (from != "" && m >= 2) {
                xm = sx / m; ym = sy / m; sxy = 0; sxx = 0
                for (i = 1; i <= m; i++) { sxy += (wx[i] - xm) * (wy[i] - ym); sxx += (wx[i] - xm) ^ 2 }
                b1 = (sxx > 0) ? sxy / sxx : 0; b0 = ym - b1 * xm
                x1 = wx[1]; x2 = wx[m]
                printf "<line x1=\"%.1f\" x2=\"%.1f\" y1=\"%.1f\" y2=\"%.1f\" stroke=\"#dc2626\" stroke-width=\"1.5\" stroke-dasharray=\"6 3\"/>\n", \
                    L + pw * (x1 - xmin) / (xmax - xmin), L + pw * (x2 - xmin) / (xmax - xmin), \
                    top + PH - PH * (b0 + b1 * x1 - lo) / (hi - lo), top + PH - PH * (b0 + b1 * x2 - lo) / (hi - lo)
            }
        }
    }
    printf "<text x=\"%d\" y=\"%d\" text-anchor=\"middle\" fill=\"#4b5563\">%s</text>\n", L + pw / 2, H - 8, esc(xname)
    print "</svg>"
}' "$csv"

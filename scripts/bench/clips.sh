#!/usr/bin/env bash
# Makes the reference clips of the benchmark (docs/measurements.md) with
# ffmpeg's test source: 10 seconds each, every frame different, no audio;
# and one still picture.
#
#   scripts/bench/clips.sh [DIR]     (default target/bench/clips)
#
# Clips that already exist are kept. Run it inside `nix develop .#headless`.

set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
dir=${1:-$root/target/bench/clips}
mkdir -p "$dir"
command -v ffmpeg >/dev/null || { echo "ffmpeg is missing: run inside nix develop .#headless" >&2; exit 2; }

# name, size, rate, encoder options
clips=(
    "h264-1080p30.mp4|1920x1080|30|-c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p"
    "h264-1080p60.mp4|1920x1080|60|-c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p"
    "h264-2160p60.mp4|3840x2160|60|-c:v libx264 -preset veryfast -crf 23 -pix_fmt yuv420p"
    "hevc-2160p60.mp4|3840x2160|60|-c:v libx265 -preset veryfast -crf 26 -pix_fmt yuv420p -tag:v hvc1"
    "vp9-1080p60.webm|1920x1080|60|-c:v libvpx-vp9 -deadline realtime -cpu-used 8 -b:v 4M -pix_fmt yuv420p"
    "av1-1080p60.mkv|1920x1080|60|-c:v libsvtav1 -preset 10 -crf 35 -pix_fmt yuv420p -svtav1-params log-level=1"
    "still-1080p.png|1920x1080|1|-frames:v 1"
)

for clip in "${clips[@]}"; do
    IFS='|' read -r name size rate options <<< "$clip"
    if [ -s "$dir/$name" ]; then
        echo "kept $dir/$name"
        continue
    fi
    echo "making $dir/$name"
    # shellcheck disable=SC2086 # the options are words
    ffmpeg -hide_banner -loglevel error -y \
        -f lavfi -i "testsrc2=size=$size:rate=$rate:duration=10" \
        $options -an "$dir/.part-$name"
    mv "$dir/.part-$name" "$dir/$name"
done

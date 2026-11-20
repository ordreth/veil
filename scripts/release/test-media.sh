#!/usr/bin/env bash
# The files scripts/release/distro-check.sh shows: a picture, an H.264 clip
# in MP4 and a VP9 clip in WebM, small and short (3 s at 30 fps).
#
#   scripts/release/test-media.sh DIR
#
# Needs ffmpeg with libx264 and libvpx (nix develop .#headless).
set -euo pipefail

dir=${1:?usage: test-media.sh DIR}
command -v ffmpeg > /dev/null || { echo "ffmpeg is missing: run inside nix develop .#headless" >&2; exit 2; }
mkdir -p "$dir"
source=(-f lavfi -i testsrc2=duration=3:size=320x180:rate=30)
ffmpeg -loglevel error -y -f lavfi -i testsrc2=size=320x180 -frames:v 1 "$dir/picture.png"
ffmpeg -loglevel error -y "${source[@]}" -c:v libx264 -pix_fmt yuv420p "$dir/h264.mp4"
ffmpeg -loglevel error -y "${source[@]}" -c:v libvpx-vp9 -pix_fmt yuv420p "$dir/vp9.webm"
ls -l "$dir"

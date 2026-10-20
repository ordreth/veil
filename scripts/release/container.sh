#!/usr/bin/env bash
# The release build, inside the build container (scripts/release/build.sh
# starts it there; do not run it on your own machine: it installs packages).
# Expects the source in /src and writes the tarball into /out.
#
# Debian 12 is the build system on purpose, because an old glibc keeps the
# binary's requirement low: it needs a glibc that has the newest symbol
# version it was linked against, which is printed below (GLIBC_2.34 for
# 0.1.0, so Ubuntu 22.04 and Debian 12 run it). GStreamer, GLib and the
# Wayland client library are linked dynamically and come from the system.
set -euo pipefail

: "${VEIL_TESTS:=1}"

export DEBIAN_FRONTEND=noninteractive
packages=(
  pkg-config libwayland-dev libxkbcommon-dev
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
)
if [[ $VEIL_TESTS == 1 ]]; then
  # The GStreamer tests skip without the plugins a pipeline needs.
  packages+=(gstreamer1.0-plugins-base gstreamer1.0-plugins-good
    gstreamer1.0-plugins-bad gstreamer1.0-libav)
fi
apt-get update -q
apt-get install -qy --no-install-recommends "${packages[@]}"

cd /src
target=$(rustc -vV | sed -n 's/^host: //p')
printf 'Building veil for %s with %s, GStreamer %s\n' \
  "$target" "$(rustc --version)" "$(pkg-config --modversion gstreamer-1.0)"

if [[ $VEIL_TESTS == 1 ]]; then
  # A registry of their own, and a home for what the tests write there.
  GST_REGISTRY_1_0=$(mktemp -d)/registry.bin HOME=$(mktemp -d) cargo test --locked
fi
cargo build --release --locked

binary=target/release/veil
printf 'Dynamic libraries:\n'
objdump -p "$binary" | awk '/NEEDED/ { print "  " $2 }'
printf 'Newest glibc symbol: %s\n' \
  "$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -n1)"

tarball=$(scripts/release/package.sh "$binary" "$target" /out)
if [[ -n ${HOST_UID-} ]]; then
  chown "$HOST_UID:${HOST_GID:-$HOST_UID}" "$tarball"
fi

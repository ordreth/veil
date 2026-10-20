#!/usr/bin/env bash
# Builds the release tarball of veil for this machine's architecture (x86_64
# or aarch64), exactly as the release workflow does: in a Debian 12 container
# with a pinned Rust, from the files git tracks (with their contents in the
# working tree, so uncommitted changes are built too).
#
#   scripts/release/build.sh [--out DIR] [--no-tests]
#
# Needs Docker. Writes DIR/veil-<version>-<target>.tar.gz (default
# target/release-dist). --no-tests leaves out `cargo test`, which the release
# workflow runs.
set -euo pipefail

# Rust 1.98.1 on Debian 12, by digest, so a rebuilt tag cannot change the
# build. Raise both together.
image=rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730

root=$(cd "$(dirname "$0")/../.." && pwd)
out=$root/target/release-dist
tests=1
while [[ $# -gt 0 ]]; do
  case $1 in
  --out)
    out=$2
    shift 2
    ;;
  --no-tests)
    tests=0
    shift
    ;;
  *)
    printf 'Usage: %s [--out DIR] [--no-tests]\n' "$0" >&2
    exit 2
    ;;
  esac
done
mkdir -p "$out"
out=$(cd "$out" && pwd)

cd "$root"
# Every file's time in the tarball: the commit's, unless one was chosen.
epoch=${SOURCE_DATE_EPOCH:-$(git log -1 --format=%ct)}

# The tracked files that exist (a deleted one is still listed until it is
# committed), streamed into the container: nothing untracked, no target/.
git ls-files -z --cached |
  while IFS= read -r -d '' file; do
    [[ -e $file ]] && printf '%s\0' "$file"
  done |
  tar --null -T - -cf - |
  docker run --rm -i \
    -e SOURCE_DATE_EPOCH="$epoch" -e VEIL_TESTS="$tests" \
    -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
    -v "$out:/out" "$image" \
    bash -c 'mkdir /src && tar -x -C /src && exec /src/scripts/release/container.sh'

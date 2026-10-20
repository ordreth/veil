#!/usr/bin/env bash
# Packs a release binary of veil into its tarball.
#
#   scripts/release/package.sh target/release/veil x86_64-unknown-linux-gnu dist/
#
# writes dist/veil-<version>-<target>.tar.gz. The completions and the man
# pages come from the binary itself (`veil completions`, `veil man-pages`), so
# the binary must run here. The tarball's layout keeps the paths of the
# repository for the documents, so the README's links work in it:
#
#   veil-<version>-<target>/
#     veil                          the binary
#     install.sh                    installs everything below into a prefix
#     README.md CHANGELOG.md LICENSE ADDITIONAL-PERMISSIONS.md THIRD-PARTY-NOTICES
#     completions/                  veil.bash _veil veil.fish veil.nu
#     man/                          veil.1 veil-set.1 ...
#     contrib/systemd/veil.service  ExecStart=/usr/bin/veil (install.sh changes it)
#     contrib/config.example.toml
#     docs/                         the user documents
#
# With SOURCE_DATE_EPOCH set (the release build sets the commit's time), every
# file gets that time and the archive has a fixed order and owner, so the same
# binary always gives the same tarball.
set -euo pipefail
umask 022

root=$(cd "$(dirname "$0")/../.." && pwd)

[[ $# -eq 3 ]] || {
  printf 'Usage: %s BINARY TARGET OUTDIR\n' "$0" >&2
  exit 2
}
binary=$1
target=$2
outdir=$3

version=$("$binary" --version | awk '{ print $2 }')
[[ -n $version ]] || {
  printf 'package.sh: %s --version printed no version\n' "$binary" >&2
  exit 1
}
name=veil-$version-$target

staging=$(mktemp -d)
trap 'rm -rf "$staging"' EXIT
dir=$staging/$name
mkdir -p "$dir/completions" "$dir/man" "$dir/contrib/systemd" "$dir/docs"

install -m755 "$binary" "$dir/veil"
install -m755 "$root/scripts/release/install.sh" "$dir/install.sh"
install -m644 "$root/README.md" "$root/CHANGELOG.md" "$root/LICENSE" "$root/ADDITIONAL-PERMISSIONS.md" "$root/THIRD-PARTY-NOTICES" "$dir/"
install -m644 "$root/contrib/systemd/veil.service" "$dir/contrib/systemd/"
install -m644 "$root/contrib/config.example.toml" "$dir/contrib/"
# What a user reads; manual-tests.md is for the people who work on veil.
for doc in configuration ipc known-issues measurements troubleshooting; do
  install -m644 "$root/docs/$doc.md" "$dir/docs/"
done

"$dir/veil" completions bash >"$dir/completions/veil.bash"
"$dir/veil" completions zsh >"$dir/completions/_veil"
"$dir/veil" completions fish >"$dir/completions/veil.fish"
"$dir/veil" completions nushell >"$dir/completions/veil.nu"
"$dir/veil" man-pages "$dir/man"
chmod 644 "$dir"/completions/* "$dir"/man/*

mkdir -p "$outdir"
tarball=$(cd "$outdir" && pwd)/$name.tar.gz
epoch=${SOURCE_DATE_EPOCH:-$(date +%s)}
tar --sort=name --mtime="@$epoch" --owner=0 --group=0 --numeric-owner \
  --format=gnu -C "$staging" -cf - "$name" | gzip -9n >"$tarball"
printf '%s\n' "$tarball"

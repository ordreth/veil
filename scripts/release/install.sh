#!/bin/sh
# Installs veil from its release tarball: the binary, the systemd user unit,
# the man pages, the shell completions and the documentation.
#
#   sudo ./install.sh                     # into /usr/local
#   ./install.sh --prefix ~/.local        # for one user, without root
#   ./install.sh --prefix ~/.local --uninstall
#
# (This file is scripts/release/install.sh in veil's repository; the release
# tarball has it at its top, next to the files it installs.)
set -eu

usage() {
  cat <<'EOF'
Usage: ./install.sh [--prefix DIR] [--uninstall]

Installs veil from this directory into DIR (default /usr/local):
  DIR/bin/veil
  DIR/share/systemd/user/veil.service    with ExecStart=DIR/bin/veil
  DIR/share/man/man1/veil*.1
  DIR/share/bash-completion/completions/veil
  DIR/share/zsh/site-functions/_veil
  DIR/share/fish/vendor_completions.d/veil.fish
  DIR/share/nushell/vendor/autoload/veil.nu
  DIR/share/doc/veil/                    README, changelog, licence and permissions, documents
--uninstall removes these files again.
EOF
}

prefix=/usr/local
uninstall=false
while [ $# -gt 0 ]; do
  case $1 in
  --prefix)
    [ $# -ge 2 ] || {
      usage >&2
      exit 2
    }
    prefix=$2
    shift 2
    ;;
  --prefix=*)
    prefix=${1#--prefix=}
    shift
    ;;
  --uninstall)
    uninstall=true
    shift
    ;;
  -h | --help)
    usage
    exit 0
    ;;
  *)
    printf 'install.sh: unknown argument: %s\n\n' "$1" >&2
    usage >&2
    exit 2
    ;;
  esac
done

case $prefix in
/*) ;;
*)
  printf 'install.sh: the prefix must be an absolute path: %s\n' "$prefix" >&2
  exit 2
  ;;
esac
case $prefix in
*[[:space:]%\\\"\']*)
  # The unit's ExecStart would split the path at a space, and systemd reads
  # `%` as a specifier.
  printf 'install.sh: the prefix must not contain spaces, %%, quotes or backslashes: %s\n' "$prefix" >&2
  exit 2
  ;;
esac
prefix=${prefix%/}
[ -n "$prefix" ] || {
  printf 'install.sh: the prefix must not be /\n' >&2
  exit 2
}

here=$(cd "$(dirname "$0")" && pwd)
[ -x "$here/veil" ] || {
  printf 'install.sh: %s/veil is missing; run me from the extracted tarball\n' "$here" >&2
  exit 1
}

# Every file: where it is in this directory, where it goes under the prefix,
# and its mode. No name has a space in it.
manifest() (
  cd "$here"
  echo "veil bin/veil 755"
  echo "contrib/systemd/veil.service share/systemd/user/veil.service 644"
  for page in man/*.1; do
    echo "$page share/man/man1/${page#man/} 644"
  done
  echo "completions/veil.bash share/bash-completion/completions/veil 644"
  echo "completions/_veil share/zsh/site-functions/_veil 644"
  echo "completions/veil.fish share/fish/vendor_completions.d/veil.fish 644"
  echo "completions/veil.nu share/nushell/vendor/autoload/veil.nu 644"
  for doc in README.md CHANGELOG.md LICENSE ADDITIONAL-PERMISSIONS.md THIRD-PARTY-NOTICES contrib/config.example.toml docs/*.md; do
    echo "$doc share/doc/veil/${doc##*/} 644"
  done
)

if $uninstall; then
  manifest | while read -r _ dest _; do
    if [ -e "$prefix/$dest" ]; then
      rm -f "$prefix/$dest"
      printf 'removed %s\n' "$prefix/$dest"
    fi
  done
  rmdir "$prefix/share/doc/veil" 2>/dev/null || true
  exit 0
fi

manifest | while read -r source dest mode; do
  [ -f "$here/$source" ] || {
    printf 'install.sh: %s/%s is missing; is the tarball complete?\n' "$here" "$source" >&2
    exit 1
  }
  mkdir -p "$(dirname "$prefix/$dest")"
  if [ "$dest" = share/systemd/user/veil.service ]; then
    # The unit names the binary it starts. Escape what sed would read in the
    # replacement: the delimiter and `&` (backslashes are refused above).
    bin=$(printf '%s\n' "$prefix/bin/veil" | sed 's/[|&]/\\&/g')
    sed "s|^ExecStart=/usr/bin/veil |ExecStart=$bin |" "$here/$source" >"$prefix/$dest.tmp"
    grep -qF "ExecStart=$prefix/bin/veil daemon" "$prefix/$dest.tmp" || {
      rm -f "$prefix/$dest.tmp"
      printf 'install.sh: could not set ExecStart in the systemd unit\n' >&2
      exit 1
    }
    chmod "$mode" "$prefix/$dest.tmp"
    mv -f "$prefix/$dest.tmp" "$prefix/$dest"
  else
    # Copied next to the destination and renamed over it, so a running
    # daemon keeps its binary and nothing is left half-written.
    cp "$here/$source" "$prefix/$dest.tmp"
    chmod "$mode" "$prefix/$dest.tmp"
    mv -f "$prefix/$dest.tmp" "$prefix/$dest"
  fi
  printf 'installed %s\n' "$prefix/$dest"
done

cat <<EOF

veil is installed in $prefix. To start it with the graphical session:
  systemctl --user daemon-reload
  systemctl --user enable --now veil
Started by hand instead, give it one allocator arena as the unit does
(MALLOC_ARENA_MAX=1 veil daemon), or its memory grows with every video
switch.
EOF
case $prefix in
/usr | /usr/local) ;;
"${HOME:-/nonexistent}/.local")
  cat <<EOF
$prefix/bin must be on your PATH. zsh does not look in
$prefix/share/zsh/site-functions by itself: add
  fpath+=($prefix/share/zsh/site-functions)
to ~/.zshrc before compinit.
EOF
  ;;
*)
  cat <<EOF
$prefix is not one of the directories systemd, man and the shells look in by
themselves: $prefix/bin must be on your PATH, and the unit, the man pages and
the completions may need to be linked to where they are looked for.
EOF
  ;;
esac

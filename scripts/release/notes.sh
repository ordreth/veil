#!/usr/bin/env bash
# Prints the release notes of a version tag, or refuses the tag.
#
#   scripts/release/notes.sh v1.0.0 > notes.md
#
# The notes are the tag's section of CHANGELOG.md, `## [1.0.0] - YYYY-MM-DD`,
# without its heading. The tag is refused (exit status 1, the reason on
# stderr) when it is not `v` and a semantic version, when Cargo.toml or
# Cargo.lock has another version, or when CHANGELOG.md has no such section,
# an empty one, or no link reference for it at the bottom. The release
# workflow runs this before it builds anything.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)

fail() {
  printf 'notes.sh: %s\n' "$*" >&2
  exit 1
}

[[ $# -eq 1 ]] || {
  printf 'Usage: %s vX.Y.Z\n' "$0" >&2
  exit 2
}
tag=$1

# Semantic versioning 2.0.0, without build metadata (`+...` has no place in a
# tag that names a release).
semver='(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-[0-9A-Za-z.-]+)?'
[[ $tag =~ ^v$semver$ ]] || fail "the tag '$tag' is not 'v' followed by a version such as 1.0.0"
version=${tag#v}

# The version of the package itself: the first `version =` of [package].
cargo_version=$(awk '
  /^\[/ { in_package = ($0 == "[package]") }
  in_package && /^version *=/ { gsub(/^version *= *"|".*$/, ""); print; exit }
' "$root/Cargo.toml")
[[ $cargo_version == "$version" ]] ||
  fail "Cargo.toml has version '$cargo_version', the tag is '$tag'"

lock_version=$(awk '
  $0 == "name = \"veil\"" { getline; gsub(/^version = "|"$/, ""); print; exit }
' "$root/Cargo.lock")
[[ $lock_version == "$version" ]] ||
  fail "Cargo.lock has veil at '$lock_version', the tag is '$tag' (run cargo build to update it)"

changelog=$root/CHANGELOG.md
escaped=${version//./\\.}
headings=$(grep -cE "^## \[$escaped\] - [0-9]{4}-[0-9]{2}-[0-9]{2}\$" "$changelog" || true)
[[ $headings -eq 1 ]] ||
  fail "CHANGELOG.md needs exactly one heading '## [$version] - YYYY-MM-DD' (found $headings)"
grep -qE "^\[$escaped\]: https://" "$changelog" ||
  fail "CHANGELOG.md has no link reference '[$version]: https://...' at the bottom"

# The section: from its heading to the next `## ` heading or the link
# references, without the blank lines at its start (the command substitution
# drops those at its end).
notes=$(awk -v heading="## [$version] - " '
  index($0, heading) == 1 { inside = 1; next }
  inside && (/^## / || /^\[[^]]+\]: /) { exit }
  inside { print }
' "$changelog" | sed -e '/./,$!d')
[[ -n $notes ]] || fail "the section of $version in CHANGELOG.md is empty"

printf '%s\n' "$notes"

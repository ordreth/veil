#!/usr/bin/env bash
# What each shell offers to complete, with the completion scripts a package
# installs: the subcommands after `veil `, and the values of --pause-on,
# --mode and of `veil completions`. bash, zsh and fish are asked to complete
# a command line; nushell, which completes only in its line editor, is asked
# which commands the script declares.
#
# Usage: completions.sh SHARE, where SHARE is the package's share directory.
# Run by the flake's check `completions` (`nix flake check`); it needs an
# interactive bash (for `compgen`), zsh, fish and nu on PATH, and the zsh
# helper next to it. SHELLS (default: all four) limits it to the shells
# named, for a machine that lacks some.
# The complete_* functions are called through `check`, by name.
# shellcheck disable=SC2329
set -euo pipefail

share=${1:?usage: completions.sh SHARE}
here=$(dirname "${BASH_SOURCE[0]}")
failed=0
shells=${SHELLS:-bash zsh fish nushell}
wants() { [[ " $shells " == *" $1 "* ]]; }

subcommands="clear completions config daemon outputs pause ping quit resume set status toggle"

# Complete LINE in bash, as readline would with the cursor at its end.
complete_bash() {
    bash --norc --noprofile -c '
        source "$1"
        line=$2
        read -ra COMP_WORDS <<< "$line"
        [[ $line == *" " ]] && COMP_WORDS+=("")
        COMP_CWORD=$(( ${#COMP_WORDS[@]} - 1 ))
        COMP_LINE=$line
        COMP_POINT=${#line}
        _veil veil "${COMP_WORDS[COMP_CWORD]}" "${COMP_WORDS[COMP_CWORD - 1]}"
        printf "%s\n" "${COMPREPLY[@]}"
    ' _ "$share/bash-completion/completions/veil.bash" "$1"
}

complete_zsh() {
    zsh -f "$here/zsh-complete.zsh" "$share/zsh/site-functions" "$1"
}

complete_fish() {
    HOME=${TMPDIR:-/tmp} fish --no-config -c \
        "source $share/fish/vendor_completions.d/veil.fish; complete -C '$1'" | cut -f1
}

# Checks that SHELL offers, for LINE, every word of WORDS; with `exact`, also
# nothing else. Options (`-h`, `--help`) are left out of what it offers.
check() {
    local shell=$1 line=$2 words=$3 how=${4:-}
    local found
    found=$("complete_$shell" "$line" | sed '/^-/d' | sort -u | tr '\n' ' ')
    for word in $words; do
        if [[ " $found" != *" $word "* ]]; then
            echo "$shell: '$line' does not offer '$word'; it offers: $found" >&2
            failed=1
        fi
    done
    if [ "$how" = exact ]; then
        local expected
        expected=$(tr ' ' '\n' <<< "$words" | sort -u | tr '\n' ' ')
        if [ "$found" != "$expected" ]; then
            echo "$shell: '$line' offers $found instead of $expected" >&2
            failed=1
        fi
    fi
}

if wants bash && ! bash -c 'type compgen' > /dev/null 2>&1; then
    echo "bash has no compgen: an interactive bash is needed" >&2
    exit 1
fi
for shell in bash zsh fish; do
    wants "$shell" || continue
    check "$shell" "veil " "$subcommands"
    check "$shell" "veil daemon --pause-on " "never fullscreen covered" exact
    check "$shell" "veil set --mode " "fill fit stretch center tile" exact
    check "$shell" "veil completions " "bash zsh fish nushell" exact
done

# nushell: every subcommand is declared.
if wants nushell; then
    declared=$(HOME=${TMPDIR:-/tmp} nu --no-config-file -c \
        "source $share/nushell/vendor/autoload/veil.nu; scope commands | where name starts-with 'veil ' | get name | to text")
    for command in $subcommands "config check"; do
        if ! grep -qx "veil $command" <<< "$declared"; then
            echo "nushell: 'veil $command' is not declared; it declares: $declared" >&2
            failed=1
        fi
    done
fi

[ "$failed" = 0 ] && echo "completions check: $shells offer what veil takes"
exit "$failed"

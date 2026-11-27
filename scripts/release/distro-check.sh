#!/usr/bin/env bash
# Checks the README's installation on one distribution. It runs as root in a
# container of that distribution (the workflow `install`) and installs
# packages: do not run `tarball` or `source` on your own machine.
#
#   scripts/release/distro-check.sh tarball TARBALL MEDIA
#   scripts/release/distro-check.sh source MEDIA
#   scripts/release/distro-check.sh commands tarball|source DISTRO
#   scripts/release/distro-check.sh smoke VEIL MEDIA
#
# tarball: the packages of the README's "Prebuilt binaries" for this
#   distribution, then TARBALL installed with its install.sh, into /usr/local
#   and into ~/.local, as the README does.
# source: the commands of the README's "From source" for this distribution,
#   then its build commands, in this checkout.
# Both then run `smoke` with the installed binary.
# commands: prints the commands the README gives for DISTRO (`arch`,
#   `debian-13`, `fedora-44`, ...) as they would be run, and runs nothing.
# smoke: a headless sway that renders on the CPU, as an ordinary user (the
#   user `tester`, made for it, when run as root), and `veil daemon` showing
#   MEDIA/picture.png, then `veil set` of MEDIA/h264.mp4 and MEDIA/vp9.webm
#   (scripts/release/test-media.sh makes them): each must be shown, or play
#   with frames the compositor took.
#
# The packages and commands come from README.md itself, so that what it says
# is what is checked.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
readme=$root/README.md

fail() {
    echo "distro-check: $*" >&2
    exit 1
}

# This system as the README's sections name it: `arch`, `debian-13`, ...
distro() {
    # shellcheck source=/dev/null
    . /etc/os-release
    case $ID in
    arch) echo arch ;;
    *) echo "$ID-$VERSION_ID" ;;
    esac
}

# The lines of README.md from the line that is exactly $1 to the next line
# that starts with $2 (exclusive), with continued lines (`\`) joined.
section() {
    awk -v start="$1" -v end="$2" '
        $0 == start { inside = 1; next }
        inside && index($0, end) == 1 { exit }
        inside { print }
    ' "$readme" | awk '{ if (sub(/\\$/, "")) { line = line $0; next } print line $0; line = "" }'
}

# The first ```bash block of stdin after a line that starts with $1.
block_after() {
    awk -v heading="$1" '
        index($0, heading) == 1 { found = 1; next }
        found && /^```bash$/ { inside = 1; next }
        inside && /^```$/ { exit }
        inside { print }
    '
}

# A command of the README as it runs here: as root and without questions.
unattended() {
    sed -E \
        -e 's/^sudo //' \
        -e 's/^apt install /apt-get install -y /' \
        -e 's/^dnf install /dnf install -y /' \
        -e 's/^pacman -S /pacman -S --noconfirm /' \
        -e 's/\| sh$/| sh -s -- -y/' \
        -e 's/[[:space:]]+#.*$//' \
        -e 's/[[:space:]]+/ /g'
}

# The commands that install the packages for $2 (`tarball` or `source`) on
# distribution $1.
package_commands() {
    local distro=$1 mode=$2
    case $mode in
    tarball)
        local family
        case $distro in
        debian-* | ubuntu-*) family="# Debian, Ubuntu" ;;
        fedora-*) family="# Fedora" ;;
        arch) family="# Arch Linux" ;;
        *) fail "the README names no packages for $distro" ;;
        esac
        section "### Prebuilt binaries" "### " | grep -F -- "$family" | unattended
        ;;
    source)
        local heading
        case $distro in
        arch) heading="**Arch Linux**" ;;
        debian-*) heading="**Debian ${distro#debian-}**" ;;
        ubuntu-*) heading="**Ubuntu ${distro#ubuntu-}**" ;;
        fedora-*) heading="**Fedora ${distro#fedora-}**" ;;
        *) fail "the README has no build from source for $distro" ;;
        esac
        section "### From source" "### " | block_after "$heading" | grep -v '^[[:space:]]*$' | unattended
        ;;
    esac
}

# The README's commands after the packages: installing the tarball, or
# building and installing from this checkout (its `git clone` left out).
install_commands() {
    case $1 in
    tarball)
        section "### Prebuilt binaries" "### " | awk '/^```bash$/ { n++; next } n == 1 && /^```$/ { exit } n == 1' |
            grep -E '^(tar |sudo \./install\.sh|\./install\.sh)' | unattended
        ;;
    source)
        section "### From source" "### " | block_after "Then build veil" | grep -v '^git clone' | unattended
        ;;
    esac
}

# Before the README's packages: the package index, which a container lacks.
refresh() {
    case $1 in
    debian-* | ubuntu-*) apt-get update -q ;;
    arch) pacman -Syu --noconfirm ;;
    fedora-*) : ;;
    esac
}

# What the check needs besides the README's packages.
test_tools() {
    case $1 in
    debian-* | ubuntu-*) DEBIAN_FRONTEND=noninteractive apt-get install -y sway jq ;;
    fedora-*) dnf install -y sway jq util-linux ;;
    arch) pacman -S --noconfirm --needed sway jq util-linux ;;
    esac
}

# Runs the commands in $1, one per line, in this shell and not in a pipeline's
# subshell, so that what one of them changes in the environment holds for the
# next ones and for the build: rustup's `. "$HOME/.cargo/env"` puts cargo on
# PATH.
run_commands() {
    local command
    while IFS= read -r command; do
        [ -n "$command" ] || continue
        printf '+ %s\n' "$command"
        DEBIAN_FRONTEND=noninteractive eval "$command"
    done <<< "$1"
}

smoke() {
    local veil=$1 media=$2 work
    work=$(mktemp -d)
    chmod 755 "$work"
    cp "$media"/picture.png "$media"/h264.mp4 "$media"/vp9.webm "$work"/
    chmod 644 "$work"/*
    local as=()
    local home=$work/home
    if [ "$(id -u)" = 0 ]; then
        id tester > /dev/null 2>&1 || useradd -m tester
        as=(runuser -u tester --)
        home=/home/tester
    else
        mkdir -p "$home"
    fi
    local run=$work/run
    mkdir -m 700 "$run"
    [ "$(id -u)" = 0 ] && chown tester "$run"
    session() {
        "${as[@]}" env -i HOME="$home" PATH="/usr/local/bin:$PATH" XDG_RUNTIME_DIR="$run" "$@"
    }

    printf 'xwayland disable\noutput HEADLESS-1 resolution 640x360\n' > "$work/sway.conf"
    chmod 644 "$work/sway.conf"
    session WLR_BACKENDS=headless WLR_RENDERER=pixman WLR_HEADLESS_OUTPUTS=1 \
        WLR_LIBINPUT_NO_DEVICES=1 LIBSEAT_BACKEND=noop \
        sway -c "$work/sway.conf" > "$work/sway.log" 2>&1 &
    local sway_pid=$! display=
    for _ in $(seq 100); do
        for socket in "$run"/wayland-[0-9]; do
            [ -S "$socket" ] && display=${socket##*/} && break
        done
        [ -n "$display" ] && break
        if ! kill -0 "$sway_pid" 2> /dev/null; then
            cat "$work/sway.log"
            command -v getcap > /dev/null && echo "sway's file capabilities: $(getcap "$(command -v sway)")"
            echo "this process's capability bounding set: $(grep CapBnd /proc/self/status)"
            fail "sway exited"
        fi
        sleep 0.1
    done
    [ -n "$display" ] || fail "sway does not listen"
    veil() {
        session WAYLAND_DISPLAY="$display" MALLOC_ARENA_MAX=1 "$veil" "$@"
    }
    # Waits until the output is in state $1, and for a video until the
    # compositor took frames of it.
    wait_for() {
        local state=$1 status=
        for _ in $(seq 300); do
            status=$(veil status --json 2> /dev/null || true)
            if jq -e --arg state "$state" \
                '.outputs[0] | .state == $state and ($state == "shown" or .video.rendered >= 10)' \
                <<< "$status" > /dev/null 2>&1; then
                return 0
            fi
            sleep 0.1
        done
        echo "last status: $status" >&2
        tail -n 30 "$work/daemon.log" >&2
        return 1
    }

    veil daemon --input "$work/picture.png" > "$work/daemon.log" 2>&1 &
    local daemon_pid=$!
    wait_for shown || fail "the picture is not shown"
    echo "smoke: the picture is shown"
    for clip in h264.mp4 vp9.webm; do
        veil set --input "$work/$clip" || { tail -n 30 "$work/daemon.log" >&2; fail "veil set of $clip failed"; }
        wait_for playing || fail "$clip does not play"
        echo "smoke: $clip plays ($(grep -o 'video decoder: [^ ]* ([a-z]*)' "$work/daemon.log" | tail -n 1))"
    done
    veil quit > /dev/null
    wait "$daemon_pid" || fail "the daemon exited with $?"
    kill "$sway_pid" 2> /dev/null || true
    wait "$sway_pid" 2> /dev/null || true
    echo "smoke: $("$veil" --version) on $(distro): a picture, H.264 and VP9"
}

mode=${1:-}
case $mode in
commands)
    [ $# = 3 ] || fail "usage: distro-check.sh commands tarball|source DISTRO"
    package_commands "$3" "$2"
    install_commands "$2"
    ;;
smoke)
    [ $# = 3 ] || fail "usage: distro-check.sh smoke VEIL MEDIA"
    smoke "$2" "$3"
    ;;
tarball)
    [ $# = 3 ] || fail "usage: distro-check.sh tarball TARBALL MEDIA"
    tarball=$(realpath "$2")
    media=$(realpath "$3")
    distro=$(distro)
    refresh "$distro"
    commands=$(package_commands "$distro" tarball)
    run_commands "$commands"
    test_tools "$distro"
    work=$(mktemp -d)
    cp "$tarball" "$work"/
    tarball=${tarball##*/}
    commands=$(install_commands tarball)
    (cd "$work" && run_commands "$commands")
    test -x "$HOME/.local/bin/veil" || fail "install.sh --prefix ~/.local installed no binary"
    smoke /usr/local/bin/veil "$media"
    ;;
source)
    [ $# = 2 ] || fail "usage: distro-check.sh source MEDIA"
    media=$(realpath "$2")
    distro=$(distro)
    refresh "$distro"
    commands=$(package_commands "$distro" source)
    run_commands "$commands"
    test_tools "$distro"
    commands=$(install_commands source)
    (cd "$root" && run_commands "$commands")
    install -m 755 "$HOME/.local/bin/veil" /usr/local/bin/veil
    smoke /usr/local/bin/veil "$media"
    ;;
*)
    fail "usage: distro-check.sh tarball TARBALL MEDIA | source MEDIA | commands tarball|source DISTRO | smoke VEIL MEDIA"
    ;;
esac

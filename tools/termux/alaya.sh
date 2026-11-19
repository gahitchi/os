#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Boot a ready-made Alaya image in QEMU from Termux on Android (also works on
# any Linux with bash, curl and qemu-system-x86_64). Nothing is built here:
# the image comes from the project's CI.
#
#   alaya.sh run  [image]   boot and show the serial console (Ctrl+A then x quits)
#   alaya.sh test [image]   boot, pass if the kernel reports a clean boot
#   alaya.sh fetch          only download the latest image
#
# Options (environment):
#   ALAYA_CPUS=2            virtual cores (default 1)
#   ALAYA_TIMEOUT=300       seconds before "test" gives up (default 300)
#   ALAYA_IMAGE_URL=...     download from somewhere else
#   ALAYA_DIR=...           where images are kept (default ~/alaya)

set -euo pipefail

REPO_URL="https://github.com/gahitchi/os"
IMAGE_URL="${ALAYA_IMAGE_URL:-$REPO_URL/releases/download/nightly/alaya-x86_64.iso}"
DIR="${ALAYA_DIR:-$HOME/alaya}"
CPUS="${ALAYA_CPUS:-1}"
TIMEOUT="${ALAYA_TIMEOUT:-300}"
OK_MARKER="alaya: boot ok"
PANIC_MARKER="alaya: panic"

say() { printf 'alaya.sh: %s\n' "$*" >&2; }
die() { say "$*"; exit 1; }

need_tools() {
    local missing=()
    command -v qemu-system-x86_64 >/dev/null || missing+=(qemu-system-x86-64-headless)
    command -v curl >/dev/null || missing+=(curl)
    [ ${#missing[@]} -eq 0 ] && return
    if command -v pkg >/dev/null; then
        say "installing ${missing[*]}"
        pkg install -y "${missing[@]}"
    else
        die "please install: qemu-system-x86_64 and curl"
    fi
}

fetch() {
    mkdir -p "$DIR"
    local image="$DIR/alaya-x86_64.iso"
    say "downloading $IMAGE_URL"
    curl -fL --retry 3 -o "$image.part" "$IMAGE_URL" || die "download failed"
    if curl -fsL -o "$image.sha256" "$IMAGE_URL.sha256" 2>/dev/null; then
        local want have
        want=$(cut -d' ' -f1 <"$image.sha256")
        have=$(sha256sum "$image.part" | cut -d' ' -f1)
        [ "$want" = "$have" ] || die "checksum mismatch, download again"
        say "checksum ok"
    fi
    mv "$image.part" "$image"
    echo "$image"
}

qemu_args() {
    # No KVM on a phone: plain TCG emulation. Slower, but enough for the kernel.
    printf '%s\n' -accel tcg -machine q35 -m 256M -smp "$CPUS" -no-reboot \
        -display none -monitor none -cdrom "$1"
}

run() {
    local image="$1"
    say "booting $image on $CPUS core(s)"
    say "after 'alaya: boot ok' the kernel just waits; quit with Ctrl+A, then lowercase x"
    say "(or Ctrl+A, then c, then type quit). 'alaya.sh test' exits on its own."
    mapfile -t args < <(qemu_args "$image")
    exec qemu-system-x86_64 "${args[@]}" -serial mon:stdio
}

test_boot() {
    local image="$1" log
    log=$(mktemp "${TMPDIR:-/tmp}/alaya-serial.XXXXXX")
    mapfile -t args < <(qemu_args "$image")
    say "boot test on $CPUS core(s), timeout ${TIMEOUT}s (TCG can take a while)"
    qemu-system-x86_64 "${args[@]}" -serial "file:$log" </dev/null &
    local pid=$! result=timeout start=$SECONDS
    while [ $((SECONDS - start)) -lt "$TIMEOUT" ]; do
        if grep -q "$OK_MARKER" "$log"; then result=pass; break; fi
        if grep -q "$PANIC_MARKER" "$log"; then result=panic; break; fi
        kill -0 "$pid" 2>/dev/null || { result=exited; break; }
        sleep 1
    done
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    tr -d '\r' <"$log" | sed 's/^/  | /'
    rm -f "$log"
    case "$result" in
        pass) say "PASS in $((SECONDS - start))s" ;;
        panic) die "FAIL: kernel panicked" ;;
        exited) die "FAIL: QEMU exited before the kernel booted" ;;
        *) die "FAIL: no boot marker within ${TIMEOUT}s" ;;
    esac
}

image_arg() {
    if [ -n "${1:-}" ]; then
        [ -f "$1" ] || die "no such image: $1"
        echo "$1"
    elif [ -f "$DIR/alaya-x86_64.iso" ]; then
        echo "$DIR/alaya-x86_64.iso"
    else
        fetch
    fi
}

cmd="${1:-run}"
shift || true
need_tools
case "$cmd" in
    run) run "$(image_arg "${1:-}")" ;;
    test) test_boot "$(image_arg "${1:-}")" ;;
    fetch) fetch >/dev/null ;;
    -h | --help | help) sed -n '4,17p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) die "unknown command $cmd (use run, test or fetch)" ;;
esac

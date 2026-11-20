#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Boot a ready-made Alaya image in QEMU from Termux on Android, or on any
# Linux PC with bash, curl and qemu-system-x86_64 (there it uses KVM when it
# can). Nothing is built here: the image comes from the project's CI.
#
#   alaya.sh run  [image]   boot and show the serial console (Ctrl+A then x quits)
#   alaya.sh test [image]   boot, pass if the kernel reports a clean boot
#   alaya.sh fetch          only download the latest image
#
# Options (environment):
#   ALAYA_CPUS=2            virtual cores (default 1); "all" = every host core
#   ALAYA_ACCEL=auto        kvm, tcg or auto (default: kvm if /dev/kvm is usable)
#   ALAYA_TIMEOUT=600       seconds before "test" gives up (default 600)
#   ALAYA_IMAGE_URL=...     download from somewhere else
#   ALAYA_DIR=...           where images are kept (default ~/alaya)

set -euo pipefail

REPO_URL="https://github.com/gahitchi/os"
IMAGE_URL="${ALAYA_IMAGE_URL:-$REPO_URL/releases/download/nightly/alaya-x86_64.iso}"
DIR="${ALAYA_DIR:-$HOME/alaya}"
CPUS="${ALAYA_CPUS:-1}"
ACCEL="${ALAYA_ACCEL:-auto}"
TIMEOUT="${ALAYA_TIMEOUT:-600}"
OK_MARKER="alaya: boot ok"
PANIC_MARKER="alaya: panic"
# Each core prints "core <n>: ready, ..." once it passed its self-tests.
READY_PATTERN='^core [0-9]+: ready'

if [ "$CPUS" = all ]; then
    CPUS=$(nproc)
fi
case "$CPUS" in
    '' | *[!0-9]*) printf 'alaya.sh: ALAYA_CPUS must be a number or "all"\n' >&2; exit 1 ;;
esac

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

kvm_usable() {
    [ -r /dev/kvm ] && [ -w /dev/kvm ]
}

# Decide once: "kvm" or "tcg".
resolve_accel() {
    case "$ACCEL" in
        kvm) kvm_usable || die "ALAYA_ACCEL=kvm, but /dev/kvm is missing or not accessible (are you in the kvm group? log out and back in after adding yourself)"
             ACCEL=kvm ;;
        tcg) ;;
        # A phone has no KVM: plain emulation (TCG), slower but enough.
        auto) if kvm_usable; then ACCEL=kvm; else ACCEL=tcg; fi ;;
        *) die "ALAYA_ACCEL must be kvm, tcg or auto" ;;
    esac
}

accel_name() {
    if [ "$ACCEL" = kvm ]; then echo "KVM"; else echo "emulation (TCG)"; fi
}

qemu_args() {
    if [ "$ACCEL" = kvm ]; then
        # The host CPU as it is: TSC-deadline timer, x2APIC, invariant TSC.
        printf '%s\n' -accel kvm -cpu host,+invtsc
    else
        printf '%s\n' -accel tcg
    fi
    # The device memory guard (IOMMU) and the small virtual disk the boot
    # checks use (milestone 4).
    local disk="$DIR/disk.img"
    if [ ! -f "$disk" ]; then
        mkdir -p "$DIR"
        head -c 1048576 /dev/zero >"$disk"
    fi
    printf '%s\n' -machine q35 -m 256M -smp "$CPUS" -no-reboot \
        -display none -monitor none \
        -device intel-iommu,aw-bits=48,intremap=off \
        -drive "file=$disk,if=none,id=disk0,format=raw" \
        -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on \
        -cdrom "$1"
}

run() {
    local image="$1"
    say "booting $image on $CPUS core(s) with $(accel_name)"
    say "after 'alaya: boot ok' the kernel just waits; quit with Ctrl+A, then lowercase x"
    say "(or Ctrl+A, then c, then type quit). 'alaya.sh test' exits on its own."
    mapfile -t args < <(qemu_args "$image")
    exec qemu-system-x86_64 "${args[@]}" -serial mon:stdio
}

test_boot() {
    local image="$1" log
    log=$(mktemp "${TMPDIR:-/tmp}/alaya-serial.XXXXXX")
    mapfile -t args < <(qemu_args "$image")
    say "boot test on $CPUS core(s) with $(accel_name), timeout ${TIMEOUT}s"
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
    local ready
    ready=$(tr -d '\r' <"$log" | grep -cE "$READY_PATTERN" || true)
    rm -f "$log"
    if [ "$result" = pass ] && [ "$ready" -ne "$CPUS" ]; then
        result=cores
    fi
    case "$result" in
        pass) say "PASS in $((SECONDS - start))s: all $CPUS core(s) ready" ;;
        cores) die "FAIL: only $ready of $CPUS core(s) reported ready" ;;
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
resolve_accel
case "$cmd" in
    run) run "$(image_arg "${1:-}")" ;;
    test) test_boot "$(image_arg "${1:-}")" ;;
    fetch) fetch >/dev/null ;;
    -h | --help | help) sed -n '4,17p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) die "unknown command $cmd (use run, test or fetch)" ;;
esac

#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-only
#
# Boot a ready-made Alaya image in QEMU from Termux on Android, or on any
# Linux PC with bash, curl and QEMU (there it uses KVM when it can). Nothing
# is built here: the image comes from the project's CI. Both the PC image
# and the ARM64 image (ALAYA_ARCH=aarch64) run in QEMU only.
#
#   alaya.sh run  [image]   boot and show the serial console (Ctrl+A then x quits)
#   alaya.sh test [image]   boot, pass if the kernel reports a clean boot
#   alaya.sh fetch          only download the latest image
#
# Options (environment):
#   ALAYA_ARCH=aarch64      boot the ARM64 image instead of the PC one
#   ALAYA_CPUS=2            virtual cores (default 1); "all" = every host core
#   ALAYA_ACCEL=auto        kvm, tcg or auto (default: kvm if /dev/kvm is usable)
#   ALAYA_TIMEOUT=600       seconds before "test" gives up (default 600)
#   ALAYA_IMAGE_URL=...     download from somewhere else
#   ALAYA_DIR=...           where images are kept (default ~/alaya)

set -euo pipefail

REPO_URL="https://github.com/gahitchi/os"
ARCH="${ALAYA_ARCH:-x86_64}"
case "$ARCH" in
    x86_64 | pc) ARCH=x86_64 ;;
    aarch64 | arm64 | arm) ARCH=aarch64 ;;
    *) printf 'alaya.sh: ALAYA_ARCH must be x86_64 or aarch64\n' >&2; exit 1 ;;
esac
IMAGE_NAME="alaya-$ARCH.iso"
QEMU="qemu-system-$ARCH"
IMAGE_URL="${ALAYA_IMAGE_URL:-$REPO_URL/releases/download/nightly/$IMAGE_NAME}"
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
    if [ "$ARCH" = aarch64 ]; then
        command -v "$QEMU" >/dev/null || missing+=(qemu-system-aarch64-headless)
    else
        command -v "$QEMU" >/dev/null || missing+=(qemu-system-x86-64-headless)
    fi
    command -v curl >/dev/null || missing+=(curl)
    [ ${#missing[@]} -eq 0 ] && return
    if command -v pkg >/dev/null; then
        say "installing ${missing[*]}"
        pkg install -y "${missing[@]}"
    else
        die "please install: $QEMU and curl"
    fi
}

fetch() {
    mkdir -p "$DIR"
    local image="$DIR/$IMAGE_NAME"
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

# ARM machines start from UEFI firmware, which comes with QEMU.
arm_firmware() {
    local f
    for f in "${ALAYA_AAVMF:-}" \
        "${PREFIX:-/usr}/share/qemu/edk2-aarch64-code.fd" \
        /usr/share/qemu-efi-aarch64/QEMU_EFI.fd \
        /usr/share/AAVMF/AAVMF_CODE.fd \
        /usr/share/qemu/edk2-aarch64-code.fd \
        /usr/share/edk2/aarch64/QEMU_EFI.fd; do
        if [ -n "$f" ] && [ -f "$f" ]; then
            echo "$f"
            return
        fi
    done
    die "no ARM UEFI firmware found (install qemu-efi-aarch64, or set ALAYA_AAVMF)"
}

disk_image() {
    local disk="$DIR/disk.img"
    # 64 MiB: the first MiB for the disk tests, the rest for the store.
    if [ ! -f "$disk" ] || [ "$(wc -c <"$disk")" -lt 67108864 ]; then
        mkdir -p "$DIR"
        truncate -s 64M "$disk"
    fi
    echo "$disk"
}

qemu_args() {
    if [ "$ARCH" = aarch64 ]; then
        if [ "$ACCEL" = kvm ]; then
            printf '%s\n' -accel kvm -cpu host
        else
            printf '%s\n' -accel tcg -cpu max,pauth-impdef=on
        fi
        # QEMU's generic ARM board: GICv3 with its message translator, the
        # SMMUv3 device memory guard, and the virtual disk.
        printf '%s\n' -machine virt,gic-version=3,iommu=smmuv3 -m 512M -smp "$CPUS" -no-reboot \
            -display none -monitor none -bios "$(arm_firmware)" \
            -drive "file=$(disk_image),if=none,id=disk0,format=raw" \
            -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on \
            -drive "file=$1,if=none,id=cd0,format=raw,readonly=on" \
            -device virtio-scsi-pci,id=scsi0 -device scsi-cd,drive=cd0,bus=scsi0.0,bootindex=0
        return
    fi
    if [ "$ACCEL" = kvm ]; then
        # The host CPU as it is: TSC-deadline timer, x2APIC, invariant TSC.
        printf '%s\n' -accel kvm -cpu host,+invtsc
    else
        printf '%s\n' -accel tcg
    fi
    # The device memory guard (IOMMU) and the virtual disk the boot
    # checks use (milestone 4).
    local disk
    disk=$(disk_image)
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
    exec "$QEMU" "${args[@]}" -serial mon:stdio
}

test_boot() {
    local image="$1" log
    log=$(mktemp "${TMPDIR:-/tmp}/alaya-serial.XXXXXX")
    mapfile -t args < <(qemu_args "$image")
    say "boot test on $CPUS core(s) with $(accel_name), timeout ${TIMEOUT}s"
    "$QEMU" "${args[@]}" -serial "file:$log" </dev/null &
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
    elif [ -f "$DIR/$IMAGE_NAME" ]; then
        echo "$DIR/$IMAGE_NAME"
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
    -h | --help | help) sed -n '4,19p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) die "unknown command $cmd (use run, test or fetch)" ;;
esac

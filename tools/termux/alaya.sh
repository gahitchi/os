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
#   alaya.sh plug [n]       pull the plug n times (default 3): each boot opens
#                           the saved files the last boot left, checks nothing
#                           promised was lost, keeps saving, and is then
#                           switched off at a random moment (milestone 5)
#   alaya.sh fetch          only download the latest images
#
# Options (environment):
#   ALAYA_ARCH=aarch64      boot the ARM64 image instead of the PC one
#   ALAYA_CPUS=2            virtual cores (default 1); "all" = every host core
#   ALAYA_ACCEL=auto        kvm, tcg or auto (default: kvm if /dev/kvm is usable)
#   ALAYA_TIMEOUT=600       seconds before "test" gives up (default 600)
#   ALAYA_IMAGE_URL=...     download from somewhere else
#   ALAYA_PLUG_IMAGE=...    use this pull-the-plug image (no download)
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
PLUG_NAME="alaya-$ARCH-plug.iso"
QEMU="qemu-system-$ARCH"
RELEASE_URL="$REPO_URL/releases/download/nightly"
IMAGE_URL="${ALAYA_IMAGE_URL:-$RELEASE_URL/$IMAGE_NAME}"
PLUG_URL="${ALAYA_PLUG_URL:-$RELEASE_URL/$PLUG_NAME}"
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

# fetch [name url]: download one image (default: the normal boot image).
fetch() {
    local name="${1:-$IMAGE_NAME}" url="${2:-$IMAGE_URL}"
    mkdir -p "$DIR"
    local image="$DIR/$name"
    say "downloading $url"
    curl -fL --retry 3 -o "$image.part" "$url" || die "download failed"
    if curl -fsL -o "$image.sha256" "$url.sha256" 2>/dev/null; then
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

# The backup stick (milestone 7): a second virtual disk, shown to Alaya
# the way a USB stick will be. 'alaya.sh test' starts with an empty one.
stick_image() {
    local stick="$DIR/stick.img"
    if [ "${FRESH_STICK:-0}" = 1 ]; then
        rm -f "$stick"
    fi
    if [ ! -f "$stick" ] || [ "$(wc -c <"$stick")" -lt 67108864 ]; then
        mkdir -p "$DIR"
        truncate -s 64M "$stick"
    fi
    echo "$stick"
}

stick_devices() {
    printf '%s\n' \
        -drive "file=$(stick_image),if=none,id=stick0,format=raw" \
        -device virtio-blk-pci,drive=stick0,disable-legacy=on,iommu_platform=on
}

# The screen card, keyboard and mouse (milestone 6), behind the IOMMU like
# the disk. Keep in sync with SEAT_DEVICES in xtask.
seat_devices() {
    printf '%s\n' \
        -device virtio-gpu-pci,id=gpu0,disable-legacy=on,iommu_platform=on,xres=1280,yres=800 \
        -device virtio-keyboard-pci,id=kbd0,disable-legacy=on,iommu_platform=on \
        -device virtio-tablet-pci,id=tablet0,disable-legacy=on,iommu_platform=on
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
        printf '%s\n' -machine virt,gic-version=3,iommu=smmuv3 -m 1G -smp "$CPUS" -no-reboot \
            -display none -monitor none -bios "$(arm_firmware)" \
            -drive "file=$(disk_image),if=none,id=disk0,format=raw" \
            -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on \
            $(seat_devices) \
            -drive "file=$1,if=none,id=cd0,format=raw,readonly=on" \
            -device virtio-scsi-pci,id=scsi0 -device scsi-cd,drive=cd0,bus=scsi0.0,bootindex=0 \
            $(stick_devices)
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
    printf '%s\n' -machine q35 -m 512M -smp "$CPUS" -no-reboot -vga none \
        -display none -monitor none \
        -device intel-iommu,aw-bits=48,intremap=off \
        -drive "file=$disk,if=none,id=disk0,format=raw" \
        -device virtio-blk-pci,drive=disk0,disable-legacy=on,iommu_platform=on \
        $(seat_devices) \
        $(stick_devices) \
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
    mapfile -t args < <(FRESH_STICK=1 qemu_args "$image")
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

# Pull the plug (milestone 5). A plug image opens the store the last boot
# left on the virtual disk and prints "soak: found C" (the counter it found
# in two files it keeps equal), then keeps changing both files and saving:
# "soak: saving C" before each save, "soak: promised C" once that save is
# safe. We switch QEMU off at a random moment and boot again. Every boot
# must find at least what was promised and nothing that was never saved.
plug_test() {
    local image="$1" boots="$2" log boot
    case "$boots" in '' | *[!0-9]* | 0) die "plug needs a number of boots, like: alaya.sh plug 3" ;; esac
    local promised='' saving=''
    say "pulling the plug $boots times on $CPUS core(s) with $(accel_name)"
    say "(each boot first runs all the boot checks again, so this takes a while)"
    for ((boot = 1; boot <= boots; boot++)); do
        log=$(mktemp "${TMPDIR:-/tmp}/alaya-plug.XXXXXX")
        mapfile -t args < <(qemu_args "$image")
        "$QEMU" "${args[@]}" -serial "file:$log" </dev/null &
        local pid=$! start=$SECONDS saves=$((1 + RANDOM % 12)) result=timeout found=''
        while [ $((SECONDS - start)) -lt "$TIMEOUT" ]; do
            if grep -qE "$PANIC_MARKER|FAILED" "$log"; then result=panic; break; fi
            if [ -z "$found" ]; then
                found=$(tr -d '\r' <"$log" | sed -n 's/.*soak: found \([0-9]*\) (.*/\1/p' | head -n1 || true)
            fi
            if [ -n "$found" ] && [ "$(grep -c 'soak: saving ' "$log")" -ge "$saves" ]; then
                # Inside a save or between two: a random 0-0.4 s later.
                sleep "0.$((RANDOM % 4))$((RANDOM % 10))"
                result=pulled
                break
            fi
            kill -0 "$pid" 2>/dev/null || { result=exited; break; }
            sleep 0.2
        done
        kill -9 "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        [ -n "$found" ] && tr -d '\r' <"$log" | grep -m1 'soak: found' | sed 's/^/  | /'
        case "$result" in
            pulled) ;;
            panic) tr -d '\r' <"$log" | tail -n 20 | sed 's/^/  | /'; rm -f "$log"
                   die "FAIL on boot $boot: Alaya reported a problem (see above)" ;;
            exited) tr -d '\r' <"$log" | tail -n 20 | sed 's/^/  | /'; rm -f "$log"
                    die "FAIL on boot $boot: QEMU stopped on its own" ;;
            *) tr -d '\r' <"$log" | tail -n 20 | sed 's/^/  | /'; rm -f "$log"
               die "FAIL on boot $boot: no progress within ${TIMEOUT}s" ;;
        esac
        if [ -n "$promised" ] && [ "$found" -lt "$promised" ]; then
            rm -f "$log"; die "FAIL on boot $boot: found $found, but save $promised was promised (a saved change was lost)"
        fi
        if [ -n "$saving" ] && [ "$found" -gt "$saving" ]; then
            rm -f "$log"; die "FAIL on boot $boot: found $found, but only $saving was ever saved"
        fi
        if [ -n "$promised" ]; then
            say "boot $boot: found save $found (promised $promised): nothing lost, good"
        else
            say "boot $boot: started with save $found"
        fi
        # What the boot printed before it was switched off. Only whole
        # lines count: a kill while printing "soak: saving 400" can leave
        # "soak: saving 40" as the last, unfinished line.
        local p s
        if [ -n "$(tail -c1 "$log")" ]; then
            sed -i '$d' "$log"
        fi
        p=$(tr -d '\r' <"$log" | sed -n 's/.*soak: promised \([0-9]*\).*/\1/p' | tail -n1)
        s=$(tr -d '\r' <"$log" | sed -n 's/.*soak: saving \([0-9]*\).*/\1/p' | tail -n1)
        promised="${p:-$found}"
        saving="${s:-$found}"
        rm -f "$log"
        say "boot $boot: plug pulled after save $promised was promised (save $saving was under way)"
    done
    # One more boot to check what the last pull left behind.
    say "one last boot to check what the last pull left"
    log=$(mktemp "${TMPDIR:-/tmp}/alaya-plug.XXXXXX")
    mapfile -t args < <(qemu_args "$image")
    "$QEMU" "${args[@]}" -serial "file:$log" </dev/null &
    local pid=$! start=$SECONDS found=''
    while [ $((SECONDS - start)) -lt "$TIMEOUT" ] && [ -z "$found" ]; do
        if grep -qE "$PANIC_MARKER|FAILED" "$log"; then break; fi
        kill -0 "$pid" 2>/dev/null || break
        found=$(tr -d '\r' <"$log" | sed -n 's/.*soak: found \([0-9]*\) (.*/\1/p' | head -n1 || true)
        sleep 0.2
    done
    kill -9 "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    if [ -z "$found" ]; then
        tr -d '\r' <"$log" | tail -n 20 | sed 's/^/  | /'; rm -f "$log"
        die "FAIL on the last boot: the saved files could not be opened"
    fi
    tr -d '\r' <"$log" | grep -m1 'soak: found' | sed 's/^/  | /'
    rm -f "$log"
    [ "$found" -ge "$promised" ] || die "FAIL: found $found, but save $promised was promised (a saved change was lost)"
    [ "$found" -le "$saving" ] || die "FAIL: found $found, but only $saving was ever saved"
    say "PASS: pulled the plug $boots times; nothing promised was lost"
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
    plug)
        if [ -n "${ALAYA_PLUG_IMAGE:-}" ]; then
            plug_image="$ALAYA_PLUG_IMAGE"
            [ -f "$plug_image" ] || die "no such image: $plug_image"
        elif [ -f "$DIR/$PLUG_NAME" ]; then
            plug_image="$DIR/$PLUG_NAME"
        else
            plug_image=$(fetch "$PLUG_NAME" "$PLUG_URL")
        fi
        plug_test "$plug_image" "${1:-3}" ;;
    fetch) fetch >/dev/null; fetch "$PLUG_NAME" "$PLUG_URL" >/dev/null ;;
    -h | --help | help) sed -n '4,24p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) die "unknown command $cmd (use run, test, plug or fetch)" ;;
esac

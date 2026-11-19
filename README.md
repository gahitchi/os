# Alaya

Alaya is a new operating system, written from scratch in Rust for x86-64. It is
built around a small capability microkernel (in the style of seL4) and a
signature idea called **history and grants**: every change on the machine is
kept and attributed, and apps only ever see what the person hands them, with
rights and a moment in time attached.

The full design is in [`docs/design/`](docs/design/), starting with
[`alaya-design.md`](docs/design/alaya-design.md). The build order is in
[`roadmap.md`](docs/design/roadmap.md).

## Status

**Milestone 1: the kernel boots on every core.** The kernel boots through
[Limine](https://github.com/limine-bootloader/limine) on BIOS and UEFI,
builds its own page tables from the memory map (with 2 MiB and 1 GiB pages
and guarded per-core stacks), catches every CPU exception with a full report
on the serial port, measures the TSC and drives the TSC-deadline timer, and
brings up every core under the big kernel lock. Each core then runs a short
self-test, and the kernel prints `alaya: boot ok`. CI boots it on 1, 2 and 8
virtual cores, both emulated and with KVM, and checks that every core
reports in.

## Try it

### On a PC (Linux)

Needs Rust (the version in `rust-toolchain.toml` is installed automatically by
rustup), `git`, `make`, a C compiler, `xorriso` and `qemu-system-x86_64`. On
Debian or Ubuntu:

```sh
sudo apt install build-essential git xorriso qemu-system-x86 ovmf
cargo xtask run              # build the image and boot it, serial on this terminal
cargo xtask test --cpus 8    # boot and check every core came up
cargo xtask run --uefi       # boot with UEFI firmware instead of BIOS
cargo xtask test --fault page  # crash on purpose and check the fault report
cargo xtask image            # only build build/alaya-x86_64.iso
```

`cargo xtask --help` lists every option. Press Ctrl+A then x (lowercase) to leave QEMU.
QEMU uses KVM when `/dev/kvm` is usable (`--accel kvm|tcg|auto` to choose).
Testing with KVM on a PC, including without building anything:
[`docs/testing-on-a-pc.md`](docs/testing-on-a-pc.md).

### On an Android phone

No build needed: see [`docs/termux.md`](docs/termux.md).

```sh
pkg install qemu-system-x86-64-headless curl
curl -fLO https://raw.githubusercontent.com/gahitchi/os/main/tools/termux/alaya.sh
bash alaya.sh run
```

## Layout

| Path | What it is |
| --- | --- |
| `kernel/` | The kernel (`no_std`, target `x86_64-unknown-none`) |
| `xtask/` | Build, image and QEMU runner (`cargo xtask`) |
| `boot/limine.conf` | Bootloader configuration |
| `tools/termux/` | Script to boot a CI image from Termux or any Linux |
| `docs/design/` | The design and its decision log |
| `.github/workflows/ci.yml` | CI: lint, build, boot tests, image publishing |

## License

Alaya is licensed under the [GNU GPL, version 3](LICENSE). System libraries
that apps link against carry an additional
[linking exception](LICENSE-EXCEPTION.md), so apps and games may use any
license. The kernel, servers and drivers are GPL-3.0 only.

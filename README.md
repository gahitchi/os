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

**Milestone 0: repository and tools.** The kernel boots through
[Limine](https://github.com/limine-bootloader/limine) in QEMU, on BIOS and
UEFI, reports the cores and memory it was given, and prints `alaya: boot ok`
on the serial port. CI builds it and boots it on 1, 2 and 8 virtual cores.

## Try it

### On a PC (Linux)

Needs Rust (the version in `rust-toolchain.toml` is installed automatically by
rustup), `git`, `make`, a C compiler, `xorriso` and `qemu-system-x86_64`. On
Debian or Ubuntu:

```sh
sudo apt install build-essential git xorriso qemu-system-x86 ovmf
cargo xtask run              # build the image and boot it, serial on this terminal
cargo xtask test --cpus 8    # boot and check the kernel came up
cargo xtask run --uefi       # boot with UEFI firmware instead of BIOS
cargo xtask image            # only build build/alaya-x86_64.iso
```

`cargo xtask --help` lists every option. Press Ctrl+A then x (lowercase) to leave QEMU.

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

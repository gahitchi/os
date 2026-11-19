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

**Milestone 2: capabilities and memory.** On top of milestone 1 (Limine boot
on BIOS and UEFI, the kernel's own page tables, fault reports, the
TSC-deadline timer, every core up under the big kernel lock), the kernel now
hands every free byte of memory to the first program as untyped capabilities
and runs it in user mode. The program retypes memory into capability tables,
endpoints, notifications and pages, copies, mints, moves, deletes and
revokes capabilities through system calls, and checks the results. Long
revokes and retypes are interruptible: the kernel stops them after a short
slice and the call restarts by itself. CI boots it on 1, 2 and 8 virtual
cores, emulated and with KVM.

The capability rules were checked before the code was written: a
[TLA+ model](formal/revoke/Revoke.tla) of revoke on 2 and 3 cores (TLC checks
that nothing names freed memory and that every revoke ends), and
[Kani proofs](cap/src/proofs.rs) of the capability code itself. How it all
works: [`docs/capabilities.md`](docs/capabilities.md).

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
cargo test -p alaya-cap      # capability code tests on the host
formal/check.sh              # TLA+ model checks (needs Java)
cargo kani -p alaya-cap      # Kani proofs (needs cargo install kani-verifier)
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
| `cap/` | Capability tables, untyped memory, retype and revoke (used by the kernel; tested on the host and with Kani) |
| `abi/` | Numbers and layouts shared by the kernel and programs |
| `user/init/` | The first program |
| `user/sys/` | System call wrappers for programs |
| `formal/` | TLA+ models, checked with TLC (`formal/check.sh`) |
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

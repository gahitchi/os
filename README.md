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

**Milestone 4: the engine.** Drivers run as ordinary programs, each locked to
its own device by the processor's device memory guard (IOMMU), and each looked
after by a *keeper*. A driver can crash, freeze or be replaced by a new version
while apps use the disk, and the apps never notice: the keeper holds every
unfinished job and has it sent again. A driver that keeps crashing is switched
off with a clear error, and a failed update goes back to the old version by
itself. Programs are started from a file, find each other by name and get
memory from a helper that keeps each app within its budget. Games can give
their work a deadline (the next screen refresh) that travels with every call
they make.

Before the code: a [TLA+ model](formal/keeper/Keeper.tla) of the keeper's
crash-and-resend rule. After it: 43 boot checks on 1 to 8 cores, a 10-minute
run that kills the driver at random every half second, 1,000 live updates in a
row, and a fuzzer that now attacks the helpers too. How it all works:
[`docs/engine.md`](docs/engine.md).

Milestone 3 (threads, time budgets and messages) is described in
[`docs/threads-and-messages.md`](docs/threads-and-messages.md), with its own
[TLA+ model](formal/sched/Sched.tla).
Milestone 2 (capabilities and memory) is described in
[`docs/capabilities.md`](docs/capabilities.md), with its own
[TLA+ model](formal/revoke/Revoke.tla) and [Kani proofs](cap/src/proofs.rs).

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
cargo xtask test --fuzz 60   # boot checks, then a minute of kernel fuzzing
cargo xtask test --cpus 8 --long  # also kill the disk driver at random for 10 minutes
cargo test -p alaya-cap -p alaya-sched  # capability and time-budget tests on the host
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
| `sched/` | Time budgets with sporadic refills (used by the kernel; tested on the host) |
| `abi/` | Numbers and layouts shared by the kernel and programs |
| `user/init/` | The first program: the boot checks and the kernel fuzzer |
| `user/sys/` | System call wrappers, the program starter and the small runtime for programs |
| `user/programs/` | Programs in the boot image: the driver keeper, the disk driver (two versions) and a disk test app |
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

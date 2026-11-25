# Alaya

Alaya is a new operating system, written from scratch in Rust for PCs (x86-64)
and ARM64. It is
built around a small capability microkernel (in the style of seL4) and a
signature idea called **history and grants**: every change on the machine is
kept and attributed, and apps only ever see what the person hands them, with
rights and a moment in time attached.

The full design is in [`docs/design/`](docs/design/), starting with
[`alaya-design.md`](docs/design/alaya-design.md). The build order is in
[`roadmap.md`](docs/design/roadmap.md).

## Status

**Milestone 8 (in progress): apps that are not built in.** Apps come as
signed bundles you install and approve line by line; updates can never take
more on their own, go on trial, and slip back by themselves when clearly
slower; a rehearsal runs a new app on a copy of your files first. An app can
get an answer instead of your file ("the total of column 2": one number,
never the document), a full-screen game freezes what runs in the
background, notes from a task wait for you in that task, and the editor
hands a document to another app at the same line. How it works:
[`docs/apps.md`](docs/apps.md) and [`docs/sdk.md`](docs/sdk.md).

**Milestone 7, step 1: going back.** One undo for the whole computer:
the Alaya key plus Z, or the Undo button in the top strip, undoes your last
action anywhere (a deleted file, a setting, a closed window), and asks first
if something else changed it since. "What happened?" lists changes in
plain words with an undo on each line; the timeline shows your documents
as they were at any kept moment, brings one back and names versions; the
file browser tells each file's story. Underneath, the store gives every
file its own lock key (so a forgotten file can never be read again) and
learns side branches (checks 76 to 84). How it works:
[`docs/going-back.md`](docs/going-back.md).

**Milestone 6, step 4 (6c): "is this still up to date?"** The first show
of living validity, something no other system does. The chart maker tells
Alaya what its picture and summary were made from; the store checks that
it really read all of that file. Change the numbers in the text editor and
the file browser marks the chart and the summary "Needs a look"; save the
chart again and they are "Up to date". A result made from part of a file,
or from a file that changed while it was read, is never "Up to date"
(checks 72 to 75). How it works: [`docs/desktop.md`](docs/desktop.md).

**Milestone 6, step 3 (6b): people and apps.** First setup and a login
screen with a PIN box no app can fake: your own picture shows in the top
strip only while the real one asks for your PIN. The PIN becomes your key
through a slow, memory-heavy step. A text editor that saves and comes back
exactly where you were, a file browser, a chart maker, Settings with two
looks, and loose window groups. After its checks the first program keeps
restarting crashed drivers and apps (checks 59 to 71). How it works:
[`docs/desktop.md`](docs/desktop.md).

**Milestone 6, step 2: the first desktop.** Alaya draws its own desktop:
windows you move, resize, snap to the screen's edges and tile, a dock, and
a strip at the top that no app can draw on. Apps are built with Alaya's
own toolkit, which reads font files and draws letters itself, and writes
down every button and text box for screen readers. The first app is a
small text editor. `cargo xtask test` drags a window and types into it
through QEMU (checks 59 to 63). How it works:
[`docs/desktop.md`](docs/desktop.md).

**Milestone 6, step 1: screen, keyboard and mouse.** Alaya has its own
drivers for QEMU's virtual screen card, keyboard and mouse, each locked to
its own memory. The driver never owns the screen: the pixels belong to the
program that draws, so a crashed driver comes back and the picture with it.
`cargo xtask test` checks a screenshot, types a key and clicks the mouse
through QEMU, and crashes every driver once (checks 54 to 58, on PCs and
ARM64).

**Milestone 5: safe storage.** The Alaya store keeps files so that a power
cut at any moment leaves every file either fully saved or exactly as it was
before. It never writes over anything: a save writes the changes to free
places, waits until the disk has them, and only then writes a small front
page that points at them. Every block is scrambled (each person under their
own key) and fingerprinted, so damage is noticed and, for the store's own
map, repaired from a spare copy. Every save records which app changed which
file, and which app read which version of a document. Documents keep their
history, apps one step back, caches none, and a full disk can always delete.

Before the code: a [TLA+ model](formal/commit/Commit.tla) of a commit with
power cuts and a damaged front page. After it: a power cut at every single
disk write and flush in the host tests (3,168 cuts), every block damaged in
turn, ten new boot checks (44 to 53) on x86-64 and ARM64, and `cargo xtask
test --plug <n>`, which kills QEMU at random moments and boots again from
the same disk. How it works: [`docs/store.md`](docs/store.md).

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

**Milestone 4b: the processor wall.** Everything in the kernel that only
makes sense on one kind of processor sits in one folder behind a fixed list
of things every processor must provide (`kernel/src/arch/`), and a CI check
keeps the wall whole. Behind it there are now two: the PC (x86-64) and
ARM64. The ARM64 build passes the same 43 boot checks, fault reports and
fuzzer in QEMU's ARM board, with ARM's interrupt controller (GICv3 and its
ITS) and device memory guard (SMMUv3). It is tested in QEMU only. How it
works: [`docs/processor-wall.md`](docs/processor-wall.md).

Milestone 3 (threads, time budgets and messages) is described in
[`docs/threads-and-messages.md`](docs/threads-and-messages.md), with its own
[TLA+ model](formal/sched/Sched.tla).
Milestone 2 (capabilities and memory) is described in
[`docs/capabilities.md`](docs/capabilities.md), with its own
[TLA+ model](formal/revoke/Revoke.tla) and [Kani proofs](cap/src/proofs.rs).

## Try it

### On a PC (Linux)

Needs Rust (the version in `rust-toolchain.toml` is installed automatically by
rustup), `git`, `make`, a C compiler, `clang` (for Alaya's own C library), `xorriso` and `qemu-system-x86_64`. On
Debian or Ubuntu:

```sh
sudo apt install build-essential git xorriso clang qemu-system-x86 ovmf
cargo xtask run              # build the image and boot it, serial on this terminal
cargo xtask test --cpus 8    # boot and check every core came up
cargo xtask run --uefi       # boot with UEFI firmware instead of BIOS
cargo xtask test --fault page  # crash on purpose and check the fault report
cargo xtask image            # only build build/alaya-x86_64.iso
cargo xtask test --fuzz 60   # boot checks, then a minute of kernel fuzzing
cargo xtask test --cpus 8 --long  # also kill the disk driver at random for 10 minutes
cargo test -p alaya-cap -p alaya-sched  # capability and time-budget tests on the host
cargo xtask test --arch aarch64 --cpus 4  # the ARM64 build, in QEMU's ARM board
                             # (needs qemu-system-arm and qemu-efi-aarch64)
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
| `store/` | The Alaya store: files, people, history, encryption and safe saving (used by the store server; tested on the host) |
| `abi/` | Numbers and layouts shared by the kernel and programs |
| `user/init/` | The first program: the boot checks and the kernel fuzzer |
| `user/sys/` | System call wrappers, the program starter and the small runtime for programs |
| `user/programs/` | Programs in the boot image: the driver keeper, the disk driver (two versions), a disk test app and the store server |
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

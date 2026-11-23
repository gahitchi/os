# The engine (milestone 4)

Milestone 4 turns the kernel of milestones 2 and 3 into a system that looks
after itself. Drivers run as ordinary programs, locked to their own device;
a driver can crash, freeze or be replaced while apps use it, and the apps
never notice. The parts that later make up the system's own start (finding
devices, starting programs, handing out memory, letting programs find each
other) run for the first time.

This page explains how it works. The choices behind it are decisions 54,
55, 70 and 77 to 81 in [`design/decisions.md`](design/decisions.md).

## The foundation (decision 77)

Before the new parts, the kernel gained what every later milestone needs:

- **Processor safety switches.** SMEP (the kernel never runs program code),
  SMAP (the kernel touches program memory only where it means to) and UMIP
  (programs cannot read the kernel's descriptor tables). `cargo xtask test
  --fault smap` and `--fault smep` crash the kernel on purpose to show them
  working.
- **Tagged memory maps (PCID).** Each address space has a tag, so switching
  between programs no longer throws away the processor's cache of address
  translations. A core that had a program's map loaded learns of changes to
  it the next time it switches to it.
- **Guessing-leak guards.** On processors that need them: enhanced IBRS,
  a branch-predictor barrier (IBPB) when a core switches into or out of a
  *guarded* program (one holding secrets, such as the store; decision 107:
  `SPACE_GUARD`), a refill of the return-address predictor on every switch
  between programs, buffer clearing (VERW) on the way back to
  a program, and a fence after every handle bounds check.
- **Format labels.** Every kernel object carries a small header with its
  format number, and `kernel/src/kstate.rs` lists every piece of kernel-wide
  state, saying which would be carried over to a new kernel and which
  rebuilt. This is the groundwork for replacing the kernel while it runs,
  much later.
- **The big kernel lock, measured.** The kernel still has one lock. Check 28
  makes every core call a server as fast as it can and reports how often the
  lock was busy, so the day it needs splitting is a measurement, not a
  guess.

## Devices

### The device memory guard (IOMMU)

A device can write anywhere in memory by itself (DMA). A crashed or
malicious driver could point it at the kernel. So the kernel turns on the
IOMMU (Intel VT-d; in QEMU, `-device intel-iommu`) at boot, before any
program runs: from then on a device reaches **nothing** until it is given
something.

An **I/O space** is a capability-backed map of what one device may reach,
like an address space for a program. The device manager (for now the first
program) binds a PCI device to its own I/O space; whoever holds the I/O
space maps pages into it at device addresses of their choosing. A device
outside every I/O space can reach no memory at all.

The kernel finds the IOMMU and the PCI configuration space in the ACPI
tables (DMAR and MCFG, `kernel/src/acpi.rs`). Interrupt remapping is not
used yet; interrupts are already safe because a program cannot program a
device's interrupt address to anything but what the kernel hands out (see
below), and remapping comes with real hardware.

### Device registers

The **device control** capability makes page capabilities for physical
addresses that are not memory: a device's registers. The kernel refuses
RAM and its own devices (the local APIC and the IOMMU). Such pages are
mapped uncached.

### Interrupts as messages

The **interrupt control** capability hands out a free interrupt vector as an
**interrupt** capability. A driver points it at one of its notifications;
when the device raises the interrupt, the driver's notification gets its
bits, so a driver waits for its device the way it waits for any message.
`IRQ_MSI` tells the driver what to write into the device's MSI or MSI-X
table. Old pin-based interrupts are not used.

## Starting programs

The **program starter** (`user/sys/src/program.rs`) builds a program from an
ELF file and **one untyped memory capability**: its table, address space,
page tables, thread, code, stack and the memory it gets for itself all come
from it. So ending a program and taking back everything it had is one
revoke of that untyped, which is exactly how a driver is restarted.

A started program gets a start page with its name, its arguments, how much
memory it owns and which capabilities it was given. `user/sys/src/rt.rs`
is the small runtime around that: an entry point, printing with the
program's name in front, a crash on panic (so whoever looks after it sees
it), threads, memory and a clock.

The boot image now carries more than the first program: the keeper, two
versions of the disk driver and a disk test app, in
`user/programs/`.

## The registry

Programs find each other by name. A program registers a name with a
capability (for example the keeper registers `disk` with a send-only
copy of its endpoint); another looks the name up and gets a copy. Today the
registry is a thread of the first program; it moves into the system's own
start later.

## The memory helper

The memory helper hands out memory in **big chunks** (untyped memory, 4 KiB
to 64 MiB) and **huge pages** (2 MiB, for games' big buffers), and keeps
every app within its **budget**.

Each app has an **account**: an untyped block the helper keeps, the size of
the app's budget. What the app gets is made from its account and **moved**
to it (untyped memory has one owner at a time, so a capability to it can
only be moved, never copied; messages can now move a capability). The
helper counts what each account gave out and refuses what would go over
the budget. Because everything an app got comes from its account, one
revoke of the account takes all of it back when the app ends.

## The driver keeper

A **keeper** looks after one driver (decisions 70, 78 to 80). Apps never
talk to the driver, only to the keeper:

```
  app ──call──▶ keeper ──job page──▶ driver ──registers──▶ device
                  ▲  (keeper's memory)    │                   │
                  └──────── done ◀────────┘     DMA only to the keeper's
                                                 pages, in the I/O space
```

- **The job page.** The keeper writes each request into a slot of a page it
  shares with the driver and keeps the app's reply object. The driver sends
  each waiting job to the device once and marks it done. The ring the
  device reads, the request headers and the data buffers are also the
  keeper's pages, and the only memory in the device's I/O space. So a
  driver crash loses nothing: none of the state that matters lives in the
  driver.
- **Crashes.** A driver fault arrives at the keeper as a message. The keeper
  ends the driver (one revoke), starts a new one from the same image, and
  the new driver resets the device and finds every unfinished job still in
  the job page: it sends them all again (decision 79). A disk job done twice
  changes nothing (the same data is written again), so apps get their
  answers as if nothing happened.
- **Freezes.** A watchdog: if jobs wait and nothing finishes for two
  seconds, the keeper treats the driver as crashed.
- **The crash rule (decision 78).** More than three restarts within a
  minute switch the device off: waiting and new requests get a clear
  "device off" answer, and a note says so. "Try again" switches it back on.
- **Live updates (decision 80).** The keeper asks the running driver to
  park: it finishes what it is doing, writes its state into a **handover
  page** (a versioned format: the ring positions and which job it sent to
  the device when) and waits. The new version starts, reads the handover
  page and takes the running device over **without resetting it**. The old
  version stays parked for a **ten-minute trial**; if the new one crashes in
  that time, the old one comes back by itself, resets the device and sends
  every unfinished job again, so nothing is lost. After the trial the old
  version is ended.

The two driver versions in the boot image (`blk1`, `blk2`) are the same
code with a different version number; they understand the same handover
format, so the tests can update back and forth.

The keeper's rules are also checked as a [TLA+ model](../formal/keeper/Keeper.tla):
in every ordering of app requests, device work, crashes, restarts and
updates, apps never get a wrong answer, the disk always agrees with what
apps were told, and every request is answered. The model has teeth: a
restart that does not reset the device lets an old duplicate job finish a
newer one (caught), and a keeper that drops unfinished jobs leaves apps
waiting (caught).

## Deadlines and time

### Frame deadlines (decisions 70 and 81)

A scheduling context can carry a **deadline** (`SC_DEADLINE`): when the work
it pays for is due, for a game the next screen refresh. Among threads of
**equal priority**, the one whose deadline is soonest runs first (earliest
deadline first). **Priority always wins**: a game's deadline can never push
aside a more important system part.

The deadline belongs to the time budget, not to the thread, so it **travels
with calls**: a server running on its caller's lent time is ordered by its
caller's deadline. A chain of programs working on one frame (input, game,
GPU driver, compositor) is ordered by that frame's deadline end to end.

When a deadline passes before the work is done, the kernel records the miss
and **which thread held the budget at that moment**: the link in the chain
that was late. The compositor and the GPU driver that use this come in
milestones 6 and 9.

### Timeout messages (decision 55 follow-up)

A server running on its caller's lent time that runs out waits for the
caller's next refill; the slow caller pays for itself (decision 55). Now a
server can also ask (`THREAD_TIMEOUTS`) to have its handler told: a
**timeout message** goes to its fault endpoint, saying how much time it used
and how long until the refill. When the handler answers, the server goes
on waiting as before. Without the request nothing changes.

### The core-type helper (decision 54)

On processors with performance and efficiency cores, the kernel reads each
core's kind from the processor (Intel's hybrid information) and the helper
(`user/sys/src/cores.rs`) chooses where each budget may run: games and what
the person is using on performance cores, background work on efficiency
cores, system parts anywhere. In a virtual machine every core looks the
same, so every budget may use any core. Intel's finer, changing hints (the
hardware feedback table) need the kernel to read them and come with the
real-hardware milestones.

## How it is tested

`cargo xtask test --cpus 8` runs 43 boot checks. The milestone 4 ones:

| Check | What it shows |
| --- | --- |
| 26 | A program cannot read the kernel's tables (UMIP) |
| 27 | Every kernel object carries its format label |
| 28 | The big kernel lock measured with every core making calls |
| 29 | PCI scan: the disk is found, locked to its own I/O space, given an interrupt |
| 30 | The memory helper gives memory and keeps each account within its budget |
| 31 | The keeper, the driver and the test app run as separate programs |
| 32 | An app finds the memory helper in the registry, gets a huge page, is held to its budget |
| 33 | The IOMMU blocks a device pointed at memory it was not given |
| 34 | Random driver crashes and freezes while the app uses the disk: no wrong answer |
| 35 | A driver that keeps crashing is switched off with a clear error, and comes back on "try again" |
| 36 | The driver updated 1,000 times in a row while in use: no error, no device reset |
| 37 | A broken update goes back to the old version by itself, with nothing lost |
| 38 | After an update the old version is kept for ten minutes |
| 39 | Soonest deadline first among equals; priority always wins |
| 40 | Deadlines travel with calls |
| 41 | A missed deadline is recorded with the thread that missed it |
| 42 | A server out of lent time tells its handler, then waits for the refill |
| 43 | The core-type helper's choices |

Check 34 kills the driver for 8 seconds in a normal run. `cargo xtask test
--cpus 8 --long` kills it for **10 minutes**, on and off, by crashing it at
a random instruction or freezing it, every 0.4 to 0.9 seconds (CI runs this
with KVM). For these tests the crash rule's minute is shortened to a
second, so the driver can be killed often without the device being
switched off; check 35 tests the rule itself.

`cargo xtask test --fuzz <seconds>` now also attacks the helpers: the
fuzzing threads get send-only copies of the registry's, the memory helper's
and the keeper's endpoints and throw calls at them with labels and words
they know, or nearly. Afterwards the registry must still find the disk, the
keeper must still read it, and the memory helper's books must still add up.

`formal/check.sh` checks the keeper model along with the earlier ones.

## What is not there yet

- One kind of driver (the virtual disk) and one keeper per device. More
  drivers come with the milestones that need them (display, input, sound,
  network).
- The registry and the memory helper run inside the first program. They
  become separate programs when the system's own start replaces the boot
  checks.
- The IOMMU's interrupt remapping is off; it comes with real hardware.
- The kernel still has one big lock (see check 28's numbers).

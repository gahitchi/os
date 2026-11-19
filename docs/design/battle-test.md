# Battle test of the design

19 November 2026. Every decision in `decisions.md` and the proposal in `concept-history-and-grants.md`, tested against three requirements the person set:

1. **A new kind of OS**, clearly different from Linux, Windows and macOS.
2. **Stable under stress**: no stalls, freezes or data loss when the machine is loaded.
3. **Runs games**: fast graphics, steady frame times, low input and audio latency.

Each item gets a verdict: **keep**, **keep with rules** (the decision stands, but the rules listed become part of it), or **change** (the decision should be revised). Changes are proposals until the person accepts them.

## Summary

| Verdict | Decisions |
| --- | --- |
| Keep | 1 kind of OS, 2 scope, 3 x86-64, 4 Rust, 6 Limine, 7 capabilities, 14 compositor, 15 own filesystem |
| Keep with rules | 5 microkernel, 8 messages, 9 memory, 12 program interface, 13 desktop first, 16 copy-on-write, history and grants |
| Change | 10 one core first, 11 fixed priorities, 19 drivers, 20 license |
| Not decided yet, and needed | GPU stack, game compatibility target, memory pressure, timers, audio, input, networking, testing |

The three findings that matter most:

- **Games make the GPU the most important driver**, and we have not chosen one. Without hardware-accelerated Vulkan, no modern game runs. This is the biggest piece of work in the whole OS.
- **Stable frame times need CPU budgets and several cores.** Fixed priorities alone (11) and one core (10) both fail under game load.
- **History must not apply to everything.** Keeping every version of a 100 GB game install would fill the disk in one update. Storage needs classes.

## The decisions, one by one

### 1. Own kernel, QEMU first: keep

- In QEMU, games need accelerated graphics. QEMU's virtio-gpu with Venus passes Vulkan calls through to the host GPU, which is enough to develop and test the graphics stack before real GPU drivers exist.
- Rule: every milestone must also boot on one real reference PC once real drivers exist, so QEMU never hides a timing bug.

### 2. Scope (no website, no microcontroller, nothing from the old spec): keep

### 3. x86-64: keep

Games ship as x86-64 binaries. This is the only sensible first target for games.

### 4. Rust: keep with rules

- Rule: the kernel never panics on anything a program sends it. Bad input gets an error code, never a crash.
- Rule: `unsafe` stays in small, named modules (page tables, context switch, device registers), each with a written reason.
- Risk: the GPU user-space stack will be C (see GPU below), so the OS cannot be Rust-only. That is fine. The kernel and system servers stay Rust.

### 5. Microkernel: keep with rules

The fear is that microkernels are slow because every request is a message. For games, that fear is wrong if one rule is kept.

- **Rule: control goes through messages, data goes through shared memory.** A game never sends a message per draw call. It writes GPU commands into a buffer shared with the GPU driver and rings a doorbell (a notification). The same pattern serves audio, input and disk streaming.
- Rule: a crashed driver restarts without a reboot (the restartable drivers idea, kept from the earlier list). A GPU driver crash must cost a black frame, not the session.
- Risk: Spectre-era protections make every switch between programs more expensive. Mitigation: use PCID (tagged address spaces) so switches do not flush the TLB, and keep the number of switches per frame small and measured.

### 6. Limine: keep

Limine boots on UEFI and BIOS and hands over the framebuffer and memory map. Secure Boot support can come later.

### 7. Capabilities: keep with one rule

- Rule: **revoking a capability must be interruptible.** Taking back a capability that was copied thousands of times can take a long time. If the kernel does it in one go, everything freezes. seL4 solves this with preemption points; we must too.

### 8. Calls plus notifications: keep with rules

- Rule: shared-memory rings with notifications for every stream (GPU commands, audio, input, disk). See 5.
- **Rule: a server works at its caller's priority, on its caller's time budget.** Otherwise a low-priority program can make the file server busy and starve a high-priority game: priority inversion. This needs "scheduling contexts" that move with a call, which is exactly why decision 11 must change.

### 9. Memory decided in user space (seL4 style): keep with rules

Games allocate gigabytes, map and unmap constantly, and suffer from TLB misses.

- Rule: **huge pages** (2 MB and 1 GB) are supported from the start, both in the kernel's page tables and in the memory server's allocator. Games and GPU buffers need them.
- Rule: the memory server hands out memory in large chunks that a program then divides itself, so a game's allocation does not cost a message.
- Rule: a program can ask for its memory to be filled in up front (prefault), so a game never stalls mid-frame on a page fault.
- Gap: **what happens when memory runs out** is not decided. See "Not decided yet".

### 10. One core first: change

**Proposal: design for many cores from day one; the first milestone may run on one.**

- Games run a render thread, a game thread, audio and asset streaming at once. On one core, frame times collapse.
- Adding multi-core later means rewriting the scheduler, the capability code and every server's locking. That rewrite is where instability comes from.
- How: one big lock around the kernel. This sounds slow, but a microkernel spends very little time inside the kernel, and seL4 does the same. Each core keeps its own run queue. Servers are written thread-safe from the start.

### 11. Fixed priorities only: change

**Proposal: fixed priorities plus time budgets (scheduling contexts, as in seL4's MCS version).**

A budget says "this thread may run up to X microseconds every Y milliseconds". It fixes four problems that fixed priorities alone cannot:

1. A stuck high-priority driver can no longer freeze the desktop. It uses up its budget and waits.
2. Audio gets a guaranteed slice every few milliseconds, so it never crackles under load.
3. A game and the compositor can be given guaranteed time per frame, which gives steady frame pacing.
4. Servers run on their caller's budget (see 8), which ends priority inversion.

This adds one kernel object, the scheduling context. It is the single most important change for "stable under stress".

### 12. Native API now, POSIX later: keep with rules

- Rule: the POSIX layer offers `posix_spawn` but not `fork()`, which does not fit capabilities.
- Rule: threads, futex-style waiting, high-resolution clocks and memory mapping are in the native API from the start. Game engines and Vulkan drivers need all four.
- Games depend on which compatibility target we choose. See question 1.

### 13. Graphical desktop first: keep with rules

- Rule: the desktop is GPU-accelerated from the start. A software-drawn desktop cannot host games.
- Risk: graphics is the longest road. The first desktop will run in QEMU on virtio-gpu.

### 14. Compositor with shared buffers: keep with rules

This is the right model for games. The rules make it fast:

- **Direct scanout.** A full-screen game's buffer goes straight to the display. The compositor steps aside and adds no frame of delay.
- **Explicit sync.** Buffers carry fences that say when the GPU has finished, so the compositor never waits blindly or shows half-drawn frames.
- **Variable refresh rate (FreeSync / G-Sync compatible) and tearing control** on request.
- GPU buffers are passed as capabilities, which fits decision 7 exactly.

### 15. Our own filesystem: keep

### 16. Copy-on-write: keep with rules

Copy-on-write fails in three known ways under stress. Each needs a rule:

| Failure | What happens | Rule |
| --- | --- | --- |
| Rewriting the middle of big files (game patches, databases, VM images) | Every small write moves blocks; the file fragments; reads slow down | Big files can be marked **in-place** (like btrfs's nodatacow): no history, no fragmentation. Plus background defragmentation |
| Disk almost full | Copy-on-write needs free space even to delete, so a full disk can deadlock | Keep a reserved pool (for example 2 %) that only deletes and the cleanup may use |
| Many small writes | Each commit writes metadata again | Commit in batches every few seconds, and immediately on an explicit sync |

Data integrity: every block is checksummed, so silent corruption is detected rather than returned.

### 17 and 18. Shared tree and rights on handles

Replaced by the history-and-grants concept if it is adopted. Tested below.

### 19. Virtio and real PC drivers from the start: change

**Proposal: virtio first, then one real reference PC, chosen by the person.**

- Real PC hardware for games means: an NVMe disk, USB (keyboard, mouse, controllers, headsets), HD Audio, Ethernet or Wi-Fi, and above all **one GPU family**. PS/2 and AHCI are not enough on a modern PC.
- Each of these is large. The USB stack and the GPU driver are each bigger than the rest of the kernel.
- Writing two driver sets from day one halves our speed on the part that matters most.

### 20. GPL-3.0: change

**Proposal: GPL-3.0 for the kernel, servers and drivers, plus a linking exception for the system libraries apps link against.**

- Without the exception, every game, launcher and app built for this OS would have to be GPL. No commercial game would ship.
- Linux uses the same idea (its "syscall exception"), and so does GCC's runtime library.
- Mesa (the GPU user-space stack we would port) is MIT-licensed, which is compatible.

## History and grants: tested

### The store and history

| Issue | Rule |
| --- | --- |
| A 100 GB game update would keep the old 100 GB | **Storage classes.** *Documents* keep full history. *Apps and games* keep the current version plus one pre-update snapshot until the update is confirmed working, then drop it. *Cache and scratch* (shader caches, temp files) keep none |
| Game saves get corrupted | Saves are documents: full history. Rolling back a corrupted save becomes a standard feature |
| Retention cleanup runs while a game is playing | Cleanup runs on its own small time budget (decision 11) and pauses under load |
| The ledger itself grows forever | The ledger follows the same retention rule as the history it describes |

### Attribution

| Issue | Rule |
| --- | --- |
| An app passes its grant to a helper app. Who wrote the file? | Each grant carries a badge naming its origin. A commit records both the app that wrote it and the grant it used |
| Per-write attribution slows heavy writers | Attribution is per commit batch, not per write |

### Grants with time

| Issue | Rule |
| --- | --- |
| A grant expires while the app has the file mapped into memory | The file server revokes the memory mapping too, and the app gets an error on its next access, not a crash of the system |
| A game launcher needs a whole library folder; mods need folders | Install-time request for the library folder; mods are granted by drag and drop |
| People click "yes" to every install-time request | The permission screen shows what each app holds right now, in plain words, with one-click revoke |

### Branches and undo

| Issue | Rule |
| --- | --- |
| Undoing one app's changes when another app changed the same file since | The system shows both versions and asks. It never silently overwrites |
| Merging a branch with conflicting binary files | "Keep mine or keep theirs" per file. Content merging is offered only for file types that support it |
| Replay of a game would record gigabytes of reads | Replay is opt-in per app, for debugging, never on by default |

### Performance of the file server

- It must be multi-threaded (needs decision 10 to change) and run requests on the caller's budget (needs decision 11 to change).
- If it crashes, copy-on-write means it restarts from the last committed root, and nothing committed is lost.

## Not decided yet, and needed

| Topic | Why it matters | What I would propose |
| --- | --- | --- |
| **GPU stack** | No accelerated Vulkan means no games | Port Mesa's Vulkan drivers (MIT) to run as user-space libraries, and write the kernel-side GPU driver for one GPU family. In QEMU, virtio-gpu with Venus first |
| **Game compatibility target** | Decides how much of POSIX and Windows we must imitate | Question 1 below |
| **Running out of memory** | Under stress, something has to give | Each app gets a memory budget. When memory is short, the memory server first takes back caches, then asks apps to free memory, and only then stops the lowest-priority app, never the foreground game or the compositor |
| **Timers** | Frame pacing needs precise timers | TSC-deadline timer as the clock source, with HPET as a fallback; deadlines in nanoseconds in the native API |
| **Audio** | Crackling is the first thing people notice | A user-space audio server on a guaranteed budget, with buffers of 5 ms or less, fed through shared rings |
| **Input** | Input latency is felt directly in games | Input events go through shared rings from the driver to the compositor to the focused app. No polling |
| **Networking** | Online games, updates | A user-space network stack; apps receive network grants just like file grants (for example "may connect to this server") |
| **Anti-cheat** | Many online games need kernel-level anti-cheat | Honest answer: those games will not run, just as many do not on Linux. Games with server-side or user-space anti-cheat can |
| **Laptop hardware (Archy)** | The reference PC is a laptop | Hybrid graphics (built-in GPU drives the screen, NVIDIA renders, frames handed across as shared buffers), ACPI power and battery, suspend and resume, I2C touchpad, Wi-Fi, backlight. Wi-Fi and suspend are among the hardest drivers on any OS |
| **Performance and efficiency cores (Archy's i7 13th gen)** | A game thread landing on a slow efficiency core causes stutter | The scheduler knows each core's type and reads Intel's hardware feedback (Thread Director) hints. Threads with a time budget for frames or audio run on performance cores; background work and cleanup go to efficiency cores |
| **How we battle-test in practice** | "Stable under stress" must be measured, not hoped | Fuzzing every kernel call; stress tests that load all cores, memory and disk while a game-like benchmark measures frame-time spread; power-cut simulation for the filesystem; model checking of capability revocation and the scheduler |

## Questions for the person

1. **Which games should run?** Native games built for this OS, existing Linux/Vulkan games ported through SDL, or Windows games through a Wine/Proton-style layer.
2. **Which GPU is in the PC you would test on?** AMD, NVIDIA or Intel. That decides which kernel GPU driver comes first.
3. **Do you accept the four changes** (decisions 10, 11, 19, 20)?

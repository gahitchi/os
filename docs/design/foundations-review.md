# Alaya: a second look at the foundations

20 November 2026. You asked: knowing everything we know now (principles, new ideas, roadmap v2, frame deadlines, live updates, the history store), would I change anything at the very bottom of the system? I read the code built in milestones 0 to 3 (about 11,000 lines: boot, memory and page tables, capabilities, time budgets, messages, the Rust setup) and checked each part against what comes later.

## The short answer

**I would not redo anything.** The foundations are sound, and several early choices turn out to be exactly what the new ideas need. There is **one real problem** (how the computer starts, which needs your choice) and **four small additions** that are cheap now and expensive later. None of them throws away work.

---

## 1. What to keep, and why it was the right call

| Part | What it is, plainly | Why it helps what comes later |
| --- | --- | --- |
| The kernel has no memory of its own (decision 9) and one work area per core | Every piece of kernel information lives in memory that programs handed over, in plain, listed objects. Nothing is hidden in the middle of a half-finished task | This is the best possible starting point for **swapping the kernel without a restart** (decision 70) and, one day, **a computer that never turns off**: all state is in objects that can be found, saved and handed to a new kernel |
| Permissions as keys, one flat table per program (7, 51), with name tags on callers ("badges") | A program can only use what it holds a key to, and servers know who is calling | Needed for **quiet driver crashes** (a helper holds copies of a driver's keys to restart it) and for **signing every change** with the app that made it (22) |
| Time budgets with exact refills, and lent time (11, 53, 55) | Each program gets a fixed share of time; a server works on its caller's share | The base for **frame deadlines** (70: the deadline travels with the lent time) and for a future **true battery screen** |
| Small messages in registers (6 words and 1 key) | Fast calls between programs | Fine for everything planned; big data goes through shared memory, as designed |
| Rust, stable version, proofs (TLA+, Kani), the fuzzer | Safer language, mathematical checks, random-junk attacks | Exactly what "can't break" needs. Keep doing it for every new kernel part |
| Precise timers (TSC deadline) | The processor's own clock wakes the kernel exactly when needed | Needed for frame deadlines and steady sound |
| One big lock in the kernel (10) | Only one core is inside the kernel at a time | Fine for now (seL4 does the same). **Watch item**: measure it in milestone 4 with many programs talking at once; if it slows games, add a fast path for call-and-answer only |

---

## 2. The one real problem: how the computer starts (needs your choice)

Alaya starts with **Limine**, a ready-made boot program (decision 6). It is excellent for development, and it is why milestones 0 to 3 went fast. But three later decisions assume things Limine does not do, as far as I could find in its documentation:

1. **It can only read simple disk formats** (FAT and the CD format, ISO 9660). It cannot read Alaya's own store. Decision 28 says each update is a branch of the store and "the machine boots from it". Limine can't look inside the store to find that branch.
2. **I found no way for it to count failed starts** and fall back to the previous version by itself. Decision 28 relies on that counter.
3. **I found no support for recording what it starts in the laptop's security chip (TPM).** Decision 74 (warn you if the login screen changed) and the disk-encryption PIN (29) are only trustworthy if every step of starting up is recorded in the TPM. Otherwise a dishonest admin could replace the kernel itself and hide the change.

Limine can do Secure Boot through a signed checksum of its settings, and it has no problem with any of milestones 0 to 10.

**Options:**
- **Keep Limine for development, and write our own small start-up program before the ready-to-boot check (milestone 13)** (recommended). It would be a small Rust program for UEFI computers (your laptop is one) that reads the store, counts failed starts, records each step in the TPM, and then starts the kernel. Limine stays for the virtual machine and older PCs. Nothing built so far changes.
- **Write our own start-up program now**, before milestone 4. Same result, but it costs time now for something only needed at the real boot.
- **Keep Limine forever** and work around it: copy each system version to a simple FAT area of the disk, and count failed starts from inside Alaya. Cheaper, but the kernel itself would not be recorded in the TPM, so decision 74's warning could be fooled.

---

## 3. Four small additions (cheap now, expensive later)

These follow from principles you already set (mainly 3, "can't really break"), so I propose them as the first part of milestone 4. They stand unless you object.

| # | What | Plain words | Cost now | Cost if left for later |
| --- | --- | --- | --- | --- |
| 1 | **Turn on the processor's safety switches** (SMEP, SMAP, UMIP) | Modern processors can stop the kernel from accidentally running or reading a program's memory. A whole family of attacks then stops working. They are not switched on yet | A few days | Every driver written before this would need re-checking |
| 2 | **Tagged memory maps** (PCID) | Each time the computer switches between programs, it currently forgets its memory shortcuts and has to relearn them. With tags it keeps them. Our battle test asked for this; it isn't built yet | About a week | Slower call-and-answer, measured against our under-1-microsecond goal |
| 3 | **A version label on every kernel object, and one list of all kernel-wide state** | So a future kernel can recognise and take over the old kernel's objects | A day or two | Swapping the kernel live (decision 70) would need a risky conversion of objects with no labels |
| 4 | **Protection against the processor's "guessing" leaks** (Spectre family) | Turn on the processor's built-in protections when switching between programs, and keep secrets out of the kernel's guesswork | A few days, plus measuring the speed cost | Same work, but done in a hurry after a public warning |

---

## 4. Things noted for later (no action now)

- **Deeper sleep when idle.** The kernel uses the simplest "pause" when a core has nothing to do. For battery life it needs the processor's deeper sleep states. Planned with power control (milestone 15).
- **Modern device interrupts** (MSI-X) and the **device memory guard** (IOMMU) arrive with the driver framework in milestone 4, as planned. The virtual machine needs its Intel IOMMU option turned on for testing; I'll add that to the runner.
- **Kernel address shuffling** (KASLR). The kernel always sits at the same address. With a tiny kernel and the switches in addition 1 it matters less; worth doing with our own start-up program.
- **A kernel crash still stops everything.** Today that is acceptable because the kernel is small, proven and fuzzed. Later, the same labels from addition 3 could let a crashed kernel restart and take its objects back ("micro-reboot"). That is a research-grade idea for after milestone 15.

## 5. What it costs in total

Nothing built needs to be thrown away. The four additions take roughly two to three weeks of work at the start of milestone 4. The start-up program, if built before milestone 13 as recommended, is a medium-sized piece of work done once.

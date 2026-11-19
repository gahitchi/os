# How the OS is stress-tested: draft for review

19 November 2026. "Stable under stress" is a requirement, so it has to be measured. Every milestone must pass the gates below before the next one starts.

## 1. Proof for the parts that must never be wrong

| Part | Method |
| --- | --- |
| Capability revocation and retyping | Model in TLA+, check all interleavings on 2 to 4 cores |
| The scheduler and time budgets | TLA+ model: no thread runs beyond its budget; the highest-priority ready thread always runs |
| Filesystem commits | TLA+ model of the copy-on-write commit: after a crash at any point, the store is the last commit or the one before, never a mix |
| Kernel Rust code | Kani (a Rust model checker) on page-table, capability-table and message-copy code |

## 2. Fuzzing: hostile programs

- A fuzzer sends random and malformed calls on random capabilities to the kernel, around the clock in CI. Gate: no kernel crash, no hang, no memory leak.
- Each server (file, memory, network, compositor) is fuzzed through its message protocol the same way.
- The POSIX layer is checked with existing POSIX test suites, and with Wine's own tests.

## 3. Fault injection: things breaking

- **Power cuts.** The filesystem tests cut power at every single write in a test run, then remount and check. Gate: zero corruption, zero lost commits.
- **Driver crashes.** Kill each driver at random while it works. Gate: it restarts, and the session keeps running (a GPU crash costs at most a black frame).
- **Disk errors and a full disk.** Injected bad blocks and a 100 % full disk. Gate: errors reported, nothing corrupted, deletion still works.
- **Memory exhaustion.** Apps allocate until memory runs out. Gate: decision 24 order is followed; the foreground app and the compositor survive.

## 4. Stress on Archy: numbers that must hold

All measured while a background load keeps every core busy, memory at 90 %, and the disk writing continuously.

| Measure | Gate |
| --- | --- |
| Frame times of a game-like benchmark at 60 fps | 99.9 % of frames within 16.7 ms; no frame over 33 ms |
| Audio | Zero dropouts in one hour at 5 ms buffers |
| Input to screen | Under 20 ms from key press to pixel change |
| Call and reply between two programs on one core | Under 1 microsecond |
| Boot to desktop | Under 10 seconds |
| Soak test | 72 hours of mixed load: no crash, no leak, no growth in frame-time spread |

## 4b. The timeline

| Test | Gate |
| --- | --- |
| Drag the timeline across one year of history, under full background load | Past view within 100 ms; dragging at 30 updates per second; no frame of a running game delayed |
| Power cut during a restore | After reboot: either the restore happened completely or not at all |
| Consistency | Every past view shows files from a single commit; checked by writing paired files and verifying they never mismatch |
| Forget a file | After forgetting, no version of it can be read from any moment, raw disk included |
| Isolation | One person's timeline never shows another person's files or private-file previews |
| Budget | History never exceeds its budget by more than one commit's size; oldest moments go first |

## 5. Every day

- CI boots every commit in QEMU on 1, 2 and 8 virtual cores and runs the full test set.
- A weekly run on Archy itself measures section 4 and stores the numbers, so any slowdown shows up as a trend, not a surprise.

## Not covered yet

Real-hardware results depend on drivers that come later (NVIDIA, Intel display, Wi-Fi). Until they exist, section 4 runs in QEMU with the gates relaxed and marked as such.

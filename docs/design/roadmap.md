# Alaya: the detailed roadmap (version 2)

Version 3, approved 20 November 2026: adds living validity and protected outcomes (`commitments.md`), continuity (`continuity.md`), and answers and app promises (`answers-and-contracts.md`). Replaces version 2.

It includes every decision up to 87 (71 to 75 are your answers to the five contradiction cards) and the rules in `contradictions.md`.

## How to read it

Each milestone has:
- **What you'll see**: what you can try yourself when it's done.
- **What it includes**: the parts that get built.
- **How it's tested on your laptop**: what you run, and what must pass. Every test runs inside QEMU with KVM on the laptop (your rule). The laptop's Linux stays in charge the whole time.
- **Size**: small, medium, large or very large, compared with milestones 0 to 3.

The one rule of the order: **never build on something that hasn't passed its tests.** And from the analysis: **show what makes Alaya new as early as possible.**

## The big picture

| Phase | Milestones | What it gives you |
| --- | --- | --- |
| A. The engine | 0 to 4 (0 to 3 done) | A kernel and system that can't be broken by any program |
| B. The new idea | 5 to 7b | A desktop that remembers everything, brings you back to where your work was and tells you what changed, with one undo, file stories and "What happened?"; it tells you which results are out of date, and holds any program to rules you set. Nobody else has this |
| C. Real programs | 8 to 10 | Apps, Claude Code inside Alaya, fast graphics, network and sound |
| D. Real hardware, safely | 11 to 13 | The laptop's own devices and NVIDIA card, tested one at a time from inside KVM |
| E. Games and everyday use | 14 to 16 | Windows games, the first real boot, and Alaya as an everyday system |

---

## Phase A. The engine

### Milestones 0 to 3: done

The kernel starts on every core of your laptop (inside KVM), hands out memory and permissions safely (with mathematical proofs), runs programs on time budgets that can't be stolen, and lets programs talk through messages. A fuzzer throws random junk at it for minutes at a time without a crash.

One thing still open from milestone 3: the "call and answer" speed measured on your laptop with KVM (one line of output when you run `cargo xtask test --cpus 8`).

### Milestone 4. The system's helpers (large)

**What you'll see.** Still text on the screen, but now you can watch the system heal itself: a test kills a driver while a counter keeps counting, the driver comes back, and nothing notices. A system part is swapped for a new version while it runs.

**What it includes.**
- The memory helper (gives programs memory in big chunks, huge pages for games, a memory budget per app).
- The program starter, and a registry where programs find each other.
- The driver framework: devices' interrupts as messages, each driver locked to its own device's memory (IOMMU).
- Crashed drivers restart, and their unfinished requests are sent again so apps see no error (decision 70).
- Live updates of drivers and system parts without a restart (decision 70).
- Frame deadlines carried with calls, inside the existing time budgets (decision 70, rule B7).
- A small helper that reads Intel's hints about performance and efficiency cores and sets each budget's core type (decision 54).
- Timeout messages for servers whose lent time runs out (decision 55 follow-up).
- First, the foundation additions (decision 77): the processor's safety switches, tagged memory maps for faster switching, a version label on every kernel object for swapping the kernel later, protection against "guessing" leaks, and the IOMMU turned on in the virtual machine. The big kernel lock is measured under load.

**How it's tested on your laptop.** `cargo xtask test --cpus 8` runs the boot checks plus: kill every driver at random for 10 minutes (no error ever reaches the test app); update a running server 1,000 times in a row; the fuzzer now also attacks the helpers. TLA+ model of the crash-and-resend rule.

---

## Phase B. The new idea

### Milestone 5. Safe storage (large)

**What you'll see.** A virtual disk inside QEMU that you can "pull the plug" on at any moment. After restarting, every file is either fully there or as it was just before, every time.

**What it includes.**
- The virtual disk driver.
- The Alaya store: never overwrites, checksums every block, commits in small batches, encrypted, with separate branches for separate people.
- Every commit already records which app made it (attribution), because it is cheap from the start and everything in phase B needs it.
- Every stored version gets a permanent identity, and the store records which versions an app read (moved here from milestone 7, for living validity). Only the "documents" storage class is tracked.
- Storage classes (documents keep full history, games one step back, caches none) and the reserved space that lets a full disk still delete.

**How it's tested on your laptop.** The power-cut test: thousands of runs, cutting power at every single write, then checking. Gate: zero damaged files, zero lost commits. Full-disk and bad-block tests. TLA+ model of a commit. Tracking what apps read costs at most 3 % on a file-heavy test.

### Milestone 6. A simple desktop (large)

**What you'll see.** Alaya's first graphical screen in a QEMU window on your laptop: a login screen, then a desktop with windows you can move and resize with the mouse, a simple text editor, a file browser and a clock. And the first thing no other OS does: change some numbers in the editor, and the file browser marks the chart and the document made from them "Needs a look" (living validity, first show).

**What it includes.**
- Virtual display, keyboard and mouse drivers.
- The compositor (draws the screen; each app draws into its own buffer), with frame deadlines from the screen's refresh (decision 70).
- Windows on one screen that float or tile (decision 87): snap to a half by dragging to an edge, or switch the screen to tiling; a basic keyboard layout.
- The toolkit apps are built with: buttons, text, lists, with the accessibility tree and text in any language built in from the start (decision 67).
- A login screen for separate people. It warns you if the login or key parts changed since your last login (decision 74).
- A trusted way to type a PIN that no app can fake.
- Windows can be dragged into a loose group (a label only; it changes no permissions), and the text editor reopens exactly where you were (the first return point, `continuity.md`).
- Living validity, first version: "made from" notes for Alaya apps that tell their sources, a small chart-maker demo app and a simple export, and the four marks (up to date, needs a look, as of a date, not sure) in the file browser (`commitments.md`).

**How it's tested on your laptop.** Run `cargo xtask run` and use it. Automatic tests: input-to-screen delay measured in QEMU; frame times of the desktop while every core is busy; fuzzing of the compositor's messages; kill the display driver and the desktop comes back; living validity marks are right in a scripted pipeline, and nothing that wasn't fully seen is ever marked "up to date".

### Milestone 7. The remembering desktop (very large; the heart of Alaya)

**What you'll see.** The things no other OS does:
- **One undo** for the whole computer: move a file by mistake, change a setting, close a window, press one key, it comes back.
- **The timeline.** Drag back and see your files and desktop as they were.
- **"What happened?"** Plain sentences about what changed and which app did it, with undo on each line.
- **File stories.** Right-click any file to see where it came from and what was made from it.
- **Run it again**: reopen an app with exactly the files it saw on a past day.
- **Is it still up to date?** Every result made from other files shows whether its sources changed since, for all apps (older apps clearly marked as Alaya's guess), across copy and paste, with "show the difference" and "make again".
- **Jobs with rules (protected outcomes).** Start a program on a job with one screen of rules: what it may change, what must stay unchanged, which checks must pass before its work is kept. Alaya enforces them, runs the checks itself and says plainly which wishes it can't check.
- The Open and Save windows hand single files to apps; the permission screen shows what each app can touch; forget a file for good.

**What it includes.** Grants with time (live, pinned, until), the ledger, per-app undo, side branches, thinning and the 15 % history budget, named versions, forget with per-file keys, desktop search with results as grants, backup to an external (virtual) drive, the private-file guard with trusted carriers (rule B1), and every rule in `contradictions.md` section B that concerns history and undo. Tasks move to milestone 7a, because this milestone was getting too big. Living validity in full and the protected-outcomes core (`commitments.md` sections 2 and 3, rules R1 to R7).

**How it's tested on your laptop.** The timeline gates (past view in under 100 ms across a year of fake history, under full load); power cut during a restore; "forget" leaves nothing readable, raw disk included; one person never sees another's files; a test app that tries to reach files it wasn't given gets nothing; one undo walked back 1,000 steps without a mismatch; the living-validity mark tests (renames, restores, thinning, forget, hidden reads); the protected-outcomes attack suite (zero unauthorised changes kept, zero lost changes, no job kept when its checks fail). You use it by hand as well: this is the milestone to show people.

### Milestone 7a. Continuity (large)

**What you'll see.** Work the way you already do, and let Alaya help around it (`continuity.md`):
- **Loose groups**: drag windows and files together, or accept a suggestion, with nothing to name or file. A group is only a label.
- **Protected tasks**: "make this a task" for work that needs a boundary. Apps opened in it see only its files. Music and chat live in an "everywhere" area (decision 72). A paused task uses nothing.
- **Resume**: come back after days and land where you were, with your own "next step" note and a short "since you left" list (what changed, what needs a look, what is waiting).
- **The decision inbox**: things waiting for you, sorted into now, about what you're doing, can wait, and just so you know. Nothing urgent is ever held back.
- **Finish**: keep the results, archive the rest, hand back permissions, and undo all of it if you change your mind.
- **Copy or reference** when you drag between apps. A reference is marked "needs a look" when its source changes.

**What it includes.** Groups and tasks over the store and grants, return points, the briefing built only from the ledger, apps' saved state and your notes, the inbox with a channel only the system can use, closure, and handoff by copy or pinned reference. Rules in `contradictions.md` section B about tasks (B3, B11, B12) move here.

**How it's tested on your laptop.** Groups never grant (a test app in a loose group gets nothing), nothing invented (every briefing line traces to a record), nothing lost on Finish (and every Finish undoes), "now" events never held back in a busy-day test, and resuming a scripted task at least 25 % faster than reopening by hand. Opening one file in one app takes no extra steps.

### Milestone 7b. The full modern desktop (large)

**What you'll see.** Two or more screens (QEMU can show several), each with its own scaling; typing in Chinese, Japanese or Arabic; a screen reader that reads the desktop aloud; high contrast; an on-screen keyboard.

**What it includes.** Multiple monitors with hotplug and fractional scaling, the input-method server, the screen reader, full keyboard use, high contrast, right-to-left text (decisions 42, 48, 50, timing from 67).

**How it's tested on your laptop.** Plug and unplug virtual screens while apps run; type in several scripts; a screen-reader test that walks every window without the mouse.

---

## Phase C. Real programs

### Milestone 8. Apps and the old world (very large)

**What you'll see.** Install an app from a signed file; it installs on the side and you approve it. Simple open-source games and tools from Linux run on Alaya. Apps without a window stop using the CPU unless allowed.

**What it includes.** The Rust SDK with its C interface; signed app bundles with an install list you can untick; installs and updates on a branch; background and notification grants (notifications belong to tasks where possible); the terminal (sees only the task's files, one click for all (decision 71)); the POSIX layer (musl); SDL ported; the first simple ported games (software graphics until milestone 9). App notifications arrive in the decision inbox. Live handoff between two Alaya demo apps through the SDK. App promises that can never grow on their own through an update, with a difference screen, and the update trial noting what a new version tried. The answer system: an app can get an answer (a resized photo without location, a column total, "is this device connected") instead of the data (`answers-and-contracts.md`). System parts and apps update live, by themselves, never during full-screen games, with one-click undo (decision 73). The Experience Guardian: during an update's 10-minute trial Alaya also measures, and an update that is clearly slower goes back too, never a security fix (`commitments.md` section 4).

**How it's tested on your laptop.** POSIX test suites; a fuzzer for the app installer; install, update and undo 100 apps in a row; game mode test (background work pauses during a full-screen game).

### Milestone 8b. Claude Code inside Alaya (large)

**What you'll see.** Open the terminal inside Alaya (in QEMU on your laptop) and run Claude Code, working on Alaya's own code. Every change it makes shows in "What happened?" and can be undone at once.

**What it includes.** The Linux world (decision 66): a program that answers Linux requests, so the real Claude Code program and normal Linux tools run unchanged; bash, git and ripgrep for the terminal; pseudo-terminals; a simple network (QEMU's virtual network, TCP/IP, names, secure connections, network time); Claude Code sessions on a side branch or directly (decision 64); private files hidden from it. Claude Code starts with a rules screen by default (protected outcomes): what it may change, what stays protected, the checks before its work is kept, and where it may connect. The Linux world is tracked per process for living validity.

**How it's tested on your laptop.** A scripted Claude Code session in CI (start, read a file, run a command, edit, run git); git's own test suite; you run a real session by hand. A Claude Code job told to leave a file alone can't change it, even through a shell it starts.

### Milestone 9. Fast graphics (large)

**What you'll see.** 3D graphics and games running smoothly inside Alaya in QEMU. The trick: QEMU passes Alaya's graphics work to your laptop's real NVIDIA card through the host's Linux driver, so you get real speed long before Alaya has its own NVIDIA driver.

**What it includes.** Virtual GPU with Venus, Mesa's Vulkan in Alaya, explicit sync, games drawing straight to the screen when full screen, variable refresh and HDR as far as QEMU allows, frame deadlines through the graphics path (decision 70). Frame times join the Experience Guardian's measurements.

**How it's tested on your laptop.** A game-like benchmark while every core is busy and memory is 90 % full. Gate (relaxed for QEMU): 99 % of frames on time at 60 fps. The real gate (99.9 %) applies on real hardware.

### Milestone 10. Network and sound (medium)

**What you'll see.** Browse, download and back up to a cloud you choose; music and game sound that never crackles while everything else is busy.

**What it includes.** The full network stack, every connection in the ledger, the private-file guard for the network, cloud backup (S3 or WebDAV), the sound server on a guaranteed time budget, Bluetooth-style audio paths prepared. The outbox for Alaya's own mail and upload apps (hold for your OK); sound gaps join the Experience Guardian. Microphone answers ("is someone speaking", not the sound).

**How it's tested on your laptop.** One hour of music under full load: zero gaps. Network fuzzing. The leak-guard test: an app that read a private file can't go online without asking.

---

## Phase D. Real hardware, safely (decision 75)

Linux stays in charge of your laptop. Alaya keeps running inside KVM, and Linux hands it one real device at a time (this is called "device passthrough"). If an Alaya driver misbehaves, only that device is affected, and closing QEMU gives it back to Linux.

### Milestone 11. The laptop's everyday devices (very large)

**What you'll see.** Alaya, still in a window, using real hardware: a real USB keyboard, mouse or game controller plugged into the laptop, your real Wi-Fi card, real sound.

**What it includes.** USB (the controller, keyboards, mice, game controllers, USB networking for phone tethering), Bluetooth (controllers, headsets), HD Audio, Intel Wi-Fi, NVMe (on a spare or virtual drive, never your Linux disk), TPM (a virtual TPM first), updates on a branch with boot fallback, the recovery system, local crash reports. Camera answers (read a QR code without seeing the camera). Before starting: a study of borrowing Linux drivers as separate programs for Wi-Fi (analysis, recommendation 4).

**How it's tested on your laptop.** Each device handed to Alaya in KVM; driver-crash injection on real devices; the usual stress gates. A short written guide shows you how to hand a device over and get it back.

### Milestone 12. The NVIDIA card (very large)

**What you'll see.** Alaya driving your laptop's RTX 4050 directly with its own driver, still inside KVM.

**What it includes.** The NVIDIA driver using NVIDIA's GSP firmware, Mesa's NVK, hybrid graphics with the Intel display prepared. The Intel display driver itself waits for the first real boot (milestone 15), because the laptop's own screen can't be handed to a VM safely.

**How it's tested on your laptop.** The NVIDIA card handed to Alaya in KVM; the game-like benchmark; GPU driver crash injection (the target: one black frame).

### Milestone 13. Ready-to-boot check (medium)

**What you'll see.** A report: every stress gate, every proof, every crash test, all green, plus a 72-hour non-stop run inside KVM with real devices.

**What it includes.** Alaya's own start-up program (decision 76): it reads the store, counts failed starts and falls back by itself, and records every step in the security chip (TPM). The full stress suite in one command, the 72-hour soak, a final review of security and data safety, and a bootable USB stick image that never writes to the laptop's own disk.

**How it's tested on your laptop.** The soak runs while you use Linux normally. Only when everything passes, and only if you say so, do we move to phase E's real boot.

---

## Phase E. Games and everyday use

### Milestone 14. Windows games (very large)

**What you'll see.** Steam's Windows version and Windows games running on Alaya, with steady frames.

**What it includes.** Wine (or Proton) running through the Linux world (decision 66) or the POSIX layer, whichever proves simpler when we get there; DXVK and vkd3d for DirectX; a fixed set of test games; per-person mod layers (decision 63); game mode. Deep anti-cheat games stay "not yet". Driverless printing and scanning.

**How it's tested on your laptop.** The test games against the frame-time gates, in KVM with the NVIDIA card handed over.

### Milestone 15. The first real boot (large)

**What you'll see.** Alaya starting on your laptop itself, from the USB stick, in under 10 seconds, using its own screen.

**What it includes.** The Intel Iris Xe display driver, hybrid graphics for real, the touchpad, battery, sleep and resume, the thermal governor that slows down gently before the laptop's own brakes, the real TPM with your PIN.

**How it's tested on your laptop.** Only after milestone 13 passed and you agreed. Boot from the stick, never touching the laptop's disk; all stress gates at full strength (99.9 % of frames on time, zero sound gaps in an hour, under 20 ms key to screen, 72-hour soak).

### Milestone 16. Everyday Alaya (ongoing)

**What you'll see.** Alaya good enough to use every day, if you choose to install it.

**What it includes.** Fixes from daily use, installing to a disk with a recovery option, the ideas we parked (below) as you choose them.

---

## Parked ideas (from `new-ideas.md`, for later)

Asking your computer; live handoff for every app; a shared home for your data that any app can open (personal state plane); handing work to your other computer with the same limits (portable authority); your machines as one computer; a computer that never turns off; a true battery screen; an internet identity per task; the flight recorder; updating the kernel itself without a restart (after milestone 12, decision 70).

## What changed in version 3

- Living validity ("is this still up to date?"): recording what apps read moves to 5, first show in 6, full in 7.
- Protected outcomes (jobs with rules): core in 7, Claude Code's rules screen in 8b. It replaces the parked AI helper contract.
- The Experience Guardian grows out of milestone 4's update trial: 8, with frame times in 9 and sound in 10.
- The outbox, honest version (Alaya apps only), in 10; it leaves the parked list.
- Continuity (`continuity.md`): a clickable mock-up to test the desktop ideas alongside milestone 5 (decision 86); loose groups and the first return point in 6; tasks move from 7 to a new milestone 7a with resume, "since you left", the decision inbox, Finish and copy or reference handoff; app notifications and live handoff in 8.
- Answers instead of data and app promises (`answers-and-contracts.md`): app promises and data answers in 8, microphone answers in 10, camera answers in 11; the shared data home and handing work to another computer are parked.
- Windows float or tile from milestone 6 (decision 87).
- Nothing in milestones 0 to 4 or in the kernel changes.

## What changed from roadmap version 1

- Milestones 0 to 3 marked done.
- Milestone 4 gains quiet driver crashes, live updates and frame deadlines (decision 70).
- Attribution moves from 7 to 5, because it is cheap there and everything after needs it.
- Milestone 6 is a simple desktop (decision 67); 7 gains tasks, one undo, file stories, "What happened?" and run it again (68, 69); 7b is the full modern desktop.
- 8b uses the Linux world for Claude Code (66).
- 9 uses the laptop's real NVIDIA card through the host for speed in QEMU.
- Real hardware (old 11 and 12) is now tested by handing devices to Alaya inside KVM, then a ready-to-boot check (13), and the first real boot comes only at 15, after your OK (decision 75).
- Windows games (old 13) become 14, and may run through the Linux world.
- The names of the reference laptop are gone; it is "your laptop" or "the reference laptop" throughout, so this file is ready for the public repository once you approve it.

# Alaya: design

Version 3, 19 November 2026. This document brings all 50 decisions (`decisions.md`) and their rules into one place. Where a topic has its own detailed file, this document summarises it and names the file. If they ever disagree, `decisions.md` wins and this document is fixed.

## 1. What Alaya is

Alaya is a new operating system written from scratch in Rust for x86-64 PCs. It is a microkernel OS with a graphical desktop, built to run games, and built around one idea no other OS combines: **your machine remembers, and apps only get what you hand them.**

- Every change to your files is kept, and signed by the app that made it. You can see your files and desktop as they were at any moment, undo everything one app did, and try updates or apps on a branch before accepting them.
- Apps never see your file system. They get only the files you pick for them, with rights and a time attached ("this file, until I close the app"; "my photos, as of last Tuesday").

The name means the storehouse of memory that keeps every past impression.

## 2. Requirements

1. **A new kind of OS**, clearly different from Linux, Windows and macOS.
2. **Stable under stress**: no stalls, freezes or data loss when the machine is loaded.
3. **Runs games**: native games, ported Linux/Vulkan games, and later Windows games.
4. **No personal content** in the OS or the repository.

## 3. The layers

```
 apps and games        native apps · SDL ports · Wine (later)
 ------------------------------------------------------------------
 system libraries      Alaya SDK · musl POSIX layer · Mesa (Vulkan)
 ------------------------------------------------------------------
 system servers        desktop + compositor · store (files, history,
 (user space)          grants, ledger) · memory · network · audio ·
                       one program per driver
 ------------------------------------------------------------------
 kernel                capabilities · threads · time budgets ·
                       calls and notifications · address spaces
 ------------------------------------------------------------------
 boot                  Limine
```

Everything above the kernel line is an ordinary program with only the capabilities it was given. A crashed driver or server restarts without a reboot.

## 4. Kernel

Details: `kernel-interface.md`.

- **Microkernel** (decision 5): only capabilities, threads, scheduling, messages and address spaces are in the kernel.
- **10 calls** (decision 26): `call`, `send`, `try_send`, `recv`, `reply_recv`, `signal`, `wait`, `poll`, `yield`, `debug`. Everything else is a call on a capability.
- **Messages** (decision 8): synchronous call and reply, plus notifications. Rule: control travels in messages, data travels in shared memory with a notification as doorbell.
- **Capabilities** (decision 7): one permission system for everything. Revoking is interruptible, so it can never freeze the machine.
- **Never crashes on input**: every bad call returns an error.
- Kernel objects: untyped memory, capability tables, endpoints, notifications, replies, threads, scheduling contexts, address spaces, pages (4 KB, 2 MB, 1 GB), interrupts, I/O ports, IOMMU domains.

## 5. Memory

- **seL4 style** (decision 9): the kernel allocates nothing. User space turns raw memory capabilities into kernel objects.
- A memory server hands out memory in large chunks; programs divide it themselves, so allocating costs no message.
- Huge pages and prefaulting from the start, for games.
- **Running out** (decision 24): compress idle memory in RAM, then ask apps to free caches, then stop the lowest-priority app. No disk swap. The foreground game and the compositor are never stopped. Every app has a memory budget.

## 6. Cores and scheduling

- **Many cores from day one** (decision 10): one big kernel lock (kernel paths are short in a microkernel), a run queue per core, thread-safe servers.
- **Fixed priorities plus time budgets** (decision 11): a scheduling context gives a thread X microseconds every Y milliseconds. A stuck thread uses its budget and waits; audio and frames get guaranteed time.
- **Servers run on their caller's budget**, which ends priority inversion.
- **Core types**: on hybrid CPUs (the reference laptop's i7), budgeted frame and audio threads run on performance cores, background work on efficiency cores, guided by Intel's hardware feedback.

## 7. Storage and history

Details: `concept-history-and-grants.md`, `battle-test.md`.

- **Our own filesystem** (decision 15), **copy-on-write** (decision 16): never overwrite in place; write new blocks, then switch one root pointer. Every block is checksummed. A crash leaves the last commit or the one before, never a mix.
- **Every commit is attributed** (decision 22) to the capability, and so the app, that made it. Attribution is per commit batch.
- **Storage classes** (decision 33): documents keep full history; apps and games keep the current version plus one pre-update snapshot; caches keep none. Big rewrite-heavy files can be marked in-place.
- **Thinning and budget** (decision 33): every moment for a day, hourly for a week, daily for a month, weekly after; history capped at 15 % of the disk by default, oldest first.
- **Full-disk rules**: a reserved pool keeps deletion possible on a full disk; commits are batched every few seconds and immediately on sync.
- **The ledger** records which app read, wrote and deleted what, and every network connection. It follows the same retention.
- **What history enables**: per-app undo, branches for trying apps and updates, opt-in replay for debugging.

## 8. Grants and security

- **Apps see no file tree** (decisions 17, 18, 22). An app starts with its own private folder. Everything else arrives as a **grant**: a capability naming *what* (file, folder, search result), *rights* (read, write, create, delete, grant onward) and *when* (live, pinned to a moment, or until a time or until the app closes).
- Grants come only from the trusted desktop: open and save dialogs, drag and drop, and install-time requests the person accepts.
- A grant that expires also revokes any memory mapping of the file.
- **Network** (decisions 25, 27): open by default. The ledger records every connection and any app can be cut off. **Leak guard**: once an app reads a file marked private, it needs the person's OK before going online; the mark spreads to any app it passes data or shared memory to.
- **Drivers** can reach only their own device's memory, through IOMMU domains.

## 9. People, encryption and updates

- **Separate people** (decision 30): each person has their own store branch, history, grants and key. Apps and games are installed once and shared read-only.
- **Encryption** (decision 29): the whole store is encrypted with AES using the CPU's AES instructions. The TPM releases the key after a short PIN at boot. A recovery key is shown once at setup. Each person's data has its own key, unlocked at login, and each file has its own key inside that, so "forget" can erase a file from all history by destroying its key.
- **Updates** (decision 28): each update installs on a branch and the machine boots from it. Limine keeps the current and previous system; a boot counter falls back automatically if the new one fails or the person rejects it.

## 10. Desktop and graphics

Details: `timeline.md`.

- **Graphical desktop first** (decision 13), GPU-accelerated. Kernel debug output goes to the serial port.
- **Displays** (decision 48): several monitors with hotplug, per-monitor fractional scaling, high refresh rates, variable refresh rate, HDR with colour management.
- **Compositor with shared buffers** (decision 14): apps draw their own pixels into GPU buffers passed as capabilities. Explicit sync fences, direct scanout for full-screen games, variable refresh rate and tearing control.
- **Windows plus a timeline** (decision 34): familiar floating windows; a timeline bar to see files and the desktop as they were; a grants badge on each window and a permission screen listing what every app can touch.
- Timeline requirements: past view within 100 ms; every view is a single commit; past views are read-only; restores are undoable and show conflicts; the compositor records window layouts, not screenshots; apps never read history themselves; history is per person.

## 11. Drivers and hardware

- **Each driver is its own restartable program** (decision 19). A GPU driver crash costs a black frame, not the session.
- **Peripherals** (decision 49): Bluetooth first (wireless controllers, low-latency headsets, mice and keyboards), as its own restartable driver and server; pairing is a desktop action and apps reach devices only through grants. Printing and scanning later, driverless only (IPP Everywhere, eSCL).
- **Order**: virtio in QEMU first, then the reference PC.
- **Reference PC** (decision 23): a laptop with an Intel Core i7 13th gen and an NVIDIA RTX 4050. Needs NVMe, USB (xHCI and HID), the I2C touchpad, the Intel Iris Xe display, ACPI power, battery and suspend, HD Audio, Wi-Fi, TPM, and the NVIDIA GPU.
- **Graphics on the reference laptop**: hybrid. The Intel GPU drives the screen; the NVIDIA GPU renders and hands frames across as shared buffers. NVIDIA path: a kernel-side driver using NVIDIA's GSP firmware, plus Mesa's NVK Vulkan driver in user space. In QEMU, virtio-gpu with Venus.

## 12. Programs and compatibility

- **Native API first** (decision 12): the Alaya SDK, built on capabilities.
- **POSIX layer** (decisions 21, 31): a port of musl, translated to Alaya's servers, sized for Wine: threads, futex-style waits, fixed-address mmap, signals with crash handlers, dlopen, `posix_spawn`, `fork` (added for bash, decision 52), and Unix sockets that pass open handles. Ported programs see a fake `/` assembled from their grants.
- **Developer tools and Claude Code** (decisions 52 and 66): until Alaya runs on real hardware, Claude Code runs on the host and drives Alaya in QEMU. Inside Alaya, Claude Code (now a native Linux program) runs unchanged in a Linux world: a user-space Linux server answering Linux system calls, with every change going through the store and every connection through the ledger (decision 66). For the terminal, bash, git, ripgrep and basic command-line tools are recompiled for the POSIX layer, which adds Linux-style `epoll`, `eventfd`, `timerfd` and `inotify`, pseudo-terminals and a JIT memory right; Claude Code has the same limits as the terminal and every change it makes is attributed and undoable. Alaya's versions go beyond Linux: processes started from a session form one process family in the ledger that can be undone as a whole; file watching comes from the store's commit stream with no watch limit and no lost events; and a session can run on a store branch to be accepted or thrown away. Details: `linux-compat.md`.
- **Games** (decision 21): native games; Linux/Vulkan games through an SDL port; later Windows games through a Wine/Proton-style layer. Games that need kernel-level anti-cheat will not run.

## 13. Everyday use

- **Backup** (decision 36): each person's store and history stream, encrypted end to end and incrementally, to an external drive or another Alaya machine, and optionally to cloud storage the person chooses (S3-compatible or WebDAV). Restoring needs the recovery key. "Forget" also reaches backups. Pauses on battery and metered networks.
- **Apps** (decision 37): signed bundles from any source. The manifest lists install-time grants (folders, camera, microphone, controllers). Updates must carry the same developer key. Installs and updates run on a branch the person approves. Developer mode allows unsigned builds with a warning.
- **Clipboard** (decision 38): classic, readable by any app. Content copied from a privately marked app carries the mark to any app that reads it, and reads are recorded in the ledger.
- **Search** (decision 39): the desktop indexes each person's files and history; picking a result grants that file to an app; saved searches can be granted like folders. The index is encrypted and private files stay private.
- **Terminal** (decision 40): commands see the person's whole tree, like Linux, but never another person's files; changes stay attributed and undoable, and the leak guard still applies.
- **SDK** (decision 41): Rust first, with a stable versioned C interface for C, C++, Zig, Go and others.
- **Accessibility and languages** (decision 42): screen reader, full keyboard use, scaling, high contrast, UTF-8 and translatable text including right-to-left scripts, all from the first desktop.

- **Typing** (decision 50): a built-in input-method server between the keyboard and apps provides layouts, dead keys, composing scripts (Chinese, Japanese, Korean and others), emoji and an on-screen keyboard in every app. Only the focused window receives keys.
- **Clocks** (decision 47): history is ordered by a commit counter that only goes up; wall-clock times are labels only. Time comes from the hardware clock and authenticated network time (NTS).
- **Background apps** (decision 46): an app with no visible window is frozen unless it holds a background grant. Notifications need a grant, go through the desktop and are silenced during full-screen games.
- **Power and heat** (decision 45): full speed plugged in, balanced on battery; a thermal governor lowers budgets and clocks smoothly before hardware throttling, so frame times stay steady. Per-game overrides.
- **Recovery** (decision 43): a read-only recovery system on the disk (opened by a key at boot or after 3 failed boots) can roll back to any past system, repair the store, restore from backup or reset; plus an optional recovery USB stick. It needs the PIN or recovery key to touch data.
- **Crash reports** (decision 44): saved locally as readable reports, sent only after the person reviews them; never include file contents or private file names. Kernel crashes are kept in a reserved area and shown at next boot.

## 14. Testing

Details: `test-plan.md`. TLA+ and Kani proofs of revocation, scheduling and commits; fuzzing of every kernel call and server protocol; power-cut injection at every write; driver-crash injection; hard gates on the reference laptop under full load (99.9 % of frames within 16.7 ms at 60 fps, zero audio dropouts in an hour, under 20 ms input to screen, under 1 µs call and reply, 72-hour soak); timeline gates. CI boots every commit on 1, 2 and 8 virtual cores.

## 15. License

GPL-3.0 for the kernel, servers and drivers, plus a linking exception for the system libraries apps link against (decision 20), so games and apps may use any license. Mesa and musl are MIT.

## 16. Open items

| Item | Status |
| --- | --- |
| Trademark check for "Alaya" | Needed before public use |
| Byte-level formats (store, ledger, capability layout) | Set while building, then written into the docs |
| Closed firmware (NVIDIA GSP, Intel Wi-Fi) | Required by the hardware; loaded as separate files, not part of Alaya's code |
| Secure Boot | Later |
| Kernel-level anti-cheat | Not supported |

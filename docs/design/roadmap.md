# Alaya: roadmap

19 November 2026. The order in which Alaya is built. Each milestone has a goal and the tests that must pass before the next one starts (from `test-plan.md`). The order follows one rule: never build on something that has not passed its tests.

| # | Milestone | Done when |
| --- | --- | --- |
| 0 | **Repository and tools** | The Cargo workspace, the Limine boot image, a QEMU runner and CI exist; the repo has the GPL-3.0 license and the library exception; CI boots an empty kernel that prints to serial |
| 1 | **Kernel boots on every core** | Memory map, paging, interrupts, timers (TSC deadline) and all cores come up with the big kernel lock. CI boots on 1, 2 and 8 virtual cores |
| 2 | **Capabilities and memory** | Capability tables, untyped memory, retype and interruptible revoke; the first user program runs. TLA+ model of revocation passes; Kani checks the capability code |
| 3 | **Threads, time budgets, messages** | The 10 calls, scheduling contexts, core types, notifications. TLA+ model of the scheduler passes; the kernel fuzzer runs in CI without a crash; call and reply measured |
| 4 | **User-space foundations** | Memory server (huge pages, budgets), process server, service registry, driver framework with interrupts and IOMMU domains, restart of crashed drivers. Driver-crash injection passes |
| 5 | **Storage** | virtio disk driver; the Alaya store with copy-on-write, checksums, batched commits, encryption, per-person branches. TLA+ model of commits passes; power cut at every write shows zero corruption |
| 6 | **First desktop in QEMU** | virtio display and input drivers, the compositor with shared buffers, floating windows, a login screen for separate people; multiple monitors with per-monitor scaling and hotplug; the input-method server (layouts, composing scripts, emoji); the UI toolkit with its accessibility tree, keyboard use, scaling and translatable text |
| 7 | **History and grants** | Attribution, storage classes, thinning and budget, grants with time, open/save dialogs and drag and drop as grants, the permission screen, the ledger, per-app undo, branches, the timeline, forget, desktop search with results as grants, backup to an external drive. All timeline gates pass |
| 8 | **SDK, apps and POSIX layer** | The Rust SDK with its C interface; signed app bundles with manifests, installed on a branch; background and notification grants (apps without a window are frozen); the terminal; musl ported (threads, futex waits, mmap, signals, `posix_spawn`, Unix sockets with handle passing); SDL ported. POSIX test suites pass |
| 9 | **Accelerated graphics in QEMU** | virtio-gpu with Venus, Mesa Vulkan in user space, explicit sync, direct scanout, VRR and HDR with colour management; a game-like benchmark meets the frame-time gates in QEMU (relaxed) |
| 10 | **Network and audio** | virtio network driver, user-space TCP/IP, connections in the ledger, the leak guard, cloud backup; audio server on a time budget. Zero audio dropouts in one hour under load |
| 11 | **Reference laptop: base hardware** | NVMe, USB (xHCI, keyboard, mouse, controllers), I2C touchpad, Intel Iris Xe display, ACPI power, battery and suspend, the thermal governor, Bluetooth (controllers, headsets), HD Audio, TPM with PIN, updates on a branch with boot fallback, the recovery system and local crash reports. Alaya boots to its desktop on the reference laptop in under 10 seconds |
| 12 | **Reference laptop: NVIDIA and Wi-Fi** | The GSP-based NVIDIA driver with NVK, hybrid graphics with the Intel display, Intel Wi-Fi. All stress gates pass on the reference laptop, including the 72-hour soak |
| 13 | **Windows games** | Wine running on the POSIX layer; a set of Windows games tested against the frame-time gates. Driverless printing and scanning |

## Why this order

- **0 to 5 are the foundation.** Nothing above them can be stable if they are not, so they carry the formal proofs.
- **6 comes before 7** because history and grants need a desktop to issue grants and show the timeline.
- **7 comes before 8** so ported programs are built on grants from the start, not retrofitted.
- **QEMU before real hardware** (9 and 10 before 11): every subsystem is proven on virtual hardware first, so real-hardware bugs are only driver bugs.
- **NVIDIA comes late** (12) because it is the largest driver and depends on everything below it.
- **Wine is last** (13) because it needs the POSIX layer, Vulkan and real hardware.

## The biggest risks

| Risk | Where | What we do |
| --- | --- | --- |
| seL4-style memory and revocation are hard to get right | 2 | Formal model first, then code; fuzz from milestone 3 on |
| The store is the most complex program in the OS | 5, 7 | Built in stages: copy-on-write, then attribution, then grants with time, then branches |
| NVIDIA and Wi-Fi drivers are very large | 12 | Built last, on top of a proven base; QEMU graphics keeps the desktop usable meanwhile |
| Wine needs exact POSIX behaviour | 8, 13 | Use musl rather than writing our own; run POSIX and Wine test suites |

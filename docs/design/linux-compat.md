# Linux compatibility: options and decision

19 November 2026. **Decided: D now, plus B inside Alaya (decision 52).** The rules that came with the choice are in section 7. Goal set by the person: run Claude Code inside Alaya, to test and debug the OS from within.

## 1. What Claude Code needs

Claude Code is a command-line program. Today it ships for Linux both as a JavaScript bundle that runs on Node.js and as a single native x86-64 Linux executable. To be useful as a debugging tool it needs, inside Alaya:

| Need | Linux features behind it |
| --- | --- |
| Node.js or the native executable (V8 / JIT engine) | threads (`clone`), `futex`, `mmap`/`mprotect` with executable pages, signals, `epoll`, `eventfd`, `timerfd`, `getrandom` |
| A terminal | a pseudo-terminal (`/dev/ptmx`, `/dev/pts`), termios, window-size `ioctl`, ANSI/xterm escape codes |
| A shell and subprocesses | `bash` (its Bash tool needs bash or zsh), `fork`/`vfork`/`execve`, `wait4`, pipes, process groups, signals |
| Tools | `git`, `ripgrep`, coreutils, `find`, `grep`, `sed` |
| File watching | `inotify` |
| Network and HTTPS | TCP sockets, DNS, CA certificates, a correct clock |
| Files | the person's projects, `/proc/self`, `/dev/null`, `/dev/urandom`, `/tmp`, `/etc/resolv.conf` |

Rough count: about 150 to 200 Linux system calls, plus `/proc` and `/dev` basics. `io_uring` and similar can answer "not supported", because Node and git fall back.

## 2. The options

### A. Linux personality (run unmodified Linux programs)

A user-space **Linux server** speaks the Linux system-call interface. A Linux program's `syscall` instruction does not run anything in the kernel: the kernel turns it into a fault message to the Linux server (seL4 already has this "unknown system call" fault), which answers by calling Alaya's servers (store, processes, network, terminal). The same idea as Fuchsia's Starnix on the Zircon microkernel and the original WSL on Windows. An Alpine or Debian userland lives in a folder of the person's store; `apk` or `apt` install bash, git, Node and ripgrep unchanged, and Claude Code installs with its normal Linux installer.

### B. Port the tools to the POSIX layer (recompile)

Grow the musl-based POSIX layer (decision 31) until Node.js, bash, git and ripgrep can be recompiled for Alaya. Claude Code's JavaScript bundle then runs on Alaya's own Node build.

### C. Linux virtual machine

Add hardware virtualisation (Intel VT-x, EPT) to Alaya and run a real Linux kernel in a VM. Alaya serves its disk (a folder shared over virtio-fs), network and console. Like WSL2 or ChromeOS's Linux.

### D. Claude Code on the host, driving Alaya in QEMU (works today)

Not a compatibility layer, but the earliest way to use Claude Code on Alaya: it runs on the laptop's Linux, builds the image, boots QEMU, reads the serial log, attaches GDB to QEMU's stub and uses QEMU's control socket for screenshots and input. Available now, and it stays necessary forever: when Alaya itself crashes, a Claude Code running inside it dies too.

## 3. Battle test

### Security (capabilities and grants)

| | Verdict |
| --- | --- |
| A | **Good, with rules.** The Linux server is an ordinary Alaya program holding only the capabilities it was given. Each "Linux world" belongs to one person and is granted that person's tree, exactly like the terminal (decision 40). Every file change goes through the store, so it is **attributed and undoable**: per-app undo can roll back everything Claude Code did. Network goes through Alaya's network server, so every connection is in the ledger and the leak guard (27) applies to the whole Linux world. Risk: the Linux server is large; a bug in it lets a Linux program misuse the Linux server's own capabilities, never the kernel or another person's data. Rule: JIT (writable then executable pages) is a right the Linux world must hold, not a default |
| B | **Best fit.** Ported programs are normal Alaya programs with grants, attribution and the ledger, no extra trusted server |
| C | **Weakest.** The VM is one app to Alaya: all writes in it are attributed to "the Linux VM", not to Claude Code or git, so undo and the ledger lose detail. Adds a hypervisor to the kernel, a large new surface for "the kernel never crashes on input" |
| D | Nothing inside Alaya is exposed |

### Stability

| | Verdict |
| --- | --- |
| A | **Good.** Linux programs run on time budgets (decision 11) and a memory budget, so a runaway `node` cannot stall frames or audio. A Linux-server crash ends the Linux world only; the desktop and other apps keep running |
| B | **Good**, same budgets. Each port is a separate patch set to keep working |
| C | **Fair.** A VM pins a large block of memory (1 to 2 GB) and its own scheduler sits inside Alaya's; testing it on the laptop needs nested virtualisation (a VM inside QEMU/KVM), which is slower and less reliable |
| D | **Best**: it survives Alaya crashing |

### Performance

| | Verdict |
| --- | --- |
| A | Each Linux system call becomes a call and reply to the Linux server (target under 1 µs, decision 26 test gate). Node starting up makes roughly 10,000 calls, so about 10 ms extra. File-heavy work like `git status` on a big repository may be 2 to 4 times slower than on Linux. Fine for a command-line tool; hot paths (`futex`, memory faults) can be shortened later |
| B | Fastest: no translation beyond musl's |
| C | Near native for CPU, slower for files shared with Alaya (virtio-fs) |
| D | Native |

### Effort and upkeep

| | Verdict |
| --- | --- |
| A | Large but bounded: about 150 to 200 calls, `fork` done by copy-on-write in the Linux server, `/proc` and `/dev` basics, ptys, `inotify`. Upkeep is low: new versions of Node, git and Claude Code just run. Proven elsewhere (Starnix, original WSL, FreeBSD's Linux layer) |
| B | Large and never finished: Node needs its libuv and V8 platform layers ported, bash needs `fork` (decision 31 excluded it), git and ripgrep need patches, and every new version needs re-porting. Biggest risk: Claude Code's native executable is a Linux binary and cannot be recompiled; if the JavaScript bundle on Node is ever dropped, B stops working |
| C | Hypervisor in the kernel plus a small virtual-machine monitor and virtio devices. Smaller than A, but it is a second kind of OS work (virtualisation) that nothing else in Alaya needs |
| D | None |

### Is it still a new kind of OS?

| | Verdict |
| --- | --- |
| A | **Yes.** Linux is a guest world in a box, opt-in per person, running on top of Alaya's history and grants. The rest of Alaya is untouched. Bonus no other OS has: everything an AI agent changes is attributed and undoable through the timeline |
| B | Yes, but every port pulls the POSIX layer closer to Linux |
| C | Least: it is the WSL2 / ChromeOS pattern, and the interesting parts of Alaya stop at the VM's edge |
| D | Not affected |

### Games (a side effect worth knowing)

A also lets unmodified Linux games and Linux builds of Wine/Proton run later, if the Linux server learns the graphics `ioctl`s (a large extra step, not in scope now). B and C do not help games.

## 4. Recommendation

**D now, A as the Linux compatibility layer.** D costs nothing and is the right tool for kernel bugs. A runs the real Claude Code, git and Node without porting, keeps every change attributed and undoable, and stays clearly Alaya. Keep the musl POSIX layer (decision 31) for native ports and Wine; the two do not conflict.

## 5. When A can land

A needs: processes and memory server (milestone 4), the store (5), a terminal with ptys, and a network with TCP, DNS and time. Today the terminal is at 8 and the network at 10.

Proposed: a new milestone right after 8, **"Linux world"**, with a minimal network pulled forward into it (virtio network driver, user-space TCP/IP, DNS, network time). The full network work (leak guard, cloud backup, ledger views) stays at 10. That puts Claude Code inside Alaya after the desktop, grants and terminal exist, so it is built on grants from the start (the reason 7 comes before 8).

A faster alternative: land it straight after milestone 5 through a text console on QEMU's serial port, before any desktop. Sooner, but grants and attribution would be added to it afterwards, breaking the "7 before 8" rule.

## 6. Rules that would come with A

1. One Linux world per person, holding that person's tree and nothing of anyone else's (same limits as decision 40).
2. Linux programs run on budgets; the Linux world has a memory budget and cannot starve the desktop, games or audio.
3. Writable-then-executable memory (JIT) is a right held by the Linux world, not a default for Alaya apps.
4. The Linux server is our own code implementing the published interface; no Linux kernel code is copied (GPL-2.0 is not compatible with GPL-3.0-only).
5. Claude Code and distribution packages are never shipped in the Alaya image or repository; the person installs them inside their Linux world.
6. Tested by running the Linux Test Project's system-call tests, Node's and git's own test suites, and a Claude Code session in CI-style smoke tests.

## 7. Decision and rules (B, decision 52)

The person chose **D now and B inside Alaya**: Claude Code runs on the host PC while Alaya lives in QEMU, and before the first boot on real hardware the developer tools are recompiled for Alaya's POSIX layer. These rules are part of the decision.

1. **What gets ported**: Node.js (its libuv event loop and V8's platform layer), bash, git, ripgrep (through a Rust standard-library target for Alaya, which the SDK needs anyway), and basic command-line tools (coreutils, `find`, `grep`, `sed`, `less`). Claude Code's JavaScript bundle runs on the ported Node.
2. **`fork` is added to the POSIX layer** (approved by the person; this changes the rule in `battle-test.md` under decision 12, which allowed only `posix_spawn`). bash cannot work without it. The process server copies the address space copy-on-write. The child gets exactly the parent's grants and open handles, never more, and its changes are attributed to the same app. `posix_spawn` stays the fast path and the ported tools use it wherever they can.
3. **Linux-flavoured calls on top of POSIX**: `epoll`, `eventfd`, `timerfd`, `signalfd`, `inotify`, `getrandom`, `pipe2` and `/proc/self` basics, so libuv, git and others build with few patches. `inotify` is fed by the store's change events; `epoll` waits on Alaya notifications.
4. **Pseudo-terminals**: the terminal (decision 40) offers pty pairs with termios and window size, and understands xterm escape codes, so full-screen command-line programs work.
5. **JIT memory** (writable then executable pages) is a right in the app's manifest, granted to Node, not a default.
6. **Same limits as the terminal (decision 40)**: these tools see the person's whole tree and nothing of anyone else's; every change is attributed and undoable; the leak guard applies. Per-app undo can roll back everything Claude Code did.
7. **Budgets**: ported programs run on time and memory budgets like any app, so a runaway `node` cannot stall the desktop, games or audio.
8. **Upkeep**: each port is a small patch set kept in a separate ports area, rebuilt in CI for every new upstream version; tracking Node's and git's releases is part of the work. If Claude Code ever stops shipping a JavaScript bundle that runs on Node, we revisit A.
9. **Licences**: each port keeps its own licence (Node MIT, git GPL-2.0, bash and coreutils GPL-3.0, ripgrep MIT/Unlicense) and runs as a separate program, never linked into Alaya's GPL-3.0-only code. Claude Code itself is never shipped in the Alaya image or repository; the person installs it.
10. **Network**: milestone 8b pulls a minimal network forward from 10 (virtio network driver, user-space TCP/IP, DNS, network time, CA certificates). On real hardware, USB networking (phone tethering and USB Ethernet adapters) comes in milestone 11, because Wi-Fi only arrives in 12.
11. **Tests**: Node's and git's own test suites, the POSIX test suites, and a scripted Claude Code smoke test (start, read a file, run a shell command, edit a file, run git) in CI under QEMU.

## 8. Where Alaya's version goes beyond Linux

Porting brings old Unix shapes (`fork`, `inotify`, ptys) into Alaya. Each one is built on Alaya's own foundations, so the ported tools get things Linux cannot give them. These are part of decision 52.

| Unix feature | On Linux | On Alaya |
| --- | --- | --- |
| `fork` | The child inherits every right of the parent, including the whole file system; nothing records which process family did what | The child gets exactly the parent's grants, never more. Every process started from a session (`fork`, `posix_spawn`) is recorded in the ledger as one **process family**, and its changes are attributed to it, so "undo everything this Claude Code session and its subprocesses did" is one action. `fork` followed by `exec` is recognised and turned into the fast spawn path, so the copy is never made |
| `inotify` (file watching) | One watch per directory, a per-user watch limit, and events are silently lost when the queue overflows; programs must rescan | Watching is fed by the store's commit stream: one subscription covers a whole tree, there is no watch limit, and each event carries a commit number. After an overflow or a restart, a watcher asks "what changed since commit N" and gets an exact answer from history, so no change is ever missed |
| Trying changes | Changes land directly in the files; undo means git or backups | A session can start **on a branch of the store** (opt-in): Claude Code, builds and scripts work there, the person sees the difference on the timeline and accepts or throws it away in one step. The main tree is untouched until accepted |
| Network access | Any process can connect anywhere, unseen | Every connection is in the ledger, named by process family, and the leak guard holds: a session that read private files needs the person's OK before going online |
| JIT memory | Any process may make memory writable and executable | Only programs whose manifest asks for the JIT right, which the person sees at install |
| Runaway processes | A busy `node` or `git gc` can make the desktop stutter | Every process family runs on a time and memory budget, so the desktop, games and audio keep their guaranteed time |

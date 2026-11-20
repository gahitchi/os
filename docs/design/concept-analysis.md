# Alaya: an honest analysis of the whole concept

19 November 2026. Written after reading every design file (principles, principles-design, alaya-design, decisions 1 to 65, battle-test, roadmap, timeline, linux-compat-options, milestone-3-plan, kernel-interface, test-plan, concept-history-and-grants) and the repository (milestones 0, 1 and 2 merged, milestone 3 in progress, principles PR #7 open). Prior art was checked on the web where memory was not enough. Nothing in the code or the repository was changed.

---

## Part 1. The short version, in plain words

**Can it work?** Partly, and it depends on what "work" means.

- **The new idea itself (the computer remembers, apps only get what you hand them) can work.** It is hard, but it is ordinary software running inside the machine. Nothing in it needs secret knowledge or a big company. It can be built and shown inside the virtual machine (QEMU).
- **Running it as your everyday gaming laptop, with the NVIDIA card, Wi-Fi, sleep mode and Steam games, is the part most likely to fail.** Not because the ideas are wrong, but because of the sheer amount of driver work. Other from-scratch systems with many volunteers (Redox, about 11 years old) are still not everyday systems, and only got Vulkan and Node.js running this year.
- **One plan has already broken.** Decision 52 planned to run Claude Code inside Alaya by rebuilding Node.js and running Claude Code's JavaScript. Claude Code is now shipped only as one ready-made Linux program that does not use Node at all. Even the npm package installs that same program. So that plan no longer works as written. This needs your choice (card below).

**How new is it?** I counted 37 separate ideas in the design:

| Rating | Count | Meaning |
| --- | --- | --- |
| Genuinely new | **2** | I could not find it in any shipping system |
| New combination, or only in research | **11** | Exists in research papers or as separate pieces, never together in a real OS people use |
| Already exists | **24** | A shipping system already does it |

That sounds harsh, but it is normal. The iPhone and macOS were also mostly known pieces put together well. **Alaya's newness is the combination: one store where every change is remembered and signed by the app that made it, and that store is the base for undo, privacy, updates, games and AI tools at once.** No shipping OS does that. That claim is honest and defensible.

**Do the principles contradict each other?** Mostly no, but there are **6 real tensions** to fix in wording or design (Part 4). The biggest: principle 2 says "nothing gets in by default", but the network, clipboard and terminal are open by default. And "private" marks can spread to the screen and the file system itself, which would mark everything.

**My verdict:** keep going, but change two things: (1) pick a new way to run Claude Code inside Alaya, and (2) show the new idea working much earlier, before spending months on desktop polish. Details in Part 5.

---

## Part 2. Feasibility

### What has been built, and is it on track?

Milestones 0 to 2 are merged: the kernel boots on every core, has capabilities, raw memory handling, a formal proof (TLA+) of revocation and checked code (Kani). That is a real, careful foundation, done the way seL4 does it. Milestone 3 (threads, time budgets, messages) is in progress. So far the work matches the design and nothing has had to be undone. The kernel is the best-understood part of the whole plan, because seL4 has shown how to do it.

### The hardest parts, ranked

| # | Part | Why it is hard | Risk to the project |
| --- | --- | --- | --- |
| 1 | **NVIDIA graphics on the laptop (milestone 12)** | Needs our own kernel driver speaking to NVIDIA's GSP firmware, plus Mesa's NVK. NVK on Linux is close to NVIDIA's driver in older games, but newer games still lag ("a larger gap remains", Phoronix, July 2026). Laptops add hybrid graphics (Intel draws the screen, NVIDIA renders), which even Linux gets wrong often | **Very high.** This alone is bigger than the whole kernel |
| 2 | **Windows games through Wine and Steam (milestone 13)** | Wine, DXVK and vkd3d run best on Linux and rely on Linux-specific tricks (fast sync, memory tricks). Wine does run on macOS, so a non-Linux Wine is possible, but Steam is one of the hardest Windows programs to run | **High.** About 90 % of Windows games launch on Linux today after many years of work by Valve; Alaya would start far behind |
| 3 | **Wi-Fi, sleep and resume, battery, touchpad (milestones 11 and 12)** | Each is famous for breaking on every OS. Intel Wi-Fi alone is huge and needs firmware | **High** |
| 4 | **The store (history, attribution, grants, branches)** | The most complex program in the OS. Copy-on-write file systems took years to become trusted (btrfs, ZFS). Adding per-app signatures, branches and time-limited grants makes it harder | **Medium.** Hard, but it is where Alaya's value lives, and it can be built and tested entirely in QEMU |
| 5 | **Claude Code inside Alaya (milestone 8b)** | The chosen plan (rebuild Node, run the JavaScript) no longer fits how Claude Code ships (see below) | **Medium, and urgent to re-decide** |
| 6 | **Anti-cheat games** | Call of Duty, Battlefield 6, Valorant and others need deep anti-cheat that refuses Linux, so they will refuse Alaya too | **Certain to block those games.** Out of our control |
| 7 | **The rest of milestone 6** | Multi-monitor hotplug, fractional scaling, HDR, input methods for Chinese/Japanese/Korean, screen reader, all in the first desktop | **Medium.** Each is large; together they delay the new idea |

### Claude Code: the plan that broke

Claude Code's official setup page now says the recommended install is a native program, and that "the installed claude binary does not itself invoke Node". The npm package "installs the same native binary". Supported systems are macOS, Windows, and Linux (including Alpine with musl). The `principles-design.md` battle test already named this risk ("Claude Code becomes a single native program instead of JavaScript") and said a Linux-compatibility layer would then be needed. It has happened.

What still works:
- **Option D** (Claude Code on the laptop, driving Alaya in QEMU) works today and is unaffected.
- **Option A** (a "Linux world": a program inside Alaya that answers Linux requests, like Google's Starnix on Fuchsia and the first WSL on Windows) runs the real Claude Code program unchanged. It keeps everything Alaya promises: changes signed and undoable, network in the log, the leak guard.
- The ported tools from option B (bash, git, ripgrep on the POSIX layer) are still useful for the terminal. Only "run Claude Code on our own Node" is dead.

### Effort and realism

Honest comparisons:

| System | Team | Age | Where it is |
| --- | --- | --- | --- |
| Redox (Rust microkernel) | Dozens of volunteers | ~11 years | Vulkan and Node.js arrived in 2026; real hardware support still growing; not an everyday system |
| SerenityOS | Hundreds of contributors | ~6 years | Own desktop and browser; no accelerated graphics, no games |
| Haiku | Volunteers | 20+ years | Usable desktop, limited drivers, no modern GPU acceleration |
| Fuchsia (Google) | Hundreds of paid engineers | ~10 years | Shipped only on Nest Hub devices |
| seL4 | Research group plus foundation | ~15 years | Proven kernel, used in embedded and defence, not a desktop |

A tiny team helped by Claude writes code much faster than volunteers, and that is a real advantage. But driver work is limited by testing on real hardware, reading undocumented behaviour and fixing timing bugs, not by typing speed. And you have decided (sensibly) not to boot on the real laptop until Alaya is stable, which makes real-driver work slower.

My honest estimate:
- **Milestones 3 to 7 in QEMU** (kernel, storage, a first desktop, history and grants): realistic. This is a demo nobody else has.
- **Milestones 8 to 10** (apps, POSIX, tools, network, audio, virtual graphics): realistic but long.
- **Milestones 11 to 13** (the laptop's own hardware, NVIDIA, Wi-Fi, Windows games at steady frame rates): this is where most OS projects stall for years. Possible, not likely on its current path.

### What would kill it

1. **Spending years on drivers before anyone can see the new idea.** Motivation dies before the payoff.
2. **NVIDIA or Wi-Fi never becoming reliable**, so it never becomes an everyday laptop OS.
3. **The store losing data once.** For an OS whose promise is "your files are safe", one corruption bug is fatal to trust. The plan's power-cut testing is the right defence.
4. **Performance gates set too high too early** (99.9 % of frames on time on NVK through Wine on a microkernel). Failing them could look like failure of the whole idea when it is really driver maturity.
5. **Scope creep.** 65 decisions, many with "from the first version" (HDR, input methods, accessibility, Bluetooth). Each is right on its own; together they are a decade.

### Ways to cut the driver risk (not a decision yet, for before milestone 11)

Other systems avoid writing every driver from zero:
- **FreeBSD's LinuxKPI** runs Linux's graphics drivers (Intel, AMD) and Intel Wi-Fi drivers through a translation layer. This is proven and in use today.
- **Genode** runs ported Linux drivers as separate programs, which fits Alaya's "each driver is its own restartable program" rule exactly.
- **License note:** most Linux drivers are GPL-2.0-only, and Alaya's kernel is GPL-3.0-only. They cannot be mixed into one program. Run as a separate driver program they are much easier, and several key drivers are dual-licensed (Intel Wi-Fi's iwlwifi, NVIDIA's open kernel modules are MIT/GPL).

I recommend we look at this properly before milestone 11. It would not make Alaya less new: nobody chooses an OS for how its Wi-Fi driver was written.

---

## Part 3. How new is it? Every idea, rated

Ratings: **New** = I found no shipping system or published research that does it. **Combination / research** = exists in research, or as pieces in different systems, but no shipping OS has it. **Exists** = a shipping system does it.

### Kernel and core

| # | Idea | Rating | Where it already exists |
| --- | --- | --- | --- |
| 1 | Microkernel with capabilities | Exists | seL4, Fuchsia (Zircon), Genode, QNX, Redox |
| 2 | Kernel allocates nothing, user space turns raw memory into objects | Exists | seL4 |
| 3 | Time budgets, servers run on the caller's budget | Exists | seL4 MCS |
| 4 | Scheduler aware of performance and efficiency cores | Exists | Windows and Linux with Intel Thread Director, macOS |
| 5 | Each driver its own restartable program; GPU crash costs a black frame | Exists | MINIX 3, QNX; Windows already recovers from GPU crashes (TDR) since 2006 |
| 6 | Written in Rust | Exists | Redox, Theseus, Hubris, Tock |
| 7 | Formal proofs (TLA+, Kani) in CI | Exists | seL4 (much deeper), AWS uses TLA+ |

### Storage and history (the heart)

| # | Idea | Rating | Where it already exists |
| --- | --- | --- | --- |
| 8 | Copy-on-write, checksummed file system | Exists | ZFS, btrfs, APFS |
| 9 | Continuous history with thinning and a size budget | Exists | Time Machine (thinning), NILFS2 (continuous snapshots), Windows File History |
| 10 | Storage classes (documents full, games one step, caches none) | Combination | Pieces in btrfs (no-CoW files), Time Machine exclusions |
| 11 | **Every change signed by the app that made it** | Combination / research | Provenance file systems (PASS, Harvard, 2006 to 2009); no shipping OS |
| 12 | **Undo everything one app did, leave the rest** | Combination / research | MIT's Retro (2010) and "undo computing" research; no shipping OS |
| 13 | **Branch the whole machine to try an app or update, then keep or throw away** | Combination | Qubes disposable VMs, Windows Sandbox, Sandboxie, Deep Freeze, NixOS and Silverblue generations; none let you keep part of a trial |
| 14 | Opt-in replay of what an app read | Combination / research | rr (Mozilla), Retro |
| 15 | History ordered by a counter, not the clock | Exists | ZFS transaction groups, every database |
| 16 | File watching from the change stream, no limits, resume from a point | Exists | macOS FSEvents (resume from an event ID), NTFS change journal |
| 17 | Auto-save, "Save" becomes "name this version" | Exists | macOS Versions (since 2011), iOS, Google Docs |
| 18 | Forget by destroying a per-file key, reaching all history and backups | Combination | Per-file keys: Apple Data Protection; "crypto-shredding" is standard in cloud storage; never as a desktop "forget" across history |

### Grants and privacy

| # | Idea | Rating | Where it already exists |
| --- | --- | --- | --- |
| 19 | Apps see no file tree; picking a file in Open/Save grants it | Exists | macOS and iOS sandbox, Flatpak portals, Android's storage picker, Sandstorm, Genode, Capsicum |
| 20 | Grants that expire ("until I close the app", "until a time") | Exists | Android's temporary URI grants, macOS security-scoped bookmarks, capability tokens with expiry (Macaroons) |
| 21 | **Grants pinned to a past moment ("my photos as of last Tuesday")** | **New** | Closest: Plan 9 could bind a dated snapshot into a program's view, but nobody built grants around it |
| 22 | A log of what each app opened, changed and connected to | Combination | Android privacy dashboard, macOS logs, Little Snitch, Linux auditd; never one full, readable, per-app record |
| 23 | Leak guard: reading a private file means asking before going online; the mark spreads | Combination / research | HiStar, Flume, Asbestos (MIT/Stanford research), TaintDroid on Android; no shipping desktop OS |
| 24 | Permission screen of live grants, camera and mic light | Exists | iOS, Android, macOS |
| 25 | Each person has their own key; admin cannot read their files | Exists | Android per-user encryption, systemd-homed, macOS |
| 26 | Full-disk encryption with TPM and PIN, recovery key | Exists | BitLocker, FileVault, LUKS |

### Reliability, updates, everyday use

| # | Idea | Rating | Where it already exists |
| --- | --- | --- | --- |
| 27 | Updates on a branch with automatic fallback | Exists | ChromeOS and Android A/B updates, Fedora Silverblue, NixOS |
| 28 | Recovery system on the disk plus USB stick | Exists | Windows RE, macOS Recovery |
| 29 | Out of memory: squeeze, ask apps, close the least important; no swap | Exists | iOS (jetsam), Android, macOS memory compression |
| 30 | Background apps frozen unless allowed | Exists | iOS, Android |
| 31 | Signed apps from anywhere, installed on a branch you approve | Combination | Signed bundles exist everywhere; installing on a reviewable branch does not |

### Games

| # | Idea | Rating | Where it already exists |
| --- | --- | --- | --- |
| 32 | Game mode (pause background work, silence notifications) | Exists | Windows Game Mode, Feral GameMode, SteamOS |
| 33 | Gentle heat control before hardware throttling | Exists | Android's thermal framework, Steam Deck |
| 34 | Compositor with shared buffers, direct scanout, VRR, HDR | Exists | Wayland compositors, Windows DWM |
| 35 | Roll back a game update; mods in a personal layer | Combination | Mod Organizer 2's virtual file system, Linux overlay file systems, Steam beta branches |

### AI tools

| # | Idea | Rating | Where it already exists |
| --- | --- | --- | --- |
| 36 | An AI tool's whole session on a side branch, undone as one group | Combination | Containers, dev containers, git worktrees, sandboxed agent tools; never built into the OS's file history |
| 37 | **The OS treats an AI agent as something to record, contain and roll back as a first-class citizen** | **New (as an OS design goal)** | Agent sandboxes exist as add-ons; no OS is designed around them |

### Count

- **New: 2** (rows 21 and 37)
- **Combination or research only: 11** (rows 10, 11, 12, 13, 14, 18, 22, 23, 31, 35, 36)
- **Exists: 24** (all other rows)

The timeline that shows files and the desktop layout together (decision 34) sits on top of rows 9 and 11; close relatives are Windows Timeline (2018), Time Machine and the 1996 research desktop "Lifestreams", so I did not count it separately.

### What this means

The honest headline is: **Alaya has 2 genuinely new ideas and 11 that no shipping OS has, built on 24 proven ones.** The research ideas (11, 12, 14, 23) came from MIT, Harvard and Stanford papers 10 to 20 years ago and never reached a real desktop. Bringing them to a real, usable desktop is a legitimate kind of newness. Using proven ideas for the kernel is a strength, not a weakness: nobody should invent a new way to schedule threads just to be different.

A small honesty fix: the "How it differs" tables in `principles-design.md` (now in public PR #7) overstate some gaps. Windows recovers from GPU driver crashes without a blue screen (since Vista); macOS has auto-save and versions (since 2011) and keeps local hourly snapshots; macOS FSEvents already resumes from a point. I suggest correcting those rows before the PR merges, so the public claims stay true.

### Where Alaya could be more new

These all build on the store you have already designed, so they cost far less than drivers:

1. **A branch that also holds what leaves the machine.** Today, a side branch can undo file changes, but not an email already sent or a file already uploaded. If a branch also held its outgoing network messages in an "outbox" until you accept the branch, trying an app or letting an AI agent work would become truly safe. I found nothing that does this. This is the strongest idea for the AI era.
2. **"What happened?" as a first-class question.** The ledger plus signed history can answer "what changed on my computer since yesterday, and which app did it?" in plain words. No OS can answer that today.
3. **Time-limited grants for everything, not only files.** "This app may use the microphone until 6 pm", "this game may reach this one server", "this AI session may spend this much network". Same mechanism, new reach.
4. **Real merges, not "keep both".** If Alaya apps describe their documents' structure, the store could merge two branches of a document properly, like git does for code. Today the design falls back to "keep mine or theirs".
5. **The agent-native OS.** Make idea 37 the headline: every AI agent session gets a branch, a budget, a list of grants and a reviewable "diff of the world" before you accept it. This is where the market is going and nobody has an OS built for it.

---

## Part 4. Do the principles contradict each other?

Six real tensions. None is fatal; each needs a fix in wording or design.

1. **"Nothing gets in by default" vs open network, open clipboard and a full-access terminal** (principle 2 vs decisions 25, 38, 40). The headline promises more than the design does. Any app given one file can send it anywhere. *Fix:* reword principle 2's headline to "Apps only see what you hand them, and you can see everything they did", which is true.

2. **The private mark spreads to everything.** The leak guard marks any program that receives data or shared memory from a marked app (decision 27). But every app shares buffers with the compositor, and every app writes through the store. So after the first private file is opened, the compositor and the store are marked, and through them every app. This is the known problem of the research systems (HiStar, Flume). *Fix:* name a few trusted parts (compositor, store, clipboard server) that carry data without being marked, and write that rule down.

3. **"Admin cannot read another person's files" vs classic admin who "can change anything"** (decision 59). An admin who can change the system can change the login screen to capture the other person's PIN. The per-person keys protect against an admin reading files directly, not against a determined admin. *Fix:* say this honestly, or make changes to the login and key parts need every person's approval. A light fix: tell each person at login when the system's login part has changed.

4. **"See your files at any moment within a tenth of a second" vs thinning** (principle 4 vs decision 33). After a day, only hourly moments are kept; after a month, weekly. *Fix:* reword to "at any kept moment".

5. **Principle 1's "every decision that keeps an old habit says why" vs principle 6's Windows games and principle 7's Linux tools.** Both bring in the whole old Unix and Windows world. That is a reasonable choice, but inside those worlds old habits come back (the POSIX layer gives ported programs a fake `/`, `fork`, the terminal sees everything). *Fix:* none needed beyond saying so: "old programs live in a fenced-off old world; new apps get the new rules".

6. **Steady performance (rank 2) vs the leak guard and ledger (privacy, rank 4) on the hot path.** Logging every read and connection costs time on every file read and every network packet. The clash order already says privacy bends for games, which resolves it. *Fix:* the ledger must record in batches on a background budget, and this should be written as a rule before milestone 7.

One more small point: the principles say "a graphics crash costs at most one black frame". That is a goal for a driver that does not exist yet (NVIDIA through GSP). It should be listed as a target rather than a promise until milestone 12.

Everything else is consistent. The clash order (safe data, steady, games, privacy) is clear and the examples in `principles.md` apply it correctly.

---

## Part 5. Verdict and recommendations

### Verdict

**The concept is sound, genuinely different as a whole, and worth building.** Its core (remembering store plus grants) is new as a combination, technically possible, and buildable by a small team inside QEMU. **The full promise (an everyday NVIDIA gaming laptop with Steam) is the risky part,** and it is risky for every new OS, not because of Alaya's design. The newness lives in the parts that are possible; the risk lives in the parts that are not new. That is a good position to be in, if the plan puts the new parts first.

### Recommendations

1. **Re-decide how Claude Code runs inside Alaya.** The rebuild-Node plan no longer works. My recommendation: a "Linux world" (option A) that runs the real Claude Code program, with all of Alaya's undo and logging around it. Keep the bash, git and ripgrep ports for the terminal. (Decision card.)
2. **Show the new idea early.** Move a small version of history and grants (signed changes, per-app undo, a simple timeline, the Open dialog as a grant) right after storage, on a simple desktop. Move HDR, multi-monitor hotplug, input methods and full accessibility to after milestone 7. This gives you something nobody else has much sooner, and tests the riskiest new code earlier. (Decision card.)
3. **Fix the six tensions in Part 4**, mostly in wording, before PR #7 merges, and correct the over-strong comparison rows.
4. **Before milestone 11, study borrowing Linux drivers** as separate programs (FreeBSD LinuxKPI, Genode style), especially for Wi-Fi and graphics. Keep our own kernel.
5. **Lead with the AI-era ideas** in Part 3 ("branch with an outbox", "what happened?", the agent-native OS). They are the cheapest way to add more genuinely new things, and they build on what is already designed.
6. **Treat the frame-time gates as goals for real hardware, and keep relaxed gates in QEMU**, so driver maturity is not mistaken for failure of the concept.

---

## Sources

- Claude Code setup (native install, no Node, npm installs the same binary): https://code.claude.com/docs/en/setup.md
- NVK vs NVIDIA's driver, Mesa 26.2: https://phoronix.com/news/NVK-Mesa-26.2-Performance
- Windows games on Linux (almost 90 % launch) and anti-cheat: https://www.techradar.com/computing/windows/new-data-shows-most-windows-games-now-work-on-linux-and-i-hope-the-same-will-soon-apply-to-anti-cheat-titles
- Anti-cheat status list: https://www.gamingonlinux.com/anticheat/updates
- Redox in 2026 (Vulkan and Node.js; real hardware work): https://phoronix.com/news/redox-os-february, https://phoronix.com/news/redox-os-april-2026
- Retro, selective undo (MIT, OSDI 2010): https://people.csail.mit.edu/nickolai/papers/kim-retro.pdf ; Undo computing: https://css.csail.mit.edu/undo
- Provenance-aware storage (PASS): https://static.usenix.org/events/fast09/tech/full_papers/muniswamy-reddy/muniswamy-reddy.pdf
- FreeBSD LinuxKPI (Linux graphics and Wi-Fi drivers on FreeBSD): https://wiki.freebsd.org/LinuxKPI , https://github.com/freebsd/drm-kmod , https://www.freebsd.org/status/report-2026-01-2026-03/lkpi-wireless/
- Other systems named (seL4, Fuchsia, Genode, Plan 9, Qubes, NixOS, Silverblue, MINIX 3, HiStar, Flume, TaintDroid, Lifestreams) are cited from well-known published work.

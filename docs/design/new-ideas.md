# Alaya: where we can do something genuinely new

19 November 2026. A search across every part of the OS for things no existing OS does, and no research system has fully done. Follows `concept-analysis.md`, which found that Alaya today has 2 genuinely new ideas, 11 new combinations and 24 borrowed ones.

**How to read this.** Each idea has: what you would experience, what already exists (prior art), how new it really is, how hard it is, and how it fits the 8 principles. At the end the ideas are ranked.

Ratings for newness:
- **New**: I found nothing that does it, in products or research.
- **Nearly new**: only a research paper or proposal, never in a real OS.
- **New combination**: the pieces exist in different places, never together.

Difficulty is judged on top of what Alaya already plans (the remembering store, grants, ledger and branches), because most of these ideas reuse them.

---

## The big insight first

Almost every new idea below comes from the same place: **Alaya is the only OS that knows who changed what, when, and with which permission.** Other systems would need to rebuild their whole storage to get this. So the cheapest way to be new is to use that knowledge in more places, not to invent new kernel tricks.

---

## 1. Apps and safety

### 1.1 The outbox: trying things safely, including what leaves the computer

**What you experience.** You run a new app, an installer or an AI helper "on the side". Today's plan already lets you throw away its file changes. With the outbox, anything it tries to send out (an email, an upload, a message, a payment page) also waits in a list. At the end you see: "This app changed 4 files and wants to send 2 emails and upload 1 file to example.com." You tick what to allow. Nothing left your computer until you said so.

**Prior art.** Research systems in 2005 and 2006 (Speculator and "Rethink the Sync", University of Michigan) held back a program's output until a disk write was safe, but only for speed, not for your review. A 2026 research system (DeltaBox) can undo an AI agent's files and processes in milliseconds, but not its network sends. A 2026 research proposal (AgentOS) blocks suspicious agent actions, but does not hold them for review. Rubrik's "Agent Rewind" (2025) undoes agent damage afterwards, in company backups. Essays on AI safety recommend "outbox" states, but only inside each tool.

**How new.** **New.** I found no OS that holds a program's outgoing network effects together with its file changes for you to review.

**Hard?** Medium to hard. Holding back network traffic is easy for things like email and uploads; live connections (a video call, a game) can't wait, so those apps would be told "no network on the side" or be asked about up front.

**Fits principles.** 2 (private), 3 (can't break), 4 (files remember), 7 (built to build itself). Clash: games are never run on the side with an outbox, so principle 6 is unaffected.

### 1.2 The AI helper contract

**What you experience.** When you start any AI helper (Claude Code or another), Alaya gives it a "contract" you can read in one screen: which folders it can see, until when, how many emails or uploads it may send without asking (for example zero), how much it may download, and whether it works on the side. At the end you get a plain summary of everything it did, and you can undo all of it at once.

**Prior art.** AI tools have their own sandboxes and permission prompts (Claude Code, Codex, Operator). Research on "agent operating systems" (AIOS 2024, AgentOS 2026) schedules agents but does not give the person this kind of contract. No OS treats an AI helper as its own kind of program with its own limits.

**How new.** **New** as an OS feature.

**Hard?** Medium. It is mostly the outbox (1.1), grants with time, and the ledger, put together under one screen.

**Fits principles.** 2, 4, 7. This is where the market is going, and no OS is designed for it.

### 1.3 Grants with time for everything, not only files

**What you experience.** "This app may use the microphone until 6 pm." "This game may reach only its own servers." "My friend's app may read this folder for the afternoon." Everything you hand over can have an end.

**Prior art.** Phones have "allow once" and "only while using the app" (iOS, Android). Android has short-lived file grants. Capability tokens with an expiry exist on the web (Macaroons). Nobody has one rule for everything with any end time you choose.

**How new.** New combination.

**Hard?** Easy to medium: the grant system already has "until".

**Fits principles.** 2. Small risk to 6 if a game loses the microphone mid-match; fix by warning before a grant runs out.

---

## 2. Files and history

### 2.1 "What happened?": your computer tells its own story

**What you experience.** You ask "What changed since yesterday?" and get plain sentences: "The photo editor changed 12 photos at 14:02. The update installed on the side and is waiting. Claude Code changed 3 files in your project and sent 2 of them online." Each line has an undo button.

**Prior art.** Windows Recall and Rewind take screenshots of what you saw; they don't know which app changed what. Logs exist (macOS, Linux auditd), but are for experts. Provenance research (Harvard's PASS) recorded this, but never showed it to people.

**How new.** **Nearly new.** The data was recorded in research; nobody turned it into a story for ordinary people with undo on every line.

**Hard?** Medium. The ledger already holds the facts; the hard part is writing good plain summaries.

**Fits principles.** 1 (new kind), 2 (you can see everything they did), 4.

### 2.2 Undo a mistake and everything that came from it

**What you experience.** You imported a broken spreadsheet. Since then, two other apps copied numbers from it into a report and a chart. You say "undo that import". Alaya shows you the report and chart were affected too, and offers to roll them back or redo them with the right file.

**Prior art.** MIT's Retro (2010) and "undo computing" research did this for intrusions on servers. Rubrik maps agent actions to causes in backups. No desktop OS.

**How new.** **Nearly new** (research only, never on a desktop).

**Hard?** Hard. The ledger knows which app read what before it wrote what, so "what came from this" can be found; redoing work automatically is research-hard, so offer "show and roll back", not "redo".

**Fits principles.** 3, 4.

### 2.3 Run it again, exactly as last Tuesday

**What you experience.** A report you made last week came out right; this week it's wrong. You ask Alaya to open the same app with exactly the files it saw last Tuesday (pinned grants). You compare the two side by side.

**Prior art.** Pinned grants are already Alaya's new idea. Reproducible builds (Nix) do this for software, not your data. Debugging tools (rr) replay one run.

**How new.** **New** (it builds on pinned grants, which nobody else has).

**Hard?** Easy to medium: it is mostly pinned grants plus the ledger's record of what was read.

**Fits principles.** 4, 7.

### 2.4 Search the past

**What you experience.** "Find the version of my essay that still had the paragraph about the trip." Search looks inside every kept version, not only today's files.

**Prior art.** macOS can search old versions with extra steps (2026 how-to articles show it is awkward). SharePoint and Figma users ask for it. Windows semantic search looks only at current files.

**How new.** New combination.

**Hard?** Medium (index size). Fits 4. Private files stay out of the index for others.

### 2.5 Real merges for documents

**What you experience.** You edited a document on the side while the original changed too. Instead of "keep mine or theirs", Alaya merges both edits, like Google Docs does for two people at once.

**Prior art.** Merging libraries for apps exist (Automerge, Loro, "local-first software" from Ink & Switch). No OS offers it for every app's documents.

**How new.** New combination.

**Hard?** Hard: apps must store documents in a mergeable format. Only Alaya apps would get it.

**Fits principles.** 4.

---

## 3. Privacy

### 3.1 The privacy receipt

**What you experience.** After you close an app, you can open a receipt: "Opened 3 files, sent 1.2 MB to 2 places, used the camera for 40 seconds." After "forget", you get a receipt that the file is unreadable everywhere, including backups.

**Prior art.** iOS's App Privacy Report shows camera and network use per app. Nobody gives file-level receipts or proof of forgetting.

**How new.** New combination.

**Hard?** Easy: it is a view of the ledger.

**Fits principles.** 2, 4.

### 3.2 Lend a file, not a copy

**What you experience.** You give another person on the same computer (or a guest) one file or folder for an afternoon. When time runs out, it disappears from their side, because they never had a copy, only a grant.

**Prior art.** Cloud share links can expire (Google Drive, Dropbox). Inside one computer, sharing between people means copying.

**How new.** New combination.

**Hard?** Easy to medium: grants with time between people. Fits 2.

---

## 4. Games

### 4.1 A promise of smooth play you can see

**What you experience.** When a game starts, Alaya promises it a share of time per frame and shows a small badge: "Smooth play guaranteed: 99.9 % of frames on time in the last hour." If something breaks the promise, it says what (heat, another app, the driver).

**Prior art.** Real-time research kernels give time reservations. Android has hints for games (ADPF). Tools like MangoHud show frame times. No OS makes a promise and explains why it was broken.

**How new.** New combination.

**Hard?** Medium: time budgets are already in the kernel (milestone 3). Explaining causes needs the ledger and timing data.

**Fits principles.** 5, 6.

### 4.2 Save anywhere, for any game

**What you experience.** Any game can be saved at any moment, like the save states in console emulators, and resumed later, even after a restart.

**Prior art.** Emulators do it for old consoles. CRIU and new GPU checkpointing (CRIUgpu, NVIDIA's cuda-checkpoint) do it for servers and AI jobs. Nothing does it for PC games with live graphics.

**How new.** **New** for games.

**Hard?** Very hard: the graphics card's state and online games make it unreliable. Could work for single-player games on our own graphics path, years from now.

**Fits principles.** 6, but risky for 3 (a bad restore could crash the game). Not recommended now.

---

## 5. Desktop, input and accessibility

### 5.1 Helpers that use apps through grants

**What you experience.** A screen reader, a voice control tool or an AI helper can "see" and click inside only the windows you grant it, and every click it makes is in the log and can be undone.

**Prior art.** Accessibility interfaces (Windows UI Automation, macOS Accessibility) let helper programs see and control every app at once; AI agents use them today ("computer use"). None limits a helper to chosen windows.

**How new.** New combination.

**Hard?** Medium: the accessibility tree is already planned in the toolkit (decision 42).

**Fits principles.** 2, 7, and accessibility.

### 5.2 The desktop as it was

**What you experience.** Drag the timeline back and the whole desktop returns as it was on Monday morning: the same apps, windows and files, opened read-only. Click "continue from here" to make it your present.

**Prior art.** Windows Timeline (2018, removed), macOS reopens windows after restart, the 1996 research desktop Lifestreams. None restores the desktop at any past moment together with the files as they were.

**How new.** New combination, already partly in the plan (`timeline.md`). Listed so the "continue from here" step is not forgotten.

**Hard?** Medium. Fits 1, 4.

---

## 6. Updates, installs and many devices

### 6.1 "Explain this update" before you accept

**What you experience.** An update waits on the side. Before you accept, you see in plain words: "Changes 3 settings, replaces the photo app, adds network access for the music app." You can refuse one part.

**Prior art.** NixOS can list package differences between versions (for experts). Phones list new permissions for one app. Nobody explains a whole-system update in plain words with partial refusal.

**How new.** New combination.

**Hard?** Medium (partial refusal is hard; explaining is easy, because the update is a branch).

**Fits principles.** 3, 2.

### 6.2 Your computer as a branch you can carry

**What you experience.** Your files, history and grants sync between your laptop and another Alaya machine directly, with no company cloud, like git for your whole life. Each machine is a branch; changes merge, conflicts are shown.

**Prior art.** iCloud and OneDrive sync files through a company. Syncthing syncs directly, with no history or grants. Git does it for code only.

**How new.** New combination.

**Hard?** Hard (needs network, merging, encryption between machines). Fits 2, 4. Later, after milestone 10.

---

## 7. Ideas checked and dropped

| Idea | Why dropped |
| --- | --- |
| A new kind of kernel scheduling | Already solved well by seL4's time budgets, which we use |
| Files that delete themselves after a while | Exists (Windows Storage Sense, macOS) and risks principle 3 |
| Guest mode on a throwaway branch | Exists (ChromeOS guest mode) |
| Per-app energy budgets | Exists in iOS and Android |
| A verified (proven correct) kernel | Exists (seL4); far too costly for us to match |

---

## 8. Ranking

Score: newness (New 3, Nearly new 2, New combination 1) plus value to you (1 to 3) minus difficulty (Easy 0, Medium 1, Hard 2, Very hard 3).

| Rank | Idea | Newness | Value | Difficulty | Score |
| --- | --- | --- | --- | --- | --- |
| 1 | 1.2 The AI helper contract | New | 3 | Medium | 5 |
| 2 | 1.1 The outbox | New | 3 | Medium-hard (1.5) | 4.5 |
| 3 | 2.3 Run it again as last Tuesday | New | 2 | Easy-medium (0.5) | 4.5 |
| 4 | 2.1 "What happened?" | Nearly new | 3 | Medium | 4 |
| 5 | 3.1 Privacy receipt | New combination | 2 | Easy | 3 |
| 6 | 4.1 Smooth play you can see | New combination | 3 | Medium | 3 |
| 7 | 5.1 Helpers through grants | New combination | 3 | Medium | 3 |
| 8 | 1.3 Time on every grant | New combination | 2 | Easy-medium | 2.5 |
| 9 | 3.2 Lend a file | New combination | 2 | Easy-medium | 2.5 |
| 10 | 4.2 Save any game | New | 2 | Very hard | 2 |
| 11 | 6.1 Explain this update | New combination | 2 | Medium | 2 |
| 12 | 2.2 Undo what came from a mistake | Nearly new | 2 | Hard | 2 |
| 13 | 2.4 Search the past | New combination | 2 | Medium | 2 |
| 14 | 5.2 The desktop as it was | New combination (planned) | 2 | Medium | 2 |
| 15 | 2.5 Real merges | New combination | 2 | Hard | 1 |
| 16 | 6.2 Carry your computer | New combination | 2 | Hard | 1 |

The AI helper contract scores highest, but it needs the outbox, so the two belong together.

## 9. Recommendation

**Decided 19 November 2026 (decision 68):** "What happened?" (2.1) and "Run it again" (2.3) join the design in milestone 7. The outbox (1.1) and the AI helper contract (1.2) wait for later.


Adopt the top four as one package, **"Alaya knows, shows and can hold back"**:

1. **The outbox** (hold anything that leaves the computer while on the side),
2. **The AI helper contract** (one screen of limits, one summary, one undo),
3. **"What happened?"** (the computer tells its own story),
4. **Run it again as last Tuesday** (pinned grants plus the ledger).

Together they would take Alaya from 2 to **about 6 genuinely new ideas**, they reuse what is already designed (store, grants, ledger, branches), and they fit milestone 7 (history and grants) and 8b (Claude Code inside). None of them needs new drivers, so the risk found in the analysis does not grow.

The easy wins (privacy receipt, time on every grant, lend a file) can be added along the way with little cost. Saving any game is new but should wait for years.

## Part B. Bolder ideas for every day

Added 19 November 2026, after you asked for ideas that could change how people use a computer every day, not only safety features. Same method: a daily scenario, honest prior art, difficulty, and fit with Alaya's foundations.

### B1. Tasks, not apps: the computer organised around what you are doing

**Your day.** You have three things going on: "Flat move", "Thesis" and "Gaming". Each is a task. A task holds its windows, files, web pages, notes and history. You switch from Thesis to Flat move and the whole desktop changes: the rental contract, the moving company's email, the spreadsheet of costs, exactly where you left them on Tuesday. Thesis waits, paused, using no memory or battery, for a day or a month, and comes back exactly as it was.

The Alaya twist: **an app opened inside a task can see that task's files, and nothing else.** You never get a permission question for them, because putting a file in the task is the permission. Privacy comes for free from how you organise your work. Every task has its own timeline ("Thesis as it was before I rewrote chapter 2").

**Prior art.** KDE Activities (Linux, since about 2010, still in Plasma in 2026) groups windows and some files, but apps can still see everything and it has no history. macOS Spaces and Stage Manager group windows only. Windows "Sets" (tabs of apps by task) was cancelled in 2018. Browser workspaces (Arc, Edge) do it for tabs only. Nobody ties windows, files, permissions, history and pausing together.

**How new.** New combination, and a big one: it changes the basic unit of the desktop from "app" to "what I'm doing".

**Hard?** Medium to hard. Grants, history and branches already exist in the design. Pausing a whole task needs apps to save and restore their state, which Alaya apps do already (auto-keep, decision 58). Older Windows and Linux apps would be closed and reopened instead.

**Fits.** Principles 1 (new kind), 2 (privacy without prompts), 4 (task timelines), 5 (paused tasks use nothing).

### B2. One undo for the whole computer

**Your day.** You dragged a folder to the wrong place, changed a setting, and closed a window you still needed. You press one key, "undo for the computer", three times, and all three come back, in order. A small list shows what each press will undo. It works for moving, renaming and deleting files, settings, installing apps, closing windows and tabs, even days later.

**Prior art.** Windows Explorer and macOS Finder can undo the last few file moves, only in their own window and only until you close it. Windows System Restore rolls back the system in one big step. Browsers reopen closed tabs. Research on shared undo exists. No OS has one undo for everything you did.

**How new.** New combination, used many times a day.

**Hard?** Medium. Everything is already in the store's history and the ledger; this is a new way to walk it, newest first. Reopening a closed window needs the app to restore its state (as in B1).

**Fits.** Principles 3 (can't break) and 4 (files remember).

### B3. Ask your computer, and it answers from what really happened

**Your day.** You type "the flat contract Anna sent me last month". Alaya answers with the file and its story: "came in an email on 3 October, saved by the mail app, edited by you on 5 October". Click it and it opens in the app you choose, which gets only that file. You can also ask "what was I working on last Thursday afternoon?" and get the task, files and windows from that moment.

**Prior art.** Windows Recall (2024 to 2025) takes screenshots every few seconds and searches them; it was criticised heavily for privacy. Windows 11's semantic search and Apple Intelligence's personal context search current files and messages. Rewind did screen recording and was shut down. None knows which app made which file from which source; they guess from pictures or text.

**The Alaya twist.** No screenshots: answers come from the real record of what each app did (the ledger), run on your machine, never shown across people, and private files stay hidden unless you ask as their owner. The answer is a grant, so finding a file and giving it to an app are one step.

**How new.** New combination. The search part exists; answering from a true record of origins does not.

**Hard?** Medium to hard. Needs a small language model running locally (on the graphics card, so after milestone 12 for good speed; a simpler keyword version can come earlier).

**Fits.** Principles 2 (local, private) and 4. Builds directly on decision 68 ("What happened?").

### B4. Every file knows where it came from

**Your day.** You right-click a photo and see its life: "downloaded from example.com on 2 May, cropped in the photo editor, sent to Claude Code on 4 May, attached to 2 emails". One more click shows "everything made from this file".

**Prior art.** Windows marks downloaded files with their website ("Mark of the Web"); macOS keeps a "Where from" note. Both stop at the first step. Provenance research (Harvard's PASS) recorded full chains but never showed them to people.

**How new.** New combination. Easy, because the ledger already records it; this is a view. It also makes B3 possible.

**Fits.** Principles 2 and 4.

### B5. Notifications that belong to tasks

**Your day.** In the Thesis task, only things about the thesis reach you (your supervisor's emails, the shared document). Messages for Flat move wait inside that task and you see them when you switch to it. Nothing is lost and nothing interrupts the wrong work.

**Prior art.** iOS and macOS Focus modes filter by app and person. Nothing filters by the work the message belongs to.

**How new.** New combination. Easy once B1 exists, because notifications already need a grant (decision 46).

**Fits.** Principles 5 (calm) and 1.

### B6. Live links between apps, which never break

**Your day.** You drag a chart from your budget spreadsheet into your report. It stays live: change the numbers and the report's chart follows. Or you pin it, "as of Friday", and it never changes. Links never break, even if you rename or move the spreadsheet, because they point to the file itself through history, not to a path.

**Prior art.** Windows OLE (1990s) and Apple's OpenDoc (cancelled 1997) tried this; they were fragile and apps had to do a lot of work. Google Docs has linked charts, but only inside Google. Ordinary links between files break when files move.

**How new.** New combination, a fixed version of an old dream. Live and pinned links are just Alaya's live and pinned grants.

**Hard?** Hard: apps must support showing a piece of another app's document. Only Alaya apps.

**Fits.** Principles 1 and 4.

### B7. Your Alaya machines as one computer

**Your day.** You start a task on your desktop, then pick up the laptop on the train and the task is there, with its files, windows and history, synced directly between your machines without a company's cloud.

**Prior art.** Apple Handoff and iCloud (through Apple's servers, app by app), Syncthing (files only, no history), git (code only).

**How new.** New combination. Hard (network, merging, encryption between machines); it needs milestone 10 at least.

**Fits.** Principles 2 (no company cloud) and 4.

### Ranking of Part B

Same scoring as Part A (newness + everyday value − difficulty), with "everyday" weighted higher as you asked.

| Rank | Idea | How often you'd use it | Newness | Difficulty |
| --- | --- | --- | --- | --- |
| 1 | B1 Tasks, not apps | All the time; it is the desktop | New combination, big | Medium-hard |
| 2 | B2 One undo for the whole computer | Several times a day | New combination | Medium |
| 3 | B3 Ask your computer | Daily | New combination | Medium-hard |
| 4 | B4 Every file knows where it came from | Weekly; powers B3 | New combination | Easy |
| 5 | B5 Notifications that belong to tasks | All the time, quietly | New combination | Easy after B1 |
| 6 | B6 Live links between apps | Weekly for office work | Old idea done right | Hard |
| 7 | B7 Your machines as one computer | Daily if you own two | New combination | Hard, later |

**Decided 19 November 2026 (decision 69):** B1 (tasks), B2 (one undo) and B4 (file stories) join the design in milestone 7. The others wait.

**Recommendation.** B1, B2 and B4 together. Tasks change what the desktop is; one undo makes it feel safe to try anything; file stories are cheap and make the history visible. B3 (asking) builds on B4 and decision 68, and can follow once a local AI model runs well. All of them sit on the store, grants and ledger, so none needs new drivers.

## Part C. Lower-level ideas: kernel, memory, scheduling, drivers, power, network

Added 19 November 2026. These are ideas deep inside the system. Most people would never see the machinery, so for each one I say what you would *notice*.

A note on honesty first: at this level almost everything has been tried in a research lab. Most ideas below are "nearly new" (research only, never in an OS people use) rather than brand new. The good news: Alaya's microkernel design (everything is a message, every request runs on the caller's time budget, every change is in the store) makes several of these research ideas much easier for us than for Linux, Windows or macOS.

### C1. A computer that never really turns off

**What you notice.** There is no "shut down" and no "restore your session". Power cut, empty battery, kernel crash: you turn it on and everything is running exactly where it was, including apps in the middle of something. Turning on takes as long as waking from sleep.

**How.** The store already keeps everything on disk safely. This extends it to running programs: every few seconds, the memory of programs that changed is written into the store as part of a normal commit, so the whole running machine is always on disk.

**Prior art.** KeyKOS (1980s) and EROS (late 1990s) did exactly this ("orthogonal persistence") on research and niche systems; Phantom OS too. Hibernation saves memory once, on purpose. No modern desktop OS does it continuously.

**How new.** Nearly new: proven in research 30 years ago, never on a modern desktop with a GPU.

**Hard?** Hard. The graphics card and network connections cannot be frozen, so games and live calls restart; Alaya apps (which already keep their state, decision 58) come back exactly.

**Fits.** Principle 3 (can't break) and 5. Must never slow a game: the background writing runs on its own budget and pauses in game mode.

### C2. Updates that never need a restart

**What you notice.** Updates install while you work. Drivers, the store, the desktop, even the kernel are swapped live. You never see "restart to finish updating".

**How.** In Alaya, drivers and system parts are ordinary programs. To update one, start the new version, hand it the old one's state, and redirect its messages. The kernel is small, so it can be swapped by saving all kernel objects and loading the new kernel in place.

**Prior art.** MINIX 3's live update research (Vrije Universiteit, 2013 to 2016) updated whole OS parts without a reboot, in the lab. Linux live-patches small kernel fixes (Ksplice, livepatch), but not whole new versions. Phones and Windows still restart.

**How new.** Nearly new for a desktop OS.

**Hard?** Medium for drivers and servers (they already restart cleanly, decision 19); hard for the kernel itself.

**Fits.** Principle 3. It also replaces part of decision 61 ("updates apply at next restart").

### C3. Smooth frames by deadline, across every program

**What you notice.** Games and the desktop feel snappier and steadier: less delay between pressing a key and seeing the result, fewer missed frames, especially when the computer is busy.

**How.** The screen knows when its next refresh is. That time becomes a deadline that travels with every message: the key press, the game's frame, the GPU driver, the compositor. Each program in the chain runs before its share of the deadline, on the game's time budget. If the chain is going to miss, Alaya knows which link was slow.

**Prior art.** Linux's graphics stack added "deadline hints" on GPU fences in 2023. Linux has a deadline scheduler (SCHED_DEADLINE) for single programs. NVIDIA Reflex reduces game input delay inside the game. Nobody passes one deadline through every program in the chain, because on Linux or Windows the programs don't share time budgets. In Alaya they do (decisions 11 and 55).

**How new.** New combination; I found no OS that does it end to end.

**Hard?** Medium. The time budgets are being built now (milestone 3).

**Fits.** Principles 5 (steady) and 6 (games).

### C4. A battery screen that tells the truth

**What you notice.** The battery screen says "the video website in your browser used 30 % of the battery, including the graphics card and the Wi-Fi it caused". Today it would blame "System" or the graphics driver.

**How.** Every request in Alaya runs on the caller's budget, so the cost of the disk, network and graphics work can be billed to the app that asked for it, through every server in between.

**Prior art.** Cinder (Stanford, 2009 to 2011, research OS) billed energy to the right app and let you give apps energy budgets. Resource containers (1999) did similar for CPU on servers. Phones estimate per-app battery but miss work done by system parts.

**How new.** Nearly new (research only).

**Hard?** Medium. Measuring energy per device needs the hardware's counters (Intel's RAPL on the CPU, estimates elsewhere).

**Fits.** Principle 5, and fits game mode and the thermal governor (decision 45).

### C5. Driver crashes nobody notices

**What you notice.** Nothing at all, which is the point. Today's plan already restarts a crashed driver; with this, apps never even get an error. The file you were copying keeps copying.

**How.** Each driver's unfinished requests are kept by a small helper outside it. When the driver restarts, the helper sends them again.

**Prior art.** "Shadow drivers" (University of Washington, 2004, on Linux) did this in research. MINIX 3 restarts drivers but apps can still see errors.

**How new.** Nearly new.

**Hard?** Medium: it fits Alaya's message design neatly.

**Fits.** Principle 3.

### C6. A different internet identity for each task

**What you notice.** Websites and trackers can't link your "Gaming" task with your "Banking" task, because each task leaves the computer looking like a different device, with its own address and its own cookies.

**How.** The network server gives each task (decision 69) its own network address (IPv6 makes this possible on most networks) and, optionally, its own route through a privacy network.

**Prior art.** Qubes and Whonix separate identities per virtual machine; Tor Browser separates by website; Apple's Private Relay hides your address for Safari. None does it per task on an everyday desktop.

**How new.** New combination; it only makes sense because Alaya has tasks.

**Hard?** Medium (needs the network stack, milestone 10). Works best on IPv6 networks.

**Fits.** Principle 2. Games keep a normal identity so online play isn't slowed (principle 6 ranks higher).

### C7. A flight recorder for the whole computer

**What you notice.** When something goes wrong, a "replay" shows exactly what led to it. Crash reports you choose to send let the bug be fixed for sure, because it can be replayed.

**How.** In a microkernel, programs only talk through messages. Recording the messages (without file contents, and only when switched on) lets the whole system's last minutes be replayed.

**Prior art.** rr and Microsoft's Time Travel Debugging record one program. ReVirt (2002) recorded a whole virtual machine. No desktop OS records itself.

**How new.** Nearly new.

**Hard?** Hard: recording must cost almost nothing, so it is off by default or limited to the last minute.

**Fits.** Principles 3 and 7 (it makes Alaya easier to build with Claude Code). Must respect principle 2: off by default, never file contents.

### Checked and dropped

| Idea | Why |
| --- | --- |
| Files and memory as one ("single-level store") | Exists (IBM i/AS400, Multics) and makes games' memory slower |
| A trusted, unfakeable PIN box | Exists (Windows Ctrl+Alt+Del, Qubes window borders); worth doing, not new |
| Booting into any past system version | Exists (NixOS generations, ZFS boot environments); already in the plan (decision 28) |
| Preloading apps you'll probably open | Exists (Windows SuperFetch, macOS) |
| Proving the whole kernel correct | Exists (seL4); far too costly to match fully |

### Ranking of Part C

| Rank | Idea | What you notice | Newness | Difficulty |
| --- | --- | --- | --- | --- |
| 1 | C3 Smooth frames by deadline | Snappier, steadier games and desktop | New combination | Medium |
| 2 | C2 Updates that never need a restart | Never "restart to update" | Nearly new | Medium (kernel: hard) |
| 3 | C1 A computer that never turns off | Power cuts and crashes lose nothing that's running | Nearly new | Hard |
| 4 | C4 A battery screen that tells the truth | Know what really drains the battery | Nearly new | Medium |
| 5 | C5 Driver crashes nobody notices | Nothing breaks | Nearly new | Medium |
| 6 | C6 An internet identity per task | Less tracking between parts of your life | New combination | Medium |
| 7 | C7 A flight recorder | Bugs get fixed for sure | Nearly new | Hard |

**Decided 19 November 2026 (decision 70):** C3 (frame deadlines), C2 (updates without restart) and C5 (quiet driver crashes) join the design, mostly in milestone 4. The others wait.

**Recommendation.** C3, C2 and C5. They use exactly what makes Alaya's kernel different (time budgets that travel with messages, drivers as restartable programs), so they cost far less here than anywhere else, and you feel them every day: steady frames, no restarts, nothing breaking. C1 is the most spectacular but also the hardest; it can come after the store is proven (milestone 7 or later).

## Sources

- Speculator (2005) and "Rethink the Sync" (2006), University of Michigan: https://web.eecs.umich.edu/~pmchen/papers/nightingale05.pdf , https://www.usenix.org/legacy/event/osdi06/tech/nightingale/nightingale_html/index.html
- DeltaBox (2026), fast undo for AI agent sandboxes: https://arxiv.org/abs/2605.22781
- AgentOS (2026), proposal for an agent operating system: https://arxiv.org/html/2603.08938
- AIOS (2024), LLM agent operating system: https://arxiv.org/abs/2403.16971v2
- Rubrik Agent Rewind (2025): https://www.computerworld.com/article/4037957/ai-agents-make-mistakes-rubrik-has-figured-out-a-way-to-reverse-them.html
- Undo for agents at the tool layer (2026 essay): https://tianpan.co/blog/2026-05-17-undo-button-your-agent-assumes-exists
- Retro and undo computing (MIT): https://people.csail.mit.edu/nickolai/papers/kim-retro.pdf , https://css.csail.mit.edu/undo
- Searching document versions on macOS (2026): https://eclecticlight.co/2026/05/25/how-to-search-document-versions/
- Local-first software and Automerge: https://inkandswitch.com/local-first/ , https://martin.kleppmann.com/papers/automerge-mobiuk18.pdf
- GPU checkpointing (CRIUgpu, 2025): https://arxiv.org/html/2502.16631v1 ; NVIDIA cuda-checkpoint: https://github.com/NVIDIA/cuda-checkpoint
- KDE Activities (2026): https://blogs.kde.org/2026/01/17/streamline-plasma-with-activities-to-be-more-focused-and-productive
- Windows Recall and semantic indexing: https://pureinfotech.com/recall-ai-vs-search-windows-11/ , https://www.windowscentral.com/software-apps/windows-11/how-to-get-started-with-semantic-indexing-on-windows-11
- OpenDoc and OLE: https://en.wikipedia.org/wiki/OpenDoc , https://en.wikipedia.org/wiki/Object_Linking_and_Embedding
- Undo in operating systems: https://en.wikipedia.org/wiki/Undo
- Cinder energy-aware OS (Stanford): https://web.eecs.umich.edu/~prabal/teaching/eecs582-w12/readings/cinder.pdf
- MINIX 3 live update (Vrije Universiteit): https://download.vusec.net/papers/asplos-2013.pdf , https://download.vusec.net/papers/giuffrida-phdthesis.pdf
- Rebootless updates overview (University of Washington course notes): https://courses.cs.washington.edu/courses/cse451/18au/notes/adv-rebootless.pdf
- Linux GPU fence deadline hints (2023): https://lkml.rescloud.iu.edu/2303.2/06826.html
- Stream isolation in Whonix/Qubes and Tor: https://whonix.org/wiki/Stream_Isolation , https://spec.torproject.org/proposals/171-separate-streams.txt

## Part D. Living validity and protected outcomes (20 November 2026)

From your research note of 20 November. Two ideas join the design (draft, see `commitments.md`):

- **Living validity**: every result made from other files shows whether its sources changed since (up to date, needs a look, as of a date, not sure), across all apps and copy and paste. New combination; builds on B4 file stories and the ledger. First shown in milestone 6.
- **Protected outcomes**: a job with rules (what may change, what must stay unchanged, checks before keeping), enforced by Alaya for any program. It replaces 1.2 (AI helper contract). The outbox (1.1) comes back only in its honest form: Alaya apps can hold mail and uploads for your OK (milestone 10).
- **Experience Guardian** (narrow): an update that is clearly slower during its trial goes back, never a security fix. Extends decision 80.

# Alaya: the principles, designed in depth

Started 19 November 2026. We take one principle at a time. For each one: what it means in daily life, how it differs from Windows, macOS and Linux, the choices we make, and the weak spots we checked ("battle test"). The short list stays in `principles.md`.

---

## 1. A new kind of OS

### What it means in daily life

- **Monday.** You write a report for two hours. The laptop's battery dies. You turn it back on and the report is exactly as you left it, a few seconds ago. You never pressed Save.
- **Tuesday.** You install a photo editor. It asks for "your Photos folder". You say yes. It can see your photos, and nothing else: not your documents, not your other apps, not your browser passwords.
- **Wednesday.** The photo editor's update ruins your settings. You open the timeline, go back to Tuesday for that app only, and everything else stays as it is today.
- **Thursday.** You want to try a new game launcher, but you don't trust it. You install it "on the side". You play for an hour, decide you don't like it, and throw the whole side branch away. Nothing is left behind.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| What an app can see | Almost all your files | Most files, some folders ask first | All your files | Only what you hand it |
| Undo past a crash or bad update | System Restore, unreliable | Time Machine (local hourly snapshots, a drive for more); apps keep versions | Depends on setup | Built in, every few seconds, per app |
| Try an app, then remove it fully | Leftovers stay | Leftovers stay | Leftovers stay | Throw the side branch away, nothing stays |
| Who made each change | Unknown | Unknown | Unknown | Every change is signed by the app that made it |
| A driver crashes | The graphics driver recovers; other drivers often cause a blue screen | Often a restart | Often a freeze | Every driver restarts, you keep working |

### Old habits: what we drop and what we keep

Dropped (they cause real problems):
- Apps can read all your files.
- A file only exists as its latest version.
- Installing an app scatters files all over the system.
- An update can leave the computer unable to start.
- When memory is full, the computer crawls by moving memory to the disk.

Kept (people know them and they work):
- Windows, a mouse, a desktop.
- Files and folders with names.
- A terminal for those who want it.

Decided (see the choices below):
- Having to press Save: dropped.
- An all-powerful "admin" account: kept.

### Choices

1. **Save button.** Decided 19 November 2026: Alaya apps keep your work automatically, like a phone. "Save" becomes "name this version". Apps from Windows and Linux keep their own Save, and history still protects what they save.
2. **Admin account.** Decided 19 November 2026: classic admin, like Linux. You can become admin and change anything directly. Limits that still hold: becoming admin needs your PIN or passphrase each time; every admin change is still kept in history and signed, so it can be undone from the timeline or the recovery system; and admin cannot read another person's files, because they are locked with that person's own key.

### Battle test

- *"Auto-keep fills the disk with half-finished versions."* History thins out older moments and has a size limit (15 % of the disk). Only the changed parts are stored.
- *"Apps from Linux and Windows still have a Save button."* They keep it. Only apps built for Alaya keep your work automatically. History still protects what the older apps save.
- *"An admin mistake or a bad script breaks the system."* The admin's changes are kept in history like everything else. If the system won't start, the recovery system rolls back to before the change.
- *"A bad app asks for admin."* It gets nothing unless you type your PIN, and the request names the app.
- *"Developers need full control."* They have it, through admin, developer mode (unsigned apps with a warning) and the terminal.

---

## 2. Private by design

Remember the agreed order: safe data, then steady performance, then games, then privacy. Privacy is strong by default, but it bends where it would stop a game from working well.

### What it means in daily life

- **Installing a game.** The install screen lists what it wants: "game controllers, microphone (for voice chat)". You can say yes to all of it, or untick some items (see choice 1).
- **Opening a file.** You pick a file in the Open window. The app gets that one file and nothing else in the folder.
- **Looking back.** The permission screen shows each app and what it can touch. The log shows "Photo editor opened 3 photos at 14:02 and connected to example.com".
- **A private file.** You mark your tax folder as private. An app that reads it must ask you before it can go online again.
- **Camera and microphone.** A light in the top bar shows whenever any app is using them, with the app's name.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| An app sees your files | All of them | Most, some folders ask | All of them | Only the ones you hand it |
| Record of what apps did | None for you | Very little | None | A full log you can read |
| Built-in tracking | Yes (telemetry) | Some | No | No |
| Account needed | Pushed hard | Pushed | No | No |
| Another person on the same PC | Admin can read their files | Admin can read their files | Admin can read their files | Locked with their own key |

### Already decided

Apps start with only their own folder; files come through Open and Save, drag and drop, and the install list; the network is open but logged; the "private file" guard; the clipboard is shared but marked and logged; the terminal sees your own files; disk encryption with a PIN; crash reports only after you review them.

### Choices

1. **The install list.** Decided 19 November 2026: you can untick any single item an app asks for at install, and change it later on the permission screen. The app must cope without it.
2. **Screen capture.** Decided 19 November 2026: you allow it once per app; after that the app can capture without asking, a red light with its name shows whenever it does, and you can take it back on the permission screen. Apps never see other windows without this.

### Battle test

- *"Apps break if you refuse something they asked for."* Apps built for Alaya are told up front that any item can be refused. Older apps see it as "no camera found" or "folder is empty", which they already handle.
- *"Games need to see the screen for streaming or overlays."* That's choice 2. Because games rank above privacy, it must not be painful.
- *"Hidden screenshots by a bad app."* Apps can never see other windows unless you allow it, and a red light shows while anything is capturing.
- *"Admin can read everything."* Admin can change the system, but other people's files stay locked with their own key.

---

## 3. Can't really break

Parts can fail. The computer keeps going, and your work is safe. This principle is first in the clash order.

### What it means in daily life

- **The graphics driver crashes mid-game.** The screen blinks black for a moment. The driver restarts and the game keeps running.
- **An app crashes.** Because Alaya apps keep your work automatically, nothing typed is lost. What happens next is choice 1.
- **Power cut while copying files.** On restart, every file is either fully there or as it was before. Never half-copied or broken.
- **A bad update.** The computer tries the new version. If it doesn't start, or you don't like it, it goes back to the old one by itself.
- **Nothing starts at all.** A key at boot opens the recovery system: roll back, repair, restore from backup, or reset.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| Driver crash | Graphics recovers; others often a blue screen | Often a restart | Often a freeze | Every driver restarts, you keep working |
| Power cut | Can corrupt files | Rarely corrupts | Depends on setup | Files are whole or as before, always |
| Failed update | Can stop the PC from starting | Rare, hard to undo | Can stop the PC from starting | Goes back by itself |
| Forced restarts for updates | Yes | Sometimes nags | No | Never |

### Already decided

Each driver is its own program that restarts; copy-on-write storage with checksums; updates on a side branch with automatic fallback; a recovery system on the disk plus an optional USB stick; memory runs out in a fixed order with no disk swap; a crash-test suite (random driver kills, power cut at every write, full disk, out of memory).

### Choices

1. **When an app crashes.** Decided 19 November 2026: the app reopens by itself where you were, with your work as it was. A second crash within a minute shows a message instead.
2. **When updates install.** Decided 19 November 2026: updates download and get ready quietly in the background on a side branch, and take effect at your next restart. Never a forced restart, only a reminder.

### Battle test

- *"An app crashes over and over."* Auto-reopen happens once. A second crash within a minute shows a message instead, so it can't loop forever.
- *"The disk itself dies."* Nothing on one disk can save you from that. Backups to another drive or another machine do, and the recovery USB stick can restore from them.
- *"An update is needed for safety, but you never restart."* You get a reminder, never a forced restart. Alaya can't cut your work or your game.
- *"Admin breaks the system on purpose or by mistake."* The change is in history; the recovery system rolls it back.

---

## 4. Your files remember

Every file keeps its history: undo, try things on the side, and truly forget.

### What it means in daily life

- **"What did this look like last week?"** Drag the timeline back. Your files and the desktop show as they were, within a tenth of a second. You can copy something back or restore it all.
- **"Which app changed this?"** Every change is signed by the app that made it. You can undo everything one app did since Tuesday, and leave everything else alone.
- **"Name this version."** In Alaya apps, "Save" now means giving the current state a name, like "sent to the client".
- **Trying things on the side.** An app, an update or a whole work session can run on a side branch. Keep it or throw it away.
- **"Forget this."** A file you forget is gone from all history and all backups, because its key is destroyed.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| Past versions | File History, off by default, needs a drive | Auto-save and versions in many apps (since 2011); Time Machine hourly | Only with special setup | Built in for every file, every few seconds |
| Who changed it | Unknown | Unknown | Unknown | Signed by the app |
| Undo one app's changes | No | No | No | Yes |
| Truly delete a file | Copies may stay in backups | Copies stay in Time Machine | Copies may stay | Gone everywhere, including backups |

### Already decided

Only changed parts are stored. History thins out: every moment for a day, hourly for a week, daily for a month, weekly after. It is capped at 15 % of the disk, oldest first. Documents keep full history, apps and games only one step back, caches none. Apps can't read history; only you can, through the desktop. Every past view is read-only, and restoring can itself be undone.

### Choices

1. **Named versions.** Decided 19 November 2026: versions you name are never thinned out and stay until you delete them. They count toward the history space; if it fills up, Alaya warns you instead of deleting them.
2. **Forget.** Decided 19 November 2026: you confirm, the file disappears from view at once, and it is erased for good (key destroyed) after one hour. Until then you can cancel.

### Battle test

- *"Thinning deletes the version I needed."* Named versions are protected (choice 1). The thinning plan is shown in settings and can be changed.
- *"History fills the disk."* Capped at 15 %. A full disk can still delete, thanks to a reserved pool.
- *"Forget by accident."* It can't be undone once the key is gone, so choice 2 adds a safety step.
- *"Keeping a side branch clashes with changes made meanwhile."* If the same file changed in both places, Alaya keeps both copies side by side and shows you, never silently overwriting either one.
- *"A huge video file or game rewrites itself constantly."* Those use the "apps and games" or "in-place" storage class and keep little or no history.

---

## 5. Steady under pressure

Smooth beats fast. No freezes, no stutter, no sound gaps, even when the machine is busy. Second in the clash order.

### What it means in daily life

- **Gaming while a backup runs.** The game keeps a steady frame rate. The backup waits or slows down, not the game.
- **Music never cracks.** Sound always gets its share of time, even when everything else is busy.
- **Twenty browser tabs and a game.** When memory runs low, Alaya squeezes idle memory first, then asks apps to free what they can, then closes the least important app. The game in front and the screen are never closed. No crawling to the disk.
- **A hot laptop.** Alaya slows down gently before the hardware slams the brakes, so frames stay even instead of jumping.
- **A frozen app.** It uses up its own share of time and waits. The rest of the computer doesn't notice.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| Background work during games | Updates and scans can cause stutter | Some | Depends | Paused, catches up after |
| Guaranteed time for sound and frames | No | Partly | Only with special setup | Yes, built in |
| Memory runs out | Slows to a crawl (disk) | Slows down | Can freeze or kill at random | Fixed order, game and screen protected |
| Heat | Sudden drops | Smooth-ish | Sudden drops | Smooth, steady frames |

### Already decided

Guaranteed time budgets for frames and sound; background apps frozen unless allowed; fixed order when memory runs out, no disk swap; gentle heat control; full speed plugged in, balanced on battery; per-game overrides. Hard tests on the reference laptop: 99.9 % of frames on time at 60 fps, zero sound gaps in an hour, under 20 ms from key to screen, 72 hours non-stop.

### Choices

1. **Game mode.** Decided 19 November 2026: while a game is full screen, backups, search indexing and update preparation pause and catch up when it closes. Apps with a background grant (music, voice chat) keep running.
2. **Frame rate limit.** Decided 19 November 2026: games are capped to the screen's refresh rate by default (with variable refresh); any game can be set to uncapped.

### Battle test

- *"Pausing background work means backups never run for heavy gamers."* Paused work catches up as soon as the game closes, and you get a reminder if a backup is more than a day late.
- *"Voice chat and music are background apps too."* Apps with a background grant keep running during games.
- *"A cap feels slower to competitive players."* Any game can be set to uncapped.
- *"The guaranteed time is wasted when nothing needs it."* Unused time goes back to other work at once.

---

## 6. Plays games

A real gaming system, not an afterthought. Third in the clash order, above privacy.

### What it means in daily life

- **A wireless controller.** Pair it once from the desktop. The game you allow sees it, and it works.
- **A Windows game.** It runs through a translation layer (like Wine and Proton on Linux), on the NVIDIA card, at a steady frame rate.
- **Full screen.** Notifications go quiet, background work pauses, and frames are capped to your screen and kept even.
- **A broken update.** Games keep one step of history, so a bad game update can be rolled back.
- **Online games with deep anti-cheat.** They don't run for now. Games with lighter anti-cheat that works on Linux can.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| Windows games | Native | Few | Most, through Proton | Through a translation layer |
| Deep anti-cheat | Yes | No | No | Not yet |
| Game sees your files | All of them | Most | All | Only its own folder and what you give it |
| Steady frames under load | Varies | Good | Varies | Guaranteed time, tested |
| Roll back a game update | No | No | No | Yes |

### Already decided

Native Alaya games, Linux games recompiled for Alaya, Windows games through a Wine/Proton-style layer later (milestone 13); NVIDIA card with the open driver path; controllers, headsets and low-delay Bluetooth; several monitors, high refresh, variable refresh, HDR; game mode and frame cap (principle 5).

### Choices

1. **Where games come from.** Decided 19 November 2026: Steam's Windows version, running through the translation layer, is the main target. DRM-free games (GOG, itch.io) are expected to work along the way.
2. **Mods.** Decided 19 November 2026: each person gets a personal mod layer on top of each game. The game sees the mods as if installed; the shared original stays clean; removing mods is one click.

### Battle test

- *"Steam is a big, complex Windows program."* It is the hardest test of the translation layer. If Steam runs, most games behind it will too. Simpler shops and DRM-free games run along the way.
- *"Games are installed once, read-only, for everyone. Mods need to change game files."* Mods go in a personal layer on top (choice 2), so a broken mod never harms the game for anyone.
- *"Games save into odd places."* Each game gets its own folder; older Windows and Linux games see a pretend file layout built from that folder, so they don't notice.
- *"Anti-cheat."* Games whose anti-cheat runs inside the game (supported on Linux by many developers) can work. Deep anti-cheat stays "not yet".

---

## 7. Built to build itself

Claude Code and normal developer tools run inside Alaya, safely.

### What it means in daily life

- **Today.** Claude Code runs on the host PC and drives Alaya inside a virtual machine.
- **Later (milestone 8b).** Inside Alaya you open a terminal and run Claude Code, with git, bash and the usual tools.
- **Every change it makes is signed** as Claude Code's, and can be undone, alone, without touching anything else.
- **A whole session can be thrown away**, including every program it started and every file it changed.
- **File watching without limits.** Tools see every change, with no lost events and no limit on how many files they watch.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| Undo everything an AI tool did | No | No | No | Yes, as one group |
| Try a session and throw it away | No | No | Only with containers | Built in (side branch) |
| Which files the tool sent online | Unknown | Unknown | Unknown | Logged |

### Already decided

Claude Code on the host now; inside Alaya through the POSIX layer with Node.js, bash, git and ripgrep (decision 52); never more access than your own terminal, never another person's files; every change signed and undoable; process families in the log; file watching from the store's change stream; sessions can run on a side branch.

### Choices

1. **Sessions on a side branch.** Decided 19 November 2026: each Claude Code session asks at the start whether to work on a side branch (keep or throw away at the end) or directly on your files (still signed and undoable as a group).
2. **Claude Code and private files.** Decided 19 November 2026: private files are invisible to Claude Code. You can hand over a single one when needed, and the log notes that it was sent online.

### Battle test

- *"Claude Code sends file contents to Anthropic's servers to work."* True, so it's always online. That collides with the "private file" guard, so we need choice 2.
- *"A side branch is extra clicks for tiny tasks."* You choose per session, so small tasks can work directly.
- *"Claude Code becomes a single native program instead of JavaScript."* Then the plan of recompiling Node stops working. A Linux-compatibility layer would be needed. This risk is noted in `linux-compat.md`.
- *"Admin."* Claude Code never gets admin by itself. Becoming admin always needs your PIN.

---

## 8. Open and clean

### What it means in daily life

- **Anyone can read the code.** It's public, under GPL-3.0. Apps and games built on Alaya can use any license, so game makers aren't scared away.
- **No ads, no tracking, no account.** Nothing in the system reports on you.
- **No personal content.** The code and docs never contain anyone's name, private notes or computer name.
- **Updates are signed.** Your computer checks that every update really comes from the Alaya project before it installs.

### How it differs

| | Windows | macOS | Linux | Alaya |
| --- | --- | --- | --- | --- |
| Source code | Closed | Mostly closed | Open | Open |
| Ads and tracking | Yes | Some | No | No |
| Account required | Pushed hard | Pushed | No | No |

### Already decided

GPL-3.0 with a linking exception for apps; no company servers needed; backups to places you choose; crash reports only after review; firmware from NVIDIA and Intel loaded as separate files, not part of Alaya's code.

### Choices

1. **Where updates come from.** Decided 19 November 2026: updates are plain files signed by the project, published on the public repository's releases, and anyone may mirror them. The computer only downloads; it sends nothing about you.
2. **Outside contributions.** Decided 19 November 2026: not yet. Anyone can read and copy the code, but only the project changes it until the core is stable; then contributions open with clear rules.

### Battle test

- *"Update checks reveal who uses Alaya."* Update files are plain signed files that anyone can mirror; the computer only downloads, it sends nothing about you.
- *"Someone sneaks bad code into an update."* Every update is signed with the project's key, and a bad update still falls back by itself.
- *"Outside code changes the design."* Contributions stay closed until the core is stable (choice 2).

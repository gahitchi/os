# Alaya: principles

Draft 3, 20 November 2026 (wording fixes from `contradictions.md`). Waiting for approval. Each principle is designed in depth, with all choices, in `principles-design.md`.

This is the short list of what Alaya is. Every new idea, decision and milestone is checked against it. If something breaks a principle, we either change the idea or change the principle on purpose, never by accident.

Each principle has three parts:
- **Always**: what Alaya must do.
- **Never**: what Alaya will not do.
- **How we check**: the test that shows it is true.

---

## 1. A new kind of OS

Your computer remembers everything, and apps only get what you hand them. No other system works this way.

- **Always**: keep old ideas (from Unix, Windows, macOS) only when they earn their place. Every decision that keeps an old habit says why.
- **Always**: stay familiar on the surface (windows, a mouse, a terminal) so it is easy to use, while working differently underneath.
- **Always**: keep your work automatically in Alaya apps (no Save button needed).
- **Never**: copy something just because "that's how it has always been done". Kept on purpose: windows, folders, a terminal, and a classic admin (PIN each time, every change undoable).
- **How we check**: each milestone shows at least one thing other systems can't do. The decision log notes, for every borrowed idea, why we kept it.

## 2. Private by design

Apps only see what you hand them, and you can see everything they did.

- **Always**: an app starts with only its own folder, plus the files of the task it is opened in. Anything else (a file, a folder, the camera, a controller) arrives only when you hand it over: by opening or saving a file, by dragging, or by accepting the app's list when you install it.
- **Always**: let you untick any single thing an app asks for, and show a light whenever the camera, microphone or screen capture is in use.
- **Always**: keep a log you can read of what each app opened, changed, deleted and connected to, and let you cut any app off.
- **Always**: lock the whole disk with encryption, and give each person their own key.
- **Never**: let Alaya itself send anything about you anywhere without you seeing it first (no tracking, no hidden reports, crash reports only after you review them).
- **Never**: need an account, a company server or a cloud to work.
- **How we check**: a test app that tries to reach files it wasn't given gets nothing. The permission screen lists everything every app can touch. A fresh system with no apps open sends nothing over the network.
- **Agreed exceptions** (confirmed 19 November 2026): the network is open to all apps by default, like on Windows, any app can read the clipboard, and the terminal sees all of your own files. Each one is recorded in the log, and the "private file" guard still applies.

## 3. Can't really break

Parts can fail. The computer keeps going, and your work is safe.

- **Always**: a crashed driver or system part restarts on its own, without a reboot and without losing your session. Target: a graphics crash costs at most one black frame.
- **Always**: a power cut leaves your files exactly as last saved, or one step before, never half-written.
- **Always**: a system update is tried first; if it fails to start or you reject it, the computer goes back to the old one by itself.
- **Always**: reopen a crashed app where you were, and apply updates without a restart where possible (never during a full-screen game, always undoable); the rest wait for your next restart, never forced.
- **Always**: a built-in recovery system can roll back, repair or restore, even when the main system won't start.
- **Never**: one bad app or driver takes the whole machine down.
- **Never**: the core of the system crashes because a program sent it something strange.
- **How we check**: automatic tests that kill drivers at random, cut the power at every single write, fill the disk to 100 %, and run out of memory. The rule is zero lost files and zero corruption. Random junk is sent to the core around the clock with no crash allowed.

## 4. Your files remember

Every file keeps its history: undo, try things on the side, and truly forget.

- **Always**: keep past versions of your files, marked with which app made each change.
- **Always**: let you undo everything one app did, and let you try an app or update "on the side" before keeping it.
- **Always**: let you see your files and desktop as they were at any kept moment, within a tenth of a second.
- **Always**: keep versions you name forever.
- **Always**: let you forget a file for good: gone from all history and all backups, after a one-hour chance to cancel.
- **Never**: let history quietly fill your disk. It has a size limit (15 % by default) and thins out older moments.
- **Never**: let apps read your history. Only you, through the desktop, can look back.
- **How we check**: the timeline tests (speed, every past view is consistent, restores can be undone), and a "forget" test that confirms the file can no longer be read anywhere, including backups.

## 5. Steady under pressure

Smooth beats fast. No freezes, no stutter, no sound gaps, even when the machine is busy.

- **Always**: give games, the screen and sound a guaranteed share of time, so background work can't steal it.
- **Always**: when memory runs low, free it in a fixed order (squeeze, ask apps, then close the least important app). The game in front and the screen are never closed.
- **Always**: when the laptop gets hot, slow down gently before the hardware slams the brakes.
- **Always**: pause background work during full-screen games, and cap frames to the screen by default.
- **Never**: let a stuck or greedy program freeze the rest of the computer.
- **How we check**: hard limits on the reference laptop under full load: 99.9 % of frames on time at 60 frames per second, zero sound gaps in an hour, under 20 ms from key press to screen, and a 72-hour non-stop test with no crash and no slow-down.

## 6. Plays games

A real gaming system, not an afterthought.

- **Always**: run games made for Alaya, Linux games brought over, and Windows games through a translation layer (like Wine/Proton).
- **Always**: support the NVIDIA graphics card, game controllers, high refresh rate screens, variable refresh rate and HDR.
- **Always**: silence notifications during full-screen games.
- **Always**: aim at Steam's Windows version first, and keep mods in a personal layer so the game itself stays clean.
- **Not yet**: anti-cheat programs that need deep control of the whole computer are not supported today. Because games rank above privacy, this can be reconsidered later, but only in a way that can never put your data at risk (safety still wins).
- **How we check**: a fixed set of test games must meet the steady-performance limits from principle 5.

## 7. Built to build itself

Claude Code and normal developer tools run inside Alaya, safely.

- **Always**: run Claude Code, a terminal, git and common command-line tools inside Alaya.
- **Always**: record and allow undo of every change Claude Code or any tool makes, and let a whole work session run on the side, to keep or throw away.
- **Always**: ask at the start of each session whether to work on the side or directly.
- **Never**: show private files to Claude Code unless you hand one over.
- **Never**: give an AI tool more access than you have in your own terminal, and never access to another person's files.
- **How we check**: Claude Code works on Alaya's own code from inside Alaya (milestone 8b), and any session can be fully undone.

## 8. Open and clean

- **Always**: open source (GPL-3.0), while apps and games built on it may use any license.
- **Always**: ship updates as signed files anyone can mirror; checking for updates sends nothing about you.
- **For now**: anyone can read the code; outside contributions open once the core is stable.
- **Never**: ads, tracking, personal content, or the name of anyone's own computer in the code or docs.
- **How we check**: every public change is reviewed for personal content before it is added.

---

## When principles clash

Agreed order (19 November 2026), first wins:

1. Your data is safe (principle 3)
2. Steady performance (principle 5)
3. Games (principle 6)
4. Privacy (principle 2)
5. Everything else

Examples: a faster file trick that could lose data on a power cut loses to safety (1 beats 2). A privacy check that would make a game stutter is redesigned or relaxed for that game (2 and 3 beat 4). A game's anti-cheat that wants deep access may be considered, but never if it could harm your data (1 beats 3).

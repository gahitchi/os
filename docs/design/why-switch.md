# Alaya: why would anyone switch?

20 November 2026. An honest check, written after you tried the desktop mock-up and asked: "would I really change Linux or Windows for this?"

Short answer: **today, a gamer would not switch, and you are right to say so.** An everyday user might, for one or two things. A developer who works with AI helpers has the strongest reason. The fix is not to drop the new ideas. It is to stop *showing* them to people who don't need them, keep the protections working underneath for everyone, and add a few things gamers would actually feel. Your setup-question idea is a good way to do that, with a few changes (section 5).

---

## 1. Who Alaya really competes with

People don't compare a new OS with "Linux" in general. They compare it with what they would actually install.

| If you are... | You'd compare Alaya with | What that already gives you |
| --- | --- | --- |
| A gamer | **SteamOS, Bazzite**, Windows 11 | Steam's Windows games through Proton (most of the catalogue works), a console-like game screen, updates that undo themselves if they fail (both have a "go back to the old system" switch), no ads |
| An everyday user | **Windows 11, macOS**, Linux Mint or Fedora | Every app (Chrome, Office, Spotify, Discord), Time Machine on Mac, OneDrive on Windows, a familiar look |
| A developer | **macOS, Fedora/Ubuntu, Nix, Silverblue**, Windows + WSL | All the tools, containers, git, ready-made setups you can throw away |

Facts that matter (November 2026):
- Linux is now about 4 to 5 % of Steam players, its highest ever, mostly because of SteamOS and Bazzite ([GamingOnLinux, October 2026 survey](https://www.gamingonlinux.com/2026/11/steam-survey-october-2026-shows-an-uptick-for-the-linux-user-share/), [KitGuru, 5.33 % record](https://www.kitguru.net/gaming/joao-silva/steamos-propels-linux-to-record-5-33-share-in-latest-steam-survey/)). Linux gaming is good now. That is the bar.
- The big blocker for Linux gamers is still anti-cheat: Battlefield 6, Valorant, Fortnite and others don't run ([GamingOnLinux anti-cheat guide](https://www.gamingonlinux.com/guides/view/anticheat-check-which-competitive-games-actually-work-on-linux-steamos/), [Battlefield 6 on Linux](https://gamingonlinux.com/2025/10/battlefield-6-releases-today-and-it-will-not-be-playable-on-any-linux-steamos-system)). Alaya uses the same translation tools for Windows games (milestone 14), so **it will have the same blocker, and at best the same game list as SteamOS.** Alaya can't win gamers on "more games".
- Windows 11's push of ads, an online account and AI screenshots ("Recall") is a real reason people leave Windows. But Linux already answers it, so it isn't a reason to pick Alaya *over Linux*.

**The honest conclusion:** for each kind of user, Alaya must first be "**no worse**" than what they'd install otherwise, and then have **one or two things they feel in the first hour** that nobody else has. Right now the second part is strong for developers, medium for everyday users, and close to nothing for gamers.

---

## 2. The gamer

**What would stop them (in order of weight):**
1. Anti-cheat games don't run. Same as SteamOS. Can't be fixed by us.
2. Hardware: Alaya will support one laptop and its NVIDIA card for years. A gamer with a different graphics card simply can't use it.
3. Speed: at best equal to Linux with Proton, because it's the same translation tools. Any slowdown kills it.
4. Habits: Discord, overlays, mod managers, launchers (Epic, EA, Ubisoft).

**What they'd feel from today's design:**

| Feature | Felt in the first hour? | Honest note |
| --- | --- | --- |
| Smooth frames, background work can't cause stutter | Only if they notice its absence | Real, but invisible. You can't feel a stutter that didn't happen. |
| A crashed driver comes back, game keeps going | Rarely (crashes are rare) | Great when it happens, but not a reason to switch. |
| Bad update undoes itself | No | SteamOS and Bazzite already have this. |
| Game mode (background work paused) | Not visibly | Good, and it's free; but Windows has "Game Mode" too. |
| Tasks, loose groups, "needs a look", decision inbox | **Yes, as extra work** | This is the mock-up's problem: it's new words and new screens that a gamer has no use for. |

**Verdict: today Alaya has no reason for a gamer to switch.** Your doubt is correct.

**What could change that.** Alaya's "remembers everything" engine is actually very good at three things gamers complain about all the time. None of them needs new kernel work.

- **G1. "Why did my game stutter?"** After a session with hiccups, one plain sentence: "3 short stutters, at 21:14. Cause: the shader cache was writing to disk" or "Discord's overlay was late". Alaya already records which part missed the screen's deadline (decision 70). Tools like PresentMon show *that* a frame was late; none tell you *why* across the whole system. This is close to new.
- **G2. Every game save has a history.** Saves are kept like documents: go back to yesterday's save in any game, even games without cloud saves, even after a crash corrupted it. Today the design keeps games "one step back" (decision 33), and saves would fall under that. Small change, big feeling. (Tools like Ludusavi do part of this on Linux, by hand.)
- **G3. A game update broke your mods? Go back one version.** Alaya already keeps one copy of each game before an update (decision 33). Steam itself doesn't let you go back easily. Plus decision 63's mod layer: "turn all mods off" and "back to clean" are one click. (Online games that demand the newest version can't be helped.)

With G1 to G3 the gamer's pitch becomes: **"Same games as SteamOS. But it tells you why it stuttered, your saves can't be lost, and a bad update or mod is one click away from undone."** That's a real, honest pitch. It's still not enough to beat "Windows runs every game including anti-cheat" for many people, and we shouldn't pretend it is.

---

## 3. The everyday or mixed user (web, school, office, media, a few games)

**What would stop them:**
1. **Apps.** No Chrome, Office, Spotify, Discord, Adobe, banking app. This is the number one reason people go back to Windows from Linux.
2. A new look and new words to learn.
3. Printers, phones, webcams, other hardware.

**What they'd feel:**

| Feature | Felt in the first week? | Honest note |
| --- | --- | --- |
| One undo for the whole computer | **Yes**, the first time they delete or move something by mistake | Nobody else has this across everything. Strong. |
| No Save button, work never lost | **Yes** | macOS has some of this (Versions), but per app. |
| "What happened?" in plain sentences | Yes, when something changed and they don't know why | Strong when needed, invisible otherwise. Good. |
| Private by default, no ads, no tracking | Yes, if they came from Windows | Linux gives this too. |
| Updates never break the computer, never forced | Yes, over months | SteamOS/Silverblue have the "go back" part. |
| Back where you left off ("since you left") | Yes, after a weekend | Nice, but it needs Alaya apps to be truly good. |
| Tasks, protected tasks, decision inbox, Finish | Mostly as extra work | Students and office workers *might* use groups. Most won't. |
| "Needs a look" marks | Rarely | Useful for someone with a spreadsheet and a report. Narrow. |

**Verdict: medium.** The "can't lose anything, one undo for everything" story is genuinely attractive to normal people. But it's worthless if their apps don't run.

**The app answer we already own.** The "Linux world" (decision 66) was planned only for Claude Code and developer tools. But it runs *normal Linux programs unchanged*. That means Firefox, Chromium, Discord, Spotify, VS Code, LibreOffice and even **Steam's own Linux version with Proton** could run in it, and every one of them would still get Alaya's undo, history, privacy rules and quiet crashes underneath. This turns the biggest weakness ("no apps") into "**all the Linux apps, on a computer that can't break and remembers everything**." It's the single biggest change this analysis suggests (card 3).

Honest cost: it makes the Linux world a much more important part (it has to be fast and complete, not just "good enough for Claude Code"), and Alaya's own apps become less urgent.

---

## 4. The developer (and anyone working with AI helpers)

**What would stop them:**
1. Containers (Docker) need Linux kernel features Alaya doesn't have. This is a real hole for many developers.
2. Their editor and tools. The Linux world answers most of it.
3. Hardware support, again.

**What they'd feel:**

| Feature | Felt? | Honest note |
| --- | --- | --- |
| Claude Code (or any AI helper) on a side branch, keep or throw away its whole session | **First hour** | Git does this for code only. Alaya does it for *everything* the helper touched. New. |
| Jobs with rules ("may change this, must not touch that, these checks must pass") enforced by the OS | **First hour** | Nobody enforces this at the OS level. New. |
| "What happened?" for an AI helper's session | First hour | Strong. |
| Tasks: each project sees only its own files | First week | Containers and Nix give some of this, with more work. |
| "Needs a look" (a result is out of date because its inputs changed) | First week | Build tools do this for code; Alaya does it for any file. |

**Verdict: strong.** This is where Alaya is clearly a new kind of computer today. The mock-up felt "made for developers" because it is. That's not a failure; it tells us who Alaya's first real fans would be.

---

## 5. Your idea: ask at setup, choose the look, keep the same engine

**Is it good?** Yes, with three changes. The core of it, "the engine stays the same, only what you see changes", is exactly right, and it fixes the mock-up's problem. Similar things exist (SteamOS has a game screen and a desktop screen; Windows' first setup asks what you'll use the PC for, but mostly to suggest apps and ads), so the question itself isn't new. What's new is that every look sits on the same "can't lose anything" engine.

**Three changes that make it stronger:**

1. **Let people pick more than one.** Most people are mixed: games at night, school or work by day. One single choice would force a gamer who also writes reports into the wrong look. Pick any of the four, and the looks combine.
2. **Per person, changeable any time** (Settings, "What you use Alaya for"), no reinstall, nothing lost. Each person on the computer has their own (decision 30).
3. **Features still appear when they're useful, even if hidden.** If you hid "undo" from a gamer, they'd never learn it exists. So hidden features show up *once*, at the right moment: you delete a file by mistake, and a small note says "Alaya kept it. Undo?". That's how people discover them without a manual.

**The rule that never changes: protections are the same in every look.** History, one undo, privacy rules, quiet driver crashes, updates that undo themselves and smooth frames work for everyone. A look only decides what is *on screen*.

### The four looks

| | Play | Everyday | Create | Build |
| --- | --- | --- | --- | --- |
| **For** | Games | Web, school, office, videos, music | Photos, video, music, writing | Code, AI helpers like Claude Code |
| **Home screen** | A plain desktop plus a "Play" panel with all your games | A plain desktop, like Windows or Mint | Plain desktop, recent work up front | Desktop with tasks and the terminal at hand |
| **Shown by default** | Game library from every launcher; per game: saves history (G2), mods on/off and "back to clean", "go back one version" (G3), "why did it stutter?" (G1) | One undo, "What happened?", back where you left off | Everything in Everyday, plus the timeline on files, file stories, "needs a look" marks | Everything in Create, plus protected tasks, jobs with rules, side branches, the AI helper rules screen, decision inbox |
| **Hidden until useful** | Groups, tasks, inbox, "needs a look" | Tasks, jobs with rules, "needs a look" | Jobs with rules | Nothing hidden |
| **New words to learn** | None | Two ("undo everything", "what happened") | Three or four | All of them |

Picking "Play" + "Everyday" gives a plain desktop with the Play panel and the Everyday tools: probably the most common choice. A fifth button, "Show me everything", turns all of it on.

### What it costs

Little. The features are already designed as separate pieces, so a look is mostly a list of what's switched on in the desktop. The setup screen can be simple in milestone 6 (one question, two looks) and full in milestone 7a, when tasks arrive. The mock-up can show it right away.

---

## 6. Where the vision is weak (honestly)

- **"A new kind of OS" and "plays games" pull in different directions.** Everything new in Alaya is about work and files; games want to be left alone. The looks resolve this: for gamers, the new kind of OS is the one they *don't see*, plus G1 to G3.
- **The mock-up shows too many new words at once** (groups, tasks, inbox, Finish, needs a look). Even for developers, they should arrive one at a time.
- **Hardware is the real ceiling.** For years Alaya runs on one laptop. That's fine for building it, but it means "people switching" is a goal for much later. The near-term goal is that *you* would choose it on that laptop.
- **"Needs a look" is a narrow feature.** It's elegant and new, but only a few people (analysts, students with reports, developers) will feel it. It shouldn't be on the default screen for most people.
- **Containers and anti-cheat** are holes we can't close soon. Better to say so plainly than to promise them.

## 7. Proposed changes (waiting for your choices)

1. **DECIDED (decision 90, 20 November 2026): looks.** A setup question (pick one or more of Play, Everyday, Create, Build, or "show me everything"), per person, changeable any time; hidden features appear once when useful; protections the same everywhere. Simple version in milestone 6, full in 7a. The mock-up gets the setup question first, so you can try it.
2. **DECIDED (decision 91, 20 November 2026): gamer extras.** G2 (saves keep full history) and G3 (one-click back one version, mods "back to clean") in milestone 8 and 14; G1 ("why did it stutter?") in milestone 9, using decision 70's records.
3. **DECIDED (decision 92, 20 November 2026):** **the Linux world becomes the main way to run everyday apps**, not only Claude Code: Firefox, Discord, VS Code, and Steam's Linux version with Proton for games (milestone 14 picks this route). Alaya-native apps are for the things only Alaya can do.
4. **Say who Alaya is for first:** builders (developers, AI helpers) first; gamers and everyday users "no worse than SteamOS or Mint, plus a few things they'll feel". Written into `principles.md` principle 1 as the scope line suggested on 19 November.

Nothing in milestones 0 to 4 or the kernel changes.

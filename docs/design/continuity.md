# Alaya: continuity (everyday work, from start to finish)

Draft, 20 November 2026. Written from your second review, "Alaya OS: Rethinking everyday computing", and checked against every decision so far (up to 84), the principles, `contradictions.md` and `commitments.md`. Waiting for your approval and two choices (section 8).

The short version: **Alaya keeps normal windows, files and folders, and changes what happens around them.** You can just use the computer. When some work starts to matter, you can group it. When you come back days later, Alaya brings you to where you were and tells you what changed while you were away. When the work is done, one action tidies it up without throwing anything away.

---

## 1. What we take from the review, and what we change

| From the review | What we do with it |
| --- | --- |
| Continuity (resume the work, with "since you left") | **Adopt as the heart of the desktop.** It is built from parts we already designed: tasks (69), "What happened?" (68), living validity (82, 83) and app state that keeps itself (58) |
| Fluid workspaces (explore, then organise when useful) | **Adopt, and it changes decision 69.** Grouping becomes optional and light. A loose group never gives an app access to anything. Only a task you confirm sets a privacy boundary. This is choice 1 in section 8 |
| Decision inbox | **Adopt.** It replaces "notifications that belong to tasks" (B5). Nothing urgent is ever hidden, and an AI never decides what matters |
| Universal handoff (copy, reference, live) | **Adopt in steps.** Copy and reference work for everything. Live works only between Alaya apps that support it, and only later. This replaces "live links between apps" (B6, parked) |
| Work closure ("Finish") | **Adopt.** It never deletes anything. It tidies up, hands back permissions and keeps the results |
| Rejected ideas (AI-run desktop, sorting everything automatically, no more files and folders, compulsory task management) | **Agree.** Principle 1 already keeps windows, folders and a terminal on purpose |
| Test the ideas with people before building them for real | **Agree.** How we do it is choice 2 in section 8 |

The rule from both reviews carries over:

> **Everything Alaya tells you about your work comes from what really happened, from what an app saved, or from a note you wrote. A guess is always shown as a guess. Alaya never invents a summary of your work and presents it as fact.**

And one rule is new:

> **A trivial job must never get harder.** Opening one file in one app takes exactly the same steps on Alaya as on any other computer. Grouping, tasks and briefings are there if you want them, and you can ignore all of it.

---

## 2. The life of a piece of work

| Stage | What you do | What Alaya does |
| --- | --- | --- |
| **Exploring** | Just use the computer. Open pages, files and apps | Nothing to decide. Everything is remembered as usual |
| **Grouping (optional)** | Drag a few windows together, or accept a suggestion like "keep these 5 together?" | A **loose group**: a label for things that belong together. A file can be in several groups. No permissions change |
| **Protected task (optional)** | "Make this a task". Alaya shows which files apps in it will see, and you confirm | The boundary from decision 69: apps opened in the task see the task's files, and nothing else |
| **Leaving** | Switch away, close the lid, or stop for the day | A **return point**: what was open, where you were in each Alaya app, and an optional "next step" note in your own words |
| **Coming back** | Open the group or task | **Resume** with a short **"since you left"** list (section 3) |
| **Finishing** | "Finish" | **Closure** (section 6): keep the results, archive the rest, hand back permissions. All of it can be undone |

---

## 3. Continuity: resume the work, not the apps

**What you see** when you come back to "University project" after three days:

> **University project**, last used 3 days ago
> **Where you left off:** the simulation results, at section 4. Your note: "compare the two models".
> **Since you left:**
> - dataset.csv has a newer version (changed yesterday by the download manager) → the results chart **needs a look**
> - 1 message waiting for this project
> - Nothing else changed. Your earlier results are kept
> **[Resume]** **[Show what changed]**

Where each line comes from (all already designed):
- **Where you left off**: Alaya apps keep their own state (decision 58). Older apps reopen with their files (69), and Alaya says when it can't restore more.
- **Your note**: typed by you, shown as your words. Alaya never writes a "next step" for you.
- **Since you left**: "What happened?" (68), limited to this group's files, plus living-validity marks (82, 83), plus anything held for this work in the decision inbox.

**Battle test**

| Hard case | Rule |
| --- | --- |
| An older app was killed with unsaved work | Show exactly what was kept, and never claim more |
| A website login ran out | The page reopens and asks you to log in again. Alaya stores no passwords for this |
| You changed your mind about the goal | Dismiss or edit the note in one click |
| A shared or projected screen | The briefing only shows file names after the person unlocks, and private files only to their owner (B12) |
| A paused protected task held an older app in memory (rule B3) | Unchanged: it is frozen, not closed, so nothing unsaved is lost |

---

## 4. Loose groups and protected tasks (decision 85, changes 69)

**The problem.** In decision 69, putting a file into a task automatically gave the task's apps access to it ("putting a file in the task is the permission"). The review points out three risks:
- If Alaya suggests a group and you accept it without thinking, apps quietly gain access to files.
- People don't want to file everything early, so they won't make tasks, and the privacy benefit is lost.
- Organising work becomes another job.

**The proposal.** There are two levels, kept clearly apart:
- **Loose group**: only a label. It costs nothing to make, a file can be in many, and suggestions are allowed. **It never changes what an app can see.** Apps get files the usual way: you open them, drag them, or pick them in Open and Save.
- **Protected task**: you create it yourself, and Alaya shows what the boundary will be. Apps opened in it see that task's files, and only those (69, 71). This is also where the "jobs with rules" from `commitments.md` live: a job is a protected task with extra rules.
- **Turning one into the other** always asks you and shows the list of files. Taking a file out of a protected task, or moving it to another, is recorded in the ledger and can be undone.

**What stays the same:** the everywhere area (72), the terminal rules (40, 71), the private-file guard (27, B1), pausing (B3), and one undo across everything.

**What changes:** principle 2's wording becomes "...plus the files of the **protected task** it is opened in". Decision 69's "the desktop is organised by tasks" becomes "the desktop *can* be organised by groups and tasks, and works normally without them".

---

## 5. The decision inbox (replaces B5)

The notification centre becomes a short list of **things waiting for you**, sorted into four kinds:

| Kind | Examples | How it reaches you |
| --- | --- | --- |
| **Now** | Battery almost empty, disk failing, a login change warning (74) | Always shows at once, on a channel only the system can use. No app can send on it |
| **About what you're doing** | A message for the task you're in, a job's checks finished | Shows at once, quietly |
| **A decision that can wait** | A job ready for review, two versions to reconcile (B6), a result that "needs a look" and you asked to be told about (82) | Waits in the inbox with a count. Held for at most a time you choose |
| **Just so you know** | A download finished, an update was applied (73) | In the inbox, with no pop-up |

Rules:
- An app's own label is never trusted to mean "now". Only the system and **your own rules** ("always let calls from X through") can make something urgent.
- No AI decides what matters.
- "Show everything" is always one click away. A badge shows how many things are held back.
- During full-screen games nothing pops up (46, 63), except **Now**.

---

## 6. Finish: closing a piece of work

"Finish" on a group or task shows one screen:
- **Results**: the files made in it, ready to keep, name or move (named versions are kept forever, 58).
- **Temporary things**: downloads, scratch files and tabs. These are **archived out of sight, never deleted**. You can delete them by hand, and "forget" (62) still works.
- **Permissions to hand back**: the grants that apps got for this work, ticked to be revoked.
- **Still in use elsewhere**: files that are also in another active group or task stay where they are.
- **Unsaved work** in older apps is listed first, before anything closes.

The whole step can be undone with one undo. A finished group stays searchable (39) and can be reopened.

---

## 7. Handoff: copy, reference or live

When you drag or paste something between apps, Alaya can offer three ways:

| Way | What it means | Works with |
| --- | --- | --- |
| **Copy** | An independent copy (today's behaviour) | Every app |
| **Reference** | A link to this exact version. Living validity marks it "needs a look" when the source changes | Every app that can show a link or an image; it falls back to a copy otherwise |
| **Live** | Follows the source as it changes | Only Alaya apps that support it (SDK), starting with two demo apps |

Rules: a linked source that is deleted or forgotten leaves a frozen last picture, never a hole. Revoking access to the source never breaks the rest of the document. There is no promise to convert other apps' private formats without loss, and an unsupported conversion is never done silently.

---

## 8. Your choices

1. **Should a group of windows and files change what apps can see?** Decided (85): **no.** Loose groups are only labels. Only a protected task you confirm sets a boundary. This changes decision 69.
2. **How do we test these desktop ideas before building them into Alaya?** Recommended: **a clickable mock-up first**, in the browser, with a scripted "three days in the life" to click through. Then the real thing in Alaya. The alternative is a real prototype running on your laptop's Linux for a few weeks. It is more realistic, but it is weeks of work on something we then throw away.

## 9. Where it goes in the roadmap

| Milestone | Change |
| --- | --- |
| Now, alongside milestone 5 | The mock-up (if chosen), with you clicking through it and, if you like, a few friends |
| 6. A simple desktop | Windows can be dragged into a loose group. The first return point: the text editor reopens where you were |
| 7. The remembering desktop | Unchanged, plus living validity in full and jobs with rules (`commitments.md`). Tasks move out to 7a, because 7 was getting too big |
| **7a. Continuity (new)** | Loose groups and protected tasks, resume with "since you left", the decision inbox, Finish, and copy and reference handoff |
| 7b. The full modern desktop | Unchanged |
| 8. Apps and the old world | App notifications go into the decision inbox. Live handoff between two Alaya demo apps (SDK) |

## 10. Gates (chosen now, before building)

From the review, adapted to a project with one main tester (you), plus anyone you invite:
- **Resume**: returning to an interrupted piece of work takes at least 25 % less time than reopening windows by hand. Measured on a scripted task.
- **Nothing invented**: every line of a "since you left" briefing traces back to a ledger entry, an app's saved state or your own note. This is an automatic test with zero exceptions.
- **Groups never grant**: a test app in a loose group gets nothing it wasn't handed. Zero exceptions.
- **Nothing lost on Finish**: no file disappears and every Finish can be undone. Automatic test.
- **Now always arrives**: in a busy-day test, zero "Now" events are held back.
- **No extra steps**: opening one file in one app takes the same number of clicks as on a normal desktop.

## 11. How new is it?

Honestly, **every part has prior art.** Activity-based desktops have been studied since the 2000s (TaskTracer, Giornata, Laevo, KDE Activities, TaskSnap in 2026). Apple's App Intents and Microsoft Loop exist. Microsoft Research studied notifications at task breaks (Oasis). What nobody ships is these parts working together on top of an operating system that **really remembers** (store and ledger), **really knows what is out of date** (living validity) and **really enforces boundaries** (grants and protected tasks), without constant screenshots and without an AI in charge. That combination, made easy enough to ignore, is Alaya's claim.

# The timeline: implementation requirements

19 November 2026. The desktop is familiar floating windows plus a timeline (decision 34). The person asked that it be implemented properly. These are the requirements it must meet; the tests are in `test-plan.md`.

## What the timeline is

A bar at the bottom of the desktop. Dragging it back shows the person's files, folders and desktop layout as they were at that moment. Nothing changes until the person chooses to restore something.

It shows the history the store already keeps (decision 33). It adds no copies of files.

## 1. Speed

| Requirement | How |
| --- | --- |
| Dragging the bar feels instant | The list of moments is a small index kept in memory per person. Finding a moment is a lookup, not a disk scan |
| The past view appears within 100 ms | A past moment is opened as a read-only view of an old root (copy-on-write makes this free). Folder listings and thumbnails load lazily, only for what is on screen |
| Dragging never stutters a running game | Timeline work runs on its own small time budget on efficiency cores (decisions 11 and the core-type rule). It pauses if a full-screen app is running |
| Thinning and cleanup never block it | The cleanup works on moments the timeline is not showing, and both run on budgets |

## 2. Correctness

- **A moment is a whole commit.** Every file in a past view is from the same instant. You never see half of a save, or file A from 3:00 next to file B from 3:05.
- **Past views are read-only.** Nothing can write into the past. Restoring creates a new change in the present.
- **Restoring is itself undoable.** A restore is a normal commit, attributed to the person through the desktop, so it shows on the timeline and can be undone.
- **What can be restored:**
  - one file or folder, to its state at that moment;
  - everything one app changed since that moment (per-app undo);
  - the whole desktop, which is done as a branch the person can inspect before switching to it.
- **Conflicts are shown, never guessed.** If a newer change touched the same file, the person sees both versions and picks.

## 3. Desktop layout

- The compositor records the window layout (which apps were open, where, and on which file grants) as a tiny record at each moment. It does not store screenshots.
- Showing an app's content from the past means opening that app with a **pinned, read-only grant** to the file as it was. The app cannot change it.

## 4. Grants and apps

- Looking at the past never gives an app more than it had. An app sees a past version only if the person hands it one, and then only read-only.
- Apps cannot drag the timeline or read history on their own. Only the desktop can, on the person's behalf.

## 5. Privacy

- **Per person.** Each person's timeline shows only their own store (decision 30). Shared apps and games show only their install and update points.
- **Private files.** History of files marked private (decision 27) is shown only to their owner, never in previews to anyone else.
- **Forget, for real.** History keeps everything, including what the person wants gone. So there is a "forget" action: remove this file from all of history. It works by giving every file its own encryption key inside the person's key (decision 29). Forgetting destroys that key, so every old version becomes unreadable at once, without rewriting old moments. The blocks are reclaimed by the next cleanup.
- **No history, not shown.** Files in the cache class (decision 33) never appear on the timeline.

## 6. Space

- History is capped by its budget (default 15 % of the disk, decision 33). The timeline shows how much space it uses and lets the person change the budget.
- When the budget is reached, the oldest moments go first, and the timeline shows where history now begins.

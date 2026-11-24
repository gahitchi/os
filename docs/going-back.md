# Going back (milestone 7, step 1)

Milestone 7 makes Alaya's history something a person can use: one undo for
the whole computer, a list of what happened in plain words, a timeline of
earlier moments, and the story of each file. Underneath, the store learns
two things it needs for the rest of milestone 7: a lock key for every file,
and side branches. The plan is `milestone-7-plan.md`; the choices are
decisions 108 to 110 in [`design/decisions.md`](design/decisions.md).

## What a person sees

- **The one undo** (decisions 109 and 110). The Alaya key (the Windows-logo
  key) plus Z, or the **Undo** button in the top strip, undoes your last
  action, whichever app or part of Alaya it was in: a file deleted in the
  file browser comes back exactly as it was, a changed setting goes back,
  a closed window opens again. The strip says what was undone. Apps keep
  their own Ctrl+Z for typing.
- **Undo never deletes history.** An undo is a new change, so it is in the
  history too, and undoing it is a redo.
- **It asks first when something else changed it since** (rule B6). Then
  Alaya, not the app, shows what changed and asks: *Undo anyway* or *Keep
  as it is*. Enter and Escape answer too. Nothing is overwritten silently.
- **What happened?** (the strip's second button) lists changes newest
  first, in sentences: "You chose to delete summary.txt (in the file
  browser)", "The text editor changed notes.txt", "You undid: change the
  look". Each line has **Undo this**, also for changes an app made on its
  own, which are not on the undo list until then.
- **The timeline** (the Timeline tab): left and right go back and forward
  through the moments history keeps; the list shows the documents as they
  were then, with "Not here now" and "Changed since". Nothing changes
  until **Bring this back**. **Name this version** gives the moment a name;
  named versions are never thinned out.
- **File stories**: in the file browser, **Where did this come from?** (or
  W) says which app made a file, how often and by whom it was changed, and
  what was made from it. It uses only the record of changes and the "made
  from" notes, never names or contents (rule R2).
- **Your history is yours.** Every one of these asks the store as the
  person logged in, and the store answers only from that person's own
  record (decision 68).

## How it works

### The store

- **A lock key for every file.** Each document gets its own key, kept in a
  list locked with the person's key. Names in folders are scrambled with
  the file's key too. *Forget* (milestone 7, step 2, has the screen for
  it) removes a file and destroys its key, so none of its versions can be
  read again, by anyone. The blocks that held old copies of the key list
  are wiped with zeros when they are given up. On an SSD, overwriting is
  not guaranteed to reach the chip itself; until the TPM (milestone 13)
  Alaya says "unreadable to Alaya", not "erased".
- **Side branches.** A copy of a person's files that costs nothing until
  something changes. Work happens there; then it is kept in one save, or
  thrown away and its blocks given back. If the person changed the same
  file meanwhile, keeping stops and says which files (rule B6). The start
  of a side branch is pinned, so thinning keeps it. Used by jobs with rules
  (step 3), installs (milestone 8) and Claude Code (8b).
- **The undo list.** Apps say when the person did something ("deleted
  summary.txt"): first `ACTION_BEGIN` (what was gathered before is saved,
  so the action stands alone), then the change, then `RECORD_ACTION` (saved
  at once). An undo puts back the files that app changed in that commit,
  as they were at the moment before, after checking that nobody changed
  them since. Settings, windows and groups have no files to put back: the
  store marks them undone and the desktop puts them back.
- **Undo a line of "What happened?"** (`UNDO_CHANGE`): the change goes on
  the undo list first, then it is undone like any action.
- **Thinning** now runs once a day in Alaya (decision 33: every moment for
  a day, hourly for a week, daily for a month, weekly after that, within
  15 % of the disk). Named versions and side branches' starts stay.
- **New file keys** come from a generator stirred, when the store server
  starts, with the processor's random numbers (on PCs that have them) and
  the jitter of its clock.
- The disk layout changed (format 2). A disk from an earlier milestone is
  started again empty, with a message; a real update will move files over
  (milestone 8).

### The desktop and the apps

- The desktop does every undo, as itself, so the question and the strip
  are always Alaya's own. "What happened?" asks the desktop to undo a line
  (`UNDO`); no other app may.
- The file browser records deletes; Settings records a change of look; the
  desktop records closing a window (with its close button) and moving it
  into a loose group.
- "What happened?" and the timeline are one Alaya app (role 5), opened from
  the strip.
- Window pictures are now kept per window slot and wiped before the next
  window uses them: memory given back by the kernel does not return to
  where it came from, so making new pictures for every window ran out.

## How it is tested

**On the host** (`cargo test --release -p alaya-store`):

- Forgetting leaves nothing readable (old copies of the key list included);
  forgetting a folder forgets what is in it.
- Named versions survive thinning; side branches change nothing until
  kept, give back their blocks when thrown away, and stop on conflicts.
- Undo puts a moved file back, a deleted folder with its files back, asks
  when something changed since, and refuses to overwrite a name.
- **Undo walk**: 1,000 random changes by several apps, undone one by one to
  the start, matching exactly at every step, then redone.
- A change that was not an action can be undone from "What happened?", and
  that undo undone.
- **A year of history** (a moment every 6 hours, 1,460 in all) thins to
  about 100 kept moments, and every one opens. Alaya's own check (84)
  makes one moment a week, because every moment is a real save to the
  disk, and emulated ARM with more cores than its host saves very slowly.
- **Power cuts during keep, undo and forget**, at every write: the store
  always opens as it was after the last commit it promised, or the one it
  was making.
- **The formal model** (`formal/commit/`) now includes wiping given-up key
  blocks; `EarlyScrub.cfg` checks that wiping one commit too early breaks
  it.

**Inside Alaya** (`cargo xtask test`), checks 76 to 84:

| Check | What it shows |
| --- | --- |
| 76 | A file deleted in the file browser comes back exactly with Alaya key + Z |
| 77 | The Undo button in the top strip does the same |
| 78 | A setting goes back too; when something else changed it since, undo asks; "Keep as it is" changes nothing, "Undo anyway" does it |
| 79 | Closing a window is undone: the app opens again |
| 80 | "What happened?" lists changes in plain words; undo on a line puts it back |
| 81 | The timeline shows the documents as they were; "Bring this back" and "Name this version" work |
| 82 | A file's story says which app made it and what was made from it |
| 83 | Another person sees none of this person's history, undo list, moments or stories |
| 84 | A year of history: every kept moment opens while every core is busy |

Check 84 prints the slowest moment. The gate, under 100 ms, is for real
speed: `cargo xtask test` enforces it when it runs Alaya with KVM and the
host has a core for each of Alaya's. Plain emulation (TCG) is many times
slower; there a moment must open within two seconds. With more cores
than the host has, the host pauses Alaya's cores (one of which may hold
the kernel lock), which measures the host, so the number is only shown;
and once a minute has gone by with every core busy, the rest of the
moments are opened with the cores at rest.

## What is not there yet

- Undoing an admin change asks for the PIN (rule B5): there are no admin
  changes yet.
- The timeline lives in the "What happened?" window; the file browser and
  the desktop following the slider come with the Open and Save windows in
  step 2.
- "What happened?" shows one line per save; grouping lines by time ("the
  editor changed 3 files between 14:00 and 14:10"), with one undo for the
  group, comes next.
- File stories open from a button and W; a right-click menu comes with the
  toolkit's menus.
- Forget for good, grants with time, search, backup and the private-file
  guard are step 2; jobs with rules, "Run it again" and living validity in
  full are step 3.

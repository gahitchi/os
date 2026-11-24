# Groups, protected tasks and resume (milestone 7a, steps 1 and 2)

Milestone 7a is about continuity: you work the way you already do, and
Alaya helps around it. Steps 1 and 2 add groups you make, protected
tasks, and coming back to work with "since you left". The design is
`continuity.md`; the plan is `milestone-7a-plan.md`. The choices made
while the project's owner was away are decisions 121 to 124 in
[`design/decisions.md`](design/decisions.md), marked "to review".

## What a person sees

- **Groups are only labels.** Next to the dock (in the "everything" look)
  are your groups; you start with Work and Home. Drop a window on a
  label to put it in that group. In the file browser, **G** puts the
  chosen file in a group (Alaya asks which). A file can be in several
  groups. A group never changes what any app can see.
- **The group screen.** **Alaya key + G** (or a right click on a label)
  shows a group: what is in it and what you can do. **N** makes a new
  group, **R** renames it, **Delete** takes the chosen file out, **W**
  writes your note.
- **Make it a task.** **T** first shows exactly which files apps in the
  task will see ("Apps you open in it will see these, and nothing else
  of yours"). Only when you confirm does it become a protected task.
  An app whose window is in a protected task sees the task's files (and
  what is inside its folders), **and nothing else of yours, even files
  it was handed before**. What it makes there joins the task. **T** again
  makes it a loose group, and every app is let go.
- **Pause.** **P** pauses a task: its apps are frozen where they are,
  not closed, so nothing unsaved is lost. Their windows stay, dimmed,
  saying so. **P** again lets them go on.
- **Leaving and coming back.** Going into another group (clicking its
  label) or locking the screen *leaves* the group you were in: Alaya
  keeps which apps were open in it and when. When you come back and
  something changed, or you left a note, you see:
  - **Where you left off**: the apps that were open, and **your note**,
    in your own words. Alaya never writes a "next step" for you.
  - **Since you left**: each file of the group that changed (the newest
    change, by which app, when, and how many changes), and each result
    in it that now needs a look.
  - **Resume** (**R**, or the button) opens the apps again and brings the
    group's windows to the front.
- **Receipts on every line.** Press Enter on a line of "since you left"
  to see the record it comes from: the moment in your history, the time,
  the app, what it did. **T** shows that moment on the timeline. A line
  with no record behind it is never shown.

## How it works

- **The store keeps the groups** with the person's app files, so they
  survive a restart: each group (name, colour, task, paused, when left),
  its files, and which app is in which task. `alaya_store::groups`.
- **The wall is in one place.** `Store::access`, which decides what an
  app may touch, first asks whether the app is in a task; if it is, the
  task's files are the whole answer. Every request an app makes goes
  through it, so listing, looking up, reading, writing and making files
  all follow the wall. Alaya's own carriers (the file browser, Settings,
  "What happened?") are not walled.
- **Pausing** is the desktop asking the first program to stop the apps'
  threads (`dk::PAUSE_SHIFT` and `dk::GO_ON_SHIFT` bits on the session
  notification); the windows take no keys meanwhile.
- **The briefing is built only from records.** `Store::since_left` walks
  the ledger back to the moment the person left, keeps the changes to
  files in the group, and adds the "needs a look" marks of results in
  it. Each line carries its receipt (moment, time, app, what; or the
  mark's source). `alaya_store::resume`.

## How it is checked

Host tests: `store/tests/groups.rs` and `store/tests/resume.rs`. Checks
97 to 100 in a full boot (`user/init/src/m7d.rs`):

- 97: a loose group is only a label (a stranger app gets nothing, and
  cannot be put inside a group that is not a task);
- 98: a task is a wall you confirm (the confirm screen comes first; the
  app reaches the task's files and nothing else, not even what it was
  handed before; what it makes joins the task; taking a file out takes
  it out of reach, and undo puts it back);
- 99: a window dropped on a task's label goes inside the wall; pausing
  freezes the app's thread and keeps its window; going on starts it;
  a loose group again lets every app go;
- 100: leaving keeps a return point with the apps open in it; your note
  is kept word for word; coming back shows "since you left"; **every
  line is checked against the ledger or a mark** ("nothing invented"),
  and a file outside the group is never told of;
- 101: a busy day: 40 "URGENT" messages from an app are all kept, none
  shown as "now"; an app cannot use Alaya's own channel; with a window
  full screen, Alaya's three "now" messages all show at once; the
  person's rule (A in the inbox) lets that app reach them at once.

## The decision inbox, looks and hints

Things waiting for you come in four kinds: **now**, **about what you're
doing**, **can wait** and **just so you know**. Only Alaya itself and your
own rules can make something "now"; an app's own label is never
trusted. Alaya key + I shows everything, sorted; the strip counts what
is held. In the inbox, Enter opens the thing's app, Delete is done with
it, and A means "always let this app through".

The look (Everyday, Everything, Play, Create, Build) is chosen in
Settings and changes only what is on screen; every protection works the
same. A few hints show once, the first time they help, and never again.

Waiting for the laptop: the "resume at least 25 % faster than reopening
by hand" gate needs a person and a stopwatch.

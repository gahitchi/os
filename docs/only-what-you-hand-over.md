# Only what you hand over (milestone 7, step 2)

Step 2 of milestone 7 makes Alaya's apps see only what you give them. The
plan is `milestone-7-plan.md`; the choices made while the project's owner
was away are decisions 111 to 113 in
[`design/decisions.md`](design/decisions.md), marked "to review".

## What a person sees

- **Alaya's Open and Save windows.** When an app wants a document (the
  text editor's Ctrl+O, or Ctrl+N to keep a copy under a new name), Alaya,
  not the app, shows the window. The app gets the one document picked
  there, and nothing else. Escape hands over nothing.
- **How long it keeps it.** The window says how long the app may keep
  what you pick, and Tab changes it: until you take it back (the first
  choice, so an app opens again where it was), until the app closes, for
  one hour, or just this version, read only (Open only).
- **What apps can see** (Settings): every hand-over in plain words ("The
  text editor · numbers.txt · until you take it back · may change it"),
  and the files each app made itself. Up and Down choose one; T, or
  **Take it back**, takes it back at once.
- **Forget for good** (the file browser): F, then F again. The document
  leaves its folder at once; for an hour, K keeps it and it comes back
  exactly as it was. Then the store destroys its key: no version of it,
  old ones included, can be read again by anyone, and every app that had
  it loses it.

## How it works

- **Grants** are kept by the store with the person's app files (no
  history), locked with the person's key, so they survive a restart. Each
  says which app, which file, read or read and write, and how long: live,
  pinned to one commit, until a time, until the app closes.
- **The store server enforces them.** Every app (numbers from
  `app_id(0)` up) except Alaya's own carriers (the file browser, Settings
  and "What happened?") is checked on every request that names a
  document: reading, writing, cutting, looking a name up, listing a folder
  (it sees only its entries), removing, renaming, and "made from" notes
  (each source must be handed over too). A refusal says no more than "not
  found". History, the timeline, stories, undo and grants themselves are
  for Alaya's carriers only.
- **What an app makes is its own**: creating a document gives the app a
  live grant marked as its own, which a shorter hand-over never replaces.
- **A pinned grant** reads the exact version: the store serves the past
  version from history, and refuses writes.
- **The desktop draws the windows** (`dk::PICK`) and answers later with a
  `PICKED` event, so the app never waits on a person. Closing an app's
  last window takes back what it had "until the app closes".
- **Forgetting later**: the store keeps a record of each document waiting
  to be forgotten; its server checks the records every tick and destroys
  due keys, then saves. A document brought back meanwhile (K, or the one
  undo) is not forgotten.

## How it is tested

On the host (`cargo test --release -p alaya-store --test grants`): grants
start empty, end on time and when the app closes, pinned grants read one
version only, own files stay own and survive a restart, forgetting takes a
file back from every app, and forgetting waits, can be cancelled, then
leaves old versions unreadable.

Inside Alaya (`cargo xtask test`), checks 85 to 89:

| Check | What it shows |
| --- | --- |
| 85 | A stranger app gets nothing it was not handed, by any route (listing, names, guessed numbers, history, stories, undo, grants, "made from"); what it makes is its own |
| 86 | Hand-overs last as long as you said: one version, until a time, until the app closes; taken back, they are gone |
| 87 | The text editor's Open window hands over only what is picked; Escape hands over nothing; the Save window saves a copy under a new name |
| 88 | Settings lists what each app can see and takes it back |
| 89 | Forget for good: gone at once, kept with K, then no version can be read and no app has it |

## What is not there yet

- Search, backup and the private-file guard are the rest of step 2.
- App files (settings, return points) are not yet kept apart per app.
- The chart maker still finds its numbers by name (it made them); it
  gets an Open window when it can draw charts of other files.

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

Inside Alaya (`cargo xtask test`), checks 85 to 92:

| Check | What it shows |
| --- | --- |
| 85 | A stranger app gets nothing it was not handed, by any route (listing, names, guessed numbers, history, stories, undo, grants, "made from"); what it makes is its own |
| 86 | Hand-overs last as long as you said: one version, until a time, until the app closes; taken back, they are gone |
| 87 | The text editor's Open window hands over only what is picked; Escape hands over nothing; the Save window saves a copy under a new name |
| 88 | Settings lists what each app can see and takes it back |
| 89 | Forget for good: gone at once, kept with K, then no version can be read and no app has it |
| 90 | Search finds a sentence that is no longer in a file, never a forgotten file; an app cannot search; Alaya key + F, typing and Enter hand the editor that one older version, read only |
| 91 | Private files: P in the file browser marks one; an app that reads it is marked until it closes; Alaya's own programs never are |
| 92 | Backup: locked on the stick (no words or names in the clear), only what changed, forgetting reaches the stick, a wrong recovery key opens nothing, the right one brings the documents back; B in Settings backs up |

## Search that remembers

Alaya key + F (or **Search** on the strip) opens Alaya's search box. As
you type, the store looks through your documents: names first, then the
words in them, and then, new here, **what they used to say**. If a
sentence was deleted from a file, or the file was removed, search still
finds it in the moments history keeps ("it said this at 14:02"), and
Enter opens that older version read only in the text editor; Ctrl+S
keeps it under a new name.

- **No index.** Search reads the files when asked. There is no second
  copy of your words to lock, leak or go stale.
- **Apps never search.** Only Alaya's own programs may ask. The result
  you open is handed to the editor like a pick in the Open window: live
  for a document as it is now, one version read only for an older one.
- **Forgotten means forgotten.** A forgotten file's key is gone, so its
  old versions are not found.
- **Your files only.** Each person's search sees only their own
  documents.

## Backup to a stick

Settings has a **Backup** part. B backs up now; after that Alaya backs
up by itself every ten minutes when something changed. In QEMU the
stick is a second virtual drive (`build/stick.img`), with its own driver
keeper, so its driver can crash without touching your disk. Real USB
sticks come in milestone 11.

- **Locked with your recovery key.** The first time, Settings shows a
  recovery key: 32 letters and digits like `ABCD-EFGH-...`. Write it
  down. It is the only way to open the stick on another Alaya. R shows it
  again while you are logged in.
- **Only what changed.** The machine remembers what the stick got last
  time; a run writes only documents and folders that changed, and notes
  what was removed.
- **Forget reaches the stick.** Each document has its own key on the
  stick. When you forget a document, the next run leaves its key out and
  writes both key tables, so its old copies there cannot be read either.
- **A cut-off run harms nothing.** Two headers and two key tables are
  used in turn; the newest one that opens wins.
- **Getting them back.** G in Settings, type the recovery key, Enter.
  The documents come back as they were at the last run, in a folder
  called "From the stick" (or straight into your documents when they are
  empty, as on a fresh Alaya).

## Private files

P in the file browser marks a document **private** (decision 27). An app
that reads a private document is **marked** until it closes, and
Settings says so. A marked app will need your OK before it goes online;
Alaya has no network yet, so for now the mark is recorded and shown. The
mark spreads to an app that takes data from a marked one, but never to
Alaya's own carriers (the desktop, the store, the clipboard): otherwise
everything would be marked after one private file (rule B1). Apps cannot
mark or unmark anything.

## What is not there yet
- App files (settings, return points) are not yet kept apart per app.
- The chart maker still finds its numbers by name (it made them); it
  gets an Open window when it can draw charts of other files.

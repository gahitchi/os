# Concept: history and grants

Status: **proposal**, 19 November 2026. If it is adopted, it replaces decisions 17 and 18 in `decisions.md`.

## The idea in one paragraph

Every change on the machine is kept, and every change is signed by the capability that made it. Apps never see the file system. They see only what the person handed them, and each handed-over thing carries a moment in time as well as rights. From those two rules come features no shipping OS has together: undo for one app across the whole machine, letting an app try its changes on a branch of the system before you accept them, and giving an app "my photos as they were last Tuesday".

## The four layers

### 1. The store: everything has history

- The copy-on-write file system (decision 16) keeps old versions instead of freeing them at once. Each commit is a moment: a root pointer plus a time.
- Every commit records **who wrote it**: the capability, and through it the app, that made the change. Since every write in a microkernel arrives as a message on a capability, this costs one field per commit.
- Old moments are thinned by a retention rule: every moment for a day, hourly for a week, daily for a month, weekly after that. History also has a disk budget (default 15 % of the disk); when it is reached, the oldest moments go first. The cleanup then reclaims their blocks.
- Files that change constantly (databases, caches, swap) can be marked **no history**, so they do not fill the disk.

### 2. Grants: capabilities that carry time

A capability to stored data names three things:

| Part | Examples |
| --- | --- |
| What | one file, one folder, the results of a search |
| Rights | read, write, create, delete, grant onward |
| When | **live** (follows changes), **pinned** (a fixed moment, read-only), or **until** (expires at a time or when the app closes) |

"Read my Photos folder as it was last Tuesday" and "write this one document until I close the app" are both ordinary capabilities. As far as I know, no shipping OS has capabilities with a time component.

### 3. Views: one tree for the person, nothing for apps

- **The person** sees a normal folder tree in the desktop's file manager, plus saved searches and tags as other ways to look at the same files.
- **Apps** see no tree at all. An app starts with only its own private folder. Everything else arrives as a grant.
- Grants come from three places, all run by the trusted desktop, never by the app:
  1. the system's open and save dialogs (choosing a file *is* granting it),
  2. drag and drop onto the app,
  3. an install-time request the person accepts ("this editor wants your Documents folder").
- The desktop can show, for any app, everything it can touch right now, and revoke any of it.

This is the change to decision 17: the shared tree stays, but only the person and the trusted desktop see it.

### 4. The ledger: what each app did

Because every commit is signed and every grant is recorded, the system keeps a ledger of which app read, wrote and deleted what, and when. From the ledger:

- **Undo for one app.** "Undo everything this app changed since 3 pm" reverts only that app's writes and leaves everything else alone. If someone else changed the same file since, the system asks.
- **Try on a branch.** An app, an installer or an update can run against a branch of the store, a private copy that costs nothing until it writes. Afterwards the person looks at what it changed and merges it or throws it away.
- **Replay.** The ledger records what an app read and at which moment, so a run can be replayed against the same versions to find a bug.

## What exists elsewhere, and what is new

| Piece | Where it exists already |
| --- | --- |
| Snapshots and rollback of a whole disk | ZFS, btrfs, Time Machine, NixOS (for system settings) |
| The file picker grants access | Research systems: Sandstorm, Genode, Capsicum; partly macOS and Android sandboxes |
| Programs see only what they are given | Plan 9, Fuchsia |
| Branch and merge | git, for source code only |

New in this combination:

1. Every write is attributed to the capability that made it, so undo and history can work **per app**, not only per disk.
2. Capabilities that carry **time**: pinned, live and expiring grants.
3. **Branch and merge for the whole machine**, used for trying apps and updates safely.
4. **An honest permission screen.** What an app can touch is a list of live grants, not a promise in a settings page.

## Costs and risks

- **Disk space.** History costs space. The retention rule and the "no history" mark keep it bounded, but disks fill faster than on other systems.
- **Write speed.** Each commit carries extra metadata. Programs that write heavily (databases) need the "no history" mark, or batched commits.
- **Merging can conflict.** When two branches change the same file, someone must choose. For many file types the system can only offer "keep this one or that one".
- **Some apps need wide access.** Backup tools, IDEs and file managers need folder grants. The install-time request covers them, but people tend to click yes.
- **POSIX gets harder.** Ported tools expect a full `/`. The POSIX layer gives them a fake `/` assembled from their grants, which suits command-line tools but not every program.
- **It is the hardest part of the OS.** The file server becomes the most complex program on the machine, so it should be built in stages: plain copy-on-write first, then attribution, then grants with time, then branches.

## Decisions this would change

| # | Was | Would become |
| --- | --- | --- |
| 17 | One shared tree for every program | One tree for the person and the trusted desktop; apps see only their own folder and their grants |
| 18 | Rights on the program's file-server handle | Grants: capabilities with what, rights and when, issued by the open/save dialogs, drag and drop, and install-time requests |
| new | | Every commit is attributed to a capability; history is kept by a retention rule; a ledger enables per-app undo, branches and replay |

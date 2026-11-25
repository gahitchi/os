# The store (milestone 5)

Milestone 5 gives Alaya a place to keep files that cannot be damaged by a
power cut, keeps every person's files under their own key, and remembers,
for every saved version, which app wrote it and which apps read it. It is
the first piece of Alaya's signature idea, history and grants: from here on
every change is kept and attributed.

This page explains how it works. The plan, with the choices it needed, is
in the milestone plan; the design behind it is decisions 16, 33 and 68 in
[`design/decisions.md`](design/decisions.md).

## In one paragraph

The store never writes over anything. A save writes the changed pieces to
free places on the disk, waits until the disk says they are really there,
and only then writes a small *front page* that points at the new state.
Until that front page is down, the old state is untouched; once it is, the
new state is complete. A power cut at any moment therefore leaves one or
the other, never a mix. Every piece is scrambled (encrypted) and carries a
fingerprint, so damage is noticed instead of read as data, and a spare copy
of the store's map can repair it.

## The parts

The store's logic is a library, `store/` (crate `alaya-store`), with no
dependencies and no `unsafe`. It knows nothing about Alaya: it talks to a
disk through a four-function `Disk` trait (blocks, read, write, flush). On
the host the tests give it a simulated disk; inside Alaya the *store
server* (`user/programs/src/bin/store.rs`) gives it the real one, through
the disk keeper of milestone 4.

| File | What it is |
| --- | --- |
| `store/src/crypto.rs` | AES-256, SHA-256 and HMAC, written here and checked against the official test vectors |
| `store/src/format.rs` | The on-disk layout: block pointers, tree nodes, front pages |
| `store/src/disk.rs` | The `Disk` trait and `MemDisk`, a simulated disk with a write cache that can lose power at any write |
| `store/src/space.rs` | Which blocks are free |
| `store/src/tree.rs` | The copy-on-write tree everything is kept in |
| `store/src/lib.rs` | The store: people, files, history, classes, saving, checking |

### Blocks, fingerprints and scrambling

The disk is cut into 4 KiB blocks. Blocks 0 and 1 are the two front pages;
everything else is reached from them through *pointers*. A pointer holds
the block's address, the address of its spare copy (if it has one), the
commit that wrote it, and its 16-byte fingerprint (a keyed HMAC-SHA-256 of
the block's plain contents, its address and its commit). A parent block
holds its children's fingerprints, so the front page's fingerprint vouches
for the whole tree (a Merkle tree). Reading a block always checks it.

Each block is scrambled with AES-256 in counter mode, starting from its own
fingerprint (the "synthetic IV" construction of RFC 5297). Two blocks share
a key stream only if they have the same address, commit and contents, and
then they are the same bytes anyway. So even if a commit number were ever
used twice, nothing leaks. The TLA+ model below found that this can
happen (a newer front page with a bad spot hides the last number used),
which is why the store scrambles this way.

### Keys and people

The machine has a key for the store's own map. Each person has their own
key, and their files live in their own *branch* of the store, scrambled
with it. Without the key, a person's branch is noise; a wrong key opens
nothing (check 46), and a raw scan of the disk shows no file contents
(check 51). For now the machine key is a fixed test key; it moves into the
login and the TPM (decision 29) with the login screen in milestone 6.

### Commits

Changes are gathered in memory and saved together in a *commit*: by
default every 5 seconds (choice A), or when an app asks. A commit:

1. writes every changed block to a free place (never over a block that a
   kept state still uses),
2. flushes the disk (waits until the blocks are really on it),
3. writes the front page of its number (even numbers on page 0, odd on
   page 1), and
4. flushes again. Only now is the commit *promised*.

Blocks a commit gives up are not reused until two commits later. That
keeps the older front page's state whole, so if the newest front page gets
a bad spot, the store opens one step back instead of not at all.

Opening the store picks the newest front page whose whole tree checks out,
and writes a fresh front page for it at once (on the other page, so the
one it opened stays as the older page).

### History, attribution and reads

Each person's branch has three *storage classes* with their own trees:

| Class | History | For |
| --- | --- | --- |
| Documents | every version kept, thinned over time | your files |
| Apps | one step back while an update runs | installed apps and games |
| Caches | none | things that can be made again |

Every commit records which app changed which file. Every stored version
has a permanent identity: the file's number and the commit that made it.
When an app reads a document, the store notes which version it saw; the
note goes into the next commit. This is what "Run it again" and living
validity (decision 68, `design/commitments.md`) will be built on. Only
documents are tracked: it costs 1.9 % on a file-heavy host test (the goal
was at most 3 %). Check 52 also measures a worst case inside Alaya, where
every read is new; under emulation its result is within the noise of the
measurement (between about -7 % and +15 % from run to run).

**Made from (milestone 6c).** An app can say which exact versions of
which documents it made a result from; the store keeps that note with the
result (key kind 3 in the documents tree, so it has history) and checks
it against what it saw the app read. `mark` then says whether the result
is up to date. Each document also counts its changes (inode word 4, above
the flags), because several saves can share one version until the next
commit. See [`desktop.md`](desktop.md), step 4.

Old versions are thinned as decision 33 says (all for a day, then hourly,
daily, weekly), within a history budget of 15 % of the disk.

### A full disk

2 % of the disk (at least 64 blocks) is kept back so that deleting,
saving and making room always work, even on a full disk. When space runs
out, the oldest history goes first, but the last day is always kept
(choice B). Check 53 fills a disk completely, deletes everything and gets
the room back.

### Cleaning (choice D)

The store cleans up after apps by itself, but never removes a person's own
files:

- **Caches** nobody used for 30 days are emptied once a day, and when the
  disk runs low caches go first, least recently used first, before any
  history. A cleaned cache file stays, empty, so the app just makes it
  again. The store notes the day a cache was last used, at most once a day
  per file, so reading caches costs nearly nothing.
- **Apps**: the copy kept from before an update is let go as soon as the
  update is kept.
- **Your own files** are never removed by the store. It can list big files
  you have not changed for a long time (`cleanup_suggestions`), so that the
  desktop (milestone 6) can ask you. A file you agree to remove still stays
  in history, so it can be brought back.

Until Alaya reads the real clock (a later milestone), dates come from the
time since the machine started, so "30 days" counts time switched on.

### Spare copies

The store's map (its own tree and the free-space list) has a spare copy of
every block on another part of the disk (choice C). A damaged block of the
map is read from the spare and repaired in place. Documents can have spare
copies too, as an option.

## Inside Alaya

The store server runs as an ordinary program. It talks to the disk keeper
with four new requests (read, write and flush 4 KiB blocks, and ask the
disk size) that only the store may send, and only beyond the first MiB,
which stays for the disk tests of milestone 4. The disk driver now asks
the virtual disk for its flush command, so "really on the disk" means what
it says.

Apps talk to the store server with messages; each app's badge carries its
person and app number, so the store knows who wrote and who read without
the app saying so. Data travels through shared transfer pages.

## How it is tested

**On the host** (`cargo test --release -p alaya-store`):

- The official test vectors for AES-256 (FIPS-197, SP 800-38A), SHA-256 and
  HMAC (RFC 4231), and a check that two different blocks with the same
  address and commit never share a key stream.
- **Power cut at every write.** A scripted session of files being created,
  changed, renamed and deleted is cut at every single disk write and flush,
  with three different ways the disk's cache may have kept or lost what was
  in it: 3,168 cuts. After each, the store must open, pass its own full
  check, and show exactly the last promised commit or a later one. Also
  without spare copies, with spare copies of documents, in long sessions,
  and with a bad spot on the newest front page.
- **Damaged blocks.** Every block of a store is damaged in turn; the store
  must notice, repair what has a spare, and report the rest.
- **Read tracking cost**: at most 3 % extra on a file-heavy workload.

**The TLA+ model** (`formal/commit/`, run by `formal/check.sh`): commits,
a disk that keeps writes in a cache until a flush, power cuts at any
moment (any part of the cache reaches the disk, a front page may be torn),
a bad spot on the newest front page, and the reopening. TLC checks that the
store always opens, and never loses a promised commit (one step back only
when the newest front page itself was damaged), with states of one block
over four commits (`Small.cfg`) and of two blocks over three
(`TwoBlocks.cfg`). It also checks that the model has teeth: reusing
given-up blocks after one commit instead of two (`EarlyReuse.cfg`), or
writing the front page without flushing the blocks
first and then losing the older page to a bad spot (`NoFlush.cfg`), each
break it.

**Inside Alaya** (`cargo xtask test`), checks 44 to 53:

| Check | What it shows |
| --- | --- |
| 44 | The store formats the disk through the keeper, in 4 KiB blocks |
| 45 | Files come back exactly after the store starts again |
| 46 | Each person's files are under their own key; a wrong key opens nothing |
| 47 | History keeps every version; each commit records which app changed and read what |
| 48 | Documents keep history, apps one step back, caches none |
| 49 | The store is stopped dead at 40 exact points inside saves (200 with `--long`): two files changed together always match, no promised commit is lost |
| 50 | The disk driver is crashed again and again while the store saves: no error reaches the store, every block is right |
| 51 | A raw scan of the disk shows no file contents |
| 52 | What recording reads costs, printed |
| 53 | A full disk can still delete, and old history makes room |

**Pulling the plug** (`cargo xtask test --plug <n>`): each boot opens the
store the last boot left on the virtual disk, checks two files that are
always changed together, prints what it found, and keeps saving; the host
kills QEMU at a random moment and boots again. Every boot must find both
files whole and equal, with a value at least the last one promised and at
most the last one being saved. Killing QEMU loses what the virtual machine
had not handed to the disk yet; a cut that also loses the disk's own cache
is what the host tests do at every write.

## What is not there yet

- The machine key is a test key until the login (milestone 6) and the TPM.
- Scrambling is in software. Hardware AES comes later (on the PC the
  kernel does not save the vector registers for programs yet).
- One undo, the timeline, named versions, a lock key for every file and
  side branches came in milestone 7, step 1:
  [`going-back.md`](going-back.md). Grants with time and forgetting files
  for good (its screen) are step 2.

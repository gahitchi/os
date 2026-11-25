// SPDX-License-Identifier: GPL-3.0-only
//! What the first program, the driver keeper, the disk driver and the disk
//! test app agree on: message labels, badges, signal bits, where shared
//! pages sit, and the layout of the pages they share.
//!
//! The keeper looks after one driver (decisions 70, 78, 79 and 80):
//!
//! - Apps call the keeper, never the driver. The keeper writes each request
//!   into a slot of the **job page** it shares with the driver and remembers
//!   it until the driver says it is done.
//! - If the driver crashes or hangs, the keeper ends it (one revoke of the
//!   memory it was made from), starts it again, and the new driver finds
//!   every unfinished job still in the job page and sends it to the device
//!   again. Doing a disk job twice changes nothing, so apps see no error.
//! - More than [`keeper::RESTART_LIMIT`] restarts within a minute switch the
//!   device off: waiting and new requests get [`status::DEVICE_OFF`] until
//!   someone asks the keeper to try again.
//! - A live update parks the running driver, which writes its state into
//!   the **handover page**; the new version takes the device over from that
//!   state without resetting it. The old one stays parked for a trial
//!   period; if the new one crashes in that time, the old one comes back.

#![no_std]

pub mod virtio;

use core::sync::atomic::{AtomicU64, Ordering};

/// Bytes in a disk sector.
pub const SECTOR: usize = 512;

/// Status words in answers to apps.
pub mod status {
    pub const OK: u64 = 0;
    /// The device was switched off after too many crashes (decision 78).
    pub const DEVICE_OFF: u64 = 1;
    /// The device reported an error.
    pub const IO_ERROR: u64 = 2;
    /// A bad request (sector out of range, unknown label).
    pub const BAD_REQUEST: u64 = 3;
    /// The keeper has no room for another waiting request.
    pub const BUSY: u64 = 4;
}

/// The driver keeper's endpoint.
pub mod keeper {
    /// Badges on the keeper's endpoint.
    pub const CONTROL: u64 = 1;
    pub const CLIENT: u64 = 2;
    /// Faults of driver slot `n` arrive with badge `FAULT + n`.
    pub const FAULT: u64 = 0x100;

    /// App labels. READ: sector → status, checksum of the data read.
    /// WRITE: sector, seed → status (the data is [`super::pattern`]).
    pub const READ: u64 = 1;
    pub const WRITE: u64 = 2;
    /// SIZE: → sectors on the disk.
    pub const SIZE: u64 = 3;
    /// Block labels, for the store only (badge [`STORE`]); blocks are
    /// 4 KiB and the data is in buffer slot `slot` of the pages the store
    /// shares with the keeper ([`CLIENT_BUF`]).
    /// BREAD: block, slot → status. BWRITE: block, slot → status.
    pub const BREAD: u64 = 4;
    pub const BWRITE: u64 = 5;
    /// FLUSH: → status, once everything written before is kept for good.
    pub const FLUSH: u64 = 6;
    /// BLOCKS: → first block of the store's area, blocks in it.
    pub const BLOCKS: u64 = 7;
    /// Badge of the store's copy of the keeper's endpoint.
    pub const STORE: u64 = 3;
    /// The disk is split in two: the first MiB for the disk tests of
    /// milestone 4 (sector labels), the rest for the store (block labels).
    pub const STORE_FIRST_BLOCK: u64 = 256;
    /// Where the store's buffer pages are, in the keeper.
    /// (Next to the driver images, so it shares their page tables.)
    pub const CLIENT_BUF: u64 = 0x6000_0200_0000;
    pub const BUF_SLOTS: u64 = 32;

    /// Control labels (badge [`CONTROL`]).
    /// STATS: → restarts, resent jobs, updates, rollbacks, version,
    /// state ([`state`]).
    pub const STATS: u64 = 10;
    /// CHAOS: on, shortest gap ms, longest gap ms, seed: end the driver at
    /// random moments, by crashing it or by freezing it.
    pub const CHAOS: u64 = 11;
    /// UPDATE: image index, trial ms, flags ([`super::driver::FLAG_FAULTY`])
    /// → status, version now running. Answers once the new version runs.
    pub const UPDATE: u64 = 12;
    /// POLICY: restarts allowed, window ms (decision 78; the tests shorten
    /// the minute so they can crash the driver often).
    pub const POLICY: u64 = 13;
    /// TRY_AGAIN: switch a device that was turned off back on.
    pub const TRY_AGAIN: u64 = 14;
    /// CRASH_NOW: crash the driver once, now.
    pub const CRASH_NOW: u64 = 15;
    /// FREEZE_NOW: stop the driver where it is, once (the watchdog must
    /// notice).
    pub const FREEZE_NOW: u64 = 17;
    /// MORE: → watchdog restarts, crash restarts, device faults seen by
    /// the guard probe, jobs done, trial left ms.
    pub const MORE: u64 = 16;

    /// Signal bits on the keeper's own notification.
    pub const DONE_BIT: u64 = 1;
    pub const TICK_BIT: u64 = 2;
    pub const PARKED_BIT: u64 = 4;

    /// Restarts allowed per window (decision 78).
    pub const RESTART_LIMIT: u64 = 3;
    pub const RESTART_WINDOW_MS: u64 = 60_000;
    /// A driver that holds work this long without finishing any is hung.
    pub const WATCHDOG_MS: u64 = 2_000;
    /// How long the old version is kept after a live update (decision 80).
    pub const TRIAL_MS: u64 = 10 * 60 * 1000;

    /// Device states in STATS.
    pub mod state {
        pub const RUNNING: u64 = 0;
        pub const OFF: u64 = 1;
        pub const UPDATING: u64 = 2;
        pub const TRIAL: u64 = 3;
    }

    /// Where the driver images are mapped in the keeper.
    pub const IMAGES: u64 = 0x6000_0000_0000;
    pub const IMAGE_STEP: u64 = 0x100_0000;

    /// Args: bdf, image count, image sizes (2), device page count, then the
    /// device page codes ([`super::driver::bar_code`]).
    pub const ARG_BDF: usize = 0;
    pub const ARG_IMAGES: usize = 1;
    pub const ARG_IMAGE_SIZE: usize = 2;
    pub const ARG_DEV_PAGES: usize = 4;
    pub const ARG_DEV_CODES: usize = 5;
    /// Given capabilities, from `start::FIRST_GIVEN`: its endpoint, the
    /// I/O space of the device, the device's interrupt, the registry, two
    /// scheduling contexts for drivers, the PCI configuration page, then
    /// the device pages.
    pub const G_ENDPOINT: u64 = 0;
    pub const G_IO: u64 = 1;
    pub const G_IRQ: u64 = 2;
    pub const G_REGISTRY: u64 = 3;
    pub const G_SC: u64 = 4;
    pub const G_CONFIG: u64 = 6;
    pub const G_DEV: u64 = 7;
}

/// The keeper's side of a driver.
pub mod driver {
    /// Jobs in flight at once.
    pub const JOBS: usize = 32;
    /// Virtqueue size: three descriptors per job.
    pub const QUEUE: u16 = 128;

    /// Bits on the driver's notification.
    pub const IRQ_BIT: u64 = 1;
    pub const JOBS_BIT: u64 = 2;
    pub const PAUSE_BIT: u64 = 4;
    /// Bit on the park notification: the new version failed, take over.
    pub const ROLLBACK_BIT: u64 = 1;

    /// Where things are in the driver's address space.
    pub const JOB_PAGE: u64 = 0x5000_0000_0000;
    pub const HANDOVER_PAGE: u64 = 0x5000_0001_0000;
    pub const RING_PAGE: u64 = 0x5000_0002_0000;
    pub const HEADER_PAGE: u64 = 0x5000_0003_0000;
    pub const CONFIG_PAGE: u64 = 0x5000_0004_0000;
    pub const BARS: u64 = 0x5100_0000_0000;

    /// The page of BAR `bar` at `page` in the driver.
    pub const fn bar_page(bar: u64, page: u64) -> u64 {
        BARS + (bar << 24) + page * 4096
    }

    /// How a device page is named in args: BAR << 16 | page within it.
    pub const fn bar_code(bar: u64, page: u64) -> u64 {
        bar << 16 | page
    }

    /// Device addresses (behind the IOMMU) of the keeper's DMA pages.
    pub const IOVA_RING: u64 = 0x10_0000;
    pub const IOVA_HEADER: u64 = 0x11_0000;
    pub const IOVA_DATA: u64 = 0x20_0000;
    /// Never mapped: the guard probe points the device here.
    pub const IOVA_FORBIDDEN: u64 = 0x7000_0000;
    /// One 4 KiB data page per job.
    pub const DATA_PAGES: u64 = JOBS as u64;

    /// Given capabilities, from `start::FIRST_GIVEN`: its notification,
    /// the park notification, the keeper's notification, the interrupt.
    pub const G_NTFN: u64 = 0;
    pub const G_PARK: u64 = 1;
    pub const G_KEEPER: u64 = 2;
    pub const G_IRQ: u64 = 3;

    /// Args: flags, device page count, the device's PCI function (bus << 8
    /// | device << 3 | function), then the device page codes.
    pub const ARG_FLAGS: usize = 0;
    pub const ARG_DEV_PAGES: usize = 1;
    pub const ARG_BDF: usize = 2;
    pub const ARG_DEV_CODES: usize = 3;
    /// Take the device over from the handover page instead of resetting it.
    pub const FLAG_HANDOVER: u64 = 1;
    /// Before starting, point the device at memory it does not own once,
    /// to see the guard stop it.
    pub const FLAG_PROBE: u64 = 2;
    /// Test only: crash after a few jobs (a broken new version).
    pub const FLAG_FAULTY: u64 = 4;
}

/// One request slot of the job page. The keeper owns `posted`, `op` and
/// `sector`; the driver owns `done` and `status`. A job is waiting while
/// `posted != done`.
#[repr(C)]
pub struct Job {
    pub posted: AtomicU64,
    pub done: AtomicU64,
    pub op: AtomicU64,
    pub sector: AtomicU64,
    pub status: AtomicU64,
    /// Sectors: 1 for the milestone 4 tests, 8 for a 4 KiB block.
    pub count: AtomicU64,
    _pad: [u64; 2],
}

/// Driver states in the job page.
pub mod run {
    pub const STARTING: u64 = 0;
    pub const RUNNING: u64 = 1;
    pub const PARKED: u64 = 2;
    pub const FAILED: u64 = 3;
}

/// The job page.
#[repr(C)]
pub struct JobPage {
    /// The running driver's version and state ([`run`]).
    pub version: AtomicU64,
    pub state: AtomicU64,
    /// Jobs the driver finished, since the keeper started.
    pub finished: AtomicU64,
    /// Disk size in sectors, as the driver read it.
    pub sectors: AtomicU64,
    /// Guard probe: 1 once the driver pointed the device at memory it was
    /// not given.
    pub probe: AtomicU64,
    /// Why the driver failed to start, when it did.
    pub error: AtomicU64,
    _pad: [u64; 26],
    pub jobs: [Job; driver::JOBS],
}

/// Disk operations in [`Job::op`].
pub const OP_READ: u64 = 0;
pub const OP_WRITE: u64 = 1;
/// Make every finished write permanent (the device's write cache).
pub const OP_FLUSH: u64 = 2;

const _: () = assert!(core::mem::size_of::<JobPage>() <= 4096);

/// What a parked driver hands to its next version.
#[repr(C)]
pub struct Handover {
    pub magic: u64,
    /// Layout of this page; a driver refuses formats it does not know.
    pub format: u64,
    pub from_version: u64,
    pub queue_size: u64,
    /// Next free index of the available ring, last used index handled.
    pub avail_idx: u64,
    pub used_seen: u64,
    pub sectors: u64,
    /// Where the notify register is, as an offset among the device pages.
    pub notify_offset: u64,
    /// The job posted number each slot was last sent to the device with.
    pub submitted: [u64; driver::JOBS],
    /// 1 when the device has a write cache to flush.
    pub flush: u64,
}

pub const HANDOVER_MAGIC: u64 = 0x4841_4e44_4f56_4552;
pub const HANDOVER_FORMAT: u64 = 2;

const _: () = assert!(core::mem::size_of::<Handover>() <= 4096);

/// Word `i` of the data written to `sector` with `seed`.
pub fn pattern(seed: u64, sector: u64, i: u64) -> u64 {
    let mut x =
        seed ^ sector.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ i.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 31;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ x >> 29
}

/// The checksum of a sector's words.
pub fn checksum(words: impl Iterator<Item = u64>) -> u64 {
    words.fold(0xcbf2_9ce4_8422_2325, |h, w| {
        (h ^ w).wrapping_mul(0x0100_0000_01b3)
    })
}

/// The checksum a sector written with `seed` must read back as.
pub fn expected(seed: u64, sector: u64) -> u64 {
    checksum((0..(SECTOR / 8) as u64).map(|i| pattern(seed, sector, i)))
}

/// The registry: where programs find each other by name.
pub mod registry {
    /// REGISTER: name (two words), with the capability to hand out.
    pub const REGISTER: u64 = 1;
    /// LOOKUP: name → status, with the capability.
    pub const LOOKUP: u64 = 2;

    /// A name of up to 16 bytes, as two words.
    pub fn name(s: &str) -> [u64; 2] {
        let mut b = [0u8; 16];
        let n = s.len().min(16);
        b[..n].copy_from_slice(&s.as_bytes()[..n]);
        [
            u64::from_le_bytes(b[..8].try_into().unwrap()),
            u64::from_le_bytes(b[8..].try_into().unwrap()),
        ]
    }
}

/// The memory helper (badge: the app's number).
pub mod memory {
    /// UNTYPED: size bits → status, with an untyped capability.
    pub const UNTYPED: u64 = 1;
    /// HUGE_PAGE: → status, with a 2 MiB page.
    pub const HUGE_PAGE: u64 = 2;
    /// USAGE: → bytes used, budget.
    pub const USAGE: u64 = 3;

    pub const OK: u64 = 0;
    pub const OVER_BUDGET: u64 = 1;
    pub const NO_MEMORY: u64 = 2;
    pub const BAD_REQUEST: u64 = 3;

    /// Smallest and largest chunk.
    pub const MIN_BITS: u8 = 12;
    pub const MAX_BITS: u8 = 26;
}

/// The page the disk test app reports on, shared with the first program.
#[repr(C)]
pub struct TestPage {
    /// Set by the first program: stop the test threads.
    pub stop: AtomicU64,
    /// Requests that came back right, came back wrong, or got the clear
    /// "device off" answer.
    pub ok: AtomicU64,
    pub wrong: AtomicU64,
    pub off: AtomicU64,
    /// The last wrong answer: label, sector, status, checksum.
    pub last_wrong: [AtomicU64; 4],
    pub threads: AtomicU64,
    /// Memory helper checks: 1 passed, 2 failed.
    pub memory: AtomicU64,
    pub huge_pages: AtomicU64,
    pub refused: AtomicU64,
    /// Sectors the disk has.
    pub sectors: AtomicU64,
    /// Set when the app could not start; what failed.
    pub error: AtomicU64,
}

pub const TEST_PAGE: u64 = 0x5800_0000_0000;

/// Disk test args: threads, sectors per thread.
pub const DISKTEST_THREADS: usize = 0;
pub const DISKTEST_SPAN: usize = 1;
/// Given: the registry, then a scheduling context per thread.
pub const DISKTEST_G_REGISTRY: u64 = 0;
pub const DISKTEST_G_SC: u64 = 1;

/// Read a word of a page shared with someone else.
pub fn load(a: &AtomicU64) -> u64 {
    a.load(Ordering::Acquire)
}

pub fn store(a: &AtomicU64, v: u64) {
    a.store(v, Ordering::Release)
}

/// The store server (milestone 5): how apps talk to it.
///
/// Each app holds a badged copy of the store's endpoint. The badge says
/// who it is: `person << 48 | app << 8 | channel`. The person decides
/// whose files it sees; the app is what every change and read is recorded
/// under; the channel picks the app's transfer pages, where names and data
/// travel.
pub mod files {
    /// Labels. Every answer starts with a status word ([`st`]).
    /// UNLOCK: secret (4 words) → open the badge's person's files.
    pub const UNLOCK: u64 = 1;
    /// ADD_PERSON: secret (4 words) → add the badge's person.
    pub const ADD_PERSON: u64 = 2;
    /// ROOT: class → file.
    pub const ROOT: u64 = 3;
    /// CREATE: dir, is_dir, name length (name in the transfer pages) → file.
    pub const CREATE: u64 = 4;
    /// LOOKUP: dir, name length → file.
    pub const LOOKUP: u64 = 5;
    /// WRITE: file, offset, length (data in the transfer pages).
    pub const WRITE: u64 = 6;
    /// READ: file, offset, length → bytes read (data in the transfer pages).
    pub const READ: u64 = 7;
    /// STAT: file → size, version, is_dir, edits (documents).
    pub const STAT: u64 = 8;
    /// REMOVE: dir, name length.
    pub const REMOVE: u64 = 9;
    /// SAVE: → commit number, once everything gathered is safe.
    pub const SAVE: u64 = 10;
    /// HISTORY: → moments; (commit, time) pairs in the transfer pages.
    pub const HISTORY: u64 = 11;
    /// CHANGES: commit → count; (app, file, what) triples in the pages.
    pub const CHANGES: u64 = 12;
    /// READS: commit → count; (app, file, version) triples in the pages.
    pub const READS: u64 = 13;
    /// PAST_READ: commit, file, offset, length → bytes read.
    pub const PAST_READ: u64 = 14;
    /// CHECK: → problems, blocks checked, moments checked, data blocks.
    pub const CHECK: u64 = 15;
    /// STATS: → commits, last commit, blocks used, blocks, history blocks,
    /// repaired blocks.
    pub const STATS: u64 = 16;
    /// TRUNCATE: file, size.
    pub const TRUNCATE: u64 = 17;
    /// KEEP_APPS / RELEASE_APPS: the copy of the apps from before an update.
    pub const KEEP_APPS: u64 = 18;
    pub const RELEASE_APPS: u64 = 19;
    /// MAKE_ROOM: blocks, now → status (choice B).
    pub const MAKE_ROOM: u64 = 20;
    /// TRACK: on → record reads or not (to measure what tracking costs).
    pub const TRACK: u64 = 21;
    /// LIST: dir → count; entries in the pages as (file, name length,
    /// name in 32 bytes), 48 bytes each.
    pub const LIST: u64 = 22;
    /// PAST_STAT: commit, file → size. PAST_LOOKUP: commit, dir, name
    /// length → file.
    pub const PAST_LOOKUP: u64 = 23;
    /// DIE_AFTER: n → after n more writes or flushes to the disk, stop
    /// dead (signal the dying notification, then never run again): a
    /// power cut at an exact point, for the tests.
    pub const DIE_AFTER: u64 = 24;
    /// MADE_FROM: result, count; the sources in the transfer pages as
    /// (file, version, edits), 24 bytes each (living validity: the app
    /// says what it made `result` from).
    pub const MADE_FROM: u64 = 25;
    /// MARK: file → kind | why << 8, source, its version now, the app that
    /// made it, how many sources (see `alaya_store::made`).
    pub const MARK: u64 = 26;
    /// AS_OF: file, on → the result shows a fixed moment (or not).
    pub const AS_OF: u64 = 27;

    // ---- Going back (milestone 7). ----

    /// RENAME: dir, to dir, name length, new name length (the name at 0,
    /// the new name at [`NAME2`] in the transfer pages).
    pub const RENAME: u64 = 28;
    /// ACTION_BEGIN: what was gathered so far is saved, so the action the
    /// app is about to do starts clean; saving by itself waits (at most
    /// [`ACTION_MS`]) until RECORD_ACTION.
    pub const ACTION_BEGIN: u64 = 29;
    /// RECORD_ACTION: kind, label length (label in the pages, at most
    /// [`LABEL`] bytes) → action number. Saved at once.
    pub const RECORD_ACTION: u64 = 30;
    /// ACTIONS: before (0: from the newest), most → count; [`ACTION_BYTES`]
    /// each: number, commit, app, kind | undone << 8 | undoes << 16, label.
    pub const ACTIONS: u64 = 31;
    /// UNDO: action, force → [`undo`] result, count of files (in the pages,
    /// 8 bytes each), the kind of the action, its label (2 words).
    pub const UNDO: u64 = 32;
    /// UNDO_CHANGE: commit, app, force → as UNDO (a "What happened?" line
    /// that was not an action).
    pub const UNDO_CHANGE: u64 = 33;
    /// HAPPENINGS: before (0: from the newest), most → rows, commits;
    /// [`HAPPENING_BYTES`] each: commit, time, app, file, what.
    pub const HAPPENINGS: u64 = 34;
    /// NAME_OF: file, commit (0: now) → name length, folder (name in the
    /// pages).
    pub const NAME_OF: u64 = 35;
    /// NAME_VERSION: commit, name length (0: take the name away).
    pub const NAME_VERSION: u64 = 36;
    /// NAMED: → count; [`NAMED_BYTES`] each: commit, time, name length,
    /// name in 32 bytes.
    pub const NAMED: u64 = 37;
    /// PAST_LIST: commit, dir → count; [`PAST_ENTRY_BYTES`] each: file,
    /// name length, size, is_dir, name in 32 bytes.
    pub const PAST_LIST: u64 = 38;
    /// RESTORE: commit, count (files in the pages) → [`undo`] result,
    /// count (files that are back, or that cannot go back).
    pub const RESTORE: u64 = 39;
    /// STORY: file, most → made by (app), made in (commit), made at
    /// (time), changes, files made from it; changes are
    /// [`STORY_BYTES`] each (commit, time, app, what), then the files.
    pub const STORY: u64 = 40;
    /// FAKE_HISTORY: days, hours between moments → moments made (tests
    /// only: a year of history, made fast, as if it had been lived).
    pub const FAKE_HISTORY: u64 = 41;
    /// GRANT: app, file, write, how long (kind), its argument → status.
    /// Hand a document to an app (the desktop only: the Open and Save
    /// windows, milestone 7 step 2). Kinds as `alaya_store::grants::Lasts`:
    /// 0 live, 1 pinned to a commit, 2 until a time (seconds), 3 until the
    /// app closes.
    pub const GRANT: u64 = 42;
    /// REVOKE: app, file → 1 if it had it.
    pub const REVOKE: u64 = 43;
    /// GRANTS: app (0: every app) → count; [`GRANT_BYTES`] each in the
    /// transfer page (app, file, kind | write << 8 | own << 9, argument).
    pub const GRANTS: u64 = 44;
    /// APP_CLOSED: app → grants taken back (the desktop, when an app's
    /// last window closes).
    pub const APP_CLOSED: u64 = 45;
    /// FORGET_LATER: folder, name length, seconds from now → file. The
    /// document leaves its folder at once; its key is destroyed then,
    /// unless it was kept meanwhile (decision 62: an hour to change your
    /// mind).
    pub const FORGET_LATER: u64 = 46;
    /// FORGETTING: → count; (file, when, seconds left, the commit before)
    /// each, 32 bytes.
    pub const FORGETTING: u64 = 47;
    /// KEEP_FORGOTTEN: file → status. It goes back where it was.
    pub const KEEP_FORGOTTEN: u64 = 48;
    /// The grace time before a document is forgotten for good.
    pub const FORGET_GRACE: u64 = 3600;
    /// SEARCH: words length (the words in the transfer pages), most,
    /// moments to look back through → count; [`SEARCH_BYTES`] each: file,
    /// folder, where (u64::MAX: the name matched), the moment it was found
    /// in (0: now), that moment's time, gone, name length, line length,
    /// then the name (32 bytes) and the line (64 bytes). Not for apps that
    /// see only what they are handed: they get the person's pick instead.
    pub const SEARCH: u64 = 49;
    /// BACKUP: erase (1: start the stick again even if it holds another
    /// backup) → run, written, gone, blocks used, bytes. To the backup
    /// stick (milestone 7, decision 36); Alaya's own programs only.
    pub const BACKUP: u64 = 50;
    /// RECOVERY_KEY: → 1 if it was just made; the key's 32 letters and
    /// digits (39 bytes with dashes) in the transfer pages. The desktop,
    /// Settings and the first program only.
    pub const RECOVERY_KEY: u64 = 51;
    /// RESTORE_BACKUP: folder name length → folders, documents, the
    /// folder they went into, run. The recovery key as typed (39 bytes)
    /// at the start of the transfer pages, the folder's name at byte 64.
    pub const RESTORE_BACKUP: u64 = 52;
    /// STICK: → there is one (1), its blocks, and the person's last
    /// backup: run, moment (0 0: none).
    pub const STICK: u64 = 53;
    /// STICK_FIND: length (bytes in the transfer pages) → 1 if the stick
    /// holds them anywhere in the clear (the first program's checks).
    pub const STICK_FIND: u64 = 54;
    /// PRIVATE: file, what (0: not private, 1: private, 2: just ask) → is
    /// it private. Marking is the person's (the file browser); apps that
    /// see only what they are handed cannot ask (decision 27).
    pub const PRIVATE: u64 = 55;
    /// MARKS: → count; (app, the private file it read, since) 24 bytes
    /// each: the apps that read a private file and need the person's OK
    /// before going online.
    pub const MARKS: u64 = 56;
    /// JOB_START: app, folder, checks (`alaya_store::jobs::check` bits),
    /// the checklist file (0: none), how many must stay unchanged (their
    /// numbers in the transfer pages), label length (the label at byte
    /// [`JOB_LABEL_AT`]) → side branch. Alaya's own programs only (the
    /// desktop, after the person approved the rules screen). From then on
    /// the app's document requests go to the job's side copy, only inside
    /// the folder (milestone 7, step 3).
    pub const JOB_START: u64 = 57;
    /// JOB: → whether the asking app has a job (1), its folder, whether it
    /// said it is done, its checks. For the app itself.
    pub const JOB: u64 = 58;
    /// JOB_DONE: the asking app says its job is done: it can change
    /// nothing more, and Alaya runs the checks.
    pub const JOB_DONE: u64 = 59;
    /// JOB_CHECK: app → passed, checks, stale files; then each check as
    /// (check, line, ok, file) 32 bytes, then the stale files' numbers.
    pub const JOB_CHECK: u64 = 60;
    /// JOB_KEEP: app, anyway (1: the person keeps it although the checks
    /// failed), how many conflicts the person's version wins (their
    /// numbers in the transfer pages) → [`job_kept`], count, then files.
    pub const JOB_KEEP: u64 = 61;
    /// JOB_DISCARD: app: throw the job's work away.
    pub const JOB_DISCARD: u64 = 62;
    /// RUN_AGAIN: app, commit → how many files, how many exact; each
    /// (file, version, moment, exact) 32 bytes. "Run it again" (Alaya's
    /// own programs only): until its last window closes, the app sees
    /// exactly the documents it had read by that commit, each at the
    /// version it read, read only (rule B2: the nearest kept moment when
    /// that one was thinned, and then not exact).
    pub const RUN_AGAIN: u64 = 63;
    /// COPY: file, offset, length → copied: the clipboard (kept by the
    /// store, a trusted carrier) holds that part of that exact version of
    /// the document, at most [`CLIP_MAX`] bytes.
    pub const COPY: u64 = 64;
    /// COPY_TEXT: length (the text in the transfer pages): text that
    /// belongs to no saved document.
    pub const COPY_TEXT: u64 = 65;
    /// PASTE: → length, the source (file, version, edits; 0 when none);
    /// the bytes in the transfer pages. A result made from what was
    /// pasted may name that source, and carries its mark (living
    /// validity); a private source marks the app (decision 27).
    pub const PASTE: u64 = 66;
    /// WHAT_CHANGED: result → source, exact, bytes then, bytes now; then
    /// at 0 and now at [`DIFF_AT`] in the transfer pages ("show the
    /// difference"; Alaya's own programs only).
    pub const WHAT_CHANGED: u64 = 67;
    /// Groups and protected tasks (milestone 7a; Alaya's own programs
    /// only). GROUPS: → n; entries of [`GROUP_BYTES`] in the transfer
    /// pages: id, flags (1 task, 2 paused), colour, made, left, then the
    /// name (16 bytes, zero padded).
    pub const GROUPS: u64 = 68;
    /// GROUP_NEW: colour, name length (the name in the transfer pages) →
    /// id.
    pub const GROUP_NEW: u64 = 69;
    /// GROUP_SET: id, flags, colour, left, name length (the name in the
    /// transfer pages).
    pub const GROUP_SET: u64 = 70;
    /// GROUP_REMOVE: id (its files stay where they are).
    pub const GROUP_REMOVE: u64 = 71;
    /// GROUP_ADD: id, file → 1 when it was not in it.
    pub const GROUP_ADD: u64 = 72;
    /// GROUP_TAKE: id, file → 1 when it was in it.
    pub const GROUP_TAKE: u64 = 73;
    /// GROUP_FILES: id → n; the files' numbers in the transfer pages.
    pub const GROUP_FILES: u64 = 74;
    /// OPEN_IN_TASK: app, task (0: none). Only a protected task holds
    /// apps; inside it, an app sees the task's files and nothing else.
    pub const OPEN_IN_TASK: u64 = 75;
    /// TASK_OF: app → task (0: none).
    pub const TASK_OF: u64 = 76;
    /// Resume (milestone 7a, step 2; Alaya's own programs only).
    /// GROUP_LEAVE: group, roles open in it (bit r): the person left it
    /// now; changes after this moment are "since you left".
    pub const GROUP_LEAVE: u64 = 77;
    /// GROUP_NOTE: group, length (the note in the transfer pages; at most
    /// [`NOTE_MAX`]; empty clears it).
    pub const GROUP_NOTE: u64 = 78;
    /// RETURN_POINT: group → left (0: never), moment, roles, note length;
    /// the note in the transfer pages.
    pub const RETURN_POINT: u64 = 79;
    /// SINCE_LEFT: group → n; entries of [`SINCE_BYTES`] in the transfer
    /// pages: kind (1 changed, 2 needs a look), file, moment, time, app,
    /// what, source, count. Each is a ledger record or a mark: its receipt.
    pub const SINCE_LEFT: u64 = 80;
    /// Finish (milestone 7a, step 3; Alaya's own programs only).
    /// FINISH_PLAN: group → n; entries of [`FINISH_BYTES`] in the
    /// transfer pages: fate (0 keep, 1 archive, 2 elsewhere, 3 hand back),
    /// file, other (the other group, or the app), write.
    pub const FINISH_PLAN: u64 = 81;
    /// FINISH: group, n (the files to archive in the transfer pages) →
    /// the work receipt.
    pub const FINISH: u64 = 82;
    /// FINISH_UNDO: group. Everything Finish did comes back.
    pub const FINISH_UNDO: u64 = 83;
    /// JOB_TRACE: app → how many documents its job read, how many it
    /// changed; in the transfer pages the read ones (8 bytes each), then
    /// each changed one with its `what` bits (16 bytes each). A
    /// rehearsal (milestone 8) shows this before throwing the copy away.
    /// Alaya's own programs only.
    pub const JOB_TRACE: u64 = 84;
    /// Answers instead of data (milestone 8, step 3; R8 to R10).
    /// ANSWER_GRANT: app, file, question ([`question`]), how many a day
    /// (1 to [`ANSWERS_PER_DAY`]; 0 takes it back) → status. The desktop
    /// only, when the person picked the file.
    pub const ANSWER_GRANT: u64 = 85;
    /// ANSWER: file → the answer (an i64), questions left today, 1 when
    /// the whole document was read. Any app holding an answer grant;
    /// [`st::USED_UP`] once today's questions are used.
    pub const ANSWER: u64 = 86;
    /// ANSWERS: before (a moment; 0: the newest), most → n; entries of
    /// [`ANSWER_BYTES`] in the transfer pages: moment, time, app, file,
    /// question, the answer. Alaya's own programs only.
    pub const ANSWERS: u64 = 87;
    pub const ANSWER_BYTES: usize = 48;
    /// QUIET: on (1) or off (0) → backups put off since it went on.
    /// Game mode (the desktop only): the store puts off its backups by
    /// itself while on, for at most [`QUIET_MAX`] seconds.
    pub const QUIET: u64 = 88;
    pub const QUIET_MAX: u64 = 2 * 3600;
    pub const ANSWERS_PER_DAY: u64 = 10;
    /// A question on Alaya's fixed list.
    pub const fn question_ok(q: u64) -> bool {
        matches!(
            (q & 0xff, q >> 8),
            (question::LINES, 0) | (question::WORDS, 0) | (question::TOTAL, 1..=32)
        )
    }
    /// Alaya's fixed list of questions (`alaya_store::answers::Question`).
    pub mod question {
        pub const LINES: u64 = 1;
        pub const WORDS: u64 = 2;
        /// The total of a column: `TOTAL | column << 8` (1 to 32), in
        /// hundredths.
        pub const TOTAL: u64 = 3;
    }
    pub const FINISH_BYTES: usize = 32;
    pub mod fate {
        pub const KEEP: u64 = 0;
        pub const ARCHIVE: u64 = 1;
        pub const ELSEWHERE: u64 = 2;
        pub const HAND_BACK: u64 = 3;
    }
    /// In GROUPS' flags: the group was finished.
    pub const GROUP_DONE: u64 = 4;
    pub const NOTE_MAX: usize = 160;
    pub const SINCE_BYTES: usize = 64;
    pub const GROUP_BYTES: usize = 56;
    pub const GROUP_TASK: u64 = 1;
    pub const GROUP_PAUSED: u64 = 2;
    pub const CLIP_MAX: usize = 4096;
    pub const DIFF_AT: usize = 32 * 1024;
    pub const JOB_LABEL_AT: usize = 512;
    pub mod job_kept {
        pub const KEPT: u64 = 0;
        pub const HELD: u64 = 1;
        pub const CONFLICTS: u64 = 2;
    }
    /// Back up by itself this often, when something changed.
    pub const BACKUP_EVERY: u64 = 600;

    pub const NAME2: usize = 64;
    pub const LABEL: usize = 16;
    pub const ACTION_BYTES: usize = 48;
    pub const HAPPENING_BYTES: usize = 40;
    pub const NAMED_BYTES: usize = 56;
    pub const PAST_ENTRY_BYTES: usize = 64;
    pub const STORY_BYTES: usize = 32;
    pub const GRANT_BYTES: usize = 32;
    pub const SEARCH_BYTES: usize = 160;
    pub const ACTION_MS: u64 = 2000;

    /// Kinds of action (`alaya_store::history::act`).
    pub mod act {
        pub const FILES: u64 = 1;
        pub const SETTINGS: u64 = 2;
        pub const WINDOW: u64 = 3;
        pub const GROUP: u64 = 4;
        pub const UNDO: u64 = 5;
    }

    /// What an undo or a restore did.
    pub mod undo {
        pub const DONE: u64 = 0;
        /// Something else changed it since: nothing was done (rule B6).
        pub const CONFLICTS: u64 = 1;
    }

    /// What changed in a file (`alaya_store::what`).
    pub mod what {
        pub const CREATED: u64 = 1;
        pub const WROTE: u64 = 2;
        pub const DELETED: u64 = 4;
        pub const RENAMED: u64 = 8;
        pub const MADE: u64 = 16;
        pub const FORGOTTEN: u64 = 32;
        pub const RESTORED: u64 = 64;
        pub const KEPT: u64 = 128;
    }

    /// Status words.
    pub mod st {
        pub const OK: u64 = 0;
        pub const NOT_FOUND: u64 = 1;
        pub const EXISTS: u64 = 2;
        pub const FULL: u64 = 3;
        pub const LOCKED: u64 = 4;
        pub const WRONG_KEY: u64 = 5;
        pub const DAMAGED: u64 = 6;
        pub const BAD_REQUEST: u64 = 7;
        pub const IO: u64 = 8;
        pub const OTHER: u64 = 9;
        pub const NO_SUCH_MOMENT: u64 = 10;
        /// An answer grant asked all it may today.
        pub const USED_UP: u64 = 11;
    }

    pub const fn badge(person: u64, app: u64, channel: u64) -> u64 {
        person << 48 | app << 8 | channel
    }

    /// The machine's own files (the people who can log in, what the login
    /// screen looked like): a person no app ever gets a badge for.
    pub const MACHINE_PERSON: u64 = 0xfff;
    /// People numbers for those who log in: 1 to [`MAX_PEOPLE`].
    pub const MAX_PEOPLE: u64 = 4;
    /// App numbers the store records changes and reads under.
    pub const DESKTOP_APP: u64 = 1;
    pub const INIT_APP: u64 = 2;
    pub const fn app_id(role: u64) -> u64 {
        0x100 + role
    }
    /// The name people know an app by.
    pub fn app_label(app: u64) -> &'static str {
        use super::desktop::role;
        match app {
            a if a == app_id(role::EDITOR) => "the text editor",
            a if a == app_id(role::FILES) => "the file browser",
            a if a == app_id(role::CHART) => "the chart maker",
            a if a == app_id(role::SETTINGS) => "Settings",
            a if a == app_id(role::BACK) => "the timeline",
            a if a == app_id(role::HELPER) => "the tidy-up helper",
            a if a == app_id(role::HOSTILE) => "the hostile test helper",
            DESKTOP_APP => "the desktop",
            INIT_APP => "Alaya's checks",
            _ => "another app",
        }
    }
    pub const fn app_channel(role: u64) -> u64 {
        1 + role
    }
    /// Classes, as the store numbers them.
    pub const DOCS: u64 = 0;
    pub const APPS: u64 = 1;
    pub const CACHES: u64 = 2;

    /// Transfer channels and their size. The first program's checks use
    /// channels 0 to 3 (milestone 5); after them, channel 0 is the first
    /// program's, 1 the desktop's and 2 on each app's ([`app_channel`]).
    pub const CHANNELS: u64 = 13;
    pub const XFER_PAGES: u64 = 16;
    pub const XFER_BYTES: usize = (XFER_PAGES * 4096) as usize;
    /// Where the transfer pages are, in the store server.
    pub const XFER: u64 = 0x6200_0000_0000;
    /// Where the keeper's buffer pages are, in the store server.
    pub const BUF: u64 = 0x6300_0000_0000;

    /// Args: flags, and the most blocks to format (0: the whole area).
    pub const ARG_FLAGS: usize = 0;
    pub const ARG_BLOCKS: usize = 1;
    /// Format the disk even if a store is there.
    pub const FLAG_FORMAT: u64 = 1;
    /// Given: its endpoint, the keeper (store badge).
    pub const G_ENDPOINT: u64 = 0;
    pub const G_KEEPER: u64 = 1;
    /// A notification the store signals when it stops dead (DIE_AFTER).
    pub const G_DYING: u64 = 2;
    /// The backup stick's keeper (store badge), with [`FLAG_STICK`].
    pub const G_STICK: u64 = 3;
    /// There is a backup stick.
    pub const FLAG_STICK: u64 = 2;
    /// Where the stick keeper's buffer pages are, in the store server.
    pub const STICK_BUF: u64 = BUF + 0x10_0000;

    /// Save by itself this often (milestone 5, choice A).
    /// Caches nobody used for this many days are emptied (choice D).
    pub const CACHE_UNUSED_DAYS: u64 = 30;
    pub const AUTOSAVE_MS: u64 = 5000;
}

/// The screen, keyboard and mouse (milestone 6): what their drivers share
/// with the program that uses them (for now the first program; the
/// compositor next).
///
/// Each driver gets one shared page at [`driver::JOB_PAGE`] and nothing
/// else of its client. A driver that crashes is started again with the
/// same pages: the screen driver shows the picture still in the screen
/// memory, the input drivers carry on writing events after the last one.
pub mod seat {
    use core::sync::atomic::AtomicU64;

    /// PCI device numbers (virtio, modern).
    pub const VIRTIO_VENDOR: u16 = 0x1af4;
    pub const GPU_DEVICE: u16 = 0x1050;
    pub const INPUT_DEVICE: u16 = 0x1052;

    /// Given capabilities: its notification, the client's, the interrupt.
    pub const G_NTFN: u64 = 0;
    pub const G_CLIENT: u64 = 1;
    pub const G_IRQ: u64 = 2;

    /// Bits on a driver's notification.
    pub const IRQ_BIT: u64 = 1;
    /// The client posted a frame.
    pub const FRAME_BIT: u64 = 2;
    /// Test only: crash now.
    pub const CRASH_BIT: u64 = 4;

    /// Test only: look at the screens again (a pretend screen was
    /// plugged in or out).
    pub const PLUG_BIT: u64 = 8;

    /// Bits on the client's notification.
    pub const SHOWN_BIT: u64 = 1;
    pub const INPUT_BIT: u64 = 2;
    /// The screens changed: read them again from the shared page, and
    /// draw everything.
    pub const SCREENS_BIT: u64 = 4;

    /// At most this many screens at once (milestone 7b).
    pub const MAX_SCREENS: usize = 4;

    /// The screen memory: in the driver, and as the device sees it.
    pub const SCREEN: u64 = 0x5200_0000_0000;
    pub const IOVA_SCREEN: u64 = 0x4000_0000;
    /// Screen memory comes in 2 MiB pages; at most this many (8 MiB: two
    /// screens of 1280 by 800, or one of 1920 by 1080).
    pub const SCREEN_PAGES: u64 = 4;
    /// Args after the device page codes (same places as [`super::driver`]).
    pub const ARG_SCREEN_PAGES: usize = 15;

    /// The screen driver's shared page.
    #[repr(C)]
    pub struct ScreenPage {
        /// [`super::run`] state, and why it failed.
        pub state: AtomicU64,
        pub error: AtomicU64,
        /// How many times a driver started on this page.
        pub starts: AtomicU64,
        /// The screen's size in pixels, from the device. Pixels are 32-bit
        /// words 0x00RRGGBB, row after row.
        pub width: AtomicU64,
        pub height: AtomicU64,
        /// Frames the client posted, and the last one on the screen.
        pub posted: AtomicU64,
        pub shown: AtomicU64,
        /// The part of the screen that changed: x, y, width, height (the
        /// union of everything posted since the last frame shown).
        pub dirty: [AtomicU64; 4],
        /// Time the last frame took in the driver, in microseconds.
        pub flush_us: AtomicU64,
        /// The screens shown (milestone 7b), side by side in the picture:
        /// how many, then for each: x, y, width, height in the picture,
        /// and the output number on the card. `width` and `height` above
        /// are the whole picture's.
        pub screens: AtomicU64,
        pub rects: [[AtomicU64; 5]; MAX_SCREENS],
        /// Test only: outputs to show as if a screen were plugged into
        /// them (bit per output), and their size (width << 32 | height).
        pub pretend: AtomicU64,
        pub pretend_size: AtomicU64,
        /// Changes of screens so far (the client compares).
        pub layout: AtomicU64,
    }

    /// Kinds of input device, as the driver finds out from its name.
    pub mod kind {
        pub const UNKNOWN: u64 = 0;
        pub const KEYBOARD: u64 = 1;
        pub const POINTER: u64 = 2;
    }

    /// Event types and codes (the same numbers Linux uses, which is what
    /// virtio input sends).
    pub mod ev {
        pub const SYN: u16 = 0;
        pub const KEY: u16 = 1;
        pub const REL: u16 = 2;
        pub const ABS: u16 = 3;
        pub const ABS_X: u16 = 0;
        pub const ABS_Y: u16 = 1;
        pub const BTN_LEFT: u16 = 0x110;
        pub const BTN_RIGHT: u16 = 0x111;
        /// Pointer positions run from 0 to this.
        pub const ABS_MAX: u32 = 0x7fff;
    }

    pub const EVENTS: usize = 500;

    /// An input driver's shared page: a ring of events the driver writes
    /// and the client reads at its own pace.
    #[repr(C)]
    pub struct InputPage {
        pub state: AtomicU64,
        pub error: AtomicU64,
        pub starts: AtomicU64,
        pub kind: AtomicU64,
        /// Events written so far; event `n` is at `n % EVENTS`.
        pub head: AtomicU64,
        _pad: [u64; 3],
        /// type << 48 | code << 32 | value.
        pub events: [AtomicU64; EVENTS],
    }

    pub const fn pack(kind: u16, code: u16, value: u32) -> u64 {
        (kind as u64) << 48 | (code as u64) << 32 | value as u64
    }

    pub const fn unpack(e: u64) -> (u16, u16, u32) {
        ((e >> 48) as u16, (e >> 32) as u16, e as u32)
    }

    const _: () = assert!(core::mem::size_of::<ScreenPage>() <= 4096);
    const _: () = assert!(core::mem::size_of::<InputPage>() <= 4096);
}

/// The compositor ("desktop") and the apps that open windows on it
/// (milestone 6).
///
/// An app calls the desktop's endpoint (badged with the app's number).
/// Each window's picture is two 2 MiB pages the desktop owns and lends to
/// the app: the app draws into them, then says which part changed. The
/// picture always has room for the whole screen; the window's current
/// size says how much of it is shown. Events for the app wait in the
/// desktop; it signals the app's notification and the app collects them.
pub mod desktop {
    /// OPEN: content width, height, the app's notification (sent along)
    /// → status, window number.
    pub const OPEN: u64 = 1;
    /// PAGE: window, page index → status, with the page.
    pub const PAGE: u64 = 2;
    /// TITLE: window, length, then up to 32 bytes in four words.
    pub const TITLE: u64 = 3;
    /// DAMAGE: window, x, y, width, height (in the content).
    pub const DAMAGE: u64 = 4;
    /// EVENT: window → status, kind, then the event's words.
    pub const EVENT: u64 = 5;
    /// CLOSE: window.
    pub const CLOSE: u64 = 6;
    /// SIZE: window → status, content width, height.
    pub const SIZE: u64 = 7;
    /// TILING: on (1) or off (0). Test and keyboard use.
    pub const TILING: u64 = 8;
    /// STATE: → windows, focused window, tiling, frames drawn, whether a
    /// question is asked (1) and what the strip says now (its length,
    /// << 1; milestone 7), whether the Open or Save window is up
    /// (1 << 62), and the search box: 0 when it is not up, else 1 + how
    /// many results it shows (<< 52, 9 bits).
    pub const STATE: u64 = 9;
    /// WINDOW: window → status, x, y, width, height, place.
    pub const WINDOW: u64 = 10;
    /// LOCK: show the login screen (the first program only).
    pub const LOCK: u64 = 11;
    /// INJECT: kind, code, value: an input event as if from a device
    /// (the first program only, for runs without a host typing).
    pub const INJECT: u64 = 12;
    /// FORGET: app: close every window of an app that was stopped (the
    /// first program only).
    pub const FORGET: u64 = 13;
    /// SESSION: → status, stage ([`stage`]), person, look, wrong PINs,
    /// login parts ([`parts`]) (the first program only).
    pub const SESSION: u64 = 14;
    /// LOOK_CHANGED: an app (Settings) changed the person's look: read it.
    pub const LOOK_CHANGED: u64 = 15;
    /// GROUP: window → status, group (0: none) (the first program only).
    pub const GROUP: u64 = 16;
    /// PLACES: what → status, x, y, width, height of a part of the screen:
    /// [`places::DOCK`] + i, [`places::CHIP`] + i (the first program only).
    pub const PLACES: u64 = 17;
    /// APP: window → status, the app (its role) (the first program only).
    pub const APP: u64 = 18;
    /// RAISE: window: bring it to the front (the first program only).
    pub const RAISE: u64 = 19;
    /// WINDOW_OF: app (its role) → status, its newest window (the first
    /// program only).
    pub const WINDOW_OF: u64 = 20;
    /// UNDO: how ([`undo_by`]), action or commit, app. The desktop undoes
    /// it as the one undo does: it asks first when something else changed
    /// the same thing since, and puts windows and the look back. Only
    /// "What happened?" (and the first program) may ask.
    pub const UNDO: u64 = 21;
    /// PICK: window, save (1) or open (0), write, how long it suggests
    /// (a `Lasts` kind; 0: let the person choose) → status. Alaya shows
    /// the Open or Save window; the answer comes later as a PICKED event
    /// (milestone 7, step 2). Save 2 is the answer window (milestone 8,
    /// step 3): the third word is the question (`files::question`), and
    /// the app gets an answer grant on the document picked, not the
    /// document.
    pub const PICK: u64 = 22;
    /// JOB: window, what (0: ask for a job, 1: the job is done) → status.
    /// Asking shows the rules screen: the person picks the one folder it
    /// may change and sees the rules before it starts; the app hears back
    /// as a JOB event. Done shows what Alaya's checks found, and the
    /// person keeps the work, throws it away, or keeps it anyway.
    pub const JOB: u64 = 23;
    /// RUN_AGAIN: app, commit → status, files, how many exact. "Run it
    /// again": the app opens again (a running copy is closed first) on
    /// exactly what it had read by that commit, read only. "What
    /// happened?" (and the first program) may ask.
    pub const RUN_AGAIN: u64 = 24;
    /// MAKE_AGAIN: a result → status. "Make again" (living validity): the
    /// app that made it makes it again from its sources as they are now
    /// (it gets a MAKE_AGAIN event; started first when it is not
    /// running). The file browser (and the first program) may ask.
    pub const MAKE_AGAIN: u64 = 25;
    /// GROUP_SCREEN: group (0: the focused window's) → status: show a
    /// group's screen (the first program only; people use Alaya key + G
    /// or a right click on its label).
    pub const GROUP_SCREEN: u64 = 26;
    /// GROUP_FILE: file → status: ask the person which group to put it in
    /// (the file browser, and the first program).
    pub const GROUP_FILE: u64 = 27;
    /// GROUP_GO: group (0: the window in front's) → status, how many held
    /// things came forward: as if the person clicked its label (the
    /// first program only). Leaves the group they were in; coming back
    /// to one they left shows "since you left". What the group's apps
    /// said meanwhile comes forward (milestone 8, step 3).
    pub const GROUP_GO: u64 = 28;
    /// BRIEF_LINE: i → status, kind (1 changed, 2 needs a look), file,
    /// moment, app, what: line i of the briefing up now, and its receipt
    /// (the first program only: "nothing invented" is checked).
    pub const BRIEF_LINE: u64 = 29;
    /// NOTIFY: kind wished ([`inbox`]) | text length << 8, then the text
    /// (32 bytes in four words): something for the person. An app's wish
    /// is never trusted: "now" becomes "about what you're doing" or "can
    /// wait", unless the person's own rule lets the app through.
    pub const NOTIFY: u64 = 30;
    /// NOW: the same from Alaya itself (the first program only): a
    /// channel no app can use, whose "now" always shows at once.
    pub const NOW: u64 = 31;
    /// INBOX: → status, "now" things shown at once, then how many of each
    /// kind wait (now, about, can wait, just so you know) (the first
    /// program only).
    pub const INBOX: u64 = 32;
    /// SCALE: window, percent → status, the scale now, what was chosen:
    /// how big things are drawn on the screen that window is on (0:
    /// Alaya picks from the screen's size; `u64::MAX`: only ask). Every
    /// window there keeps its size in points and draws again.
    pub const SCALE: u64 = 33;
    /// The scales a person can choose, in percent (0: Alaya picks).
    pub const SCALES: [u64; 6] = [0, 100, 125, 150, 175, 200];
    /// SEEING: window, bits ([`seeing`]) or `u64::MAX` to only ask →
    /// status, the bits now: high contrast and the screen reader, for
    /// the person (kept with their app files). Every window hears of a
    /// change as a resize of the same size, and draws again.
    pub const SEEING: u64 = 34;
    pub mod seeing {
        /// Strong colours: black ground, white text, yellow focus.
        pub const CONTRAST: u64 = 1;
        /// The screen reader: says what has the keyboard.
        pub const READER: u64 = 2;
    }
    /// READER: → status, how many times it spoke, the role of what has
    /// the keyboard (`reader_role`), how many of the person's files the
    /// window's app can see, and the first 16 bytes of what it said
    /// (the first program only, for checks).
    pub const READER: u64 = 35;
    /// TYPING: window, layout (`alaya_ui::input::Layout` code) or
    /// `u64::MAX` to only ask → status, the layout, the input mode
    /// (Latin, Japanese, Chinese). Kept with the person's app files.
    pub const TYPING: u64 = 36;
    /// TYPED: → status, how many characters the input method handed to
    /// apps, then the last 32 bytes of them (the first program only, for
    /// checks).
    pub const TYPED: u64 = 37;
    /// OFFER_INSTALL: a file of the person's (milestone 8) → status. The
    /// desktop reads it, checks it is a signed app bundle and shows its
    /// install sheet; only the person can go on from there. The file
    /// browser (Enter on the file) and the first program may ask.
    pub const OFFER_INSTALL: u64 = 38;
    /// INSTALLED: place → status, state | what it may << 8 | version <<
    /// 32, the app's number, the install sheet's stage (0 none, 1 the
    /// promise, 2 on the side, 3 installed, 4 a message or receipt), the
    /// last removal's role | what was left << 16, and what was taken back
    /// | what it made << 32 (the first program only, for checks).
    pub const INSTALLED: u64 = 39;
    /// STARTING: role, version → status. The first program is starting an
    /// installed app now; the desktop times how long until its window
    /// opens (the Experience Guardian, milestone 8 step 2). The first
    /// program only.
    pub const STARTING: u64 = 40;
    /// TRIAL: place → status, starts left on trial | what it tried beyond
    /// what it may << 8 | the version the Experience Guardian went back
    /// from << 16 | new lines not asked about yet << 32 | flags << 48, the usual start time in ms, the last
    /// start time in ms, and the version kept for undo (the first program
    /// only, for checks).
    pub const TRIAL: u64 = 41;
    /// BACKGROUND: → status. An app asks to keep running when its last
    /// window closes (milestone 8). Alaya's own apps may; an installed
    /// app only with the promise line "Keep running with no window open"
    /// left ticked. Without it, an installed app whose last window closes
    /// is ended: no processor time with no window.
    pub const BACKGROUND: u64 = 42;
    /// GAME: → status, on, the roles frozen for it (bit r), install
    /// offers waiting. Game mode (milestone 8, step 3): while a
    /// full-screen window is in front, installed apps with no window are
    /// frozen, the store puts off its backups, and install offers wait.
    /// The first program only, for checks.
    pub const GAME: u64 = 43;
    /// HANDOFF: window, to role, file, place (a line, from 0) → status.
    /// Live handoff (milestone 8, step 3): an app hands the document it
    /// holds to another app, which goes on at the same place. Alaya asks
    /// the person first; the other app gets the document until it closes
    /// and a HANDOFF event. From the first program: → status, from, to,
    /// file, place, write of the last handoff done.
    pub const HANDOFF: u64 = 44;
    pub mod reader_role {
        pub const WINDOW: u64 = 0;
        pub const LABEL: u64 = 1;
        pub const BUTTON: u64 = 2;
        pub const TEXT_BOX: u64 = 3;
        pub const LIST: u64 = 4;
        pub const LIST_ITEM: u64 = 5;
        pub const GROUP: u64 = 6;
        /// Something of Alaya's own: the strip, the dock, a group label.
        pub const SHELL: u64 = 7;
    }
    /// The decision inbox's kinds.
    pub mod inbox {
        pub const NOW: u64 = 0;
        pub const ABOUT: u64 = 1;
        pub const WAIT: u64 = 2;
        pub const KNOW: u64 = 3;
    }
    /// Session notification bits the desktop signals: stop (freeze) the
    /// app with role r, `1 << (PAUSE_SHIFT + r)`, or start it again,
    /// `1 << (GO_ON_SHIFT + r)` (a paused task, rule B3).
    /// (Sixteen bits each: roles up to [`role::LAST`].)
    pub const PAUSE_SHIFT: u64 = 16;
    pub const GO_ON_SHIFT: u64 = 48;
    /// Bit `END_SHIFT + r` on the first program's session notification:
    /// end the running copy of the app of role r for good (an installed
    /// app removed, taken away, gone back from or done rehearsing). Its
    /// windows alone closing is not enough: an app could ignore that.
    pub const END_SHIFT: u64 = 33;
    pub mod job_event {
        /// The person approved the rules: work in the folder.
        pub const STARTED: u64 = 1;
        /// The person said no.
        pub const REFUSED: u64 = 2;
        /// The work was kept.
        pub const KEPT: u64 = 3;
        /// The work was thrown away.
        pub const THROWN: u64 = 4;
    }
    pub mod undo_by {
        /// The last action (as Alaya key + Z).
        pub const LAST: u64 = 0;
        /// One action on the list, by its number.
        pub const ACTION: u64 = 1;
        /// What one app changed in one commit.
        pub const CHANGE: u64 = 2;
    }

    pub mod places {
        pub const DOCK: u64 = 0;
        pub const CHIP: u64 = 16;
        /// The strip's buttons: Undo, What happened?
        pub const UNDO: u64 = 32;
        pub const HAPPENED: u64 = 33;
        /// The question's buttons, while one is asked.
        pub const YES: u64 = 40;
        pub const NO: u64 = 41;
        /// The on-screen keyboard's key with this key code, while shown.
        pub const KEY: u64 = 0x100;
        /// A cell of the emoji picker, while shown.
        pub const EMOJI: u64 = 0x200;
    }

    /// What the desktop shows.
    pub mod stage {
        pub const SETUP: u64 = 0;
        pub const LOGIN: u64 = 1;
        pub const OPEN: u64 = 2;
    }

    /// What the check of the login parts found (decision 74).
    pub mod parts {
        /// First login: nothing to compare with yet.
        pub const FIRST: u64 = 0;
        pub const SAME: u64 = 1;
        pub const CHANGED: u64 = 2;
    }

    /// The apps of milestone 6, by role. An app's badge on the desktop
    /// endpoint is its role.
    pub mod role {
        pub const EDITOR: u64 = 1;
        pub const FILES: u64 = 2;
        pub const CHART: u64 = 3;
        pub const SETTINGS: u64 = 4;
        /// "What happened?" and the timeline (milestone 7).
        pub const BACK: u64 = 5;
        /// The tidy-up helper (milestone 7, step 3): works only through a
        /// job with rules.
        pub const HELPER: u64 = 6;
        /// A deliberately hostile helper, for the attack checks: it tries
        /// every way around a job's rules. Never in the dock.
        pub const HOSTILE: u64 = 7;
        pub const COUNT: u64 = 7;
        /// Installed apps (milestone 8): one role per place, from
        /// `INSTALLED` to [`LAST`], for the person logged in.
        pub const INSTALLED: u64 = 8;
        pub const INSTALLED_MAX: u64 = 4;
        pub const LAST: u64 = INSTALLED + INSTALLED_MAX - 1;
        /// Whether role `r` is an installed app's place.
        pub const fn installed(r: u64) -> bool {
            r >= INSTALLED && r <= LAST
        }
    }

    pub const OK: u64 = 0;
    pub const NO_WINDOW: u64 = 1;
    pub const NO_MEMORY: u64 = 2;
    pub const EMPTY: u64 = 3;
    pub const BAD_REQUEST: u64 = 4;
    /// Its promise does not say so, or the person unticked the line.
    pub const NOT_ALLOWED: u64 = 5;

    /// Event kinds.
    pub mod event {
        /// Key: code, pressed (1) / released (0) / repeat (2), shift held.
        pub const KEY: u64 = 1;
        /// Pointer: x, y, button (0: none), down, moved.
        pub const POINTER: u64 = 2;
        /// The content has a new size: width, height.
        pub const RESIZED: u64 = 3;
        /// The close button was pressed.
        pub const CLOSE: u64 = 4;
        /// The window got (1) or lost (0) the keyboard.
        pub const FOCUS: u64 = 5;
        /// The Open or Save window closed: the file handed over (0:
        /// nothing), save, write, how long (a `Lasts` kind).
        pub const PICKED: u64 = 6;
        /// About its job: what ([`super::job_event`]), the folder.
        pub const JOB: u64 = 7;
        /// Make this result again from its sources as they are now.
        pub const MAKE_AGAIN: u64 = 8;
        /// Show this moment (a commit) on the timeline: a receipt of
        /// "since you left" (milestone 7a).
        pub const MOMENT: u64 = 9;
        /// Typed text, from the input method (milestone 7b): a character.
        /// Apps get text only this way; the keys that make it stay with
        /// the desktop.
        pub const TEXT: u64 = 10;
        /// Go on with this document at this place (live handoff): the
        /// file, the place (a line, from 0), the role it came from, write.
        pub const HANDOFF: u64 = 11;
    }

    /// Places, as WINDOW reports them.
    pub mod place {
        pub const FREE: u64 = 0;
        pub const LEFT: u64 = 1;
        pub const RIGHT: u64 = 2;
        pub const FULL: u64 = 3;
        pub const TILED: u64 = 4;
    }

    /// Installed apps (milestone 8): what the desktop keeps in the
    /// machine's app files, where no app can reach. The file [`FILE`]
    /// holds one record of [`RECORD`] bytes per person and place; each
    /// app's bundle is in its own file ([`bundle_file`]).
    ///
    /// A record: state (byte 0), what the person left ticked (bytes 4 to
    /// 8), the version (8 to 12), the app's lasting number (16 to 24) and
    /// its name (24 to 56, padded with zeros).
    pub mod installed {
        pub const FILE: &str = "installed";
        /// A place's record: state (byte 0), what it may (4), version
        /// (8), promise (12), the app's number (16), name (24 to 56),
        /// starts left on trial (56), flags (57), the usual start time in
        /// ms (60); the version kept for undo: what it may (64), version
        /// (68), promise (72), flags (76); what it tried beyond what it
        /// may during its trial (80); the version the Experience Guardian
        /// went back from (84); the last start time in ms (88).
        pub const RECORD: usize = 128;
        /// How many starts a new version is on trial for.
        pub const TRIAL_STARTS: u8 = 3;
        /// Nothing in this place.
        pub const EMPTY: u8 = 0;
        /// Installed on the side: kept, not yet approved.
        pub const ASIDE: u8 = 1;
        /// Approved: in the dock, and it may run.
        pub const ON: u8 = 2;
        /// Running in a rehearsal (milestone 8): on a side copy of the
        /// person's documents, thrown away after; not in the dock.
        pub const REHEARSAL: u8 = 3;
        /// Where the record of `person`'s place `slot` starts.
        pub const fn at(person: u64, slot: u64) -> usize {
            (((person - 1) * super::role::INSTALLED_MAX + slot) as usize) * RECORD
        }
        pub const BYTES: usize = at(super::super::files::MAX_PEOPLE + 1, 0);
        /// The name of the file holding a place's bundle.
        pub fn bundle_file(person: u64, slot: u64, out: &mut [u8; 8]) -> &str {
            *out = *b"app-0-0\0";
            out[4] = b'0' + person as u8;
            out[6] = b'0' + slot as u8;
            core::str::from_utf8(&out[..7]).unwrap()
        }
        /// The file holding the version kept for undo (the one before
        /// an update, or after it, once undone).
        pub fn kept_file(person: u64, slot: u64, out: &mut [u8; 8]) -> &str {
            bundle_file(person, slot, out);
            out[..3].copy_from_slice(b"old");
            core::str::from_utf8(&out[..7]).unwrap()
        }
    }

    /// Bit on an app's notification: events are waiting.
    pub const EVENT_BIT: u64 = 1;
    /// Bits on the first program's session notification: start the app
    /// of role r (bit `1 << r`); the person logged in.
    pub const LOGGED_IN_BIT: u64 = 1 << 32;

    /// Pages per window picture, and the picture's row length in pixels.
    pub const PAGES: u64 = 2;
    pub const STRIDE: u64 = 1280;
    pub const MAX_W: u64 = 1280;
    pub const MAX_H: u64 = 800;
    /// Where a window's accessibility tree is, in its picture pages,
    /// after the pixels: a change count (u32), the length (u32), then
    /// the tree (`alaya_ui::widgets::encode_tree`). The screen reader
    /// reads it there; nothing else does.
    pub const TREE_AT: u64 = STRIDE * MAX_H * 4;
    pub const TREE_MAX: u64 = 16 << 10;

    /// Where the desktop finds what the first program shares with it. All
    /// of it, and the window pictures, sit within one 512 GiB part of the
    /// address space, so few page tables are needed (a program's pool of
    /// page tables is small).
    pub const SCREEN_PAGE: u64 = 0x5300_0000_0000;
    pub const INPUT_PAGES: u64 = 0x5300_0001_0000;
    pub const SCREEN: u64 = 0x5300_0040_0000;
    pub const FONTS: u64 = 0x5300_0100_0000;
    /// Window pictures: in the desktop, one after another; in an app, its
    /// window's.
    pub const PICTURES: u64 = 0x5300_4000_0000;
    pub const PICTURE_STEP: u64 = 0x80_0000;
    /// Each font gets this much address space.
    pub const FONT_STEP: u64 = 0x20_0000;

    /// The store's transfer pages of the program's channel (desktop and
    /// apps).
    pub const XFER: u64 = 0x5300_0002_0000;

    /// Given capabilities: its endpoint, the screen driver's notification,
    /// the notification the drivers signal, the first program's session
    /// notification, the store as the machine, then the store as each
    /// person 1 to `MAX_PEOPLE`.
    pub const G_EP: u64 = 0;
    pub const G_SCREEN: u64 = 1;
    pub const G_SEAT: u64 = 2;
    pub const G_SESSION: u64 = 3;
    pub const G_STORE_MACHINE: u64 = 4;
    pub const G_STORE_PERSON: u64 = 5;
    /// An untyped block the window pictures are made from (milestone 8).
    pub const G_PICTURES: u64 = G_STORE_PERSON + super::files::MAX_PEOPLE;

    /// Args: screen pages, input pages, then the four font sizes (text,
    /// strong, mono, fallback), the fingerprint of the login parts (4
    /// words), the key of the machine's own files (4 words), and the
    /// memory the PIN step uses (in 32-byte entries).
    pub const ARG_SCREEN_PAGES: usize = 0;
    pub const ARG_INPUTS: usize = 1;
    pub const ARG_FONTS: usize = 2;
    pub const ARG_PARTS: usize = 6;
    pub const ARG_MACHINE_KEY: usize = 10;
    pub const ARG_PIN_WORK: usize = 14;

    /// Apps: given its desktop endpoint, its notification and the store
    /// (as the person logged in); fonts at [`FONTS`] like the desktop,
    /// sizes from [`ARG_FONTS`] on; its role at [`ARG_ROLE`] and its
    /// store channel at [`ARG_CHANNEL`].
    pub const APP_G_DESKTOP: u64 = 0;
    pub const APP_G_NTFN: u64 = 1;
    pub const APP_G_STORE: u64 = 2;
    pub const ARG_ROLE: usize = 0;
    pub const ARG_CHANNEL: usize = 1;
    /// An installed app's version, from its bundle, and what it may
    /// (promise bits).
    pub const ARG_VERSION: usize = 6;
    pub const ARG_MAY: usize = 7;

    /// Key codes (the numbers virtio input sends) for typed text on a US
    /// keyboard: each character of `spec` as (code, modifiers: 1 Shift,
    /// 2 Ctrl), and `{name}` for a key without a character (tab, enter,
    /// left, right, up, down, backspace, escape) or `{ctrl+x}`. Used by
    /// the tests to type, here and on the host.
    pub fn keys(spec: &str, mut f: impl FnMut(u16, u8)) {
        let mut rest = spec;
        while let Some(c) = rest.chars().next() {
            if c == '{'
                && let Some(end) = rest.find('}')
            {
                let name = &rest[1..end];
                let named = |n: &str| -> Option<(u16, bool)> {
                    let c = match n {
                        "tab" => 15,
                        "enter" => 28,
                        "left" => 105,
                        "right" => 106,
                        "up" => 103,
                        "down" => 108,
                        "backspace" => 14,
                        "escape" => 1,
                        "home" => 102,
                        "end" => 107,
                        "delete" => 111,
                        "space" => 57,
                        "compose" => 127,
                        _ => {
                            let mut cs = n.chars();
                            return match (cs.next(), cs.next()) {
                                (Some(c), None) => key_code(c),
                                _ => None,
                            };
                        }
                    };
                    Some((c, false))
                };
                // `{super+x}` is the Alaya key (the Windows-logo key) with
                // a key; `{ctrl+x}` likewise; a key may be named (`up`).
                // Modifiers may chain: `{super+shift+right}`.
                let mut k = name;
                let mut mods = 0u8;
                loop {
                    if let Some(r) = k.strip_prefix("super+") {
                        (k, mods) = (r, mods | 4);
                    } else if let Some(r) = k.strip_prefix("ctrl+") {
                        (k, mods) = (r, mods | 2);
                    } else if let Some(r) = k.strip_prefix("shift+") {
                        (k, mods) = (r, mods | 1);
                    } else {
                        break;
                    }
                }
                let code = if mods != 0 {
                    named(k).map(|(c, s)| (c, mods | u8::from(s)))
                } else {
                    named(name).filter(|_| name.len() > 1).map(|(c, _)| (c, 0))
                };
                if let Some((c, m)) = code {
                    f(c, m);
                }
                rest = &rest[end + 1..];
                continue;
            }
            if let Some((k, s)) = key_code(c) {
                f(k, u8::from(s));
            }
            rest = &rest[c.len_utf8()..];
        }
    }

    /// The key code and Shift for a character on a US keyboard.
    pub fn key_code(c: char) -> Option<(u16, bool)> {
        const LOWER: &[u8] = b"1234567890-=qwertyuiop[]asdfghjkl;'`\\zxcvbnm,./";
        const UPPER: &[u8] = b"!@#$%^&*()_+QWERTYUIOP{}ASDFGHJKL:\"~|ZXCVBNM<>?";
        const CODES: [u16; 47] = [
            2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27,
            30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52,
            53,
        ];
        if c == ' ' {
            return Some((57, false));
        }
        if c == '\n' {
            return Some((28, false));
        }
        let b = u8::try_from(c).ok()?;
        if let Some(i) = LOWER.iter().position(|&x| x == b) {
            return Some((CODES[i], false));
        }
        UPPER.iter().position(|&x| x == b).map(|i| (CODES[i], true))
    }
}

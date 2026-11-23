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
    /// STAT: file → size, version, is_dir.
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
    }

    pub const fn badge(person: u64, app: u64, channel: u64) -> u64 {
        person << 48 | app << 8 | channel
    }

    /// Transfer channels and their size.
    pub const CHANNELS: u64 = 4;
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

    /// Bits on the client's notification.
    pub const SHOWN_BIT: u64 = 1;
    pub const INPUT_BIT: u64 = 2;

    /// The screen memory: in the driver, and as the device sees it.
    pub const SCREEN: u64 = 0x5200_0000_0000;
    pub const IOVA_SCREEN: u64 = 0x4000_0000;
    /// Screen memory comes in 2 MiB pages; at most this many (8 MiB, for
    /// 1920 by 1080).
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
    /// STATE: → windows, focused window, tiling, frames drawn.
    pub const STATE: u64 = 9;
    /// WINDOW: window → status, x, y, width, height, place.
    pub const WINDOW: u64 = 10;

    pub const OK: u64 = 0;
    pub const NO_WINDOW: u64 = 1;
    pub const NO_MEMORY: u64 = 2;
    pub const EMPTY: u64 = 3;
    pub const BAD_REQUEST: u64 = 4;

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
    }

    /// Places, as WINDOW reports them.
    pub mod place {
        pub const FREE: u64 = 0;
        pub const LEFT: u64 = 1;
        pub const RIGHT: u64 = 2;
        pub const FULL: u64 = 3;
        pub const TILED: u64 = 4;
    }

    /// Bit on an app's notification: events are waiting.
    pub const EVENT_BIT: u64 = 1;

    /// Pages per window picture, and the picture's row length in pixels.
    pub const PAGES: u64 = 2;
    pub const STRIDE: u64 = 1280;
    pub const MAX_W: u64 = 1280;
    pub const MAX_H: u64 = 800;

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
    pub const FONT_STEP: u64 = 0x10_0000;

    /// Given capabilities: its endpoint, the screen driver's notification,
    /// the notification the drivers signal.
    pub const G_EP: u64 = 0;
    pub const G_SCREEN: u64 = 1;
    pub const G_SEAT: u64 = 2;

    /// Args: screen pages, input pages, then the four font sizes (text,
    /// strong, mono, fallback).
    pub const ARG_SCREEN_PAGES: usize = 0;
    pub const ARG_INPUTS: usize = 1;
    pub const ARG_FONTS: usize = 2;

    /// Apps: given its desktop endpoint and its notification; fonts at
    /// [`FONTS`] like the desktop, sizes from [`ARG_FONTS`] on.
    pub const APP_G_DESKTOP: u64 = 0;
    pub const APP_G_NTFN: u64 = 1;
}

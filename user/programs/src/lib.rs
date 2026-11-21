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
    pub const DATA_PAGES: u64 = (JOBS * super::SECTOR / 4096) as u64;

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
    _pad: [u64; 3],
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
}

pub const HANDOVER_MAGIC: u64 = 0x4841_4e44_4f56_4552;
pub const HANDOVER_FORMAT: u64 = 1;

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

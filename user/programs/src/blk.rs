// SPDX-License-Identifier: GPL-3.0-only
//! The virtual disk driver (virtio block device, modern PCI interface).
//!
//! It owns no memory the device can reach: the ring, the request headers
//! and the data buffers are the keeper's pages, and the IOMMU lets the
//! device reach only those. So a crash of the driver loses nothing; the
//! next instance resets the device and sends every unfinished job again.
//!
//! Both versions (`blk1`, `blk2`) are this file with another version
//! number. They understand the same handover format, so either can take
//! the device over from the other while it runs.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use alaya_programs::driver::*;
use alaya_programs::{
    HANDOVER_FORMAT, HANDOVER_MAGIC, Handover, JobPage, OP_FLUSH, OP_WRITE, load, run, status,
    store,
};
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::say;

// Device status bits.
const ACKNOWLEDGE: u8 = 1;
const DRIVER: u8 = 2;
const DRIVER_OK: u8 = 4;
const FEATURES_OK: u8 = 8;
const NEEDS_RESET: u8 = 64;

// Feature bits.
const VERSION_1: u64 = 1 << 32;
const ACCESS_PLATFORM: u64 = 1 << 33;
/// The device has a write cache that a flush request empties.
const BLK_FLUSH: u64 = 1 << 9;

// Request types.
const T_IN: u32 = 0;
const T_OUT: u32 = 1;
const T_FLUSH: u32 = 4;

// Common configuration registers.
const C_DEVICE_FEATURE_SELECT: u64 = 0x00;
const C_DEVICE_FEATURE: u64 = 0x04;
const C_DRIVER_FEATURE_SELECT: u64 = 0x08;
const C_DRIVER_FEATURE: u64 = 0x0c;
const C_MSIX_CONFIG: u64 = 0x10;
const C_STATUS: u64 = 0x14;
const C_QUEUE_SELECT: u64 = 0x16;
const C_QUEUE_SIZE: u64 = 0x18;
const C_QUEUE_MSIX: u64 = 0x1a;
const C_QUEUE_ENABLE: u64 = 0x1c;
const C_QUEUE_NOTIFY_OFF: u64 = 0x1e;
const C_QUEUE_DESC: u64 = 0x20;
const C_QUEUE_DRIVER: u64 = 0x28;
const C_QUEUE_DEVICE: u64 = 0x30;

const NO_VECTOR: u16 = 0xffff;

// Descriptor flags.
const D_NEXT: u16 = 1;
const D_WRITE: u16 = 2;

// Ring layout in the ring page (queue size 128).
const AVAIL: u64 = 16 * QUEUE as u64;
const USED: u64 = (AVAIL + 6 + 2 * QUEUE as u64).next_multiple_of(4);
const _: () = assert!(USED + 6 + 8 * QUEUE as u64 <= 4096);
const STATUS_BYTES: u64 = 1024;

fn r8(a: u64) -> u8 {
    // SAFETY: `a` is inside a page this driver mapped.
    unsafe { read_volatile(a as *const u8) }
}
fn r16(a: u64) -> u16 {
    // SAFETY: as above.
    unsafe { read_volatile(a as *const u16) }
}
fn r32(a: u64) -> u32 {
    // SAFETY: as above.
    unsafe { read_volatile(a as *const u32) }
}
fn w8(a: u64, v: u8) {
    // SAFETY: as above.
    unsafe { write_volatile(a as *mut u8, v) }
}
fn w16(a: u64, v: u16) {
    // SAFETY: as above.
    unsafe { write_volatile(a as *mut u16, v) }
}
fn w32(a: u64, v: u32) {
    // SAFETY: as above.
    unsafe { write_volatile(a as *mut u32, v) }
}
fn w64(a: u64, v: u64) {
    w32(a, v as u32);
    w32(a + 4, (v >> 32) as u32);
}

/// Where the device's register blocks are, from its PCI capabilities.
struct Regs {
    common: u64,
    device: u64,
    notify: u64,
    notify_mult: u32,
    msix_cap: u64,
    msix_table: u64,
}

fn find_regs() -> Option<Regs> {
    let cfg = CONFIG_PAGE;
    let mut r = Regs {
        common: 0,
        device: 0,
        notify: 0,
        notify_mult: 0,
        msix_cap: 0,
        msix_table: 0,
    };
    if r16(cfg + 6) & 0x10 == 0 {
        return None;
    }
    let mut at = u64::from(r8(cfg + 0x34) & !3);
    let mut guard = 0;
    while at != 0 && guard < 48 {
        guard += 1;
        let id = r8(cfg + at);
        if id == 0x09 {
            let kind = r8(cfg + at + 3);
            let bar = u64::from(r8(cfg + at + 4));
            let off = u64::from(r32(cfg + at + 8));
            let va = bar_page(bar, 0) + off;
            match kind {
                1 => r.common = va,
                2 => {
                    r.notify = va;
                    r.notify_mult = r32(cfg + at + 16);
                }
                4 => r.device = va,
                _ => {}
            }
        } else if id == 0x11 {
            r.msix_cap = at;
            let t = r32(cfg + at + 4);
            r.msix_table = bar_page(u64::from(t & 7), 0) + u64::from(t & !7);
        }
        at = u64::from(r8(cfg + at + 1) & !3);
    }
    (r.common != 0 && r.notify != 0 && r.device != 0 && r.msix_cap != 0).then_some(r)
}

struct Driver {
    version: u64,
    regs: Regs,
    jobs: &'static JobPage,
    keeper: u64,
    ntfn: u64,
    park: u64,
    msi: (u64, u64),
    /// Where queue 0's notify register is.
    notify: u64,
    avail_idx: u16,
    used_seen: u16,
    submitted: [u64; JOBS],
    sectors: u64,
    /// Jobs this instance finished (a faulty test version counts them).
    finished_here: u64,
    faulty: bool,
    /// The device has a write cache to flush.
    flush: bool,
    clock_hz: u64,
}

/// Why starting the device failed, in the job page's error word.
mod why {
    pub const NO_REGS: u64 = 1;
    pub const NO_RESET: u64 = 2;
    pub const NO_VERSION_1: u64 = 3;
    pub const FEATURES: u64 = 4;
    pub const QUEUE_SIZE: u64 = 5;
    pub const MSIX: u64 = 6;
}

impl Driver {
    fn status(&self) -> u8 {
        r8(self.regs.common + C_STATUS)
    }

    fn set_status(&self, s: u8) {
        w8(self.regs.common + C_STATUS, s);
    }

    /// Reset the device and set it up from nothing.
    fn reset_and_start(&mut self) -> Result<(), u64> {
        let c = self.regs.common;
        self.set_status(0);
        let mut spins = 0;
        while self.status() != 0 {
            spins += 1;
            if spins > 10_000_000 {
                return Err(why::NO_RESET);
            }
            core::hint::spin_loop();
        }
        self.set_status(ACKNOWLEDGE);
        self.set_status(ACKNOWLEDGE | DRIVER);
        w32(c + C_DEVICE_FEATURE_SELECT, 0);
        let lo = u64::from(r32(c + C_DEVICE_FEATURE));
        w32(c + C_DEVICE_FEATURE_SELECT, 1);
        let offered = lo | u64::from(r32(c + C_DEVICE_FEATURE)) << 32;
        if offered & VERSION_1 == 0 {
            return Err(why::NO_VERSION_1);
        }
        let want = offered & (VERSION_1 | ACCESS_PLATFORM | BLK_FLUSH);
        self.flush = want & BLK_FLUSH != 0;
        w32(c + C_DRIVER_FEATURE_SELECT, 0);
        w32(c + C_DRIVER_FEATURE, want as u32);
        w32(c + C_DRIVER_FEATURE_SELECT, 1);
        w32(c + C_DRIVER_FEATURE, (want >> 32) as u32);
        self.set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK);
        if self.status() & FEATURES_OK == 0 {
            return Err(why::FEATURES);
        }

        // The interrupt: MSI-X entry 0, pointed at the vector the kernel
        // gave this device.
        let t = self.regs.msix_table;
        w32(t, self.msi.0 as u32);
        w32(t + 4, (self.msi.0 >> 32) as u32);
        w32(t + 8, self.msi.1 as u32);
        w32(t + 12, 0);
        let mc = CONFIG_PAGE + self.regs.msix_cap + 2;
        w16(mc, (r16(mc) | 0x8000) & !0x4000);
        w16(c + C_MSIX_CONFIG, NO_VECTOR);

        w16(c + C_QUEUE_SELECT, 0);
        let max = r16(c + C_QUEUE_SIZE);
        if max < QUEUE {
            return Err(why::QUEUE_SIZE);
        }
        w16(c + C_QUEUE_SIZE, QUEUE);
        w16(c + C_QUEUE_MSIX, 0);
        if r16(c + C_QUEUE_MSIX) == NO_VECTOR {
            return Err(why::MSIX);
        }
        // SAFETY: the ring page is mapped writable here.
        unsafe { core::ptr::write_bytes(RING_PAGE as *mut u8, 0, 4096) };
        w64(c + C_QUEUE_DESC, IOVA_RING);
        w64(c + C_QUEUE_DRIVER, IOVA_RING + AVAIL);
        w64(c + C_QUEUE_DEVICE, IOVA_RING + USED);
        w16(c + C_QUEUE_ENABLE, 1);
        self.notify = self.regs.notify
            + u64::from(r16(c + C_QUEUE_NOTIFY_OFF)) * u64::from(self.regs.notify_mult);
        self.set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
        self.avail_idx = 0;
        self.used_seen = 0;
        self.submitted = [0; JOBS];
        let d = self.regs.device;
        self.sectors = u64::from(r32(d)) | u64::from(r32(d + 4)) << 32;
        Ok(())
    }

    /// Point descriptor chain `i` at a request and put it in the ring.
    /// A flush has no data; without a write cache it is not sent at all.
    fn queue(&mut self, i: usize, op: u64, sector: u64, count: u64, data: u64) {
        let write = op == OP_WRITE;
        let hdr = HEADER_PAGE + i as u64 * 16;
        let kind = match op {
            OP_WRITE => T_OUT,
            OP_FLUSH => T_FLUSH,
            _ => T_IN,
        };
        w32(hdr, kind);
        w32(hdr + 4, 0);
        w64(hdr + 8, sector);
        w8(HEADER_PAGE + STATUS_BYTES + i as u64, 0xff);
        let d = 3 * i as u64;
        let desc = |n: u64, addr: u64, len: u32, flags: u16, next: u16| {
            let a = RING_PAGE + n * 16;
            w64(a, addr);
            w32(a + 8, len);
            w16(a + 12, flags);
            w16(a + 14, next);
        };
        desc(d, IOVA_HEADER + i as u64 * 16, 16, D_NEXT, (d + 1) as u16);
        if op == OP_FLUSH {
            desc(d + 1, IOVA_HEADER + STATUS_BYTES + i as u64, 1, D_WRITE, 0);
        } else {
            desc(
                d + 1,
                data,
                (count.clamp(1, 8) * alaya_programs::SECTOR as u64) as u32,
                D_NEXT | if write { 0 } else { D_WRITE },
                (d + 2) as u16,
            );
            desc(d + 2, IOVA_HEADER + STATUS_BYTES + i as u64, 1, D_WRITE, 0);
        }
        let slot = u64::from(self.avail_idx % QUEUE);
        w16(RING_PAGE + AVAIL + 4 + 2 * slot, d as u16);
        self.avail_idx = self.avail_idx.wrapping_add(1);
    }

    fn kick(&self) {
        fence(Ordering::SeqCst);
        w16(RING_PAGE + AVAIL + 2, self.avail_idx);
        fence(Ordering::SeqCst);
        w16(self.notify, 0);
    }

    /// Send every waiting job the device does not have yet.
    fn submit(&mut self) {
        let mut any = false;
        for i in 0..JOBS {
            let j = &self.jobs.jobs[i];
            let posted = load(&j.posted);
            if posted != load(&j.done) && self.submitted[i] != posted {
                let (op, sector, count) = (load(&j.op), load(&j.sector), load(&j.count));
                self.submitted[i] = posted;
                if op == OP_FLUSH && !self.flush {
                    // No write cache: every finished write is kept already.
                    store(&j.status, status::OK);
                    store(&j.done, posted);
                    self.jobs.finished.fetch_add(1, Ordering::AcqRel);
                    let _ = alaya_sys::signal(self.keeper, alaya_programs::keeper::DONE_BIT);
                    continue;
                }
                self.queue(i, op, sector, count, IOVA_DATA + i as u64 * 4096);
                any = true;
            }
        }
        if any {
            self.kick();
        }
    }

    /// Report the jobs the device finished.
    fn collect(&mut self) {
        fence(Ordering::SeqCst);
        let used = r16(RING_PAGE + USED + 2);
        let mut any = false;
        while self.used_seen != used {
            let e = RING_PAGE + USED + 4 + 8 * u64::from(self.used_seen % QUEUE);
            let id = r32(e) as usize;
            self.used_seen = self.used_seen.wrapping_add(1);
            let i = id / 3;
            if i >= JOBS || !id.is_multiple_of(3) {
                continue;
            }
            let s = r8(HEADER_PAGE + STATUS_BYTES + i as u64);
            let j = &self.jobs.jobs[i];
            store(
                &j.status,
                if s == 0 { status::OK } else { status::IO_ERROR },
            );
            store(&j.done, self.submitted[i]);
            self.jobs.finished.fetch_add(1, Ordering::AcqRel);
            self.finished_here += 1;
            any = true;
            if self.faulty && self.finished_here >= 20 {
                say!("version {} is broken on purpose: crashing", self.version);
                // SAFETY: deliberately faults (a test of the way back).
                unsafe { write_volatile(16 as *mut u64, 1) };
            }
        }
        if any {
            let _ = alaya_sys::signal(self.keeper, alaya_programs::keeper::DONE_BIT);
        }
    }

    /// Point the device at memory it was not given, once, and wait until
    /// it gave up or answered. The IOMMU must block the access; the keeper
    /// and the first program check its record of that.
    fn probe(&mut self) {
        self.queue(0, alaya_programs::OP_READ, 0, 1, IOVA_FORBIDDEN);
        self.kick();
        let end = alaya_sys::clock() + self.clock_hz / 4;
        while alaya_sys::clock() < end {
            if self.status() & NEEDS_RESET != 0 || r16(RING_PAGE + USED + 2) != 0 {
                return;
            }
            core::hint::spin_loop();
        }
    }

    /// Take the device over from the version before, without a reset.
    fn adopt(&mut self) -> bool {
        // SAFETY: the handover page is mapped here.
        let h = unsafe { &*(HANDOVER_PAGE as *const Handover) };
        if h.magic != HANDOVER_MAGIC
            || h.format != HANDOVER_FORMAT
            || h.queue_size != u64::from(QUEUE)
        {
            return false;
        }
        let c = self.regs.common;
        if self.status() & DRIVER_OK == 0 {
            return false;
        }
        w16(c + C_QUEUE_SELECT, 0);
        self.notify = self.regs.notify
            + u64::from(r16(c + C_QUEUE_NOTIFY_OFF)) * u64::from(self.regs.notify_mult);
        self.avail_idx = h.avail_idx as u16;
        self.used_seen = h.used_seen as u16;
        self.submitted = h.submitted;
        self.sectors = h.sectors;
        self.flush = h.flush != 0;
        true
    }

    /// Hand the device to the next version and wait: either the keeper
    /// ends this instance, or the new version failed and this one comes
    /// back.
    fn park(&mut self) {
        self.collect();
        // SAFETY: the handover page is mapped here, writable.
        let h = unsafe { &mut *(HANDOVER_PAGE as *mut Handover) };
        h.magic = HANDOVER_MAGIC;
        h.format = HANDOVER_FORMAT;
        h.from_version = self.version;
        h.queue_size = u64::from(QUEUE);
        h.avail_idx = u64::from(self.avail_idx);
        h.used_seen = u64::from(self.used_seen);
        h.sectors = self.sectors;
        h.notify_offset = self.notify - BARS;
        h.submitted = self.submitted;
        h.flush = u64::from(self.flush);
        fence(Ordering::SeqCst);
        store(&self.jobs.state, run::PARKED);
        let _ = alaya_sys::signal(self.keeper, alaya_programs::keeper::PARKED_BIT);
        loop {
            let b = alaya_sys::wait(self.park).unwrap_or(0);
            if b & ROLLBACK_BIT != 0 {
                break;
            }
        }
        // The device is in an unknown state: start it again from nothing,
        // and send every unfinished job again.
        say!(
            "version {} is back: resetting the disk and sending unfinished jobs again",
            self.version
        );
        let _ = alaya_sys::irq_set(start::FIRST_GIVEN + G_IRQ, self.ntfn, IRQ_BIT);
        if let Err(e) = self.reset_and_start() {
            self.fail(e);
        }
        store(&self.jobs.version, self.version);
        store(&self.jobs.state, run::RUNNING);
    }

    fn fail(&self, e: u64) -> ! {
        store(&self.jobs.error, e);
        store(&self.jobs.state, run::FAILED);
        say!("cannot start the disk (reason {e})");
        // Crash, so the keeper sees it.
        alaya_sys::crash()
    }
}

pub fn run(info: &'static StartInfo, version: u64) -> ! {
    let flags = info.args[ARG_FLAGS];
    // SAFETY: the keeper maps the job page here before starting us.
    let jobs = unsafe { &*(JOB_PAGE as *const JobPage) };
    let g = start::FIRST_GIVEN;
    let regs = find_regs().unwrap_or_else(|| {
        store(&jobs.error, why::NO_REGS);
        panic!("no virtio registers")
    });
    let irq = g + G_IRQ;
    let ntfn = g + G_NTFN;
    let msi = alaya_sys::irq_msi(irq, info.args[ARG_BDF] as u16)
        .map(|(a, d, _)| (a, d))
        .unwrap_or((0, 0));
    let mut d = Driver {
        version,
        regs,
        jobs,
        keeper: g + G_KEEPER,
        ntfn,
        park: g + G_PARK,
        msi,
        notify: 0,
        avail_idx: 0,
        used_seen: 0,
        submitted: [0; JOBS],
        sectors: 0,
        finished_here: 0,
        faulty: flags & FLAG_FAULTY != 0,
        flush: false,
        clock_hz: info.clock_hz.max(1),
    };
    store(&jobs.version, version);
    let _ = alaya_sys::irq_set(irq, ntfn, IRQ_BIT);
    // PCI command: memory space, bus mastering, no legacy interrupts.
    let cmd = CONFIG_PAGE + 4;
    w16(cmd, r16(cmd) | 0x2 | 0x4 | 0x400);

    let adopted = flags & FLAG_HANDOVER != 0 && d.adopt();
    if !adopted {
        if let Err(e) = d.reset_and_start() {
            d.fail(e);
        }
        if flags & FLAG_PROBE != 0 {
            d.probe();
            store(&jobs.probe, 1);
            if let Err(e) = d.reset_and_start() {
                d.fail(e);
            }
        }
    }
    store(&jobs.sectors, d.sectors);
    store(&jobs.state, run::RUNNING);
    let _ = alaya_sys::signal(d.keeper, alaya_programs::keeper::DONE_BIT);
    loop {
        d.collect();
        d.submit();
        d.collect();
        let bits = alaya_sys::wait(ntfn).unwrap_or(0);
        if bits & PAUSE_BIT != 0 {
            d.park();
        }
    }
}

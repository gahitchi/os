// SPDX-License-Identifier: GPL-3.0-only
//! Device interrupts as messages on ARM: the GICv3 ITS and LPIs.
//!
//! A PCI device raises an interrupt by writing a number (the *event*) to
//! the ITS's doorbell register. The ITS looks the writer up by its device
//! id (the PCI requester id, as the firmware's IORT table maps it) and
//! turns the event into an LPI, an interrupt number from 8192 up, sent to
//! one core. Device interrupt `n` is LPI 8192 + `n`, and its event is
//! `n - FIRST_VECTOR`, so one device can hold any of the kernel's device
//! interrupts.
//!
//! Everything the ITS and the redistributors need (their tables, a small
//! pool of per-device event tables) comes from the boot allocator, so the
//! kernel allocates nothing later (decision 56).

use core::sync::atomic::{AtomicU64, Ordering};

use super::gic::{self, r32, r64, w32, w64};
use crate::device::{FIRST_VECTOR, LAST_VECTOR};
use crate::mm::frames;
use crate::sync::SpinLock;
use crate::{kprintln, limine, mm, percpu};

/// Interrupt numbers up to 2^ID_BITS - 1 exist: LPIs 8192..16383.
const ID_BITS: u32 = 14;
const LPIS: u64 = (1 << ID_BITS) - gic::FIRST_LPI as u64;
/// Device ids the device table holds (buses 0 to 31).
const DEVICES: u64 = 1 << 13;
/// Devices that can have message interrupts at once.
const MAX_DEVICES: usize = 32;
/// Events per device (8 bits).
const EVENT_BITS: u64 = 8;

const GITS_CTLR: u64 = 0x0;
const GITS_TYPER: u64 = 0x8;
const GITS_CBASER: u64 = 0x80;
const GITS_CWRITER: u64 = 0x88;
const GITS_CREADR: u64 = 0x90;
const GITS_BASER: u64 = 0x100;
/// The doorbell, in the ITS's second 64 KiB frame.
pub const TRANSLATER: u64 = 0x1_0040;

const GICR_CTLR: u64 = 0x0;
const GICR_PROPBASER: u64 = 0x70;
const GICR_PENDBASER: u64 = 0x78;

/// Inner and outer write-back cacheable, inner shareable.
const CACHED: u64 = 0b111 << 59;
const BASER_INNER_SHARE: u64 = 0b01 << 10;
const RD_CACHED: u64 = 0b111 << 7 | 0b01 << 10;

/// Physical address of the ITS registers (0: none, no message interrupts).
pub(crate) static PHYS: AtomicU64 = AtomicU64::new(0);
static REGS: AtomicU64 = AtomicU64::new(0);
/// The LPI settings table (one byte per LPI, shared by all cores).
static PROP: AtomicU64 = AtomicU64::new(0);
/// Each core's pending table (physical, 64 KiB aligned).
static PEND: [AtomicU64; percpu::MAX_CPUS] = [const { AtomicU64::new(0) }; percpu::MAX_CPUS];
/// Collections are named by redistributor address (else by core number).
static BY_ADDRESS: AtomicU64 = AtomicU64::new(0);
/// How commands name each core (see `target`).
static TARGET: [AtomicU64; percpu::MAX_CPUS] = [const { AtomicU64::new(0) }; percpu::MAX_CPUS];

struct Queue {
    /// Command queue (virtual) and the next write offset.
    base: u64,
    write: u64,
    /// Event tables: free ones, and which device holds each.
    itt: [u64; MAX_DEVICES],
    owner: [u32; MAX_DEVICES],
    /// The device each device interrupt is mapped from (id + 1; 0: none),
    /// and the core it goes to.
    vector: [(u32, u16); 256],
}

static QUEUE: SpinLock<Queue> = SpinLock::new(Queue {
    base: 0,
    write: 0,
    itt: [0; MAX_DEVICES],
    owner: [0; MAX_DEVICES],
    vector: [(0, 0); 256],
});

fn regs() -> u64 {
    REGS.load(Ordering::Relaxed)
}

pub fn present() -> bool {
    regs() != 0
}

/// Set up the ITS at `phys` and the LPI tables. Bootstrap core, at boot,
/// before the other cores start.
pub fn init(phys: u64) {
    let d = gic::distributor();
    if phys == 0 || r32(d + 0x4) & 1 << 17 == 0 {
        kprintln!("gic: no message interrupts (ITS); devices cannot interrupt");
        return;
    }
    let hhdm = limine::hhdm_offset().unwrap();
    let its = mm::map_physical(phys, 0x2_0000, true);
    let typer = r64(its + GITS_TYPER);
    let itt_entry = ((typer >> 4) & 0xf) + 1;
    let dev_bits = ((typer >> 13) & 0x1f) + 1;
    BY_ADDRESS.store(typer >> 19 & 1, Ordering::Relaxed);

    // The LPI tables the redistributors read.
    PROP.store(frames::alloc_contiguous(LPIS, 4096), Ordering::Relaxed);
    for pend in PEND.iter().take(percpu::count() as usize) {
        pend.store(
            frames::alloc_contiguous(1 << ID_BITS >> 3, 0x1_0000),
            Ordering::Relaxed,
        );
    }

    // Its own tables: devices and collections, flat, 4 KiB pages.
    for n in 0..8 {
        let reg = its + GITS_BASER + n * 8;
        let v = r64(reg);
        let kind = (v >> 56) & 7;
        let entry = ((v >> 48) & 0x1f) + 1;
        let entries = match kind {
            1 => DEVICES.min(1 << dev_bits),
            4 => percpu::MAX_CPUS as u64,
            _ => continue,
        };
        let pages = (entries * entry).div_ceil(4096).clamp(1, 256);
        let table = frames::alloc_contiguous(pages * 4096, 4096);
        let want = 1 << 63
            | CACHED
            | kind << 56
            | (v & 0x1f << 48)
            | table
            | BASER_INNER_SHARE
            | (pages - 1);
        w64(reg, want);
        assert!(
            r64(reg) & (0b11 << 8) == 0,
            "its: table {n} does not take 4 KiB pages"
        );
    }

    let mut q = QUEUE.lock();
    let cmdq = frames::alloc_zeroed();
    q.base = cmdq + hhdm;
    w64(
        its + GITS_CBASER,
        1 << 63 | CACHED | cmdq | BASER_INNER_SHARE,
    );
    w64(its + GITS_CWRITER, 0);
    let itt_bytes = (itt_entry << EVENT_BITS).next_multiple_of(256);
    let pool = frames::alloc_contiguous(itt_bytes * MAX_DEVICES as u64, 4096);
    for (i, t) in q.itt.iter_mut().enumerate() {
        *t = pool + i as u64 * itt_bytes;
    }
    drop(q);
    w32(its + GITS_CTLR, 1);
    PHYS.store(phys, Ordering::Relaxed);
    REGS.store(its, Ordering::Release);
}

/// Let this core receive LPIs and give it its collection. Every core while
/// starting, after its redistributor was found.
pub fn init_this_core(index: u32, rd: u64) {
    if !present() {
        return;
    }
    let prop = PROP.load(Ordering::Relaxed);
    let pend = PEND[index as usize].load(Ordering::Relaxed);
    w64(
        rd + GICR_PROPBASER,
        prop | RD_CACHED | u64::from(ID_BITS - 1),
    );
    // PTZ: the pending table is zero.
    w64(rd + GICR_PENDBASER, 1 << 62 | pend | RD_CACHED);
    w32(rd + GICR_CTLR, r32(rd + GICR_CTLR) | 1);
    super::cpu::dsb_sy();
    let target = target(rd);
    TARGET[index as usize].store(target, Ordering::Relaxed);
    command([0x09, 0, 1 << 63 | target | u64::from(index), 0]); // MAPC
    command([0x05, 0, target, 0]); // SYNC
}

/// How commands name a core: its redistributor's address, or its number.
fn target(rd: u64) -> u64 {
    if BY_ADDRESS.load(Ordering::Relaxed) != 0 {
        rd - limine::hhdm_offset().unwrap()
    } else {
        ((r64(rd + 0x8) >> 8) & 0xffff) << 16
    }
}

/// Queue one command and wait until the ITS has done it.
fn command(c: [u64; 4]) {
    let its = regs();
    let mut q = QUEUE.lock();
    let at = q.base + q.write;
    for (i, w) in c.iter().enumerate() {
        // SAFETY: the command queue page, reached through the direct map.
        unsafe { core::ptr::write_volatile((at + i as u64 * 8) as *mut u64, *w) };
    }
    q.write = (q.write + 32) % 4096;
    super::cpu::dsb_sy();
    w64(its + GITS_CWRITER, q.write);
    let end = crate::time::now() + crate::time::ns_to_ticks(100_000_000);
    while r64(its + GITS_CREADR) & 0xfffe0 != q.write {
        assert!(
            r64(its + GITS_CREADR) & 1 == 0,
            "its: command {:#x} stalled",
            c[0] & 0xff
        );
        assert!(
            crate::time::now() < end,
            "its: command {:#x} timed out",
            c[0] & 0xff
        );
        core::hint::spin_loop();
    }
}

fn set_lpi(vector: u8, on: bool) {
    let hhdm = limine::hhdm_offset().unwrap();
    let at = PROP.load(Ordering::Relaxed) + hhdm + u64::from(vector);
    // Priority 0xa0 (below the cores' own messages), group 1.
    // SAFETY: the LPI settings table, through the direct map.
    unsafe { core::ptr::write_volatile(at as *mut u8, 0xa0 | u8::from(on)) };
    super::cpu::dsb_sy();
}

/// Route device interrupt `vector` from device `dev` (its id as the ITS
/// sees it) to core `core`. Returns the event the device must write, or
/// None when there is no ITS or no room for another device.
pub fn map(dev: u32, vector: u8, core: u16) -> Option<u64> {
    if !present() || !(FIRST_VECTOR..=LAST_VECTOR).contains(&vector) || u64::from(dev) >= DEVICES {
        return None;
    }
    let event = u64::from(vector - FIRST_VECTOR);
    let (owner, to) = QUEUE.lock().vector[vector as usize];
    if owner == dev + 1 && to == core {
        return Some(event);
    }
    unmap(vector);
    // The device's event table, set up on first use.
    let itt = {
        let mut q = QUEUE.lock();
        if let Some(i) = q.owner.iter().position(|&o| o == dev + 1) {
            Some((q.itt[i], false))
        } else {
            let i = q.owner.iter().position(|&o| o == 0)?;
            q.owner[i] = dev + 1;
            Some((q.itt[i], true))
        }
    };
    let (itt, new) = itt?;
    let d = u64::from(dev) << 32;
    if new {
        command([0x08 | d, EVENT_BITS - 1, 1 << 63 | itt, 0]); // MAPD
    }
    set_lpi(vector, true);
    let lpi = u64::from(gic::FIRST_LPI) + u64::from(vector);
    command([0x0a | d, event | lpi << 32, u64::from(core), 0]); // MAPTI
    command([0x0c | d, event, 0, 0]); // INV
    command([0x05, 0, TARGET[core as usize].load(Ordering::Relaxed), 0]); // SYNC
    QUEUE.lock().vector[vector as usize] = (dev + 1, core);
    Some(event)
}

/// Device interrupt `vector` is given up: the device's writes for it are
/// dropped from now on.
pub fn unmap(vector: u8) {
    if !present() {
        return;
    }
    let (owner, core) = QUEUE.lock().vector[vector as usize];
    if owner == 0 {
        return;
    }
    let event = u64::from(vector - FIRST_VECTOR);
    command([0x0f | u64::from(owner - 1) << 32, event, 0, 0]); // DISCARD
    command([0x05, 0, TARGET[core as usize].load(Ordering::Relaxed), 0]); // SYNC
    set_lpi(vector, false);
    QUEUE.lock().vector[vector as usize] = (0, 0);
}

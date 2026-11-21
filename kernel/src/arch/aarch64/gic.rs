// SPDX-License-Identifier: GPL-3.0-only
//! The ARM interrupt controller, GICv3: a distributor shared by all cores,
//! one redistributor per core, and the CPU interface in system registers.
//!
//! Interrupt numbers (INTIDs) the kernel uses:
//! - 1 and 2: messages between cores (SGIs): "look at your run queue
//!   again" and "stop now" (on a panic);
//! - 27: this core's timer (a PPI);
//! - 8192 and up: device interrupts written by devices as messages (LPIs,
//!   through the ITS, `its.rs`): device interrupt `n` is INTID 8192 + n.
//!
//! The firmware's MADT table says where the parts are.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::{kprintln, mm, percpu};

pub const WAKE_SGI: u32 = 1;
pub const STOP_SGI: u32 = 2;
pub const SPURIOUS: u32 = 1023;
pub const FIRST_LPI: u32 = 8192;

/// Distributor registers (virtual address) and their physical place.
static GICD: AtomicU64 = AtomicU64::new(0);
pub(crate) static GICD_PHYS: AtomicU64 = AtomicU64::new(0);
/// The redistributor region (physical, length; virtual).
pub(crate) static GICR_PHYS: AtomicU64 = AtomicU64::new(0);
pub(crate) static GICR_LEN: AtomicU64 = AtomicU64::new(0);
static GICR: AtomicU64 = AtomicU64::new(0);
/// Each core's redistributor (virtual address of its first frame).
pub(crate) static RD: [AtomicU64; percpu::MAX_CPUS] =
    [const { AtomicU64::new(0) }; percpu::MAX_CPUS];

const GICD_CTLR: u64 = 0x0;
const GICD_CTLR_ENABLE_G1: u32 = 1 << 1;
const GICD_CTLR_ARE: u32 = 1 << 4;
const GICD_CTLR_RWP: u32 = 1 << 31;

const GICR_TYPER: u64 = 0x8;
const GICR_WAKER: u64 = 0x14;
/// The second 64 KiB frame of a redistributor: SGI and PPI settings.
const SGI_BASE: u64 = 0x1_0000;
const GICR_IGROUPR0: u64 = SGI_BASE + 0x80;
const GICR_ISENABLER0: u64 = SGI_BASE + 0x100;
const GICR_ICENABLER0: u64 = SGI_BASE + 0x180;
const GICR_IPRIORITYR: u64 = SGI_BASE + 0x400;

pub(crate) fn r32(addr: u64) -> u32 {
    // SAFETY: GIC registers, mapped as device memory.
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

pub(crate) fn w32(addr: u64, v: u32) {
    // SAFETY: as above.
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}

pub(crate) fn r64(addr: u64) -> u64 {
    // SAFETY: as above.
    unsafe { core::ptr::read_volatile(addr as *const u64) }
}

pub(crate) fn w64(addr: u64, v: u64) {
    // SAFETY: as above.
    unsafe { core::ptr::write_volatile(addr as *mut u64, v) }
}

/// Where the parts are, from the MADT.
#[derive(Default, Clone, Copy)]
pub struct Layout {
    pub gicd: u64,
    pub gicr: u64,
    pub gicr_len: u64,
    pub its: u64,
    pub version: u8,
}

pub fn parse_madt(t: &[u8]) -> Layout {
    let mut l = Layout::default();
    let mut gicc_rd = 0u64;
    let mut at = 44;
    while at + 2 <= t.len() {
        let (kind, len) = (t[at], t[at + 1] as usize);
        if len < 2 || at + len > t.len() {
            break;
        }
        let e = &t[at..at + len];
        let u64_at = |o: usize| u64::from_le_bytes(e[o..o + 8].try_into().unwrap());
        let u32_at = |o: usize| u32::from_le_bytes(e[o..o + 4].try_into().unwrap());
        match kind {
            0x0b if len >= 76 => {
                let rd = u64_at(60);
                if rd != 0 && (gicc_rd == 0 || rd < gicc_rd) {
                    gicc_rd = rd;
                }
            }
            0x0c if len >= 24 => {
                l.gicd = u64_at(8);
                l.version = e[20];
            }
            0x0e if len >= 16 => {
                l.gicr = u64_at(4);
                l.gicr_len = u64::from(u32_at(12));
            }
            0x0f if len >= 20 && l.its == 0 => l.its = u64_at(8),
            _ => {}
        }
        at += len;
    }
    if l.gicr == 0 && gicc_rd != 0 {
        // Only per-core redistributor addresses: assume they sit together.
        l.gicr = gicc_rd;
        l.gicr_len = 0x2_0000 * percpu::count() as u64;
    }
    l
}

/// The distributor's registers (virtual).
pub fn distributor() -> u64 {
    GICD.load(Ordering::Relaxed)
}

/// Map the distributor and redistributors and switch the distributor on.
/// Bootstrap core, once the kernel's memory map exists.
pub fn init(l: &Layout) {
    assert!(
        l.gicd != 0 && l.gicr != 0,
        "the firmware describes no GICv3 interrupt controller (only GICv3 and later are supported)"
    );
    GICD_PHYS.store(l.gicd, Ordering::Relaxed);
    GICR_PHYS.store(l.gicr, Ordering::Relaxed);
    GICR_LEN.store(l.gicr_len, Ordering::Relaxed);
    let d = mm::map_physical(l.gicd, 0x1_0000, true);
    GICD.store(d, Ordering::Relaxed);
    GICR.store(
        mm::map_physical(l.gicr, l.gicr_len, true),
        Ordering::Relaxed,
    );
    w32(d + GICD_CTLR, GICD_CTLR_ARE);
    wait_rwp(d);
    w32(d + GICD_CTLR, GICD_CTLR_ARE | GICD_CTLR_ENABLE_G1);
    wait_rwp(d);
    super::its::init(l.its);
    kprintln!(
        "gic: GICv{} distributor at {:#x}, redistributors at {:#x} ({} KiB){}",
        if l.version >= 3 { l.version } else { 3 },
        l.gicd,
        l.gicr,
        l.gicr_len / 1024,
        if super::its::present() {
            ", message interrupts (ITS)"
        } else {
            ""
        }
    );
}

fn wait_rwp(d: u64) {
    while r32(d + GICD_CTLR) & GICD_CTLR_RWP != 0 {
        super::spin_hint();
    }
}

/// Find this core's redistributor and wake it, then turn on the CPU
/// interface. Every core while starting.
pub fn init_this_core(index: u32) {
    let me = super::cpu::mpidr();
    let aff = (me & 0xff_ffff) | ((me >> 32) & 0xff) << 24;
    let (base, len) = (
        GICR.load(Ordering::Relaxed),
        GICR_LEN.load(Ordering::Relaxed),
    );
    let mut frame = base;
    let mut rd = 0;
    while frame < base + len {
        let typer = r64(frame + GICR_TYPER);
        if typer >> 32 == aff {
            rd = frame;
            break;
        }
        if typer & 1 << 4 != 0 {
            break; // the last one
        }
        // GICv4 redistributors (VLPIS) have two more frames.
        frame += if typer & 1 << 1 != 0 {
            0x4_0000
        } else {
            0x2_0000
        };
    }
    assert!(
        rd != 0,
        "cpu {index}: no interrupt redistributor for MPIDR {me:#x}"
    );
    RD[index as usize].store(rd, Ordering::Relaxed);
    // Wake it: clear ProcessorSleep, wait for ChildrenAsleep to clear.
    w32(rd + GICR_WAKER, r32(rd + GICR_WAKER) & !(1 << 1));
    while r32(rd + GICR_WAKER) & 1 << 2 != 0 {
        super::spin_hint();
    }
    // Every SGI and PPI in group 1, all off until wanted, middle priority.
    w32(rd + GICR_IGROUPR0, u32::MAX);
    w32(rd + GICR_ICENABLER0, u32::MAX);
    for i in 0..8 {
        w32(rd + GICR_IPRIORITYR + i * 4, 0x8080_8080);
    }
    // The CPU interface: system registers on, nothing masked by priority,
    // group 1 on.
    set_sysreg!("s3_0_c12_c12_5", sysreg!("s3_0_c12_c12_5") | 1); // ICC_SRE_EL1.SRE
    super::cpu::isb();
    set_sysreg!("s3_0_c4_c6_0", 0xff); // ICC_PMR_EL1
    set_sysreg!("s3_0_c12_c12_3", 0); // ICC_BPR1_EL1
    set_sysreg!("s3_0_c12_c12_7", 1); // ICC_IGRPEN1_EL1
    super::cpu::isb();
    enable_private(WAKE_SGI);
    enable_private(STOP_SGI);
    super::its::init_this_core(index, rd);
}

/// Turn on a private interrupt (SGI or PPI) of this core.
pub fn enable_private(intid: u32) {
    let rd = RD[crate::percpu::index() as usize].load(Ordering::Relaxed);
    w32(rd + GICR_ISENABLER0, 1 << intid);
}

/// Take the highest-priority pending interrupt (its INTID).
#[inline]
pub fn ack() -> u32 {
    let v = sysreg!("s3_0_c12_c12_0"); // ICC_IAR1_EL1
    super::cpu::dsb_sy();
    v as u32
}

/// Done with interrupt `intid`.
#[inline]
pub fn eoi(intid: u32) {
    set_sysreg!("s3_0_c12_c12_1", u64::from(intid)); // ICC_EOIR1_EL1
    super::cpu::isb();
}

/// Send SGI `intid` to the core with affinity `mpidr`.
pub fn send_sgi(mpidr: u64, intid: u32) {
    let aff0 = mpidr & 0xf;
    let aff1 = (mpidr >> 8) & 0xff;
    let aff2 = (mpidr >> 16) & 0xff;
    let aff3 = (mpidr >> 32) & 0xff;
    let rs = (mpidr & 0xff) >> 4;
    let v = aff3 << 48 | rs << 44 | aff2 << 32 | u64::from(intid) << 24 | aff1 << 16 | 1 << aff0;
    // Make earlier writes (run queues, flush requests) visible first.
    super::cpu::dsb_ish();
    set_sysreg!("s3_0_c12_c11_5", v); // ICC_SGI1R_EL1
    super::cpu::isb();
}

// SPDX-License-Identifier: GPL-3.0-only
//! Data each core keeps for itself, reached through the GS base register.
//!
//! Cores are numbered from 0 (the bootstrap core) in the order the
//! bootloader lists them. That number is the kernel's "cpu" everywhere; the
//! APIC id is kept alongside because hardware addresses cores by it.

use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch::cpu;
use crate::arch::gdt::{Gdt, Tss};

/// The most cores the kernel supports. Today's largest desktop and laptop
/// CPUs have 32 threads; servers have more, so leave room.
pub const MAX_CPUS: usize = 256;

#[repr(C)]
pub struct PerCpu {
    /// Must stay first: [`index`] reads it at GS:0.
    index: u32,
    pub apic_id: AtomicU32,
    /// The core has loaded the kernel's IDT and can take an NMI.
    pub has_idt: AtomicBool,
    /// The core finished starting up.
    pub online: AtomicBool,
    /// Big-lock self-test: how many times this core took the lock.
    pub bkl_rounds: AtomicU64,
    /// Timer self-test results.
    pub timer_ticks: AtomicU32,
    pub timer_worst_late_ns: AtomicU64,
    pub timer_next_deadline: AtomicU64,
}

impl PerCpu {
    const fn new(index: u32) -> Self {
        Self {
            index,
            apic_id: AtomicU32::new(0),
            has_idt: AtomicBool::new(false),
            online: AtomicBool::new(false),
            bkl_rounds: AtomicU64::new(0),
            timer_ticks: AtomicU32::new(0),
            timer_worst_late_ns: AtomicU64::new(0),
            timer_next_deadline: AtomicU64::new(0),
        }
    }
}

static CPUS: [PerCpu; MAX_CPUS] = {
    let mut cpus = [const { PerCpu::new(0) }; MAX_CPUS];
    let mut i = 0;
    while i < MAX_CPUS {
        cpus[i].index = i as u32;
        i += 1;
    }
    cpus
};

static mut GDTS: [Gdt; MAX_CPUS] = [const { Gdt::new() }; MAX_CPUS];
static mut TSSES: [Tss; MAX_CPUS] = [const { Tss::new() }; MAX_CPUS];

/// How many cores the bootloader reported (set once by the bootstrap core).
static COUNT: AtomicU32 = AtomicU32::new(1);

pub fn set_count(n: u32) {
    COUNT.store(n, Ordering::Relaxed);
}

pub fn count() -> u32 {
    COUNT.load(Ordering::Relaxed)
}

pub fn get(index: u32) -> &'static PerCpu {
    &CPUS[index as usize]
}

/// Point GS at core `index`'s data. The first thing every core does.
pub fn enter(index: u32) {
    cpu::set_gs_base(&CPUS[index as usize] as *const PerCpu as u64);
}

/// The running core's number.
#[inline]
pub fn index() -> u32 {
    let i: u32;
    // SAFETY: GS points at this core's `PerCpu` (see `enter`), whose first
    // field is the index.
    unsafe {
        core::arch::asm!("mov {:e}, gs:[0]", out(reg) i, options(readonly, nostack, preserves_flags))
    };
    i
}

pub fn this() -> &'static PerCpu {
    get(index())
}

/// Set up and load this core's GDT and TSS, with its interrupt stacks.
pub fn load_gdt(ist_tops: [u64; crate::arch::gdt::IST_COUNT]) {
    let i = index() as usize;
    // SAFETY: each core touches only its own slot, once, while starting.
    unsafe {
        let tss = &mut (*addr_of_mut!(TSSES))[i];
        let mut ist = tss.ist;
        ist[..ist_tops.len()].copy_from_slice(&ist_tops);
        tss.ist = ist;
        crate::arch::gdt::load(&mut (*addr_of_mut!(GDTS))[i], tss);
    }
    // Loading the segment registers cleared GS base; point it back.
    enter(i as u32);
}

// SPDX-License-Identifier: GPL-3.0-only
//! Data each core keeps for itself, reached through a register the
//! processor keeps per core (GS base on a PC, TPIDR_EL1 on ARM; see
//! `arch::set_cpu_base`).
//!
//! Cores are numbered from 0 (the bootstrap core) in the order the
//! bootloader lists them. That number is the kernel's "cpu" everywhere; the
//! hardware id (APIC id on a PC, MPIDR on ARM) is kept alongside because
//! hardware addresses cores by it.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::arch;

/// The most cores the kernel supports. Today's largest desktop and laptop
/// CPUs have 32 threads; servers have more, so leave room.
pub const MAX_CPUS: usize = 256;

#[repr(C)]
pub struct PerCpu {
    /// Must stay first: [`index`] reads it at offset 0.
    index: u32,
    /// Top of this core's kernel stack, where an entry from user mode
    /// switches to (offset 8, read by the entry code behind the wall).
    pub kernel_stack_top: AtomicU64,
    /// Scratch space for the user stack pointer while the entry code
    /// switches stacks (offset 16).
    pub user_sp: AtomicU64,
    /// The hardware's id for this core (APIC id, MPIDR).
    pub hw_id: AtomicU64,
    /// The core can take the "stop now" signal another core sends on a
    /// panic (its exception table is loaded).
    pub can_stop: AtomicBool,
    /// The core finished starting up.
    pub online: AtomicBool,
    /// Big-lock self-test: how many times this core took the lock.
    pub bkl_rounds: AtomicU64,
    /// Timer self-test results.
    pub timer_ticks: AtomicU32,
    pub timer_worst_late_ns: AtomicU64,
    pub timer_next_deadline: AtomicU64,
    /// Another core asks this one to flush its TLB.
    pub flush: AtomicBool,
    /// Big kernel lock use on this core (decision 77: measured under load):
    /// times taken, times it had to wait, clock ticks spent waiting and
    /// holding, and when it was last taken.
    pub lock_takes: AtomicU64,
    pub lock_waits: AtomicU64,
    pub lock_wait_ticks: AtomicU64,
    pub lock_held_ticks: AtomicU64,
    pub lock_since: AtomicU64,
    /// The deadline this core's timer is armed for, 0 when it is not: the
    /// kernel skips arming it again for the same or a later time (on a
    /// virtual machine each write to the timer is a trip out to the host).
    pub timer_armed: AtomicU64,
    /// The user thread-local storage bases the core holds now (x86:
    /// FS_BASE, and GS_BASE parked in KERNEL_GS_BASE), so a switch between
    /// threads with the same ones writes nothing.
    pub user_tls: [AtomicU64; 2],
}

impl PerCpu {
    const fn new(index: u32) -> Self {
        Self {
            index,
            kernel_stack_top: AtomicU64::new(0),
            user_sp: AtomicU64::new(0),
            hw_id: AtomicU64::new(0),
            can_stop: AtomicBool::new(false),
            online: AtomicBool::new(false),
            bkl_rounds: AtomicU64::new(0),
            timer_ticks: AtomicU32::new(0),
            timer_worst_late_ns: AtomicU64::new(0),
            timer_next_deadline: AtomicU64::new(0),
            flush: AtomicBool::new(false),
            lock_takes: AtomicU64::new(0),
            lock_waits: AtomicU64::new(0),
            lock_wait_ticks: AtomicU64::new(0),
            lock_held_ticks: AtomicU64::new(0),
            lock_since: AtomicU64::new(0),
            timer_armed: AtomicU64::new(0),
            user_tls: [AtomicU64::new(u64::MAX), AtomicU64::new(u64::MAX)],
        }
    }
}

/// Offsets the entry code behind the wall relies on.
pub const KERNEL_STACK_OFFSET: usize = 8;
pub const USER_SP_OFFSET: usize = 16;
const _: () = assert!(core::mem::offset_of!(PerCpu, index) == 0);
const _: () = assert!(core::mem::offset_of!(PerCpu, kernel_stack_top) == KERNEL_STACK_OFFSET);
const _: () = assert!(core::mem::offset_of!(PerCpu, user_sp) == USER_SP_OFFSET);

pub(crate) static CPUS: [PerCpu; MAX_CPUS] = {
    let mut cpus = [const { PerCpu::new(0) }; MAX_CPUS];
    let mut i = 0;
    while i < MAX_CPUS {
        cpus[i].index = i as u32;
        i += 1;
    }
    cpus
};

/// How many cores the bootloader reported (set once by the bootstrap core).
pub(crate) static COUNT: AtomicU32 = AtomicU32::new(1);

pub fn set_count(n: u32) {
    COUNT.store(n, Ordering::Relaxed);
}

pub fn count() -> u32 {
    COUNT.load(Ordering::Relaxed)
}

pub fn get(index: u32) -> &'static PerCpu {
    &CPUS[index as usize]
}

/// Point this core's per-core register at core `index`'s data. The first
/// thing every core does.
pub fn enter(index: u32) {
    arch::set_cpu_base(&CPUS[index as usize] as *const PerCpu as u64);
}

/// The running core's number.
#[inline]
pub fn index() -> u32 {
    arch::cpu_index()
}

pub fn this() -> &'static PerCpu {
    get(index())
}

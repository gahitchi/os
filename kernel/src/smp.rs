// SPDX-License-Identifier: GPL-3.0-only
//! Starting every core.
//!
//! Limine has already woken all cores and parked them in a loop of its own.
//! The bootstrap core builds everything they need (page tables, stacks, the
//! IDT), then releases each one into [`ap_entry`]. Each core then switches
//! to the kernel's page tables and its own stack and sets itself up.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch;
use crate::limine::MpInfo;
use crate::mm::layout::Stack;
use crate::{kprintln, mm, percpu, time};

/// Released by the bootstrap core when every core is online; then all
/// cores run the self-tests at the same time.
pub static GO: AtomicBool = AtomicBool::new(false);
/// Cores that finished their self-tests.
pub static DONE: AtomicU32 = AtomicU32::new(0);

/// Set up the calling core (behind the wall: its tables, interrupt
/// controller and protections), and check the hardware agrees on its id.
pub fn init_this_core(index: u32) {
    let id = arch::init_this_core(index);
    let expected = percpu::get(index).hw_id.load(Ordering::Relaxed);
    assert!(
        id == expected,
        "cpu {index}: hardware id {id:#x}, but the bootloader said {expected:#x}"
    );
}

/// Release every other core. `cpus` lists them in the bootloader's order.
pub fn start_others(cpus: impl Iterator<Item = &'static MpInfo>, bsp_hw_id: u64) {
    let mut next: u32 = 1;
    for info in cpus {
        if info.hw_id() == bsp_hw_id {
            continue;
        }
        if next as usize >= percpu::MAX_CPUS {
            break;
        }
        info.start(ap_entry, u64::from(next));
        next += 1;
    }
}

/// Where every other core starts, still on the bootloader's stack and page
/// tables.
extern "C" fn ap_entry(info: &'static MpInfo) -> ! {
    let index = info.argument() as u32;
    percpu::enter(index);
    mm::switch_this_core();
    let top = Stack::Kernel.top(index);
    arch::run_on_stack(top, ap_main, u64::from(index))
}

extern "C" fn ap_main(index: u64) -> ! {
    let index = index as u32;
    init_this_core(index);
    time::init_this_core();
    crate::sched::init_this_core(&mut crate::sync::BKL.lock().sys);
    let me = percpu::this();
    me.online.store(true, Ordering::Release);
    kprintln!(
        "cpu {index}: online, hardware id {:#x}",
        me.hw_id.load(Ordering::Relaxed)
    );

    while !GO.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    crate::selftest::run_on_this_core();
    crate::syscall::idle_this_core()
}

/// Nothing to run yet: sleep until an interrupt, forever.
pub fn idle() -> ! {
    loop {
        arch::wait_for_interrupt();
    }
}

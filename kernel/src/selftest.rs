// SPDX-License-Identifier: GPL-3.0-only
//! Checks every core runs at boot, and deliberate faults for testing the
//! fault reports.
//!
//! Every boot, every core: hammers the big kernel lock, then takes timer
//! interrupts. The bootstrap core then prints one `core N: ready` line per
//! core; the test runners count those lines.
//!
//! With `test-fault=<kind>` on the kernel command line, the kernel instead
//! crashes on purpose after the self-tests, so CI can check the report:
//! `page` (write to an unmapped address), `stack` (kernel stack overflow),
//! `invalid-opcode`, or `other-core-page` (a page fault on the last core).

use core::sync::atomic::Ordering;

use crate::{percpu, smp, sync, time};

/// Lock acquisitions per core in the big-lock test.
pub const BKL_ROUNDS: u64 = 20_000;

pub fn run_on_this_core() {
    for _ in 0..BKL_ROUNDS {
        let mut kernel = sync::BKL.lock();
        let counter = &mut kernel.selftest_counter as *mut u64;
        // SAFETY: the lock is held. Volatile, so that a read-modify-write
        // really happens and lost updates would show if the lock leaked.
        unsafe { counter.write_volatile(counter.read_volatile() + 1) };
    }
    time::selftest_this_core();

    let me = percpu::index();
    smp::DONE.fetch_add(1, Ordering::AcqRel);
    if fault_test() == Some("other-core-page") && me == percpu::count() - 1 && me != 0 {
        page_fault();
    }
}

pub fn fault_test() -> Option<&'static str> {
    crate::limine::cmdline()
        .split_whitespace()
        .find_map(|arg| arg.strip_prefix("test-fault="))
}

/// Trigger the requested fault on the bootstrap core, if any.
pub fn fault_on_bootstrap_core() {
    match fault_test() {
        None | Some("other-core-page") => {}
        Some("page") => page_fault(),
        Some("stack") => {
            let _ = recurse(0);
        }
        Some("invalid-opcode") => {
            // SAFETY: deliberately raises #UD.
            unsafe { core::arch::asm!("ud2") };
        }
        Some(other) => panic!("unknown test-fault={other}"),
    }
}

fn page_fault() {
    crate::kprintln!("selftest: writing to an unmapped address on purpose");
    // SAFETY: deliberately faults; nothing is mapped in the lower half.
    unsafe { core::ptr::write_volatile(0x0000_dead_0000 as *mut u64, 1) };
}

#[allow(unconditional_recursion)]
#[inline(never)]
fn recurse(depth: u64) -> u64 {
    let mut frame = [0u8; 1024];
    core::hint::black_box(&mut frame);
    recurse(depth + 1) + u64::from(frame[(depth % 1024) as usize])
}

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
//! `invalid-opcode`, `other-core-page` (a page fault on the last core),
//! `smap` (the kernel reads a user page) or `smep` (it runs code in one).

use core::sync::atomic::Ordering;

use crate::arch::cpu::rdtsc;
use crate::{percpu, smp, sync, time};

/// Most lock acquisitions per core in the big-lock test.
pub const BKL_ROUNDS: u64 = 20_000;
/// Time limit per core for the big-lock test. A ticket lock hands over in
/// strict order, so when a hypervisor runs more virtual cores than it has
/// real ones, each hand-over can wait for a descheduled core; the limit
/// keeps boot fast there while still testing exclusion.
const BKL_TEST_MS: u64 = 200;

pub fn run_on_this_core() {
    let end = rdtsc() + time::ns_to_tsc(BKL_TEST_MS * 1_000_000);
    let mut rounds = 0;
    while rounds < BKL_ROUNDS && rdtsc() < end {
        rounds += 1;
        let mut kernel = sync::BKL.lock();
        let counter = &mut kernel.selftest_counter as *mut u64;
        // SAFETY: the lock is held. Volatile, so that a read-modify-write
        // really happens and lost updates would show if the lock leaked.
        unsafe { counter.write_volatile(counter.read_volatile() + 1) };
    }
    percpu::this().bkl_rounds.store(rounds, Ordering::Relaxed);
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
        Some("smap") => touch_user_page(false),
        Some("smep") => touch_user_page(true),
        Some(other) => panic!("unknown test-fault={other}"),
    }
}

/// Map a program-style (user) page and read it, or run code in it, from
/// the kernel: the safety switches (SMAP, SMEP) must stop both.
fn touch_user_page(execute: bool) {
    use crate::arch::cpu;
    use crate::mm::paging::{AddressSpace, NO_EXECUTE, USER, WRITABLE};
    let hhdm = crate::limine::hhdm_offset().unwrap();
    let mut space = AddressSpace::with_root(
        crate::mm::frames::alloc_zeroed(),
        hhdm,
        cpu::features().gib_pages,
        crate::mm::kernel_root(),
    );
    let page = crate::mm::frames::alloc_zeroed();
    // `ret`, so that without SMEP the call would simply come back.
    // SAFETY: a fresh frame, reached through the direct map.
    unsafe { *((page + hhdm) as *mut u8) = 0xc3 };
    let flags = if execute {
        USER
    } else {
        USER | WRITABLE | NO_EXECUTE
    };
    space.map(USER_PAGE, page, 4096, flags);
    // SAFETY: the new tables map the kernel's half like the kernel's own.
    unsafe { cpu::write_cr3(space.root()) };
    if execute {
        crate::kprintln!("selftest: running code in a user page on purpose");
        // SAFETY: deliberately faults with SMEP on.
        unsafe { core::arch::asm!("call {}", in(reg) USER_PAGE, clobber_abi("C")) };
    } else {
        crate::kprintln!("selftest: reading a user page from the kernel on purpose");
        // SAFETY: deliberately faults with SMAP on.
        unsafe { core::ptr::read_volatile(USER_PAGE as *const u64) };
    }
    crate::kprintln!("selftest: the kernel touched a user page and nothing stopped it");
}

const USER_PAGE: u64 = 0x10000;

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

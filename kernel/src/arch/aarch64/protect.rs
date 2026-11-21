// SPDX-License-Identifier: GPL-3.0-only
//! ARM's protections, matched to the PC's where they do the same job:
//!
//! - **The kernel never runs program code:** every program page is marked
//!   privileged-execute-never (PXN), on every ARMv8 core (like SMEP).
//! - **The kernel touches program memory only on purpose:** Privileged
//!   Access Never (PAN, ARMv8.1 and later; like SMAP). On older cores the
//!   kernel still reaches program memory only through its direct map after
//!   checking the program's tables, as on a PC without SMAP.
//! - **Tagged memory maps:** address-space ids (ASIDs), always on (like
//!   PCID).
//! - **Speculation:** handles a program passes are checked with a
//!   speculation barrier (CSDB) after the bounds check.

use core::sync::atomic::{AtomicU64, Ordering};

use alaya_abi::protect as p;

use crate::kprintln;

pub(crate) static ON: AtomicU64 = AtomicU64::new(0);

/// Find what the core offers. Bootstrap core, once.
pub fn detect() {
    let f = super::cpu::features();
    // Programs can never read the kernel's system registers on ARM (the
    // twin of UMIP): every one of them is out of reach at EL0.
    let mut on = p::SMEP | p::UMIP | p::PCID;
    if f.pan {
        on |= p::SMAP;
    }
    ON.store(on, Ordering::Relaxed);
}

/// Turn the protections on for this core.
pub fn init_this_core() {
    if ON.load(Ordering::Relaxed) & p::SMAP != 0 {
        // SCTLR_EL1.SPAN = 0: PAN is set again on every exception entry.
        set_sysreg!("sctlr_el1", sysreg!("sctlr_el1") & !(1 << 23));
        // MSR PAN, #1 (ARMv8.1), as raw bits for older assemblers.
        // SAFETY: only forbids the kernel's access to user pages.
        unsafe { core::arch::asm!(".inst 0xd500419f", options(nomem, nostack)) };
    }
}

pub fn enabled() -> u64 {
    ON.load(Ordering::Relaxed)
}

pub fn report() {
    let on = enabled();
    let yes = |b: bool| if b { "yes" } else { "no" };
    kprintln!(
        "protect: kernel never runs program code (PXN) yes, kernel never touches program memory by accident (PAN) {}, programs cannot read kernel registers yes, tagged memory maps (ASID) yes",
        yes(on & p::SMAP != 0)
    );
    kprintln!("protect: speculation guards: bounds-check barriers (CSDB) yes");
}

/// Stop speculation past a bounds check on a value a program chose.
#[inline(always)]
pub fn barrier() {
    // SAFETY: CSDB only limits speculation.
    unsafe { core::arch::asm!("csdb", options(nomem, nostack, preserves_flags)) };
}

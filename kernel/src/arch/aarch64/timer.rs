// SPDX-License-Identifier: GPL-3.0-only
//! The ARM generic timer: one counter every core shares (the clock), and a
//! compare register per core (the deadline timer).
//!
//! The kernel uses the *virtual* counter and timer, which work the same at
//! EL1 on bare hardware and inside a virtual machine. The counter's rate is
//! fixed by the firmware (CNTFRQ_EL0), so nothing needs measuring. Programs
//! may read the counter themselves (`alaya_sys::clock()`).

use crate::kprintln;

/// The timer's interrupt: private peripheral interrupt 27 (virtual timer).
pub const TIMER_INTID: u32 = 27;

const CTL_ENABLE: u64 = 1 << 0;
const CTL_IMASK: u64 = 1 << 1;

/// The clock: the virtual counter.
#[inline]
pub fn now() -> u64 {
    // ISB first, so the read is not done early (out of order).
    super::cpu::isb();
    sysreg!("cntvct_el0")
}

/// The counter rate. Bootstrap core only. Returns ticks per second.
pub fn calibrate() -> u64 {
    let hz = sysreg!("cntfrq_el0");
    assert!(hz != 0, "the firmware did not set the counter frequency");
    kprintln!(
        "time: generic timer {}.{:03} MHz (from the firmware), timer is the virtual timer",
        hz / 1_000_000,
        hz / 1000 % 1000
    );
    hz
}

pub fn name() -> &'static str {
    "generic"
}

/// Set up this core's timer (off until armed); let programs read the
/// counter.
pub fn init_this_core() {
    disarm();
    // CNTKCTL_EL1.EL0VCTEN: user mode may read the virtual counter.
    set_sysreg!("cntkctl_el1", (sysreg!("cntkctl_el1") & !0x3ff) | 1 << 1);
    super::gic::enable_private(TIMER_INTID);
}

/// Fire this core's timer interrupt when the clock reaches `ticks`.
pub fn set_deadline(ticks: u64) {
    if ticks == 0 {
        return disarm();
    }
    set_sysreg!("cntv_cval_el0", ticks);
    set_sysreg!("cntv_ctl_el0", CTL_ENABLE);
    super::cpu::isb();
}

/// Stop this core's timer. The timer interrupt is level-triggered, so the
/// interrupt handler disarms first and the kernel arms it again if needed.
pub fn disarm() {
    set_sysreg!("cntv_ctl_el0", CTL_IMASK);
    super::cpu::isb();
}

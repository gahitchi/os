// SPDX-License-Identifier: GPL-3.0-only
//! Time: one clock that counts up at a fixed rate on every core, and a
//! timer per core that fires at a given clock value.
//!
//! The processor provides both behind the wall (`arch::timer`): on a PC the
//! clock is the TSC and the timer the TSC-deadline timer (or the APIC
//! one-shot timer under plain emulation); on ARM they are the generic
//! timer's counter and compare register. Everything else in the kernel
//! counts in clock *ticks* and converts with [`ns_to_ticks`].

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::timer;
use crate::{arch, percpu, sync};

/// Clock ticks per second.
pub(crate) static HZ: AtomicU64 = AtomicU64::new(0);

/// The clock, in ticks.
#[inline]
pub fn now() -> u64 {
    timer::now()
}

pub fn hz() -> u64 {
    HZ.load(Ordering::Relaxed)
}

pub fn ticks_to_ns(ticks: u64) -> u64 {
    (u128::from(ticks) * 1_000_000_000 / u128::from(hz())) as u64
}

pub fn ns_to_ticks(ns: u64) -> u64 {
    (u128::from(ns) * u128::from(hz()) / 1_000_000_000) as u64
}

/// Find the clock rate and set up the timer. Bootstrap core only.
pub fn calibrate() {
    HZ.store(timer::calibrate(), Ordering::Relaxed);
}

pub fn timer_name() -> &'static str {
    timer::name()
}

/// Set up this core's timer (masked until armed).
pub fn init_this_core() {
    timer::init_this_core();
}

/// Fire this core's timer interrupt when the clock reaches `ticks`.
pub fn set_deadline(ticks: u64) {
    timer::set_deadline(ticks);
}

/// Stop this core's timer.
pub fn disarm() {
    timer::disarm();
}

/// Self-test: how many timer interrupts each core takes, and their period.
pub const SELFTEST_TICKS: u32 = 20;
pub const SELFTEST_PERIOD_NS: u64 = 1_000_000;

/// The timer interrupt. Runs with interrupts off, like all kernel entries,
/// and takes the big kernel lock.
pub fn interrupt() {
    let now = now();
    let mut kernel = sync::BKL.lock();
    if kernel.sys.started {
        let core = percpu::index() as usize;
        crate::sched::timer(&mut kernel.sys, core);
        crate::sched::arm(&kernel.sys, core, now);
        return;
    }
    let me = percpu::this();
    let deadline = me.timer_next_deadline.load(Ordering::Relaxed);
    let late = ticks_to_ns(now.saturating_sub(deadline));
    me.timer_worst_late_ns.fetch_max(late, Ordering::Relaxed);
    let ticks = me.timer_ticks.fetch_add(1, Ordering::Relaxed) + 1;
    if ticks < SELFTEST_TICKS {
        let next = deadline + ns_to_ticks(SELFTEST_PERIOD_NS);
        me.timer_next_deadline.store(next, Ordering::Relaxed);
        set_deadline(next);
    }
}

/// Run the timer self-test on this core: take [`SELFTEST_TICKS`] interrupts
/// one period apart, then stop the timer (cores are tickless when idle).
pub fn selftest_this_core() {
    let me = percpu::this();
    let first = now() + ns_to_ticks(SELFTEST_PERIOD_NS);
    me.timer_next_deadline.store(first, Ordering::Relaxed);
    set_deadline(first);
    while me.timer_ticks.load(Ordering::Relaxed) < SELFTEST_TICKS {
        arch::wait_for_interrupt();
    }
}

/// Spin until `done` returns true; after `seconds`, call `timed_out`.
pub fn wait_for(seconds: u64, done: impl Fn() -> bool, timed_out: impl FnOnce()) {
    let end = now() + hz() * seconds;
    while !done() {
        if now() > end {
            timed_out();
            panic!("timed out after {seconds}s");
        }
        core::hint::spin_loop();
    }
}

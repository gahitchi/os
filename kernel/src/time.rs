// SPDX-License-Identifier: GPL-3.0-only
//! Time: the TSC as the clock, the TSC-deadline timer for wake-ups.
//!
//! The design (battle test, "Timers") makes the TSC the clock source and the
//! TSC-deadline timer the way a core is woken at a precise moment: the kernel
//! writes the TSC value at which the interrupt should fire, with no counting
//! or rounding in between. Every CPU the reference laptop could have has
//! it, and so does QEMU with KVM.
//!
//! Plain emulation (QEMU without KVM, as on a phone) has no TSC-deadline
//! timer, so there the kernel falls back to the local APIC's one-shot
//! count-down timer, converting deadlines to counts. Same interface, less
//! precise.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::cpu::{self, cpuid, rdtsc};
use crate::arch::trap::TIMER_VECTOR;
use crate::arch::{apic, legacy};
use crate::{kprintln, percpu, sync};

static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static DEADLINE_MODE: AtomicBool = AtomicBool::new(false);
/// APIC timer counts per 2^32 TSC ticks (one-shot fallback only).
static APIC_PER_TSC_FIXED: AtomicU64 = AtomicU64::new(0);

pub fn tsc_hz() -> u64 {
    TSC_HZ.load(Ordering::Relaxed)
}

pub fn tsc_to_ns(ticks: u64) -> u64 {
    (u128::from(ticks) * 1_000_000_000 / u128::from(tsc_hz())) as u64
}

pub fn ns_to_tsc(ns: u64) -> u64 {
    (u128::from(ns) * u128::from(tsc_hz()) / 1_000_000_000) as u64
}

/// TSC frequency the CPU reports itself, and where it came from.
fn reported_tsc_hz() -> Option<(u64, &'static str)> {
    if cpu::max_leaf() >= 0x15 {
        // Leaf 0x15: TSC = crystal * EBX / EAX. Intel CPUs since Skylake.
        let r = cpuid(0x15, 0);
        if r.eax != 0 && r.ebx != 0 && r.ecx != 0 {
            let hz = u64::from(r.ecx) * u64::from(r.ebx) / u64::from(r.eax);
            return Some((hz, "CPUID leaf 0x15"));
        }
    }
    if cpu::features().hypervisor && cpuid(0x4000_0000, 0).eax >= 0x4000_0010 {
        // The hypervisor timing leaf (QEMU/KVM, VMware): EAX = TSC in kHz.
        let khz = cpuid(0x4000_0010, 0).eax;
        if khz != 0 {
            return Some((u64::from(khz) * 1000, "hypervisor"));
        }
    }
    None
}

/// Measure the TSC (and the APIC timer, for the fallback) against the PIT.
/// Best of three 50 ms runs.
fn measure(apic_timer: bool) -> (u64, u64) {
    let mut best: Option<(u64, u64)> = None;
    for _ in 0..3 {
        let mut start = 0;
        if apic_timer {
            apic::set_one_shot(u32::MAX);
        }
        let ok = legacy::pit_wait(50, || start = rdtsc());
        let tsc = rdtsc() - start;
        let apic = if apic_timer {
            u64::from(u32::MAX - apic::timer_current())
        } else {
            0
        };
        assert!(ok, "the PIT did not count; cannot measure the TSC");
        if best.is_none_or(|(t, _)| tsc < t) {
            best = Some((tsc, apic));
        }
    }
    let (tsc, apic) = best.unwrap();
    (tsc * 20, apic * 20)
}

/// Find the TSC frequency and choose the timer mode. Bootstrap core only,
/// after its local APIC is set up.
pub fn calibrate() {
    let features = cpu::features();
    let deadline = features.tsc_deadline;
    DEADLINE_MODE.store(deadline, Ordering::Relaxed);
    if !deadline {
        // Interrupts are off, and a full count takes over a minute.
        apic::timer_one_shot(TIMER_VECTOR);
    }
    let (hz, source) = match reported_tsc_hz() {
        Some(r) if deadline => r,
        _ => {
            let (tsc, apic_hz) = measure(!deadline);
            if !deadline {
                apic::set_one_shot(0);
                let fixed = (u128::from(apic_hz) << 32) / u128::from(tsc);
                APIC_PER_TSC_FIXED.store(fixed as u64, Ordering::Relaxed);
            }
            (tsc, "measured against the PIT")
        }
    };
    TSC_HZ.store(hz, Ordering::Relaxed);
    kprintln!(
        "time: TSC {}.{:03} MHz ({source}), invariant: {}",
        hz / 1_000_000,
        hz / 1000 % 1000,
        if features.invariant_tsc { "yes" } else { "no" }
    );
    if deadline {
        kprintln!("time: timer is the TSC-deadline timer");
    } else {
        kprintln!(
            "time: this CPU has no TSC-deadline timer (plain emulation?); using the APIC one-shot timer"
        );
    }
}

pub fn timer_name() -> &'static str {
    if DEADLINE_MODE.load(Ordering::Relaxed) {
        "TSC deadline"
    } else {
        "APIC one-shot"
    }
}

/// Set up this core's timer (masked until armed).
pub fn init_this_core() {
    if DEADLINE_MODE.load(Ordering::Relaxed) {
        apic::timer_tsc_deadline(TIMER_VECTOR);
        apic::set_tsc_deadline(0);
    } else {
        apic::timer_one_shot(TIMER_VECTOR);
        apic::set_one_shot(0);
    }
}

/// Fire this core's timer interrupt when the TSC reaches `tsc`.
pub fn set_deadline(tsc: u64) {
    if DEADLINE_MODE.load(Ordering::Relaxed) {
        apic::set_tsc_deadline(tsc);
    } else {
        let delta = tsc.saturating_sub(rdtsc());
        let fixed = u128::from(APIC_PER_TSC_FIXED.load(Ordering::Relaxed));
        let count = (u128::from(delta) * fixed) >> 32;
        apic::set_one_shot(count.clamp(1, u128::from(u32::MAX)) as u32);
    }
}

/// Self-test: how many timer interrupts each core takes, and their period.
pub const SELFTEST_TICKS: u32 = 20;
pub const SELFTEST_PERIOD_NS: u64 = 1_000_000;

/// The timer interrupt. Runs with interrupts off, like all kernel entries,
/// and takes the big kernel lock.
pub fn interrupt() {
    let now = rdtsc();
    let _kernel = sync::BKL.lock();
    let me = percpu::this();
    let deadline = me.timer_next_deadline.load(Ordering::Relaxed);
    let late = tsc_to_ns(now.saturating_sub(deadline));
    me.timer_worst_late_ns.fetch_max(late, Ordering::Relaxed);
    let ticks = me.timer_ticks.fetch_add(1, Ordering::Relaxed) + 1;
    if ticks < SELFTEST_TICKS {
        let next = deadline + ns_to_tsc(SELFTEST_PERIOD_NS);
        me.timer_next_deadline.store(next, Ordering::Relaxed);
        set_deadline(next);
    }
}

/// Run the timer self-test on this core: take [`SELFTEST_TICKS`] interrupts
/// one period apart, then stop the timer (cores are tickless when idle).
pub fn selftest_this_core() {
    let me = percpu::this();
    let first = rdtsc() + ns_to_tsc(SELFTEST_PERIOD_NS);
    me.timer_next_deadline.store(first, Ordering::Relaxed);
    set_deadline(first);
    while me.timer_ticks.load(Ordering::Relaxed) < SELFTEST_TICKS {
        cpu::enable_interrupts_and_halt();
    }
}

/// Spin until `done` returns true; after `seconds`, call `timed_out`.
pub fn wait_for(seconds: u64, done: impl Fn() -> bool, timed_out: impl FnOnce()) {
    let end = rdtsc() + tsc_hz() * seconds;
    while !done() {
        if rdtsc() > end {
            timed_out();
            panic!("timed out after {seconds}s");
        }
        core::hint::spin_loop();
    }
}

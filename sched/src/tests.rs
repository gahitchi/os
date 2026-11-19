// SPDX-License-Identifier: GPL-3.0-only
//! Host tests of the refill rule: hand-written cases, then long random runs
//! that check the guarantee in every window.

use std::vec::Vec;

use super::*;

/// A small deterministic random-number generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

#[test]
fn unconfigured_has_nothing() {
    let b = Budget::EMPTY;
    assert_eq!(b.available(1000), 0);
}

#[test]
fn round_robin_never_runs_out() {
    let mut b = Budget::EMPTY;
    b.configure(100, 100, 0);
    b.charge(0, 1_000_000);
    assert_eq!(b.available(5), u64::MAX);
}

#[test]
fn used_time_returns_one_period_later() {
    let mut b = Budget::EMPTY;
    b.configure(30, 100, 0);
    assert_eq!(b.available(0), 30);
    b.charge(10, 20);
    assert_eq!(b.available(10), 10);
    b.charge(40, 10);
    assert_eq!(b.available(50), 0);
    assert_eq!(b.ready_time(), 110);
    assert_eq!(b.available(110), 20);
    assert_eq!(b.available(140), 30);
    assert_eq!(b.total(), 30);
}

#[test]
fn overrun_is_paid_back() {
    let mut b = Budget::EMPTY;
    b.configure(10, 100, 0);
    b.configure(10, 100, 0);
    b.charge(0, 4);
    b.charge(4, 8); // only 6 were usable: the timer fired 2 late
    // 2 of debt: taken off the next usable time.
    assert_eq!(b.available(50), 0);
    assert_eq!(b.available(100), 2);
    assert_eq!(b.available(104), 8);
}

#[test]
fn yield_waits_for_the_next_period() {
    let mut b = Budget::EMPTY;
    b.configure(50, 100, 0);
    b.yield_rest(0, 10);
    assert_eq!(b.available(10), 0);
    assert_eq!(b.available(99), 0);
    assert_eq!(b.available(100), 50);
}

#[test]
fn a_full_list_merges_later() {
    let mut b = Budget::EMPTY;
    b.configure(100, 1000, 0);
    for i in 0..20 {
        b.charge(i * 3, 1);
    }
    assert!(b.count as usize <= MAX_REFILLS);
    assert_eq!(b.total(), 100);
}

/// Run a thread that wants random amounts of time at random moments, the
/// way the kernel would schedule it, and return the stretches it ran.
fn simulate(seed: u64, budget: u64, period: u64, overrun: u64) -> Vec<(u64, u64)> {
    let mut rng = Rng(seed | 1);
    let mut b = Budget::EMPTY;
    b.configure(budget, period, 0);
    let mut now = 0;
    let mut runs = Vec::new();
    for _ in 0..3000 {
        // Sometimes sleep, sometimes stay ready.
        if rng.below(3) == 0 {
            now += rng.below(period * 2);
        }
        let avail = b.available(now);
        if avail == 0 {
            now = now.max(b.ready_time());
            continue;
        }
        let want = 1 + rng.below(budget);
        let late = if overrun > 0 {
            rng.below(overrun + 1)
        } else {
            0
        };
        // The timer stops it at the end of what is available, maybe late.
        let used = if want <= avail { want } else { avail + late };
        runs.push((now, used));
        match rng.below(4) {
            0 => b.yield_rest(now, now + used),
            _ => b.charge(now, used),
        }
        assert_eq!(b.total(), budget);
        assert!(b.count as usize <= MAX_REFILLS);
        now += used;
        // Preempted for a while sometimes.
        if rng.below(2) == 0 {
            now += rng.below(budget);
        }
    }
    runs
}

/// The most time used in any window of `period`. The maximum is reached by
/// a window that starts when some run starts.
fn worst_window(runs: &[(u64, u64)], period: u64) -> u64 {
    let mut worst = 0;
    for &(s, _) in runs {
        let end = s + period;
        let used: u64 = runs
            .iter()
            .map(|&(a, u)| {
                let b = a + u;
                b.min(end).saturating_sub(a.max(s))
            })
            .sum();
        worst = worst.max(used);
    }
    worst
}

#[test]
fn never_more_than_the_budget_in_any_window() {
    for seed in 1..60 {
        let (budget, period) =
            [(10, 100), (1, 7), (500, 1000), (99, 100), (3, 1000)][seed as usize % 5];
        let runs = simulate(seed, budget, period, 0);
        let worst = worst_window(&runs, period);
        assert!(
            worst <= budget,
            "seed {seed}: {worst} used in a window of {period}, budget {budget}"
        );
    }
}

#[test]
fn overruns_are_paid_back_within_a_window() {
    // A late timer can let a thread run a little long once, but that time
    // is taken off later: never more than budget plus one overrun.
    for seed in 1..40 {
        let (budget, period, overrun) = (20, 100, 5);
        let runs = simulate(seed, budget, period, overrun);
        let worst = worst_window(&runs, period);
        assert!(worst <= budget + overrun, "seed {seed}: {worst}");
    }
}

#[test]
fn a_busy_thread_gets_its_whole_budget() {
    // Always ready, never preempted: it must get budget per period in the
    // long run, not less (refills must not drift away).
    let mut b = Budget::EMPTY;
    let (budget, period) = (25, 100);
    b.configure(budget, period, 0);
    let mut now = 0;
    let mut ran = 0;
    while now < 100_000 {
        let avail = b.available(now);
        if avail == 0 {
            now = b.ready_time();
            continue;
        }
        let used = avail.min(7);
        b.charge(now, used);
        ran += used;
        now += used;
    }
    assert!(ran >= 1000 * budget - budget, "ran {ran}");
}

#[test]
fn late_timers_do_not_add_up() {
    // The timer always fires 3 ticks late: in the long run the thread must
    // still get no more than its budget per period.
    let mut b = Budget::EMPTY;
    let (budget, period, late) = (20, 100, 3);
    b.configure(budget, period, 0);
    let mut now = 0;
    let mut ran = 0;
    while now < 100_000 {
        let avail = b.available(now);
        if avail == 0 {
            now = b.ready_time();
            continue;
        }
        let used = avail + late;
        b.charge(now, used);
        ran += used;
        now += used;
    }
    assert!(
        ran <= 1000 * budget + budget + late,
        "ran {ran} in 1000 periods"
    );
}

#[test]
fn ready_time_counts_the_debt() {
    let mut b = Budget::EMPTY;
    b.configure(10, 100, 0);
    b.charge(0, 4);
    b.charge(4, 9); // 6 usable, 3 of debt
    assert_eq!(b.available(100), 1);
    assert_eq!(b.ready_time(), 100);
    b.charge(100, 1); // pays the 3 of debt too
    assert_eq!(b.debt, 0);
    assert_eq!(b.available(101), 0);
    assert_eq!(b.ready_time(), 104);
    assert_eq!(b.available(104), 6);
    // Debt alone can hold back usable time: 6 usable, 6 owed.
    let mut d = Budget::EMPTY;
    d.configure(12, 100, 0);
    d.charge(0, 18); // 12 usable, the timer was 6 late
    assert_eq!(d.debt, 6);
    assert_eq!(d.available(100), 6);
    assert_eq!(d.ready_time(), 100);
}

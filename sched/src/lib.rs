// SPDX-License-Identifier: GPL-3.0-only
//! Time budgets for the Alaya kernel: the sporadic-refill rule of a
//! scheduling context (decisions 11 and 53).
//!
//! A scheduling context grants "`budget` every `period`". Its budget is
//! kept as a short list of *refills*: amounts of time, each usable from a
//! given moment on. The amounts always add up to the whole budget.
//!
//! The rule (as in seL4's MCS kernel, after Sprunt's sporadic server):
//! time used in one stretch that starts at `s` comes back as a new refill at
//! `s + period`. So in any window of length `period`, a thread can never run
//! for more than `budget`, whatever it does: block, wake, yield, call
//! servers that run on its budget. That is the guarantee audio and frame
//! threads rely on under stress (battle test, decision 11).
//!
//! When `budget == period` the context is a plain round-robin one: it is
//! never out of budget and nothing is recorded.
//!
//! Time is in TSC ticks. All operations are O([`MAX_REFILLS`]).

#![no_std]

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;

/// Most refills one budget keeps. When a new refill does not fit, it is
/// merged into a later one, which only ever delays time (safe).
pub const MAX_REFILLS: usize = 8;

/// Some time that becomes usable at `time`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Refill {
    pub time: u64,
    pub amount: u64,
}

/// A budget. All zero means "not configured": never runnable.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Budget {
    pub budget: u64,
    pub period: u64,
    /// Time used beyond what was usable (a late timer), still to be paid:
    /// it is taken off the next time that becomes usable.
    pub debt: u64,
    /// Refills in time order; `refills[..count]` are in use.
    pub count: u64,
    pub refills: [Refill; MAX_REFILLS],
}

impl Budget {
    pub const EMPTY: Budget = Budget {
        budget: 0,
        period: 0,
        debt: 0,
        count: 0,
        refills: [Refill { time: 0, amount: 0 }; MAX_REFILLS],
    };

    /// Start over with `budget` every `period`, all of it usable at `now`.
    /// Callers check `0 < budget <= period`.
    pub fn configure(&mut self, budget: u64, period: u64, now: u64) {
        *self = Budget::EMPTY;
        self.budget = budget;
        self.period = period;
        self.count = 1;
        self.refills[0] = Refill {
            time: now,
            amount: budget,
        };
    }

    pub fn is_configured(&self) -> bool {
        self.period != 0
    }

    /// Never runs out (budget equals period).
    pub fn is_round_robin(&self) -> bool {
        self.is_configured() && self.budget >= self.period
    }

    fn live(&self) -> &[Refill] {
        &self.refills[..self.count as usize]
    }

    /// Time usable from `now` on, without a break.
    pub fn available(&self, now: u64) -> u64 {
        if !self.is_configured() {
            return 0;
        }
        if self.is_round_robin() {
            return u64::MAX;
        }
        let usable: u64 = self
            .live()
            .iter()
            .take_while(|r| r.time <= now)
            .map(|r| r.amount)
            .sum();
        usable.saturating_sub(self.debt)
    }

    /// When some budget is next usable (only meaningful when
    /// [`available`](Self::available) is 0).
    pub fn ready_time(&self) -> u64 {
        let mut sum = 0;
        for r in self.live() {
            sum += r.amount;
            if sum > self.debt {
                return r.time;
            }
        }
        u64::MAX
    }

    fn pop(&mut self) -> Refill {
        let r = self.refills[0];
        self.refills.copy_within(1..self.count as usize, 0);
        self.count -= 1;
        self.refills[self.count as usize] = Refill::default();
        r
    }

    /// Insert a refill in time order. If the list is full, the amount is
    /// added to the next later refill (or the last one, made at least as
    /// late), so time is only ever delayed.
    fn push(&mut self, r: Refill) {
        if r.amount == 0 {
            return;
        }
        let n = self.count as usize;
        let at = self
            .live()
            .iter()
            .position(|x| x.time > r.time)
            .unwrap_or(n);
        if n == MAX_REFILLS {
            let into = at.min(n - 1);
            let x = &mut self.refills[into];
            x.amount += r.amount;
            x.time = x.time.max(r.time);
            return;
        }
        self.refills.copy_within(at..n, at + 1);
        self.refills[at] = r;
        self.count += 1;
    }

    /// Take `amount` from the refills usable at `start`. Returns how much
    /// was there.
    fn consume(&mut self, start: u64, mut amount: u64) -> u64 {
        let mut taken = 0;
        while amount > 0 && self.count > 0 && self.refills[0].time <= start {
            let head = &mut self.refills[0];
            let t = head.amount.min(amount);
            head.amount -= t;
            amount -= t;
            taken += t;
            if head.amount == 0 {
                self.pop();
            }
        }
        taken
    }

    /// Record that the budget was used without a break from `start` for
    /// `used` ticks. `start` must be a moment when that time was available
    /// (the kernel stops the thread at `start + available(start)`).
    ///
    /// If the thread ran longer (a late timer: the overrun), the extra time
    /// becomes a debt, taken off the next time that becomes usable, so in
    /// the long run a thread never gets more than its budget per period.
    pub fn charge(&mut self, start: u64, used: u64) {
        if !self.is_configured() || self.is_round_robin() || used == 0 {
            return;
        }
        // Old debt is paid first, out of the same refills.
        let total = used + self.debt;
        let taken = self.consume(start, total);
        // A debt larger than half the budget (a timer far too late) is
        // partly forgiven, so the thread always gets to run again.
        self.debt = (total - taken).min(self.budget / 2);
        self.push(Refill {
            time: start + self.period,
            amount: taken,
        });
        debug_assert!(self.total() == self.budget);
    }

    /// The thread is done until its next period: charge what it used since
    /// `start`, then move everything still usable at `now` a period on from
    /// when it first became usable.
    pub fn yield_rest(&mut self, start: u64, now: u64) {
        if !self.is_configured() || self.is_round_robin() {
            return;
        }
        self.charge(start, now.saturating_sub(start));
        if self.count == 0 || self.refills[0].time > now {
            return;
        }
        let first = self.refills[0].time;
        let rest = self.consume(now, u64::MAX);
        self.push(Refill {
            time: first + self.period,
            amount: rest,
        });
        debug_assert!(self.total() == self.budget);
    }

    /// Sum of all refills; always equals `budget` once configured.
    pub fn total(&self) -> u64 {
        self.live().iter().map(|r| r.amount).sum()
    }
}

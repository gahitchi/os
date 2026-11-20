// SPDX-License-Identifier: GPL-3.0-only
//! The core-type helper (decision 54): which kind of core each time budget
//! should use, on processors with performance and efficiency cores.
//!
//! The kernel reads each core's kind from the processor (Intel's hybrid
//! information, CPUID leaf 0x1A) and hands the list to the first program.
//! This helper turns "what kind of work is this" into a core type for
//! `sc_configure`: games and what the person is using right now go to the
//! performance cores, background work to the efficiency cores, and system
//! parts may use any core. On a processor with one kind of core (or in a
//! virtual machine, which shows one kind) every budget may use any core.
//!
//! Intel's finer, changing hints (the hardware feedback table, which says
//! how fast and how efficient each core is right now) need the kernel to
//! read them; they come with the real-hardware milestones.

use crate::abi::core_type;

/// What a budget is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    /// A game, or anything with frame deadlines.
    Game,
    /// What the person is looking at and typing into.
    Interactive,
    /// Work nobody waits for: indexing, downloads, backups.
    Background,
    /// System parts: drivers, keepers, servers.
    System,
}

/// How many cores of each kind there are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cores {
    pub performance: u32,
    pub efficiency: u32,
    /// Cores of no particular kind (a processor with one kind of core).
    pub plain: u32,
}

impl Cores {
    /// Count the cores of each kind, from the kernel's list.
    pub fn from_types(types: &[u8]) -> Cores {
        let mut c = Cores::default();
        for &t in types {
            match u64::from(t) {
                core_type::PERFORMANCE => c.performance += 1,
                core_type::EFFICIENCY => c.efficiency += 1,
                _ => c.plain += 1,
            }
        }
        c
    }

    /// Both kinds are there.
    pub fn hybrid(&self) -> bool {
        self.performance > 0 && self.efficiency > 0
    }
}

/// The core type a budget for `work` should use.
pub fn choose(cores: &Cores, work: Work) -> u64 {
    if !cores.hybrid() {
        return core_type::ANY;
    }
    match work {
        Work::Game | Work::Interactive => core_type::PERFORMANCE,
        Work::Background => core_type::EFFICIENCY,
        Work::System => core_type::ANY,
    }
}

/// Configure scheduling context `sc` with `budget_us` every `period_us`
/// on the cores right for `work`.
pub fn configure(
    control: u64,
    sc: u64,
    budget_us: u64,
    period_us: u64,
    cores: &Cores,
    work: Work,
) -> Result<u64, crate::Error> {
    let t = choose(cores, work);
    crate::sc_configure(control, sc, budget_us, period_us, t, 0)?;
    Ok(t)
}

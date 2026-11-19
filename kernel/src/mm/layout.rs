// SPDX-License-Identifier: GPL-3.0-only
//! The kernel's virtual address layout (4-level paging, 48-bit addresses).
//!
//! | Range | What |
//! | --- | --- |
//! | lower half | free; user programs from milestone 2 |
//! | `HHDM` offset (Limine's, normally `0xffff_8000_0000_0000`) | direct map of physical memory |
//! | `0xffff_ff00_0000_0000` | per-core stacks, each below an unmapped guard page |
//! | `0xffff_ffff_8000_0000` | the kernel image |

use crate::percpu::MAX_CPUS;

pub const PAGE_SIZE: u64 = 4096;

/// Start of the per-core stack area (PML4 slot 510).
pub const STACKS_BASE: u64 = 0xffff_ff00_0000_0000;
/// Address space reserved per core for its stacks.
pub const STACKS_PER_CPU: u64 = 1 << 20;
/// Space per stack inside a core's area: a guard page, then the stack.
const STACK_SLOT: u64 = 128 * 1024;

/// The stacks every core gets, in slot order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stack {
    /// Where the core runs kernel code.
    Kernel,
    /// Taken by the CPU on a double fault, so a kernel stack overflow can
    /// still be reported.
    DoubleFault,
    Nmi,
    MachineCheck,
}

impl Stack {
    pub const ALL: [Stack; 4] = [
        Stack::Kernel,
        Stack::DoubleFault,
        Stack::Nmi,
        Stack::MachineCheck,
    ];

    pub const fn size(self) -> u64 {
        match self {
            Stack::Kernel => 64 * 1024,
            _ => 16 * 1024,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Stack::Kernel => "kernel",
            Stack::DoubleFault => "double-fault",
            Stack::Nmi => "NMI",
            Stack::MachineCheck => "machine-check",
        }
    }

    const fn slot(self) -> u64 {
        self as u64
    }

    /// Lowest mapped address of this stack for `cpu`; the guard page is
    /// the page right below it.
    pub const fn bottom(self, cpu: u32) -> u64 {
        STACKS_BASE + cpu as u64 * STACKS_PER_CPU + self.slot() * STACK_SLOT + PAGE_SIZE
    }

    /// Initial stack pointer (stacks grow down).
    pub const fn top(self, cpu: u32) -> u64 {
        self.bottom(cpu) + self.size()
    }
}

/// If `addr` is in a stack guard page, whose stack it guards.
pub fn guard_page_owner(addr: u64) -> Option<(u32, &'static str)> {
    let end = STACKS_BASE + MAX_CPUS as u64 * STACKS_PER_CPU;
    if !(STACKS_BASE..end).contains(&addr) {
        return None;
    }
    let off = addr - STACKS_BASE;
    let cpu = (off / STACKS_PER_CPU) as u32;
    let slot = (off % STACKS_PER_CPU) / STACK_SLOT;
    let within = off % STACK_SLOT;
    let stack = *Stack::ALL.get(slot as usize)?;
    (within < PAGE_SIZE).then_some((cpu, stack.name()))
}

// SPDX-License-Identifier: GPL-3.0-only
//! x86-64 support.

pub mod apic;
pub mod cpu;
pub mod gdt;
pub mod idt;
pub mod legacy;
pub mod port;
pub mod trap;

pub const NAME: &str = "x86-64";

/// Stop taking interrupts on this core.
#[inline]
pub fn disable_interrupts() {
    // SAFETY: `cli` only changes the interrupt flag of this core.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

/// Hint to the core that it is spinning on a lock.
#[inline]
pub fn spin_hint() {
    core::hint::spin_loop();
}

/// Park this core for good.
pub fn halt_forever() -> ! {
    disable_interrupts();
    loop {
        // SAFETY: `hlt` with interrupts off simply stops the core (an NMI
        // can still wake it, and the loop halts it again).
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

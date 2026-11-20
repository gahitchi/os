// SPDX-License-Identifier: GPL-3.0-only
//! The PC's pieces of the deliberate-fault self-tests (`crate::selftest`).

/// Machine code for "return" (`ret`).
pub const RETURN: [u8; 1] = [0xc3];

/// Run an instruction that does not exist (`ud2`).
pub fn invalid_instruction() {
    // SAFETY: deliberately raises #UD.
    unsafe { core::arch::asm!("ud2") };
}

/// Call code at `addr` (a user page, to check the kernel cannot run it).
pub fn call(addr: u64) {
    // SAFETY: deliberately faults with SMEP on.
    unsafe { core::arch::asm!("call {}", in(reg) addr, clobber_abi("C")) };
}

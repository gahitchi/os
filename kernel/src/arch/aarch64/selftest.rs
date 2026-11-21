// SPDX-License-Identifier: GPL-3.0-only
//! ARM's pieces of the deliberate-fault self-tests (`crate::selftest`).

/// Machine code for "return" (`ret`).
pub const RETURN: [u8; 4] = 0xd65f_03c0u32.to_le_bytes();

/// Run an instruction that does not exist (`udf`).
pub fn invalid_instruction() {
    // SAFETY: deliberately raises an undefined-instruction exception.
    unsafe { core::arch::asm!("udf #0") };
}

/// Call code at `addr` (a user page, to check the kernel cannot run it).
pub fn call(addr: u64) {
    // SAFETY: deliberately faults: user pages are never executable by the
    // kernel (PXN).
    unsafe { core::arch::asm!("blr {}", in(reg) addr, clobber_abi("C")) };
}

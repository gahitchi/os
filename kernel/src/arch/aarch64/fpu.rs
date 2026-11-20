// SPDX-License-Identifier: GPL-3.0-only
//! The floating-point and vector registers of user threads (FP and NEON:
//! 32 registers of 128 bits, plus the status and control registers).
//!
//! As on a PC, every thread has a save area and the kernel saves and
//! restores it on every switch between threads. The kernel itself is
//! built without floating point (the `-softfloat` target), so it never
//! touches these registers otherwise. SVE is not used yet.

use core::sync::atomic::AtomicU64;

/// Bytes saved per thread: 32 x 16 bytes, then FPSR and FPCR.
pub(crate) static SIZE: AtomicU64 = AtomicU64::new(528);

/// Let user mode use the FP and vector registers (CPACR_EL1.FPEN). Every
/// core while starting.
pub fn init_this_core() {
    set_sysreg!("cpacr_el1", sysreg!("cpacr_el1") | 3 << 20);
    super::cpu::isb();
}

pub fn describe() -> (&'static str, u64, u64) {
    ("NEON", 0, 528)
}

/// A fresh save area: all registers zero, default rounding.
pub fn init_area(area: u64) {
    // SAFETY: a thread's save area, reached through the direct map.
    unsafe { core::ptr::write_bytes(area as *mut u8, 0, 528) };
}

/// Save this core's FP state into the area at virtual address `area`.
pub fn save(area: u64) {
    // SAFETY: a 64-byte-aligned save area of at least 528 bytes.
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "stp q0, q1, [{a}, #0]",
            "stp q2, q3, [{a}, #32]",
            "stp q4, q5, [{a}, #64]",
            "stp q6, q7, [{a}, #96]",
            "stp q8, q9, [{a}, #128]",
            "stp q10, q11, [{a}, #160]",
            "stp q12, q13, [{a}, #192]",
            "stp q14, q15, [{a}, #224]",
            "stp q16, q17, [{a}, #256]",
            "stp q18, q19, [{a}, #288]",
            "stp q20, q21, [{a}, #320]",
            "stp q22, q23, [{a}, #352]",
            "stp q24, q25, [{a}, #384]",
            "stp q26, q27, [{a}, #416]",
            "stp q28, q29, [{a}, #448]",
            "stp q30, q31, [{a}, #480]",
            "mrs {t}, fpsr",
            "str {t}, [{a}, #512]",
            "mrs {t}, fpcr",
            "str {t}, [{a}, #520]",
            a = in(reg) area,
            t = out(reg) _,
            options(nostack, preserves_flags)
        )
    };
}

/// Load the FP state from the area at virtual address `area`.
pub fn restore(area: u64) {
    // SAFETY: as for `save`; the area was filled by `save` or `init_area`.
    unsafe {
        core::arch::asm!(
            ".arch_extension fp",
            ".arch_extension simd",
            "ldp q0, q1, [{a}, #0]",
            "ldp q2, q3, [{a}, #32]",
            "ldp q4, q5, [{a}, #64]",
            "ldp q6, q7, [{a}, #96]",
            "ldp q8, q9, [{a}, #128]",
            "ldp q10, q11, [{a}, #160]",
            "ldp q12, q13, [{a}, #192]",
            "ldp q14, q15, [{a}, #224]",
            "ldp q16, q17, [{a}, #256]",
            "ldp q18, q19, [{a}, #288]",
            "ldp q20, q21, [{a}, #320]",
            "ldp q22, q23, [{a}, #352]",
            "ldp q24, q25, [{a}, #384]",
            "ldp q26, q27, [{a}, #416]",
            "ldp q28, q29, [{a}, #448]",
            "ldp q30, q31, [{a}, #480]",
            "ldr {t}, [{a}, #512]",
            "msr fpsr, {t}",
            "ldr {t}, [{a}, #520]",
            "msr fpcr, {t}",
            a = in(reg) area,
            t = out(reg) _,
            options(nostack, preserves_flags)
        )
    };
}

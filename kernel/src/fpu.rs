// SPDX-License-Identifier: GPL-3.0-only
//! The FPU and vector registers of user threads (x87, SSE, AVX, AVX-512).
//!
//! Games and media need them, so every thread gets a save area (in its
//! thread object) and the kernel saves and restores it on every switch
//! between threads ("eager" switching, as Linux does; lazy switching leaks
//! state through speculative execution). The kernel itself is built
//! without floating point, so it never touches these registers.
//!
//! XSAVE is used where the CPU has it; otherwise FXSAVE (x87 and SSE only,
//! as on QEMU's plain emulated CPU).

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::arch::cpu::{self, cpuid};
use crate::obj::FPU_BYTES;

static XSAVE: AtomicBool = AtomicBool::new(false);
/// The state components saved (XCR0).
static MASK: AtomicU64 = AtomicU64::new(0);
static SIZE: AtomicU64 = AtomicU64::new(512);

const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
const CR4_OSXSAVE: u64 = 1 << 18;

/// x87, SSE, AVX, and the three AVX-512 parts.
const WANTED: u64 = 0b1110_0111;

/// Turn the FPU on for this core. Every core calls it while starting.
pub fn init_this_core() {
    let has_xsave = cpuid(1, 0).ecx & (1 << 26) != 0;
    // SAFETY: only enables the FPU and SSE for user mode.
    unsafe {
        cpu::write_cr0((cpu::cr0() | CR0_MP) & !(CR0_EM | CR0_TS));
        let mut cr4 = cpu::cr4() | CR4_OSFXSR | CR4_OSXMMEXCPT;
        if has_xsave {
            cr4 |= CR4_OSXSAVE;
        }
        cpu::write_cr4(cr4);
    }
    if has_xsave {
        let r = cpuid(0xd, 0);
        let supported = u64::from(r.eax) | u64::from(r.edx) << 32;
        let mut mask = supported & WANTED;
        // AVX-512 needs all three parts, and AVX before it.
        if mask & 0b1110_0000 != 0b1110_0000 || mask & 0b100 == 0 {
            mask &= 0b111;
        }
        // SAFETY: the mask only names components the CPU supports.
        unsafe { xsetbv(mask) };
        let size = u64::from(cpuid(0xd, 0).ebx);
        assert!(
            size <= FPU_BYTES,
            "the FPU state needs {size} bytes, more than the {FPU_BYTES} a thread has"
        );
        MASK.store(mask, Ordering::Relaxed);
        SIZE.store(size, Ordering::Relaxed);
        XSAVE.store(true, Ordering::Relaxed);
    }
}

pub fn describe() -> (&'static str, u64, u64) {
    if XSAVE.load(Ordering::Relaxed) {
        (
            "XSAVE",
            MASK.load(Ordering::Relaxed),
            SIZE.load(Ordering::Relaxed),
        )
    } else {
        ("FXSAVE", 0b11, 512)
    }
}

unsafe fn xsetbv(v: u64) {
    // SAFETY: the caller checked the value.
    unsafe {
        core::arch::asm!("xsetbv", in("ecx") 0, in("eax") v as u32, in("edx") (v >> 32) as u32,
                         options(nostack, preserves_flags))
    };
}

/// Fill a fresh save area with the state a new thread starts with: all
/// registers zero, exceptions masked (FCW 0x37f, MXCSR 0x1f80).
pub fn init_area(area: u64) {
    let p = area as *mut u8;
    // SAFETY: `area` is a thread's save area, reached through the direct
    // map, FPU_BYTES long.
    unsafe {
        core::ptr::write_bytes(p, 0, 576);
        (p as *mut u16).write(0x37f);
        (p.add(24) as *mut u32).write(0x1f80);
        (p.add(28) as *mut u32).write(0xffff); // MXCSR mask
        if XSAVE.load(Ordering::Relaxed) {
            // XSTATE_BV: x87 and SSE hold the values above, the rest is
            // in its initial state.
            (p.add(512) as *mut u64).write(0b11);
        }
    }
}

/// Save this core's FPU state into the area at virtual address `area`.
pub fn save(area: u64) {
    // SAFETY: a 64-byte-aligned save area of FPU_BYTES.
    unsafe {
        if XSAVE.load(Ordering::Relaxed) {
            let m = MASK.load(Ordering::Relaxed);
            core::arch::asm!("xsave64 [{}]", in(reg) area, in("eax") m as u32, in("edx") (m >> 32) as u32,
                             options(nostack, preserves_flags));
        } else {
            core::arch::asm!("fxsave64 [{}]", in(reg) area, options(nostack, preserves_flags));
        }
    }
}

/// Load the FPU state from the area at virtual address `area`.
pub fn restore(area: u64) {
    // SAFETY: as for `save`; the area was filled by `save` or `init_area`.
    unsafe {
        if XSAVE.load(Ordering::Relaxed) {
            let m = MASK.load(Ordering::Relaxed);
            core::arch::asm!("xrstor64 [{}]", in(reg) area, in("eax") m as u32, in("edx") (m >> 32) as u32,
                             options(nostack, preserves_flags));
        } else {
            core::arch::asm!("fxrstor64 [{}]", in(reg) area, options(nostack, preserves_flags));
        }
    }
}

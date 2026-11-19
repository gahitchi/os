// SPDX-License-Identifier: GPL-3.0-only
//! Control registers, model-specific registers, CPUID and the time-stamp
//! counter.

use core::arch::asm;
use core::arch::x86_64::{__cpuid_count, CpuidResult};

pub mod msr {
    pub const APIC_BASE: u32 = 0x1b;
    pub const TSC_DEADLINE: u32 = 0x6e0;
    pub const EFER: u32 = 0xc000_0080;
    pub const GS_BASE: u32 = 0xc000_0101;
}

pub const EFER_NXE: u64 = 1 << 11;
pub const CR0_WP: u64 = 1 << 16;
pub const CR4_PGE: u64 = 1 << 7;
pub const CR4_LA57: u64 = 1 << 12;
pub const RFLAGS_IF: u64 = 1 << 9;

/// # Safety
/// The MSR must exist on this CPU; reading some MSRs has side effects.
#[inline]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let (lo, hi): (u32, u32);
    unsafe {
        asm!("rdmsr", in("ecx") msr, out("eax") lo, out("edx") hi, options(nomem, nostack, preserves_flags))
    };
    (u64::from(hi) << 32) | u64::from(lo)
}

/// # Safety
/// The MSR must exist; writing an MSR can change any CPU behaviour.
#[inline]
pub unsafe fn wrmsr(msr: u32, value: u64) {
    unsafe {
        asm!("wrmsr", in("ecx") msr, in("eax") value as u32, in("edx") (value >> 32) as u32,
             options(nostack, preserves_flags))
    };
}

#[inline]
pub fn cpuid(leaf: u32, subleaf: u32) -> CpuidResult {
    __cpuid_count(leaf, subleaf)
}

pub fn max_leaf() -> u32 {
    cpuid(0, 0).eax
}

pub fn max_extended_leaf() -> u32 {
    cpuid(0x8000_0000, 0).eax
}

/// The features the kernel cares about, read once per core.
#[derive(Clone, Copy, Debug, Default)]
pub struct Features {
    pub x2apic: bool,
    pub tsc_deadline: bool,
    pub invariant_tsc: bool,
    pub gib_pages: bool,
    pub hypervisor: bool,
}

pub fn features() -> Features {
    let l1 = cpuid(1, 0);
    let ext = if max_extended_leaf() >= 0x8000_0001 {
        cpuid(0x8000_0001, 0).edx
    } else {
        0
    };
    let power = if max_extended_leaf() >= 0x8000_0007 {
        cpuid(0x8000_0007, 0).edx
    } else {
        0
    };
    Features {
        x2apic: l1.ecx & (1 << 21) != 0,
        tsc_deadline: l1.ecx & (1 << 24) != 0,
        hypervisor: l1.ecx & (1 << 31) != 0,
        gib_pages: ext & (1 << 26) != 0,
        invariant_tsc: power & (1 << 8) != 0,
    }
}

/// The CPU's brand string, for example "Intel(R) Core(TM) i7-...".
pub fn brand(buf: &mut [u8; 48]) -> &str {
    if max_extended_leaf() < 0x8000_0004 {
        return "unknown CPU";
    }
    for (i, leaf) in (0x8000_0002..=0x8000_0004).enumerate() {
        let r = cpuid(leaf, 0);
        for (j, word) in [r.eax, r.ebx, r.ecx, r.edx].iter().enumerate() {
            buf[i * 16 + j * 4..i * 16 + j * 4 + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(48);
    core::str::from_utf8(&buf[..end]).unwrap_or("?").trim()
}

#[inline]
pub fn rdtsc() -> u64 {
    // SAFETY: RDTSC only reads the time-stamp counter.
    unsafe { core::arch::x86_64::_rdtsc() }
}

macro_rules! cr {
    ($read:ident, $write:ident, $reg:literal) => {
        #[inline]
        pub fn $read() -> u64 {
            let v: u64;
            // SAFETY: reading a control register has no side effects.
            unsafe { asm!(concat!("mov {}, ", $reg), out(reg) v, options(nomem, nostack, preserves_flags)) };
            v
        }

        /// # Safety
        /// Changing a control register changes how the CPU runs everything.
        #[inline]
        #[allow(dead_code)]
        pub unsafe fn $write(v: u64) {
            unsafe { asm!(concat!("mov ", $reg, ", {}"), in(reg) v, options(nostack, preserves_flags)) };
        }
    };
}

cr!(cr0, write_cr0, "cr0");
cr!(cr3, write_cr3, "cr3");
cr!(cr4, write_cr4, "cr4");

#[inline]
pub fn cr2() -> u64 {
    let v: u64;
    // SAFETY: reading CR2 has no side effects.
    unsafe { asm!("mov {}, cr2", out(reg) v, options(nomem, nostack, preserves_flags)) };
    v
}

#[inline]
pub fn rflags() -> u64 {
    let v: u64;
    // SAFETY: reads the flags register through the stack.
    unsafe { asm!("pushfq", "pop {}", out(reg) v, options(nomem, preserves_flags)) };
    v
}

#[inline]
pub fn interrupts_enabled() -> bool {
    rflags() & RFLAGS_IF != 0
}

#[inline]
pub fn enable_interrupts() {
    // SAFETY: only changes this core's interrupt flag.
    unsafe { asm!("sti", options(nomem, nostack)) };
}

/// Enable interrupts and halt in one step, so a wake-up cannot slip in
/// between (STI takes effect only after the next instruction).
#[inline]
pub fn enable_interrupts_and_halt() {
    // SAFETY: waits for the next interrupt on this core.
    unsafe { asm!("sti", "hlt", "cli", options(nomem, nostack)) };
}

/// The kernel's per-core data pointer lives in GS base.
#[inline]
pub fn set_gs_base(value: u64) {
    // SAFETY: GS base is only used by the kernel for per-core data.
    unsafe { wrmsr(msr::GS_BASE, value) };
}

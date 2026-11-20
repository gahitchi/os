// SPDX-License-Identifier: GPL-3.0-only
//! Reading and writing ARM system registers, and what the CPU says about
//! itself.

/// Read system register `$reg`.
#[macro_export]
macro_rules! sysreg {
    ($reg:literal) => {{
        let v: u64;
        // SAFETY: reading a system register has no side effects.
        unsafe {
            core::arch::asm!(concat!("mrs {}, ", $reg), out(reg) v, options(nomem, nostack, preserves_flags))
        };
        v
    }};
}

/// Write system register `$reg`. The caller makes sure the value is right
/// for everything that depends on it.
#[macro_export]
macro_rules! set_sysreg {
    ($reg:literal, $v:expr) => {{
        let v: u64 = $v;
        // SAFETY: see the macro's documentation; the callers check.
        unsafe { core::arch::asm!(concat!("msr ", $reg, ", {}"), in(reg) v, options(nostack, preserves_flags)) };
    }};
}

/// Instruction barrier: later instructions see earlier system register
/// writes.
#[inline]
pub fn isb() {
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("isb", options(nostack, preserves_flags)) };
}

/// Wait until earlier memory accesses (and table writes) are visible to
/// every core and the table walkers.
#[inline]
pub fn dsb_ish() {
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb ish", options(nostack, preserves_flags)) };
}

/// This core's affinity (MPIDR without the flag bits): its hardware id.
pub fn mpidr() -> u64 {
    sysreg!("mpidr_el1") & 0xff_00ff_ffff
}

/// The CPU's own description of itself, for the boot log.
pub struct Features {
    pub implementer: u8,
    pub part: u16,
    /// ARMv8.1 Privileged Access Never (the kernel cannot touch program
    /// memory by accident).
    pub pan: bool,
    /// 16-bit address-space tags (ASIDs).
    pub asid16: bool,
    /// Physical address bits.
    pub pa_bits: u32,
    /// Large System Extensions (single-instruction atomics, ARMv8.1).
    pub atomics: bool,
}

pub fn features() -> Features {
    let midr = sysreg!("midr_el1");
    let mmfr0 = sysreg!("id_aa64mmfr0_el1");
    let mmfr1 = sysreg!("id_aa64mmfr1_el1");
    let isar0 = sysreg!("id_aa64isar0_el1");
    let pa_bits = match mmfr0 & 0xf {
        0 => 32,
        1 => 36,
        2 => 40,
        3 => 42,
        4 => 44,
        5 => 48,
        _ => 52,
    };
    Features {
        implementer: (midr >> 24) as u8,
        part: ((midr >> 4) & 0xfff) as u16,
        pan: (mmfr1 >> 20) & 0xf != 0,
        asid16: (mmfr0 >> 4) & 0xf == 2,
        pa_bits,
        atomics: (isar0 >> 20) & 0xf >= 2,
    }
}

/// A readable name for the CPU core design.
pub fn name(f: &Features) -> &'static str {
    match (f.implementer, f.part) {
        (0x41, 0xd03) => "Arm Cortex-A53",
        (0x41, 0xd04) => "Arm Cortex-A35",
        (0x41, 0xd05) => "Arm Cortex-A55",
        (0x41, 0xd07) => "Arm Cortex-A57",
        (0x41, 0xd08) => "Arm Cortex-A72",
        (0x41, 0xd09) => "Arm Cortex-A73",
        (0x41, 0xd0b) => "Arm Cortex-A76",
        (0x41, 0xd0c) => "Arm Neoverse N1",
        (0x41, 0xd40) => "Arm Neoverse V1",
        (0x41, 0xd49) => "Arm Neoverse N2",
        (0x00, 0x051) => "QEMU max CPU",
        (0x61, _) => "Apple core",
        (0x51, _) => "Qualcomm core",
        _ => "unknown ARM core",
    }
}

/// Full-system barrier.
#[inline]
pub fn dsb_sy() {
    // SAFETY: a barrier.
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

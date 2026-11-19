// SPDX-License-Identifier: GPL-3.0-only
//! The local APIC of each core: interrupt acknowledgement, inter-processor
//! interrupts and the timer.
//!
//! Works in both modes. x2APIC (registers as MSRs) is used whenever the
//! bootloader turned it on, which it does when the CPU supports it; machines
//! with many cores need it. Otherwise the classic xAPIC is reached through
//! its memory-mapped registers.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::cpu::{msr, rdmsr, wrmsr};

pub const SPURIOUS_VECTOR: u8 = 0xff;

mod reg {
    pub const ID: u32 = 0x20;
    pub const TPR: u32 = 0x80;
    pub const EOI: u32 = 0xb0;
    pub const SVR: u32 = 0xf0;
    pub const ESR: u32 = 0x280;
    pub const ICR_LOW: u32 = 0x300;
    pub const ICR_HIGH: u32 = 0x310;
    pub const LVT_TIMER: u32 = 0x320;
    pub const LVT_LINT0: u32 = 0x350;
    pub const LVT_LINT1: u32 = 0x360;
    pub const LVT_ERROR: u32 = 0x370;
    pub const TIMER_INITIAL: u32 = 0x380;
    pub const TIMER_CURRENT: u32 = 0x390;
    pub const TIMER_DIVIDE: u32 = 0x3e0;
}

const LVT_MASKED: u32 = 1 << 16;
const TIMER_ONE_SHOT: u32 = 0;
const TIMER_TSC_DEADLINE: u32 = 0b10 << 17;
const ICR_DELIVERY_PENDING: u32 = 1 << 12;
const ICR_NMI: u32 = 0b100 << 8;
const ICR_ASSERT: u32 = 1 << 14;

static X2APIC: AtomicBool = AtomicBool::new(false);
/// Virtual address of the xAPIC registers (unused in x2APIC mode).
static MMIO: AtomicU64 = AtomicU64::new(0);

/// Physical address of this core's xAPIC registers.
pub fn mmio_physical_base() -> u64 {
    // SAFETY: IA32_APIC_BASE exists on every x86-64 CPU.
    unsafe { rdmsr(msr::APIC_BASE) & 0x000f_ffff_ffff_f000 }
}

/// Choose the mode for all cores. Called once on the bootstrap core.
pub fn set_mode(x2apic: bool, mmio_virtual: u64) {
    X2APIC.store(x2apic, Ordering::Relaxed);
    MMIO.store(mmio_virtual, Ordering::Relaxed);
}

pub fn is_x2apic() -> bool {
    X2APIC.load(Ordering::Relaxed)
}

fn read(reg: u32) -> u32 {
    if is_x2apic() {
        // SAFETY: x2APIC registers are MSRs 0x800 + offset / 16.
        unsafe { rdmsr(0x800 + (reg >> 4)) as u32 }
    } else {
        // SAFETY: the register page is mapped uncached at MMIO.
        unsafe { read_volatile((MMIO.load(Ordering::Relaxed) + u64::from(reg)) as *const u32) }
    }
}

fn write(reg: u32, value: u32) {
    if is_x2apic() {
        // SAFETY: as in `read`.
        unsafe { wrmsr(0x800 + (reg >> 4), u64::from(value)) }
    } else {
        // SAFETY: as in `read`.
        unsafe {
            write_volatile(
                (MMIO.load(Ordering::Relaxed) + u64::from(reg)) as *mut u32,
                value,
            )
        }
    }
}

/// Set up this core's local APIC: enabled, nothing masked by priority,
/// legacy interrupt pins and the timer masked.
pub fn init_this_core() {
    write(reg::TPR, 0);
    write(reg::LVT_TIMER, LVT_MASKED);
    write(reg::LVT_LINT0, LVT_MASKED);
    write(reg::LVT_LINT1, LVT_MASKED);
    write(reg::LVT_ERROR, LVT_MASKED);
    // Clear any stale error status (needs a write before the read).
    write(reg::ESR, 0);
    let _ = read(reg::ESR);
    write(reg::SVR, (1 << 8) | u32::from(SPURIOUS_VECTOR));
    eoi();
}

/// This core's APIC id.
pub fn id() -> u32 {
    if is_x2apic() {
        read(reg::ID)
    } else {
        read(reg::ID) >> 24
    }
}

/// Acknowledge the interrupt being handled.
pub fn eoi() {
    write(reg::EOI, 0);
}

fn send(dest: u32, low: u32) {
    if is_x2apic() {
        // SAFETY: the x2APIC ICR is one 64-bit MSR, 0x830.
        unsafe { wrmsr(0x830, (u64::from(dest) << 32) | u64::from(low)) };
    } else {
        write(reg::ICR_HIGH, dest << 24);
        write(reg::ICR_LOW, low);
        while read(reg::ICR_LOW) & ICR_DELIVERY_PENDING != 0 {
            core::hint::spin_loop();
        }
    }
}

/// Send an NMI to one core. Used to stop the other cores on a panic.
pub fn send_nmi(dest: u32) {
    send(dest, ICR_NMI | ICR_ASSERT);
}

/// Send a fixed interrupt to one core.
#[allow(dead_code)]
pub fn send_ipi(dest: u32, vector: u8) {
    send(dest, ICR_ASSERT | u32::from(vector));
}

/// Put the timer in TSC-deadline mode on `vector`. It fires when the TSC
/// reaches the value written with [`set_tsc_deadline`].
pub fn timer_tsc_deadline(vector: u8) {
    write(reg::LVT_TIMER, TIMER_TSC_DEADLINE | u32::from(vector));
    // Intel SDM: a fence keeps the deadline write after the mode switch.
    // SAFETY: MFENCE has no side effects.
    unsafe { core::arch::asm!("mfence", options(nostack, preserves_flags)) };
}

/// Arm the TSC-deadline timer; 0 disarms it.
pub fn set_tsc_deadline(tsc: u64) {
    // SAFETY: only called after `timer_tsc_deadline` on a core with the feature.
    unsafe { wrmsr(msr::TSC_DEADLINE, tsc) };
}

/// Put the timer in one-shot count-down mode on `vector`, divide by 16.
pub fn timer_one_shot(vector: u8) {
    write(reg::TIMER_DIVIDE, 0b0011);
    write(reg::LVT_TIMER, TIMER_ONE_SHOT | u32::from(vector));
}

/// Start a one-shot count-down; 0 stops it.
pub fn set_one_shot(count: u32) {
    write(reg::TIMER_INITIAL, count);
}

pub fn timer_current() -> u32 {
    read(reg::TIMER_CURRENT)
}

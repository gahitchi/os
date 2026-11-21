// SPDX-License-Identifier: GPL-3.0-only
//! The processor's protections (decision 77): safety switches, tagged
//! memory maps, and guards against speculative-execution leaks.
//!
//! - **Safety switches.** SMEP stops the kernel from running code in a
//!   program's memory; SMAP stops it from reading or writing a program's
//!   memory (the kernel reaches program memory only through its own direct
//!   map, after checking the program's page tables, so it never needs to);
//!   UMIP stops programs from reading the kernel's descriptor tables.
//! - **Tagged memory maps (PCID).** Each address space has a tag, so a
//!   switch between programs keeps the TLB entries of both. `vspace.rs`
//!   keeps track of which tags each core must flush before using again.
//! - **Speculation guards** (the Spectre family):
//!   - enhanced IBRS where the CPU has it: what a program teaches the
//!     branch predictor cannot steer the kernel;
//!   - IBPB when a core switches to another program's address space: one
//!     program cannot steer another's predictions;
//!   - the return-stack buffer is refilled on those switches;
//!   - VERW clears the CPU's internal buffers on the way back to user mode
//!     on CPUs affected by MDS;
//!   - indexes a program hands the kernel are checked with a speculation
//!     barrier after the bounds check ([`barrier`]).
//!
//! Every switch is turned on only if the CPU reports it.

use core::arch::{asm, global_asm};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::cpu::{self, cpuid};

pub const CR4_UMIP: u64 = 1 << 11;
pub const CR4_PCIDE: u64 = 1 << 17;
pub const CR4_SMEP: u64 = 1 << 20;
pub const CR4_SMAP: u64 = 1 << 21;

const MSR_SPEC_CTRL: u32 = 0x48;
const MSR_PRED_CMD: u32 = 0x49;
const MSR_ARCH_CAPABILITIES: u32 = 0x10a;

const SPEC_CTRL_IBRS: u64 = 1 << 0;
const PRED_CMD_IBPB: u64 = 1 << 0;

const ARCH_RDCL_NO: u64 = 1 << 0;
const ARCH_IBRS_ALL: u64 = 1 << 1;
const ARCH_MDS_NO: u64 = 1 << 5;

/// What this machine has and what the kernel turned on, as
/// `alaya_abi::protect` bits (also handed to the first program).
pub(crate) static ON: AtomicU64 = AtomicU64::new(0);
static SMAP: AtomicBool = AtomicBool::new(false);
static IBPB: AtomicBool = AtomicBool::new(false);

/// Read by the return-to-user paths: non-zero means "clear the CPU's
/// buffers with VERW before going back to user mode".
#[unsafe(no_mangle)]
static mut alaya_verw_on_exit: u8 = 0;

/// A selector VERW can name (the kernel data segment).
#[unsafe(no_mangle)]
static alaya_verw_selector: u16 = super::gdt::KERNEL_DATA;

/// Find out what the CPU offers. Called once, on the bootstrap core, before
/// any core turns anything on.
pub fn detect() {
    use alaya_abi::protect as p;
    let max = cpu::max_leaf();
    let (b7, c7, d7) = if max >= 7 {
        let r = cpuid(7, 0);
        (r.ebx, r.ecx, r.edx)
    } else {
        (0, 0, 0)
    };
    let amd = if cpu::max_extended_leaf() >= 0x8000_0008 {
        cpuid(0x8000_0008, 0).ebx
    } else {
        0
    };
    let mut on = 0;
    if b7 & (1 << 7) != 0 {
        on |= p::SMEP;
    }
    if b7 & (1 << 20) != 0 {
        on |= p::SMAP;
    }
    if c7 & (1 << 2) != 0 {
        on |= p::UMIP;
    }
    if cpuid(1, 0).ecx & (1 << 17) != 0 {
        on |= p::PCID;
    }
    // IBPB: Intel's SPEC_CTRL bit, or AMD's own.
    let ibrs_ibpb = d7 & (1 << 26) != 0;
    if ibrs_ibpb || amd & (1 << 12) != 0 {
        on |= p::IBPB;
    }
    let arch = if d7 & (1 << 29) != 0 {
        rdmsr_safe(MSR_ARCH_CAPABILITIES).unwrap_or(0)
    } else {
        0
    };
    // Enhanced IBRS: Intel's ARCH_CAPABILITIES bit, or AMD's "IBRS always
    // on" (CPUID 0x8000_0008 EBX bit 16 with automatic IBRS).
    if (ibrs_ibpb && arch & ARCH_IBRS_ALL != 0) || amd & (1 << 16) != 0 {
        on |= p::EIBRS;
    }
    if d7 & (1 << 10) != 0 && arch & ARCH_MDS_NO == 0 {
        on |= p::VERW;
    }
    if arch & ARCH_RDCL_NO != 0 || is_amd() {
        on |= p::NO_MELTDOWN;
    }
    ON.store(on, Ordering::Relaxed);
    SMAP.store(on & p::SMAP != 0, Ordering::Relaxed);
    IBPB.store(on & p::IBPB != 0, Ordering::Relaxed);
    // SAFETY: written once, before any core returns to user mode.
    unsafe { *core::ptr::addr_of_mut!(alaya_verw_on_exit) = u8::from(on & p::VERW != 0) };
}

fn is_amd() -> bool {
    let r = cpuid(0, 0);
    // "AuthenticAMD"
    r.ebx == 0x6874_7541 && r.edx == 0x6974_6e65 && r.ecx == 0x444d_4163
}

/// The protections in use, as `alaya_abi::protect` bits.
pub fn enabled() -> u64 {
    ON.load(Ordering::Relaxed)
}

pub fn pcid() -> bool {
    enabled() & alaya_abi::protect::PCID != 0
}

/// Turn the protections on for this core. Called by every core while it
/// starts, after it loaded the kernel's page tables (CR3's low bits are
/// zero then, as turning on PCID requires).
pub fn init_this_core() {
    use alaya_abi::protect as p;
    let on = enabled();
    let mut cr4 = cpu::cr4();
    for (bit, flag) in [
        (p::SMEP, CR4_SMEP),
        (p::SMAP, CR4_SMAP),
        (p::UMIP, CR4_UMIP),
        (p::PCID, CR4_PCIDE),
    ] {
        if on & bit != 0 {
            cr4 |= flag;
        }
    }
    if on & p::PCID != 0 {
        // Turning PCID on requires the current tag to be 0.
        // SAFETY: the kernel's tables, tag 0.
        unsafe { cpu::write_cr3(cpu::cr3() & !0xfff) };
    }
    // SAFETY: the kernel never touches user memory through user addresses
    // and never runs user code, so these switches only stop mistakes.
    unsafe { cpu::write_cr4(cr4) };
    if on & p::EIBRS != 0 {
        // Set once; with enhanced IBRS the bit protects from then on.
        let _ = wrmsr_safe(MSR_SPEC_CTRL, SPEC_CTRL_IBRS);
    }
}

/// Print what the protections are, at boot.
pub fn report() {
    use alaya_abi::protect as p;
    let on = enabled();
    let yes = |b: bool| if b { "yes" } else { "no" };
    let has = |bit| yes(on & bit != 0);
    crate::kprintln!(
        "protect: SMEP {}, SMAP {}, UMIP {}, tagged memory maps (PCID) {}",
        has(p::SMEP),
        has(p::SMAP),
        has(p::UMIP),
        has(p::PCID)
    );
    crate::kprintln!(
        "protect: speculation guards: enhanced IBRS {}, IBPB on program switch {}, return-stack refill yes, buffer clearing (MDS) {}, bounds-check barriers yes",
        has(p::EIBRS),
        has(p::IBPB),
        if on & p::VERW != 0 {
            "yes"
        } else {
            "not needed or not offered"
        }
    );
    if on & p::NO_MELTDOWN == 0 {
        crate::kprintln!(
            "protect: this CPU does not say it is safe from Meltdown; kernel page-table isolation is not built yet"
        );
    }
}

/// Clear the alignment-check flag (SMAP's "allow access" switch) on entry
/// from an interrupt: user mode may have left it set.
#[inline]
pub fn on_interrupt_entry() {
    if SMAP.load(Ordering::Relaxed) {
        // SAFETY: CLAC only clears RFLAGS.AC.
        unsafe { asm!("clac", options(nomem, nostack)) };
    }
}

/// The core is about to run another program's code: stop what the last
/// program taught the branch predictors from steering it.
pub fn on_space_switch() {
    if IBPB.load(Ordering::Relaxed) {
        // SAFETY: IBPB only drops branch predictions.
        let _ = wrmsr_safe(MSR_PRED_CMD, PRED_CMD_IBPB);
    }
    fill_return_stack();
}

/// Overwrite the return-stack buffer with harmless entries, so returns
/// cannot speculate to addresses another program planted.
#[inline(never)]
fn fill_return_stack() {
    // 32 calls, each to the next instruction after a speculation trap, then
    // drop the 32 return addresses from the stack.
    // SAFETY: balanced: the stack pointer ends where it started.
    unsafe {
        asm!(
            ".rept 32",
            "call 2f",
            "3: pause",
            "lfence",
            "jmp 3b",
            "2:",
            ".endr",
            "add rsp, 32 * 8",
            options(nomem)
        )
    };
}

/// Stop speculation past a bounds check on a value a program chose.
#[inline(always)]
pub fn barrier() {
    // SAFETY: LFENCE only orders instructions.
    unsafe { asm!("lfence", options(nomem, nostack, preserves_flags)) };
}

// MSR access that survives a #GP (an MSR the CPU or the hypervisor does not
// have). The trap handler sees the fault at the labelled instruction and
// returns past it with rax = 1 (see [`fixup`]).
global_asm!(
    r#"
    .section .text.msr_safe, "ax"
    .global alaya_rdmsr_safe
alaya_rdmsr_safe:
    mov ecx, edi
    xor eax, eax
    .global alaya_rdmsr_insn
alaya_rdmsr_insn:
    rdmsr
    .global alaya_rdmsr_after
alaya_rdmsr_after:
    shl rdx, 32
    mov edx, eax
    mov [rsi], rdx
    xor eax, eax
    ret
    .global alaya_wrmsr_safe
alaya_wrmsr_safe:
    mov ecx, edi
    mov eax, esi
    mov rdx, rsi
    shr rdx, 32
    .global alaya_wrmsr_insn
alaya_wrmsr_insn:
    wrmsr
    xor eax, eax
    .global alaya_wrmsr_after
alaya_wrmsr_after:
    ret
    "#
);

unsafe extern "C" {
    fn alaya_rdmsr_safe(msr: u32, out: *mut u64) -> u64;
    fn alaya_wrmsr_safe(msr: u32, value: u64) -> u64;
    static alaya_rdmsr_insn: u8;
    static alaya_rdmsr_after: u8;
    static alaya_wrmsr_insn: u8;
    static alaya_wrmsr_after: u8;
}

/// Read an MSR that may not exist.
pub fn rdmsr_safe(msr: u32) -> Option<u64> {
    let mut v = 0;
    // SAFETY: a missing MSR faults and the fault is skipped (see `fixup`).
    (unsafe { alaya_rdmsr_safe(msr, &mut v) } == 0).then_some(v)
}

/// Write an MSR that may not exist. Returns false if it does not.
pub fn wrmsr_safe(msr: u32, value: u64) -> bool {
    // SAFETY: as for `rdmsr_safe`; the callers write only documented
    // values to protection MSRs.
    unsafe { alaya_wrmsr_safe(msr, value) == 0 }
}

/// A #GP in the kernel at `rip`: if it is one of the safe MSR accesses,
/// return where to continue (with rax set to 1 by the caller).
pub fn fixup(rip: u64) -> Option<u64> {
    let at = |s: &u8| s as *const u8 as u64;
    // SAFETY: only the addresses of the labels are used.
    unsafe {
        if rip == at(&alaya_rdmsr_insn) {
            // Skip the rest: the result is not stored.
            return Some(at(&alaya_wrmsr_after));
        }
        if rip == at(&alaya_wrmsr_insn) {
            return Some(at(&alaya_wrmsr_after));
        }
        let _ = at(&alaya_rdmsr_after);
    }
    None
}

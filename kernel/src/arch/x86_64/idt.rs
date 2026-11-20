// SPDX-License-Identifier: GPL-3.0-only
//! The interrupt descriptor table and the entry stubs for all 256 vectors.
//!
//! Every vector has a 16-byte stub that pushes an error code (zero when the
//! CPU does not push one) and the vector number, then jumps to a common path
//! that saves all general registers into a [`TrapFrame`] and calls
//! [`super::trap::dispatch`]. One IDT is shared by all cores.
//!
//! An interrupt from user mode swaps GS base on the way in and out, so the
//! kernel always finds its per-core data at GS and a program's own GS base
//! is kept.

use core::arch::{asm, global_asm};
use core::mem::size_of;
use core::ptr::addr_of;

use super::gdt::{IST_DOUBLE_FAULT, IST_MACHINE_CHECK, IST_NMI, KERNEL_CODE};

/// Registers saved on entry, in the order the common stub pushes them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TrapFrame {
    pub r15: u64,
    pub r14: u64,
    pub r13: u64,
    pub r12: u64,
    pub r11: u64,
    pub r10: u64,
    pub r9: u64,
    pub r8: u64,
    pub rbp: u64,
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub rcx: u64,
    pub rbx: u64,
    pub rax: u64,
    pub vector: u64,
    pub error: u64,
    // Pushed by the CPU.
    pub rip: u64,
    pub cs: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub ss: u64,
}

// Vectors for which the CPU pushes an error code: #DF, #TS, #NP, #SS, #GP,
// #PF, #AC, #CP, #VC, #SX.
global_asm!(
    r#"
    .section .text.trap, "ax"
    .global alaya_trap_stubs
    .p2align 4
alaya_trap_stubs:
    .set vec, 0
    .rept 256
    .p2align 4
    .if vec == 8 || vec == 10 || vec == 11 || vec == 12 || vec == 13 || vec == 14 || vec == 17 || vec == 21 || vec == 29 || vec == 30
    .else
    push 0
    .endif
    push vec
    jmp alaya_trap_common
    .set vec, vec + 1
    .endr

alaya_trap_common:
    cld
    // From user mode (CS RPL 3), swap in the kernel's GS base. The frame
    // holds the vector, the error code, then RIP and CS.
    test byte ptr [rsp + 24], 3
    jz 1f
    swapgs
1:
    push rax
    push rbx
    push rcx
    push rdx
    push rsi
    push rdi
    push rbp
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15
    mov rdi, rsp
    // Keep the stack 16-byte aligned for the Rust handler.
    mov rbp, rsp
    and rsp, -16
    call {dispatch}
    mov rsp, rbp
    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rbp
    pop rdi
    pop rsi
    pop rdx
    pop rcx
    pop rbx
    pop rax
    add rsp, 16
    test byte ptr [rsp + 8], 3
    jz 2f
    cmp byte ptr [rip + alaya_verw_on_exit], 0
    je 3f
    verw word ptr [rip + alaya_verw_selector]
3:
    swapgs
2:
    iretq
    "#,
    dispatch = sym super::trap::dispatch,
);

unsafe extern "C" {
    static alaya_trap_stubs: [u8; 256 * 16];
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Gate {
    offset_low: u16,
    selector: u16,
    ist: u8,
    attributes: u8,
    offset_mid: u16,
    offset_high: u32,
    reserved: u32,
}

impl Gate {
    const EMPTY: Gate = Gate {
        offset_low: 0,
        selector: 0,
        ist: 0,
        attributes: 0,
        offset_mid: 0,
        offset_high: 0,
        reserved: 0,
    };

    fn interrupt(handler: u64, ist: u8) -> Gate {
        Gate {
            offset_low: handler as u16,
            selector: KERNEL_CODE,
            ist,
            // Present, ring 0, 64-bit interrupt gate (interrupts stay off).
            attributes: 0x8e,
            offset_mid: (handler >> 16) as u16,
            offset_high: (handler >> 32) as u32,
            reserved: 0,
        }
    }
}

#[repr(C, align(16))]
pub(crate) struct Idt([Gate; 256]);

pub(crate) static mut IDT: Idt = Idt([Gate::EMPTY; 256]);

/// Fill the shared IDT. Called once, on the bootstrap core, before any core
/// loads it.
pub fn init() {
    // The stubs are 256 blocks of 16 bytes, see the assembly above.
    let base = addr_of!(alaya_trap_stubs) as u64;
    for vector in 0..256 {
        let ist = match vector {
            2 => IST_NMI,
            8 => IST_DOUBLE_FAULT,
            18 => IST_MACHINE_CHECK,
            _ => 0,
        };
        // SAFETY: runs once before any core loads the table.
        unsafe {
            (*core::ptr::addr_of_mut!(IDT)).0[vector] =
                Gate::interrupt(base + vector as u64 * 16, ist);
        }
    }
}

/// Load the shared IDT on this core.
pub fn load() {
    #[repr(C, packed)]
    struct Pointer {
        limit: u16,
        base: u64,
    }
    let ptr = Pointer {
        limit: (size_of::<Idt>() - 1) as u16,
        base: addr_of!(IDT) as u64,
    };
    // SAFETY: the table was filled by `init` and lives forever.
    unsafe { asm!("lidt [{}]", in(reg) &ptr, options(readonly, nostack, preserves_flags)) };
}

// SPDX-License-Identifier: GPL-3.0-only
//! Entering and leaving user mode on a PC: the `syscall` entry, the way
//! back (`sysret` or `iretq`), and a thread's registers.
//!
//! Every entry from user mode saves the thread's registers into a
//! [`TrapFrame`] at the top of the core's kernel stack; the generic code
//! (`crate::syscall`) copies them into the thread with [`save`], does its
//! work, and loads the next thread's registers with [`load`]. The return
//! uses `sysret` when the thread entered with `syscall` (so `rcx` and `r11`
//! are free), otherwise `iretq`.
//!
//! System call registers (the ABI, `alaya-abi`): the call number goes in
//! `rax` and the status comes back there; `rdi` holds the handle or badge,
//! `rsi` the message info or label, `rdx r10 r8 r9 r12 r13` the six data
//! words, `r14` the capability slots of a transfer and `r15` the reply
//! handle.

use core::arch::global_asm;

use super::cpu::{self, msr};
use super::gdt::{KERNEL_CODE, USER_CODE, USER_DATA};
use super::idt::TrapFrame;

/// User addresses are below this; the last page stays unmapped so that a
/// `syscall` at the very top cannot return to a non-canonical address.
pub const USER_END: u64 = 0x0000_7fff_ffff_f000;

/// Marks a frame built by the `syscall` entry (in place of a vector).
const SYSCALL_MARK: u64 = 0x100;

global_asm!(
    r#"
    .section .text.syscall, "ax"
    .global alaya_syscall_entry
alaya_syscall_entry:
    swapgs
    mov gs:[{user_rsp}], rsp
    mov rsp, gs:[{kernel_rsp}]
    push {user_data}
    push qword ptr gs:[{user_rsp}]
    push r11
    push {user_code}
    push rcx
    push 0
    push {mark}
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
    cld
    mov rdi, rsp
    call {entry}
    test rax, rax
    jz alaya_user_return
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
    mov rcx, [rsp + 16]
    mov r11, [rsp + 32]
    mov rsp, [rsp + 40]
    cmp byte ptr [rip + alaya_verw_on_exit], 0
    je 3f
    verw word ptr [rip + alaya_verw_selector]
3:
    swapgs
    sysretq

    .global alaya_user_return
alaya_user_return:
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
    cmp byte ptr [rip + alaya_verw_on_exit], 0
    je 4f
    verw word ptr [rip + alaya_verw_selector]
4:
    swapgs
    iretq
    "#,
    user_rsp = const crate::percpu::USER_SP_OFFSET,
    kernel_rsp = const crate::percpu::KERNEL_STACK_OFFSET,
    user_data = const USER_DATA as u64,
    user_code = const USER_CODE as u64,
    mark = const SYSCALL_MARK,
    entry = sym crate::syscall::syscall_entry,
);

unsafe extern "C" {
    fn alaya_syscall_entry();
    fn alaya_user_return();
}

/// RFLAGS bits cleared on entry: interrupts, direction, trap, alignment
/// check. The kernel runs with interrupts off.
const FMASK: u64 = (1 << 9) | (1 << 10) | (1 << 8) | (1 << 18);

/// RFLAGS bits a thread may have: the arithmetic flags, trap, direction
/// and alignment check. Interrupts are always on.
const USER_FLAGS: u64 = 0x1 | 0x4 | 0x10 | 0x40 | 0x80 | 0x100 | 0x400 | 0x800 | 0x40000;

/// Turn on `syscall` on this core.
pub fn init_this_core() {
    // SAFETY: these MSRs only configure the syscall instruction; the
    // selectors match the GDT layout (see gdt.rs).
    unsafe {
        let star = (u64::from(USER_DATA & !3) - 8) << 48 | u64::from(KERNEL_CODE) << 32;
        cpu::wrmsr(msr::STAR, star);
        cpu::wrmsr(msr::LSTAR, alaya_syscall_entry as *const () as u64);
        cpu::wrmsr(msr::FMASK, FMASK);
        let efer = cpu::rdmsr(msr::EFER);
        cpu::wrmsr(msr::EFER, efer | cpu::EFER_SCE);
    }
}

/// Length of the `syscall` instruction, for restarts.
const SYSCALL_LEN: u64 = 2;

/// A thread's registers as user mode sees them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Regs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub fs_base: u64,
    pub gs_base: u64,
}

impl Regs {
    /// A thread about to start at `pc` with stack `sp`, its first two
    /// arguments, and its thread-local storage pointer.
    pub fn start(pc: u64, sp: u64, arg0: u64, arg1: u64, tls: u64) -> Self {
        Regs {
            rip: pc,
            rsp: sp,
            rdi: arg0,
            rsi: arg1,
            fs_base: tls,
            rflags: cpu::RFLAGS_IF | 2,
            ..Regs::default()
        }
    }

    /// The system call number; the status on the way out.
    pub fn call_nr(&self) -> u64 {
        self.rax
    }

    pub fn status(&self) -> u64 {
        self.rax
    }

    pub fn set_status(&mut self, v: u64) {
        self.rax = v;
    }

    /// The handle a call names, or the badge a message arrives with.
    pub fn arg0(&self) -> u64 {
        self.rdi
    }

    pub fn set_arg0(&mut self, v: u64) {
        self.rdi = v;
    }

    /// The message info, or the label of a call on a kernel object.
    pub fn arg1(&self) -> u64 {
        self.rsi
    }

    pub fn set_arg1(&mut self, v: u64) {
        self.rsi = v;
    }

    /// Data words of a message, in register order.
    pub fn words(&self) -> [u64; 6] {
        [self.rdx, self.r10, self.r8, self.r9, self.r12, self.r13]
    }

    pub fn set_words(&mut self, w: [u64; 6]) {
        [self.rdx, self.r10, self.r8, self.r9, self.r12, self.r13] = w;
    }

    /// The four result words of a call on a kernel object.
    pub fn set_results(&mut self, r: [u64; 4]) {
        [self.rdx, self.r10, self.r8, self.r9] = r;
    }

    /// Capability slots of a transfer: the sender's slot in the low half,
    /// the receiver's in the high half.
    pub fn cap_slots(&self) -> u64 {
        self.r14
    }

    /// The reply object handle of a receive.
    pub fn reply_handle(&self) -> u64 {
        self.r15
    }

    pub fn pc(&self) -> u64 {
        self.rip
    }

    pub fn sp(&self) -> u64 {
        self.rsp
    }

    /// Step back onto the system call instruction, so the thread makes the
    /// call again when it next runs.
    pub fn rewind_call(&mut self) {
        self.rip -= SYSCALL_LEN;
    }
}

/// Copy a user-mode frame into `r`.
pub fn save(r: &mut Regs, f: &TrapFrame) {
    (r.rax, r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.rbp) =
        (f.rax, f.rbx, f.rcx, f.rdx, f.rsi, f.rdi, f.rbp);
    (r.r8, r.r9, r.r10, r.r11, r.r12, r.r13, r.r14, r.r15) =
        (f.r8, f.r9, f.r10, f.r11, f.r12, f.r13, f.r14, f.r15);
    (r.rip, r.rflags, r.rsp) = (f.rip, f.rflags, f.rsp);
}

/// Copy registers `r` into a frame to return to user mode with.
pub fn load(r: &Regs, f: &mut TrapFrame) {
    (f.rax, f.rbx, f.rcx, f.rdx, f.rsi, f.rdi, f.rbp) =
        (r.rax, r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.rbp);
    (f.r8, f.r9, f.r10, f.r11, f.r12, f.r13, f.r14, f.r15) =
        (r.r8, r.r9, r.r10, r.r11, r.r12, r.r13, r.r14, r.r15);
    f.rip = r.rip;
    f.rsp = r.rsp;
    f.rflags = (r.rflags & USER_FLAGS) | cpu::RFLAGS_IF | 2;
    f.cs = u64::from(USER_CODE);
    f.ss = u64::from(USER_DATA);
}

/// Return to user mode with frame `f` from anywhere in the kernel, leaving
/// the current kernel stack behind.
pub fn return_to_user(f: &TrapFrame) -> ! {
    // SAFETY: the frame describes user mode; the return path pops it and
    // `iretq`s. The stack below it is never used again.
    unsafe {
        core::arch::asm!(
            "mov rsp, {f}",
            "jmp {ret}",
            f = in(reg) f as *const TrapFrame,
            ret = sym alaya_user_return,
            options(noreturn),
        )
    }
}

/// The user-mode thread-local storage bases of the thread about to run.
pub fn load_user_tls(r: &Regs) {
    // User mode cannot change these itself (FSGSBASE is off), so the
    // values this core last loaded are still there: skip equal ones.
    let held = &crate::percpu::this().user_tls;
    // SAFETY: user-mode segment bases; the user GS base sits in
    // KERNEL_GS_BASE until `swapgs` on the way out.
    unsafe {
        if held[0].swap(r.fs_base, core::sync::atomic::Ordering::Relaxed) != r.fs_base {
            cpu::wrmsr(msr::FS_BASE, r.fs_base);
        }
        if held[1].swap(r.gs_base, core::sync::atomic::Ordering::Relaxed) != r.gs_base {
            cpu::wrmsr(msr::KERNEL_GS_BASE, r.gs_base);
        }
    }
}

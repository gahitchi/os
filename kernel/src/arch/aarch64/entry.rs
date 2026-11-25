// SPDX-License-Identifier: GPL-3.0-only
//! Entering and leaving the kernel on ARM: the exception vectors, the way
//! back to user mode (`eret`), and a thread's registers.
//!
//! Every exception from user mode lands on this core's kernel stack (the
//! CPU switches to SP_EL1 by itself), saves all registers into a
//! [`TrapFrame`] at its top, and calls the shared code. The way back pops
//! the frame, which leaves SP_EL1 at the top of the stack again for the
//! next entry.
//!
//! Exceptions in the kernel are bugs (the kernel runs with interrupts off
//! except while it waits for one), so they switch to this core's emergency
//! stack, which works even when the kernel stack overflowed, and report.
//!
//! System call registers (the ABI, `alaya-abi`): `svc #0` with the call
//! number in `x8`, where the status comes back; `x0` holds the handle or
//! badge, `x1` the message info or label, `x2`-`x7` the six data words, `x9`
//! the capability slots of a transfer and `x10` the reply handle.

use core::arch::global_asm;

use crate::percpu::{KERNEL_STACK_OFFSET, USER_SP_OFFSET};

/// User addresses are below this (the same as on a PC).
pub const USER_END: u64 = 0x0000_7fff_ffff_f000;

/// Registers saved on entry.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct TrapFrame {
    pub x: [u64; 31],
    /// The stack pointer before the exception (user mode's SP_EL0).
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
    pub esr: u64,
    pub far: u64,
    /// 0: synchronous (system call, fault), 1: interrupt, 3: system error.
    pub kind: u64,
    _pad: u64,
}

const FRAME: usize = core::mem::size_of::<TrapFrame>();
const _: () = assert!(FRAME == 304 && FRAME.is_multiple_of(16));

macro_rules! save_frame {
    () => {
        r#"
    sub sp, sp, #304
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, x9, [sp, #64]
    stp x10, x11, [sp, #80]
    stp x12, x13, [sp, #96]
    stp x14, x15, [sp, #112]
    stp x16, x17, [sp, #128]
    stp x18, x19, [sp, #144]
    stp x20, x21, [sp, #160]
    stp x22, x23, [sp, #176]
    stp x24, x25, [sp, #192]
    stp x26, x27, [sp, #208]
    stp x28, x29, [sp, #224]
    mrs x0, sp_el0
    stp x30, x0, [sp, #240]
    mrs x0, elr_el1
    mrs x1, spsr_el1
    stp x0, x1, [sp, #256]
    mrs x0, esr_el1
    mrs x1, far_el1
    stp x0, x1, [sp, #272]
"#
    };
}

global_asm!(
    r#"
    .section .text.vectors, "ax"
    .balign 2048
    .global alaya_vectors
alaya_vectors:
    // From the kernel with SP_EL0 (never used).
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_fatal
    // From the kernel with SP_EL1.
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_irq
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_fatal
    // From user mode (64-bit).
    .balign 128
    b alaya_el0_sync
    .balign 128
    b alaya_el0_irq
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el0_serror
    // From user mode (32-bit): not supported.
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_fatal
    .balign 128
    b alaya_el1_fatal

alaya_el0_sync:
"#,
    save_frame!(),
    r#"
    mov x0, #0
    str x0, [sp, #288]
    mov x0, sp
    bl {from_user}
    b alaya_user_return

alaya_el0_irq:
"#,
    save_frame!(),
    r#"
    mov x0, #1
    str x0, [sp, #288]
    mov x0, sp
    bl {from_user}
    b alaya_user_return

alaya_el0_serror:
"#,
    save_frame!(),
    r#"
    mov x0, #3
    str x0, [sp, #288]
    mov x0, sp
    bl {from_user}
    b alaya_user_return

    .global alaya_user_return
alaya_user_return:
    // Table and run-queue writes made for this thread are visible to the
    // walker before it runs.
    dsb ish
    ldp x0, x1, [sp, #256]
    msr elr_el1, x0
    msr spsr_el1, x1
    ldr x0, [sp, #248]
    msr sp_el0, x0
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    add sp, sp, #304
    eret

    // An interrupt while the kernel waited for one: on the current stack.
alaya_el1_irq:
"#,
    save_frame!(),
    r#"
    add x0, sp, #304
    str x0, [sp, #248]
    mov x0, #1
    str x0, [sp, #288]
    mov x0, sp
    bl {kernel_irq}
    ldp x0, x1, [sp, #256]
    msr elr_el1, x0
    msr spsr_el1, x1
    ldp x0, x1, [sp, #0]
    ldp x2, x3, [sp, #16]
    ldp x4, x5, [sp, #32]
    ldp x6, x7, [sp, #48]
    ldp x8, x9, [sp, #64]
    ldp x10, x11, [sp, #80]
    ldp x12, x13, [sp, #96]
    ldp x14, x15, [sp, #112]
    ldp x16, x17, [sp, #128]
    ldp x18, x19, [sp, #144]
    ldp x20, x21, [sp, #160]
    ldp x22, x23, [sp, #176]
    ldp x24, x25, [sp, #192]
    ldp x26, x27, [sp, #208]
    ldp x28, x29, [sp, #224]
    ldr x30, [sp, #240]
    add sp, sp, #304
    eret

    // A fault in the kernel: a bug. Move to the emergency stack (the
    // kernel stack may be what overflowed) and report.
alaya_el1_fatal:
    msr sp_el0, x0
    mov x0, sp
    msr tpidrro_el0, x0
    mrs x0, tpidr_el1
    ldr x0, [x0, #{emergency}]
    mov sp, x0
"#,
    save_frame!(),
    r#"
    mrs x0, sp_el0
    str x0, [sp, #0]
    mrs x0, tpidrro_el0
    str x0, [sp, #248]
    mov x0, #0
    str x0, [sp, #288]
    mov x0, sp
    bl {kernel_fault}
0:  wfi
    b 0b
"#,
    from_user = sym super::trap::from_user,
    kernel_irq = sym super::trap::kernel_irq,
    kernel_fault = sym super::trap::kernel_fault,
    emergency = const USER_SP_OFFSET,
);

unsafe extern "C" {
    static alaya_vectors: u8;
    fn alaya_user_return();
}

/// Point this core's exception vectors at the kernel's.
pub fn init_this_core() {
    let v = core::ptr::addr_of!(alaya_vectors) as u64;
    set_sysreg!("vbar_el1", v);
    super::cpu::isb();
}

/// A thread's registers as user mode sees them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Regs {
    pub x: [u64; 31],
    pub sp: u64,
    pub pc: u64,
    pub pstate: u64,
    /// The thread pointer (TPIDR_EL0).
    pub tpidr: u64,
}

/// The condition flags, the only part of PSTATE a thread may set. It runs
/// in user mode with interrupts on.
const USER_PSTATE: u64 = 0xf000_0000;

impl Regs {
    /// A thread about to start at `pc` with stack `sp`, its first two
    /// arguments, and its thread-local storage pointer.
    pub fn start(pc: u64, sp: u64, arg0: u64, arg1: u64, tls: u64) -> Self {
        let mut r = Regs {
            pc,
            sp,
            tpidr: tls,
            ..Regs::default()
        };
        r.x[0] = arg0;
        r.x[1] = arg1;
        r
    }

    pub fn call_nr(&self) -> u64 {
        self.x[8]
    }

    pub fn status(&self) -> u64 {
        self.x[8]
    }

    pub fn set_status(&mut self, v: u64) {
        self.x[8] = v;
    }

    pub fn arg0(&self) -> u64 {
        self.x[0]
    }

    pub fn set_arg0(&mut self, v: u64) {
        self.x[0] = v;
    }

    pub fn arg1(&self) -> u64 {
        self.x[1]
    }

    pub fn set_arg1(&mut self, v: u64) {
        self.x[1] = v;
    }

    pub fn words(&self) -> [u64; 6] {
        self.x[2..8].try_into().unwrap()
    }

    pub fn set_words(&mut self, w: [u64; 6]) {
        self.x[2..8].copy_from_slice(&w);
    }

    pub fn set_results(&mut self, r: [u64; 4]) {
        self.x[2..6].copy_from_slice(&r);
    }

    pub fn cap_slots(&self) -> u64 {
        self.x[9]
    }

    pub fn reply_handle(&self) -> u64 {
        self.x[10]
    }

    pub fn pc(&self) -> u64 {
        self.pc
    }

    pub fn sp(&self) -> u64 {
        self.sp
    }

    /// Step back onto the `svc` instruction (the saved pc points after it).
    pub fn rewind_call(&mut self) {
        self.pc -= 4;
    }

    /// A Linux system call: its number (x8) and six arguments (x0 to x5).
    pub fn linux_call(&self) -> (u64, [u64; 6]) {
        (self.x[8], self.x[0..6].try_into().unwrap())
    }

    /// A Linux system call's result (x0); every other register is as it
    /// was, x8 (the call number, which sending used for its status) too.
    pub fn set_linux_result(&mut self, nr: u64, v: u64) {
        self.x[0] = v;
        self.x[8] = nr;
    }

    /// The thread pointer (TPIDR_EL0).
    pub fn set_tls(&mut self, v: u64) {
        self.tpidr = v;
    }

    pub fn jump(&mut self, pc: u64, sp: u64) {
        (self.pc, self.sp) = (pc, sp);
    }
}

/// Copy a user-mode frame into `r`. Runs on the core the thread ran on, so
/// its thread pointer is still in the register.
pub fn save(r: &mut Regs, f: &TrapFrame) {
    r.x = f.x;
    r.sp = f.sp;
    r.pc = f.pc;
    r.pstate = f.pstate;
    r.tpidr = sysreg!("tpidr_el0");
}

/// Copy registers `r` into a frame to return to user mode with.
pub fn load(r: &Regs, f: &mut TrapFrame) {
    f.x = r.x;
    f.sp = r.sp;
    f.pc = r.pc;
    f.pstate = r.pstate & USER_PSTATE;
}

/// Return to user mode with frame `f` from anywhere in the kernel, leaving
/// the current kernel stack behind.
pub fn return_to_user(f: &TrapFrame) -> ! {
    let me = crate::percpu::this();
    let top = me
        .kernel_stack_top
        .load(core::sync::atomic::Ordering::Relaxed);
    let at = top - FRAME as u64;
    let from = f as *const TrapFrame as u64;
    // `f` lives deeper on this same stack (or is already at the top), so
    // copying from the end backwards never reads what it overwrote. The
    // copy is done in assembly, without the stack: the top of the stack
    // may hold this function's own saved values, which a call to `memmove`
    // would overwrite and then reload.
    debug_assert!(from <= at);
    // SAFETY: the top of this core's kernel stack, where every entry from
    // user mode puts its frame; nothing on the stack is used again.
    unsafe {
        core::arch::asm!(
            "mov x2, #{frame}",
            "1:",
            "sub x2, x2, #16",
            "add x3, x0, x2",
            "add x4, x1, x2",
            "ldp x5, x6, [x3]",
            "stp x5, x6, [x4]",
            "cbnz x2, 1b",
            "mov sp, x1",
            "b {ret}",
            frame = const FRAME,
            ret = sym alaya_user_return,
            in("x0") from,
            in("x1") at,
            options(noreturn),
        )
    }
}

/// The thread pointer of the thread about to run.
pub fn load_user_tls(r: &Regs) {
    set_sysreg!("tpidr_el0", r.tpidr);
}

/// The offset of the kernel stack top in `PerCpu`, for the record (the
/// CPU keeps SP_EL1 itself).
#[allow(dead_code)]
const _STACK: usize = KERNEL_STACK_OFFSET;

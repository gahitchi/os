// SPDX-License-Identifier: GPL-3.0-only
//! Entering and leaving the kernel from user mode: system calls, and the
//! common path every entry from user mode takes.
//!
//! The kernel has one stack per core (as seL4 does), not one per thread.
//! Every entry from user mode saves the thread's registers into a
//! [`TrapFrame`] at the top of that stack and copies them into the thread
//! object; the kernel does its work under the big lock; then it picks the
//! thread to run next ([`leave`]), copies that thread's registers into the
//! frame and returns to user mode with them. A core with nothing to run
//! halts in [`idle`] until an interrupt brings work.
//!
//! The return uses `sysret` when the thread entered with `syscall` (so
//! `rcx` and `r11` are free), otherwise `iretq`. A call that was
//! interrupted (a long revoke) is restarted: the saved instruction pointer
//! moves back onto the `syscall` instruction.

use core::arch::global_asm;

use alaya_abi::{Error, Info, Kind, call, debug, rights};
use alaya_cap::Memory;

use crate::arch::cpu::{self, msr};
use crate::arch::gdt::{KERNEL_CODE, USER_CODE, USER_DATA};
use crate::arch::idt::TrapFrame;
use crate::caps::{self, Outcome, Phys};
use crate::obj::{self, Tcb, state, tflag};
use crate::percpu::{KERNEL_RSP_OFFSET, USER_RSP_OFFSET};
use crate::sync::{BKL, BklGuard, KernelState};
use crate::{ipc, kprint, kprintln, percpu, sched, vspace};

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
    swapgs
    iretq
    "#,
    user_rsp = const USER_RSP_OFFSET,
    kernel_rsp = const KERNEL_RSP_OFFSET,
    user_data = const USER_DATA as u64,
    user_code = const USER_CODE as u64,
    mark = const SYSCALL_MARK,
    entry = sym syscall_entry,
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

/// User addresses are below this; the last page stays unmapped so that a
/// `syscall` at the very top cannot return to a non-canonical address.
pub const USER_END: u64 = 0x0000_7fff_ffff_f000;

fn tcb(t: u64) -> &'static mut Tcb {
    obj::get(t)
}

/// Copy a user-mode frame into thread `t`.
pub fn save(t: u64, f: &TrapFrame, via_syscall: bool) {
    let th = tcb(t);
    let r = &mut th.regs;
    (r.rax, r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.rbp) =
        (f.rax, f.rbx, f.rcx, f.rdx, f.rsi, f.rdi, f.rbp);
    (r.r8, r.r9, r.r10, r.r11, r.r12, r.r13, r.r14, r.r15) =
        (f.r8, f.r9, f.r10, f.r11, f.r12, f.r13, f.r14, f.r15);
    (r.rip, r.rflags, r.rsp) = (f.rip, f.rflags, f.rsp);
    if via_syscall {
        th.flags |= tflag::SYSCALL;
    } else {
        th.flags &= !tflag::SYSCALL;
    }
}

/// Copy thread `t`'s registers into a frame to return with.
fn load(t: u64, f: &mut TrapFrame) {
    let r = &tcb(t).regs;
    (f.rax, f.rbx, f.rcx, f.rdx, f.rsi, f.rdi, f.rbp) =
        (r.rax, r.rbx, r.rcx, r.rdx, r.rsi, r.rdi, r.rbp);
    (f.r8, f.r9, f.r10, f.r11, f.r12, f.r13, f.r14, f.r15) =
        (r.r8, r.r9, r.r10, r.r11, r.r12, r.r13, r.r14, r.r15);
    // `rip` only ever comes from user mode or a checked THREAD_WRITE. A
    // bad `rsp` only faults in user mode, so it is left as it is.
    assert!(
        r.rip < USER_END,
        "thread {t:#x} would return to {:#x}",
        r.rip
    );
    f.rip = r.rip;
    f.rsp = r.rsp;
    f.rflags = (r.rflags & USER_FLAGS) | cpu::RFLAGS_IF | 2;
    f.cs = u64::from(USER_CODE);
    f.ss = u64::from(USER_DATA);
}

/// Pick the next thread for this core and load it into `f`. Returns true
/// if the return may use `sysret`. If nothing can run, idles (and never
/// returns here).
pub fn leave(mut k: BklGuard<'_, KernelState>, core: usize, f: &mut TrapFrame) -> u64 {
    let next = sched::schedule(&mut k.sys, core);
    sched::switch_to(&mut k.sys, core, next);
    if next == 0 {
        idle(k, core)
    }
    load(next, f);
    u64::from(tcb(next).flags & tflag::SYSCALL != 0)
}

/// Nothing to run on this core: halt until an interrupt brings work, then
/// return to user mode with it. Never returns.
pub fn idle(mut k: BklGuard<'_, KernelState>, core: usize) -> ! {
    loop {
        let next = sched::schedule(&mut k.sys, core);
        sched::switch_to(&mut k.sys, core, next);
        if next != 0 {
            let mut f = TrapFrame::default();
            load(next, &mut f);
            drop(k);
            // SAFETY: the frame describes user mode; the return path pops
            // it and `iretq`s. The stack below it is never used again.
            unsafe {
                core::arch::asm!(
                    "mov rsp, {f}",
                    "jmp {ret}",
                    f = in(reg) &f as *const TrapFrame,
                    ret = sym alaya_user_return,
                    options(noreturn),
                )
            }
        }
        drop(k);
        cpu::enable_interrupts_and_halt();
        k = BKL.lock();
    }
}

/// Wait here (on a core that has finished starting) for threads.
pub fn idle_this_core() -> ! {
    let core = percpu::index() as usize;
    idle(BKL.lock(), core)
}

extern "C" fn syscall_entry(f: &mut TrapFrame) -> u64 {
    let mut k = BKL.lock();
    let core = percpu::index() as usize;
    let me = k.sys.cores[core].current;
    if me != 0 {
        save(me, f, true);
        if tcb(me).state == state::READY {
            handle(&mut k, me);
        } else {
            // Stopped by another core while it waited for the lock: it
            // makes this call again when started.
            tcb(me).regs.rip -= SYSCALL_LEN;
        }
    }
    leave(k, core, f)
}

/// Run the system call thread `me` made (its registers hold the call).
fn handle(k: &mut KernelState, me: u64) {
    let r = tcb(me).regs;
    let KernelState { caps: g, sys, .. } = k;
    let result: Result<Outcome, Error> = match r.rax {
        call::YIELD => {
            sched::yield_now(sys, percpu::index() as usize);
            Ok(Outcome::Done([0; 4]))
        }
        call::DEBUG => debug_call(sys, g.revoked, me, r.rdi, r.rsi, r.rdx),
        call::CALL
        | call::SEND
        | call::TRY_SEND
        | call::RECV
        | call::REPLY_RECV
        | call::SIGNAL
        | call::WAIT
        | call::POLL => message(g, sys, me, r.rax, r.rdi),
        _ => Err(Error::NoSuchCall),
    };
    let t = tcb(me);
    match result {
        Ok(Outcome::Done(res)) => {
            t.regs.rax = 0;
            [t.regs.rdx, t.regs.r10, t.regs.r8, t.regs.r9] = res;
        }
        Ok(Outcome::Message) => {}
        Ok(Outcome::Restart) => {
            sys.restarts += 1;
            t.regs.rip -= SYSCALL_LEN;
        }
        Err(e) => t.regs.rax = e as u64,
    }
}

/// The message calls, and `call` on kernel objects.
fn message(
    g: &mut alaya_cap::Globals,
    sys: &mut sched::Sys,
    me: u64,
    nr: u64,
    handle: u64,
) -> Result<Outcome, Error> {
    let r = tcb(me).regs;
    let mut m = Phys::new(sys);
    let c = m.read(ipc::lookup(&m, me, handle)?).cap;
    let need = |right: u8| {
        if c.rights & right == 0 {
            Err(Error::NoRights)
        } else {
            Ok(())
        }
    };
    let grant = c.rights & rights::GRANT != 0;
    match (nr, c.kind()) {
        (call::CALL, Some(Kind::Endpoint)) => {
            need(rights::WRITE)?;
            ipc::send(&mut m, me, c.object, c.a, grant, true, true)?;
        }
        (call::CALL, _) => {
            let args = [r.rdx, r.r10, r.r8, r.r9, r.r12, r.r13];
            return caps::invoke(g, sys, me, handle, r.rsi, args);
        }
        (call::SEND | call::TRY_SEND, Some(Kind::Endpoint)) => {
            need(rights::WRITE)?;
            ipc::send(&mut m, me, c.object, c.a, grant, false, nr == call::SEND)?;
        }
        (call::SEND | call::TRY_SEND, Some(Kind::Reply)) => {
            need(rights::WRITE)?;
            ipc::answer(&mut m, me, c.object);
            tcb(me).regs.rax = 0;
        }
        (call::RECV | call::REPLY_RECV, Some(Kind::Endpoint)) => {
            need(rights::READ)?;
            let rep = reply_object(&m, me, r.r15, nr == call::REPLY_RECV)?;
            if nr == call::REPLY_RECV {
                ipc::answer(&mut m, me, rep);
            }
            ipc::recv(&mut m, me, c.object, rep)?;
        }
        (call::RECV | call::WAIT, Some(Kind::Notification)) => {
            need(rights::READ)?;
            ipc::wait(me, c.object);
        }
        (call::POLL, Some(Kind::Notification)) => {
            need(rights::READ)?;
            let t = tcb(me);
            t.regs.rdi = core::mem::take(&mut obj::get::<obj::Notification>(c.object).word);
            t.regs.rsi = Info::SIGNAL;
            t.regs.rax = 0;
        }
        (call::SIGNAL, Some(Kind::Notification)) => {
            need(rights::WRITE)?;
            ipc::signal(m.sys, c.object, c.a | r.rdx);
            tcb(me).regs.rax = 0;
        }
        _ => return Err(Error::WrongKind),
    }
    Ok(Outcome::Message)
}

/// The reply object named by `handle` (0: none, unless `required`).
fn reply_object(m: &Phys, me: u64, handle: u64, required: bool) -> Result<u64, Error> {
    if handle == 0 && !required {
        return Ok(0);
    }
    let c = m.read(ipc::lookup(m, me, handle)?).cap;
    if !c.is(Kind::Reply) {
        return Err(Error::WrongKind);
    }
    if c.rights & rights::WRITE == 0 {
        return Err(Error::NoRights);
    }
    Ok(c.object)
}

fn debug_call(
    sys: &mut sched::Sys,
    revoked: u64,
    me: u64,
    op: u64,
    a: u64,
    b: u64,
) -> Result<Outcome, Error> {
    let r = match op {
        debug::PRINT => {
            print(me, b, a)?;
            [0; 4]
        }
        debug::REPORT => {
            if me != sys.first_thread {
                return Err(Error::NoRights);
            }
            if a != 0 {
                panic!("the first program reported failure {a}");
            }
            kprintln!("{}", crate::BOOT_OK_MARKER);
            [0; 4]
        }
        debug::COUNTERS => [sys.restarts, revoked, sys.migrations, sys.switches],
        debug::CORE => [u64::from(percpu::index()), 0, 0, 0],
        _ => return Err(Error::NoSuchCall),
    };
    Ok(Outcome::Done(r))
}

/// Print a program's message on the serial port, prefixed with "init: ".
fn print(me: u64, at: u64, len: u64) -> Result<(), Error> {
    if len > debug::MAX_PRINT as u64 {
        return Err(Error::InvalidArgument);
    }
    let space = tcb(me).slots[obj::SLOT_SPACE].cap;
    if !space.is(Kind::AddressSpace) {
        return Err(Error::InvalidArgument);
    }
    let mut buf = [0u8; debug::MAX_PRINT];
    for i in 0..len {
        let phys = at
            .checked_add(i)
            .filter(|&a| a < USER_END)
            .and_then(|a| vspace::translate(space.object, a))
            .ok_or(Error::InvalidArgument)?;
        // SAFETY: the byte is mapped for the program, so it is RAM the
        // direct map covers.
        buf[i as usize] = unsafe { *((phys + crate::limine::hhdm_offset().unwrap()) as *const u8) };
    }
    let text = core::str::from_utf8(&buf[..len as usize]).unwrap_or("(not UTF-8)");
    kprint!("init: {text}");
    Ok(())
}

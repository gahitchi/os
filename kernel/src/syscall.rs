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
//! How registers are saved and restored, and which registers carry a call,
//! is the processor's business (`arch::entry`); this file only sees a
//! thread's [`crate::arch::Regs`] through named parts (call number,
//! handle, info, data words). A call that was interrupted (a long revoke)
//! is restarted: the saved instruction pointer moves back onto the system
//! call instruction.

use core::sync::atomic::Ordering;

use alaya_abi::{Error, Info, Kind, call, debug, rights};
use alaya_cap::Memory;

use crate::arch::TrapFrame;
use crate::arch::entry;
use crate::caps::{self, Outcome, Phys};
use crate::obj::{self, Tcb, state, tflag};
use crate::sync::{BKL, BklGuard, KernelState};
use crate::{ipc, kprint, kprintln, percpu, sched, vspace};

pub use crate::arch::entry::USER_END;

fn tcb(t: u64) -> &'static mut Tcb {
    obj::get(t)
}

/// Copy a user-mode frame into thread `t`.
pub fn save(t: u64, f: &TrapFrame, via_syscall: bool) {
    let th = tcb(t);
    entry::save(&mut th.regs, f);
    if via_syscall {
        th.flags |= tflag::SYSCALL;
    } else {
        th.flags &= !tflag::SYSCALL;
    }
}

/// Copy thread `t`'s registers into a frame to return with.
fn load(t: u64, f: &mut TrapFrame) {
    let r = &tcb(t).regs;
    // The pc only ever comes from user mode or a checked THREAD_WRITE. A
    // bad stack pointer only faults in user mode, so it is left as it is.
    assert!(
        r.pc() < USER_END,
        "thread {t:#x} would return to {:#x}",
        r.pc()
    );
    entry::load(r, f);
}

/// Pick the next thread for this core and load it into `f`. Returns true
/// if the thread entered with a system call (on a PC the return may then
/// use `sysret`). If nothing can run, idles (and never
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
            entry::return_to_user(&f)
        }
        drop(k);
        crate::arch::wait_for_interrupt();
        k = BKL.lock();
    }
}

/// Wait here (on a core that has finished starting) for threads.
pub fn idle_this_core() -> ! {
    let core = percpu::index() as usize;
    idle(BKL.lock(), core)
}

/// Every system call lands here, from the entry code behind the wall.
pub extern "C" fn syscall_entry(f: &mut TrapFrame) -> u64 {
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
            tcb(me).regs.rewind_call();
        }
    }
    leave(k, core, f)
}

/// Run the system call thread `me` made (its registers hold the call).
fn handle(k: &mut KernelState, me: u64) {
    let r = tcb(me).regs;
    let KernelState { caps: g, sys, .. } = k;
    if tcb(me).flags2 & obj::tflag2::LINUX != 0 {
        // A Linux program: none of Alaya's calls, only Linux requests,
        // answered by its Linux world (decision 163).
        ipc::linux_request(&mut Phys::new(sys), me);
        return;
    }
    let result: Result<Outcome, Error> = match r.call_nr() {
        call::YIELD => {
            sched::yield_now(sys, percpu::index() as usize);
            Ok(Outcome::Done([0; 4]))
        }
        call::DEBUG => debug_call(sys, g.revoked, me, r.arg0(), r.arg1(), r.words()[0]),
        call::CALL
        | call::SEND
        | call::TRY_SEND
        | call::RECV
        | call::REPLY_RECV
        | call::SIGNAL
        | call::WAIT
        | call::POLL => message(g, sys, me, r.call_nr(), r.arg0()),
        _ => Err(Error::NoSuchCall),
    };
    let t = tcb(me);
    match result {
        Ok(Outcome::Done(res)) => {
            t.regs.set_status(0);
            t.regs.set_results(res);
        }
        Ok(Outcome::Message) => {}
        Ok(Outcome::Restart) => {
            sys.restarts += 1;
            t.regs.rewind_call();
        }
        Err(e) => t.regs.set_status(e as u64),
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
            return caps::invoke(g, sys, me, handle, r.arg1(), r.words());
        }
        (call::SEND | call::TRY_SEND, Some(Kind::Endpoint)) => {
            need(rights::WRITE)?;
            ipc::send(&mut m, me, c.object, c.a, grant, false, nr == call::SEND)?;
        }
        (call::SEND | call::TRY_SEND, Some(Kind::Reply)) => {
            need(rights::WRITE)?;
            ipc::answer(&mut m, me, c.object);
            tcb(me).regs.set_status(0);
        }
        (call::RECV | call::REPLY_RECV, Some(Kind::Endpoint)) => {
            need(rights::READ)?;
            let rep = reply_object(&m, me, r.reply_handle(), nr == call::REPLY_RECV)?;
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
            t.regs.set_arg0(core::mem::take(
                &mut obj::get::<obj::Notification>(c.object).word,
            ));
            t.regs.set_arg1(Info::SIGNAL);
            t.regs.set_status(0);
        }
        (call::SIGNAL, Some(Kind::Notification)) => {
            need(rights::WRITE)?;
            ipc::signal(m.sys, c.object, c.a | r.words()[0]);
            tcb(me).regs.set_status(0);
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
        debug::LOCK_STATS => {
            let mut r = [0u64; 4];
            for i in 0..percpu::count() {
                let c = percpu::get(i);
                r[0] += c.lock_takes.load(Ordering::Relaxed);
                r[1] += c.lock_waits.load(Ordering::Relaxed);
                r[2] += c.lock_wait_ticks.load(Ordering::Relaxed);
                r[3] += c.lock_held_ticks.load(Ordering::Relaxed);
            }
            r
        }
        debug::SWITCH_STATS => {
            let mut r = [0u64; 4];
            for i in 0..percpu::count() {
                let c = percpu::get(i);
                for (j, s) in c.switch_stats.iter().enumerate() {
                    r[j] += s.load(Ordering::Relaxed);
                }
            }
            r
        }
        _ => return Err(Error::NoSuchCall),
    };
    Ok(Outcome::Done(r))
}

/// Print a program's message on the serial port (programs add their own
/// name in front).
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
    kprint!("{text}");
    Ok(())
}

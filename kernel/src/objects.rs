// SPDX-License-Identifier: GPL-3.0-only
//! Operations on threads, scheduling contexts, address spaces and pages,
//! and what happens when capabilities to them are deleted.

use alaya_abi::{Error, Kind, MAX_PRIORITY, core_type, linux, op, rights};
use alaya_cap::{self as cap, Cap, Globals, INTERNAL, Memory, SlotRef};

use crate::caps::{Outcome, Phys};
use crate::obj::{
    self, SLOT_FAULT, SLOT_SPACE, SLOT_TABLE, Sc, Tcb, internal, scflag, state, tflag, tflag2,
};
use crate::syscall::USER_END;
use crate::{ipc, percpu, sched, time, vspace};

fn tcb(t: u64) -> &'static mut Tcb {
    obj::get(t)
}

fn sc(s: u64) -> &'static mut Sc {
    obj::get(s)
}

fn done(r: [u64; 4]) -> Result<Outcome, Error> {
    Ok(Outcome::Done(r))
}

const OK: Result<Outcome, Error> = Ok(Outcome::Done([0; 4]));

/// The capability in slot `h` of `me`'s table, checked for kind and rights.
fn arg_cap(m: &Phys, me: u64, h: u64, kind: Kind, need: u8) -> Result<(SlotRef, Cap), Error> {
    let at = ipc::lookup(m, me, h)?;
    let c = m.read(at).cap;
    if !c.is(kind) {
        return Err(Error::WrongKind);
    }
    if c.rights & need != need {
        return Err(Error::NoRights);
    }
    Ok((at, c))
}

/// Operations on a thread capability `c`, invoked by thread `me`.
pub fn thread(
    m: &mut Phys,
    g: &mut Globals,
    me: u64,
    c: Cap,
    label: u64,
    a: [u64; 6],
) -> Result<Outcome, Error> {
    let t = c.object;
    let need = if label == op::THREAD_READ {
        rights::READ
    } else {
        rights::WRITE
    };
    if c.rights & need == 0 {
        return Err(Error::NoRights);
    }
    match label {
        op::THREAD_CONFIGURE => {
            if tcb(t).state != state::STOPPED {
                return Err(Error::Busy);
            }
            let wanted = [
                (a[0], Kind::Table, SLOT_TABLE, internal::THREAD_TABLE),
                (a[1], Kind::AddressSpace, SLOT_SPACE, internal::THREAD_SPACE),
                (a[2], Kind::Endpoint, SLOT_FAULT, internal::THREAD_FAULT),
            ];
            // Check everything first, so a bad argument changes nothing.
            for &(h, kind, _, _) in &wanted {
                if h != 0 {
                    arg_cap(m, me, h, kind, 0)?;
                }
            }
            for (h, kind, slot, tag) in wanted {
                if h == 0 {
                    continue;
                }
                let (src, _) = arg_cap(m, me, h, kind, 0)?;
                let dst = t + (slot as u64) * 64;
                if m.read(dst).cap.is_live() && !cap::delete_any(m, g, dst) {
                    return Ok(Outcome::Restart);
                }
                cap::copy(m, src, dst, rights::ALL, None)?;
                let mut s = m.read(dst);
                s.cap.flags |= INTERNAL;
                s.cap._reserved = tag;
                m.write(dst, &s);
            }
            OK
        }
        op::THREAD_PRIORITY => {
            let limit = u64::from(tcb(me).mcp);
            if a[0] > limit || a[1] > limit || a[0] > MAX_PRIORITY {
                return Err(Error::NoRights);
            }
            let th = tcb(t);
            th.prio = a[0] as u8;
            th.mcp = a[1] as u8;
            if th.flags & tflag::QUEUED != 0 {
                sched::dequeue(m.sys, t);
                sched::make_ready(m.sys, t);
            } else if th.flags & tflag::CURRENT != 0 {
                sched::kick(th.core as usize);
            }
            OK
        }
        op::THREAD_WRITE => {
            let th = tcb(t);
            if th.state != state::STOPPED {
                return Err(Error::Busy);
            }
            if a[0] >= USER_END || a[1] >= USER_END || a[4] >= USER_END {
                return Err(Error::InvalidArgument);
            }
            th.regs = crate::arch::Regs::start(a[0], a[1], a[2], a[3], a[4]);
            th.flags &= !tflag::SYSCALL;
            OK
        }
        op::THREAD_READ => {
            let th = tcb(t);
            done([
                th.regs.pc(),
                th.regs.sp(),
                th.regs.status(),
                u64::from(th.state),
            ])
        }
        op::THREAD_START => {
            let th = tcb(t);
            if th.state == state::STOPPED {
                th.state = state::READY;
                sched::make_ready(m.sys, t);
            }
            OK
        }
        op::THREAD_STOP => {
            if ipc::stop(m.sys, t) {
                OK
            } else {
                Ok(Outcome::Restart)
            }
        }
        op::THREAD_LINUX => {
            let th = tcb(t);
            let stopped = th.state == state::STOPPED;
            let asking = th.flags & tflag::FAULTING != 0 && th.flags2 & tflag2::LINUX_CALL != 0;
            match a[0] {
                linux::ON | linux::OFF if !stopped => Err(Error::Busy),
                linux::ON => {
                    th.flags2 |= tflag2::LINUX;
                    OK
                }
                linux::OFF => {
                    th.flags2 &= !tflag2::LINUX;
                    OK
                }
                linux::TLS | linux::JUMP if !stopped && !asking => Err(Error::Busy),
                linux::TLS if a[1] >= USER_END => Err(Error::InvalidArgument),
                linux::TLS => {
                    th.regs.set_tls(a[1]);
                    OK
                }
                linux::JUMP if a[1] >= USER_END || a[2] >= USER_END => Err(Error::InvalidArgument),
                linux::JUMP => {
                    th.regs.jump(a[1], a[2]);
                    OK
                }
                _ => Err(Error::InvalidArgument),
            }
        }
        op::THREAD_TIMEOUTS => {
            let th = tcb(t);
            if a[0] != 0 {
                th.flags2 |= tflag2::TIMEOUTS;
            } else {
                th.flags2 &= !tflag2::TIMEOUTS;
            }
            OK
        }
        op::THREAD_BIND => {
            let th = tcb(t);
            if a[0] == 0 {
                if th.ntfn != 0 {
                    obj::get::<obj::Notification>(th.ntfn).bound = 0;
                    th.ntfn = 0;
                }
                return OK;
            }
            let (_, n) = arg_cap(m, me, a[0], Kind::Notification, rights::READ)?;
            let no = obj::get::<obj::Notification>(n.object);
            if th.ntfn != 0 || no.bound != 0 {
                return Err(Error::SlotInUse);
            }
            no.bound = t;
            th.ntfn = n.object;
            OK
        }
        _ => Err(Error::NoSuchCall),
    }
}

/// Operations on a scheduling-context capability.
pub fn sched_context(
    m: &mut Phys,
    me: u64,
    c: Cap,
    label: u64,
    a: [u64; 6],
) -> Result<Outcome, Error> {
    let s = c.object;
    let need = if label == op::SC_INFO {
        rights::READ
    } else {
        rights::WRITE
    };
    if c.rights & need == 0 {
        return Err(Error::NoRights);
    }
    let x = sc(s);
    match label {
        op::SC_BIND => {
            let (_, tc) = arg_cap(m, me, a[0], Kind::Thread, rights::WRITE)?;
            let t = tc.object;
            if x.home != 0 || x.thread != 0 || tcb(t).home_sc != 0 {
                return Err(Error::SlotInUse);
            }
            x.home = t;
            tcb(t).home_sc = s;
            if tcb(t).sc == 0 {
                tcb(t).sc = s;
                x.thread = t;
                sched::make_ready(m.sys, t);
            }
            OK
        }
        op::SC_UNBIND => {
            let home = x.home;
            if home == 0 {
                return OK;
            }
            x.home = 0;
            tcb(home).home_sc = 0;
            if x.thread == home {
                x.thread = 0;
                tcb(home).sc = 0;
                sched::dequeue(m.sys, home);
                if tcb(home).flags & tflag::CURRENT != 0 {
                    sched::kick(tcb(home).core as usize);
                }
            }
            sched::release_remove(m.sys, s);
            OK
        }
        op::SC_DEADLINE => {
            let now = time::now();
            let mut missed = 0;
            if x.deadline != 0 && now >= x.deadline {
                missed = 1;
                if x.flags & scflag::MISSED == 0 {
                    // It passed while the context was not running: blame
                    // whoever held it.
                    x.misses = x.misses.wrapping_add(1);
                    x.missed_by = x.thread;
                }
            }
            x.deadline = a[0];
            x.flags &= !scflag::MISSED;
            sched::deadline_changed(m.sys, s);
            done([u64::from(x.misses), x.missed_by, missed, 0])
        }
        op::SC_INFO => done([
            time::ticks_to_ns(x.budget.budget),
            time::ticks_to_ns(x.budget.period),
            time::ticks_to_ns(x.used),
            u64::from(x.core),
        ]),
        _ => Err(Error::NoSuchCall),
    }
}

/// `SC_CONFIGURE` on the scheduling control capability.
pub fn sched_control(m: &mut Phys, me: u64, label: u64, a: [u64; 6]) -> Result<Outcome, Error> {
    if label != op::SC_CONFIGURE {
        return Err(Error::NoSuchCall);
    }
    let (_, c) = arg_cap(m, me, a[0], Kind::SchedContext, rights::WRITE)?;
    let (budget_us, period_us, ctype, pin) = (a[1], a[2], a[3], a[4]);
    if budget_us == 0
        || budget_us > period_us
        || period_us > 10_000_000
        || ctype > core_type::EFFICIENCY
        || pin > u64::from(percpu::count())
    {
        return Err(Error::InvalidArgument);
    }
    let s = c.object;
    sched::release_remove(m.sys, s);
    let x = sc(s);
    x.budget.configure(
        time::ns_to_ticks(budget_us * 1000),
        time::ns_to_ticks(period_us * 1000),
        time::now(),
    );
    x.core_type = ctype as u8;
    x.pin = pin as u32;
    if x.thread != 0 {
        let t = x.thread;
        if tcb(t).flags & tflag::CURRENT != 0 {
            sched::kick(tcb(t).core as usize);
        } else {
            sched::dequeue(m.sys, t);
            sched::make_ready(m.sys, t);
        }
    }
    OK
}

/// Operations on an address space capability.
pub fn space(m: &mut Phys, me: u64, c: Cap, label: u64, a: [u64; 6]) -> Result<Outcome, Error> {
    if c.rights & rights::WRITE == 0 {
        return Err(Error::NoRights);
    }
    match label {
        op::SPACE_ASSIGN => {
            let (pool, _) = arg_cap(m, me, a[0], Kind::Untyped, rights::WRITE)?;
            vspace::assign(m, c.object, pool)?;
            OK
        }
        op::SPACE_GUARD => {
            crate::obj::space_meta(c.object).guarded = u64::from(a[0] != 0);
            OK
        }
        _ => Err(Error::NoSuchCall),
    }
}

/// Operations on a page capability in slot `at`.
pub fn page(
    m: &mut Phys,
    me: u64,
    at: SlotRef,
    c: Cap,
    label: u64,
    a: [u64; 6],
) -> Result<Outcome, Error> {
    match label {
        op::PAGE_MAP => {
            let (_, s) = arg_cap(m, me, a[0], Kind::AddressSpace, rights::WRITE)?;
            vspace::map_page(m, at, s.object, a[1], a[2])?;
            OK
        }
        op::PAGE_UNMAP => {
            vspace::unmap_page(m, at);
            OK
        }
        op::PAGE_ADDRESS => done([c.object, 0, 0, 0]),
        op::PAGE_IOMAP => {
            let (_, s) = arg_cap(m, me, a[0], Kind::IoSpace, rights::WRITE)?;
            crate::iommu::map_page(m, at, s.object, a[1], a[2])?;
            OK
        }
        _ => Err(Error::NoSuchCall),
    }
}

/// [`Memory::on_delete`] for the kernel: detach objects before their
/// capabilities go.
pub fn on_delete(m: &mut Phys, g: &mut Globals, at: SlotRef, c: &Cap, last: bool) -> bool {
    if c.flags & INTERNAL != 0 {
        match c._reserved {
            internal::THREAD_TABLE | internal::THREAD_SPACE => {
                // A thread without its table or memory cannot run.
                let t = at & !((1 << alaya_abi::THREAD_BITS) - 1);
                if !ipc::stop(m.sys, t) {
                    return false;
                }
            }
            internal::SPACE_POOL => vspace::clear(m, at - obj::SPACE_META),
            internal::IOSPACE_POOL => crate::iommu::clear(m.sys, at - obj::SPACE_META),
            _ => {}
        }
    }
    if c.is(Kind::Page) {
        vspace::unmap_page(m, at);
    }
    if !last {
        return true;
    }
    match c.kind() {
        Some(Kind::Thread) => finalise_thread(m, g, c.object),
        Some(Kind::SchedContext) => finalise_sc(m.sys, c.object),
        Some(Kind::Endpoint) => ipc::finalise_queue(m.sys, c.object, true),
        Some(Kind::Notification) => {
            crate::device::forget_notification(m.sys, c.object);
            ipc::finalise_queue(m.sys, c.object, false)
        }
        Some(Kind::Irq) => {
            crate::device::free_vector(m.sys, c.object);
            true
        }
        Some(Kind::IoSpace) => crate::iommu::finalise(m, g, c.object),
        Some(Kind::Reply) => {
            ipc::finalise_reply(m.sys, c.object);
            true
        }
        Some(Kind::AddressSpace) => vspace::finalise(m, g, c.object),
        _ => true,
    }
}

fn finalise_thread(m: &mut Phys, g: &mut Globals, t: u64) -> bool {
    if !ipc::stop(m.sys, t) {
        return false;
    }
    let th = tcb(t);
    if th.home_sc != 0 {
        sc(th.home_sc).home = 0;
        th.home_sc = 0;
    }
    if th.sc != 0 {
        let s = core::mem::take(&mut th.sc);
        let x = sc(s);
        x.thread = 0;
        // Lent time goes back to its own thread.
        if x.home != 0 && tcb(x.home).sc == 0 {
            x.thread = x.home;
            tcb(x.home).sc = s;
            sched::make_ready(m.sys, x.home);
        }
    }
    if th.ntfn != 0 {
        obj::get::<obj::Notification>(th.ntfn).bound = 0;
        th.ntfn = 0;
    }
    for i in 0..3u64 {
        let slot = t + i * 64;
        if m.read(slot).cap.is_live() && !cap::delete_any(m, g, slot) {
            return false;
        }
    }
    true
}

fn finalise_sc(k: &mut sched::Sys, s: u64) -> bool {
    let x = sc(s);
    if x.thread != 0 {
        let t = core::mem::take(&mut x.thread);
        tcb(t).sc = 0;
        sched::dequeue(k, t);
        if tcb(t).flags & tflag::CURRENT != 0 {
            sched::kick(tcb(t).core as usize);
        }
    }
    if x.home != 0 {
        tcb(x.home).home_sc = 0;
        x.home = 0;
    }
    while x.reply_top != 0 {
        let r = obj::get::<obj::Reply>(x.reply_top);
        x.reply_top = r.below;
        r.sc = 0;
        r.below = 0;
    }
    sched::evict_sc(k, s)
}

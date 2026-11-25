// SPDX-License-Identifier: GPL-3.0-only
//! Messages (decision 8): calls and replies through endpoints, signals
//! through notifications, and faults as messages.
//!
//! A message is an [`Info`] word, up to six data words and at most one
//! capability, all carried in registers (the rule: control in messages,
//! data in shared memory).
//!
//! **Lent time.** When a thread calls a server that has no scheduling
//! context of its own (a *passive* server), the caller's context goes with
//! the message: the server runs on the caller's time and at least at its
//! priority, and the reply brings the context back. The reply object
//! records the loan; a chain of calls stacks the loans on the context
//! (`Sc::reply_top`). A server that runs out of borrowed time waits for the
//! caller's next refill (decision 55).

use alaya_abi::{Error, FAULT_LABEL, Info, Kind, LINUX_LABEL, TIMEOUT_LABEL, linux, rights};
use alaya_cap::{self as cap, Memory, SlotRef};

use crate::caps::Phys;
use crate::obj::{
    self, Endpoint, Notification, Queue, Reply, SLOT_FAULT, SLOT_TABLE, Sc, Tcb, epstate, state,
    tflag, tflag2,
};
use crate::sched;

fn tcb(t: u64) -> &'static mut Tcb {
    obj::get(t)
}

fn sc(s: u64) -> &'static mut Sc {
    obj::get(s)
}

fn ep(e: u64) -> &'static mut Endpoint {
    obj::get(e)
}

fn ntfn(n: u64) -> &'static mut Notification {
    obj::get(n)
}

fn reply(r: u64) -> &'static mut Reply {
    obj::get(r)
}

fn push(q: &mut Queue, t: u64) {
    let th = tcb(t);
    th.next = 0;
    th.prev = q.tail;
    if q.tail == 0 {
        q.head = t;
    } else {
        tcb(q.tail).next = t;
    }
    q.tail = t;
}

fn remove(q: &mut Queue, t: u64) {
    let th = tcb(t);
    if th.prev == 0 {
        q.head = th.next;
    } else {
        tcb(th.prev).next = th.next;
    }
    if th.next == 0 {
        q.tail = th.prev;
    } else {
        tcb(th.next).prev = th.prev;
    }
    th.next = 0;
    th.prev = 0;
}

fn pop(q: &mut Queue) -> u64 {
    let t = q.head;
    if t != 0 {
        remove(q, t);
    }
    t
}

/// The slot `handle` names in thread `t`'s own table.
pub fn lookup<M: Memory>(m: &M, t: u64, handle: u64) -> Result<SlotRef, Error> {
    let table = tcb(t).slots[SLOT_TABLE].cap;
    if !table.is(Kind::Table) {
        return Err(Error::InvalidHandle);
    }
    let at = cap::table_slot(table.object, table.size_bits, handle)?;
    // The handle is the program's choice: no speculation past the check.
    crate::arch::speculation_barrier();
    if !m.read(at).cap.is_live() {
        return Err(Error::InvalidHandle);
    }
    Ok(at)
}

/// Make `t` ready with the result `e` as its call status.
pub fn wake(k: &mut sched::Sys, t: u64, e: Error) {
    let th = tcb(t);
    th.regs.set_status(e as u64);
    th.state = state::READY;
    th.waiting = 0;
    th.flags &= !(tflag::CALLING | tflag::FAULTING | tflag::SEND_GRANT);
    th.flags2 &= !(tflag2::TIMED_OUT | tflag2::LINUX_CALL);
    sched::make_ready(k, t);
}

/// Copy a message from `from` to `to`.
fn transfer(m: &mut Phys, from: u64, to: u64, badge: u64, grant: bool, called: bool) {
    let f = tcb(from);
    let (mut info, words) = if f.flags & tflag::FAULTING != 0 {
        let x = f.fault;
        if f.flags2 & tflag2::LINUX_CALL != 0 {
            // The number was kept when the request was made: sending may
            // already have reused its register for the call's status.
            let (_, args) = f.regs.linux_call();
            (Info::new(LINUX_LABEL + x[0], 6).0, args)
        } else {
            let label = if f.flags2 & tflag2::TIMED_OUT != 0 {
                TIMEOUT_LABEL
            } else {
                FAULT_LABEL
            };
            (Info::new(label, 5).0, [x[0], x[1], x[2], x[3], x[4], 0])
        }
    } else {
        let i = Info(f.regs.arg1());
        let n = i.words().min(Info::MAX_WORDS) as usize;
        let mut w = [0; 6];
        w[..n].copy_from_slice(&f.regs.words()[..n]);
        (
            i.label() << 8 | n as u64 | (i.0 & (Info::CAP | Info::MOVE)),
            w,
        )
    };
    if info & Info::CAP != 0 {
        let moving = info & Info::MOVE != 0;
        info &= !(Info::CAP | Info::MOVE);
        let r = tcb(to).regs.cap_slots() >> 32;
        let moved = grant
            && r != 0
            && lookup(m, from, f.regs.cap_slots() & 0xffff_ffff)
                .and_then(|src| {
                    let t = tcb(to).slots[SLOT_TABLE].cap;
                    if !t.is(Kind::Table) {
                        return Err(Error::InvalidHandle);
                    }
                    let dst = cap::table_slot(t.object, t.size_bits, r)?;
                    if moving {
                        cap::move_cap(m, src, dst)
                    } else {
                        cap::copy(m, src, dst, rights::ALL, None)
                    }
                })
                .is_ok();
        info |= if moved { Info::CAP } else { Info::CAP_LOST };
    }
    if called {
        info |= Info::CALLED;
    }
    let t = tcb(to);
    t.regs.set_arg1(info);
    t.regs.set_arg0(badge);
    t.regs.set_words(words);
    t.regs.set_status(0);
}

/// Hand `sender`'s message to `receiver`, who was waiting in `recv`.
fn deliver(m: &mut Phys, sender: u64, receiver: u64, badge: u64, grant: bool, call: bool) {
    transfer(m, sender, receiver, badge, grant, call);
    let r = tcb(receiver);
    let rep = r.reply;
    r.reply = 0;
    r.waiting = 0;
    r.state = state::READY;
    if rep != 0 {
        reply(rep).receiver = 0;
    }
    let s = tcb(sender);
    s.flags &= !(tflag::CALLING | tflag::SEND_GRANT);
    if call && rep != 0 {
        let rp = reply(rep);
        rp.caller = sender;
        s.state = state::WAITING_REPLY;
        s.waiting = rep;
        // A passive server runs on the caller's time.
        if r.sc == 0 && s.sc != 0 {
            let lent = s.sc;
            let c = sc(lent);
            rp.sc = lent;
            rp.below = c.reply_top;
            c.reply_top = rep;
            c.thread = receiver;
            r.sc = lent;
            s.sc = 0;
            sched::dequeue(m.sys, sender);
        }
    } else if call {
        // Nobody can answer: the receiver gave no reply object.
        wake(m.sys, sender, Error::Cancelled);
    } else {
        wake(m.sys, sender, Error::Ok);
    }
    sched::make_ready(m.sys, receiver);
}

/// `send`, `try_send` and `call` on an endpoint, by thread `me`. Returns
/// whether `me` must wait.
pub fn send(
    m: &mut Phys,
    me: u64,
    endpoint: u64,
    badge: u64,
    grant: bool,
    call: bool,
    block: bool,
) -> Result<(), Error> {
    let e = ep(endpoint);
    if e.state == epstate::RECEIVERS {
        let r = pop(&mut e.queue);
        if e.queue.head == 0 {
            e.state = epstate::IDLE;
        }
        tcb(me).regs.set_status(0);
        deliver(m, me, r, badge, grant, call);
        return Ok(());
    }
    if !block {
        return Err(Error::WouldBlock);
    }
    let t = tcb(me);
    t.state = state::SENDING;
    t.waiting = endpoint;
    t.send_badge = badge;
    if call {
        t.flags |= tflag::CALLING;
    }
    if grant {
        t.flags |= tflag::SEND_GRANT;
    }
    push(&mut e.queue, me);
    e.state = epstate::SENDERS;
    Ok(())
}

/// `recv` on an endpoint by `me`, binding callers to reply object `rep`
/// (0: none).
pub fn recv(m: &mut Phys, me: u64, endpoint: u64, rep: u64) -> Result<(), Error> {
    if rep != 0 {
        let rp = reply(rep);
        if rp.caller != 0 || (rp.receiver != 0 && rp.receiver != me) {
            return Err(Error::Busy);
        }
    }
    if take_signal(me) {
        return Ok(());
    }
    let e = ep(endpoint);
    if e.state == epstate::SENDERS {
        let s = pop(&mut e.queue);
        if e.queue.head == 0 {
            e.state = epstate::IDLE;
        }
        let st = tcb(s);
        let (badge, grant, call) = (
            st.send_badge,
            st.flags & tflag::SEND_GRANT != 0,
            st.flags & tflag::CALLING != 0,
        );
        tcb(me).reply = rep;
        deliver(m, s, me, badge, grant, call);
        return Ok(());
    }
    let t = tcb(me);
    t.state = state::RECEIVING;
    t.waiting = endpoint;
    t.reply = rep;
    if rep != 0 {
        reply(rep).receiver = me;
    }
    push(&mut e.queue, me);
    e.state = epstate::RECEIVERS;
    Ok(())
}

/// If `me`'s bound notification has signals, hand them over as the result
/// of `recv`.
fn take_signal(me: u64) -> bool {
    let n = tcb(me).ntfn;
    if n == 0 || ntfn(n).word == 0 {
        return false;
    }
    let t = tcb(me);
    t.regs.set_arg1(Info::SIGNAL);
    t.regs.set_arg0(core::mem::take(&mut ntfn(n).word));
    t.regs.set_status(0);
    true
}

/// Answer the caller held by reply object `rep`, with `me`'s message.
pub fn answer(m: &mut Phys, me: u64, rep: u64) {
    let rp = reply(rep);
    let caller = core::mem::take(&mut rp.caller);
    if caller != 0 {
        let c = tcb(caller);
        if c.flags & tflag::FAULTING != 0 {
            // A fault was handled: the thread tries the instruction again
            // (or, after a timeout, goes on waiting for its caller's time).
            // A Linux request was answered: word 0 is its result, and the
            // thread goes on after its system call.
            if c.flags2 & tflag2::LINUX_CALL != 0 {
                c.regs.set_linux_result(c.fault[0], tcb(me).regs.words()[0]);
            }
            c.flags &= !tflag::FAULTING;
            c.flags2 &= !(tflag2::TIMED_OUT | tflag2::LINUX_CALL);
        } else {
            transfer(m, me, caller, 0, true, false);
        }
        c.state = state::READY;
        c.waiting = 0;
    }
    give_back(m.sys, rep, caller);
    if caller != 0 {
        sched::make_ready(m.sys, caller);
    }
}

/// Return the time reply `rep` borrowed: to `to` if it can take it, else
/// to the context's own thread.
fn give_back(k: &mut sched::Sys, rep: u64, to: u64) {
    let rp = reply(rep);
    let lent = core::mem::take(&mut rp.sc);
    if lent == 0 {
        return;
    }
    let c = sc(lent);
    // Unlink this reply from the stack of loans.
    if c.reply_top == rep {
        c.reply_top = rp.below;
    } else {
        let mut at = c.reply_top;
        while at != 0 {
            if reply(at).below == rep {
                reply(at).below = rp.below;
                break;
            }
            at = reply(at).below;
        }
    }
    rp.below = 0;
    let holder = c.thread;
    if holder != 0 {
        if tcb(holder).sc != lent {
            return;
        }
        tcb(holder).sc = 0;
        sched::dequeue(k, holder);
        if tcb(holder).flags & tflag::CURRENT != 0 {
            sched::kick(tcb(holder).core as usize);
        }
    }
    let target = if to != 0 && tcb(to).sc == 0 {
        to
    } else if c.home != 0 && tcb(c.home).sc == 0 {
        c.home
    } else {
        0
    };
    c.thread = target;
    if target != 0 {
        tcb(target).sc = lent;
        sched::make_ready(k, target);
    }
}

/// `signal` on a notification: raise `bits`.
pub fn signal(k: &mut sched::Sys, n: u64, bits: u64) {
    let no = ntfn(n);
    no.word |= bits;
    if no.word == 0 {
        return;
    }
    let w = pop(&mut no.queue);
    if w != 0 {
        let t = tcb(w);
        t.regs.set_arg0(core::mem::take(&mut no.word));
        t.regs.set_arg1(Info::SIGNAL);
        wake(k, w, Error::Ok);
        return;
    }
    let b = no.bound;
    if b != 0 && tcb(b).state == state::RECEIVING {
        cancel(k, b);
        let t = tcb(b);
        t.regs.set_arg0(core::mem::take(&mut no.word));
        t.regs.set_arg1(Info::SIGNAL);
        wake(k, b, Error::Ok);
    }
}

/// `wait` on a notification. Returns true if `me` must wait.
pub fn wait(me: u64, n: u64) -> bool {
    let no = ntfn(n);
    let t = tcb(me);
    if no.word != 0 {
        t.regs.set_arg0(core::mem::take(&mut no.word));
        t.regs.set_arg1(Info::SIGNAL);
        t.regs.set_status(0);
        return false;
    }
    t.state = state::WAITING_SIGNAL;
    t.waiting = n;
    push(&mut no.queue, me);
    true
}

/// Take `t` out of whatever message wait it is in. It is left as it was
/// otherwise; the caller sets its state.
pub fn cancel(k: &mut sched::Sys, t: u64) {
    let th = tcb(t);
    let w = th.waiting;
    match th.state {
        state::SENDING | state::RECEIVING => {
            let e = ep(w);
            remove(&mut e.queue, t);
            if e.queue.head == 0 {
                e.state = epstate::IDLE;
            }
            if th.reply != 0 {
                reply(th.reply).receiver = 0;
                th.reply = 0;
            }
        }
        state::WAITING_SIGNAL => remove(&mut ntfn(w).queue, t),
        state::WAITING_REPLY => {
            // The server keeps any lent time until it answers; the answer
            // then gives it back to the context's own thread.
            reply(w).caller = 0;
        }
        _ => {}
    }
    let _ = k;
    th.waiting = 0;
    th.flags &= !(tflag::CALLING | tflag::FAULTING | tflag::SEND_GRANT);
    th.flags2 &= !(tflag2::TIMED_OUT | tflag2::LINUX_CALL);
}

/// Stop a thread: cancel its wait and take it off every core. Returns
/// false while another core still runs it (retry).
pub fn stop(k: &mut sched::Sys, t: u64) -> bool {
    if tcb(t).state != state::STOPPED {
        cancel(k, t);
        tcb(t).state = state::STOPPED;
    }
    sched::evict(k, t)
}

/// A user-mode exception in thread `t`: send a fault message to its fault
/// endpoint, or stop it.
pub fn fault(m: &mut Phys, t: u64, vector: u64, error: u64, addr: u64) {
    let th = tcb(t);
    th.fault = [vector, error, addr, th.regs.pc(), th.regs.sp()];
    let fe = th.slots[SLOT_FAULT].cap;
    if fe.is(Kind::Endpoint) && fe.rights & rights::WRITE != 0 {
        th.flags |= tflag::FAULTING;
        let _ = send(m, t, fe.object, fe.a, false, true, true);
        return;
    }
    m.sys.faults += 1;
    if m.sys.faults <= 20 {
        crate::kprintln!(
            "fault: thread {:#x} stopped: vector {vector}, error {error:#x}, address {addr:#x}, pc {:#x}",
            t,
            th.regs.pc()
        );
    } else if m.sys.faults == 21 {
        crate::kprintln!("fault: more threads stopped by faults; not reporting them");
    }
    let _ = stop(m.sys, t);
}

/// Thread `t`, which speaks Linux, made a system call: send it as a Linux
/// request to its fault endpoint and wait for the answer. Without a fault
/// endpoint the call fails with Linux's "not implemented".
pub fn linux_request(m: &mut Phys, t: u64) {
    let th = tcb(t);
    let fe = th.slots[SLOT_FAULT].cap;
    let (nr, _) = th.regs.linux_call();
    if !fe.is(Kind::Endpoint) || fe.rights & rights::WRITE == 0 || nr >= linux::CALLS {
        th.regs.set_linux_result(nr, LINUX_ENOSYS);
        return;
    }
    th.fault[0] = nr;
    th.flags |= tflag::FAULTING;
    th.flags2 |= tflag2::LINUX_CALL;
    let _ = send(m, t, fe.object, fe.a, false, true, true);
}

/// Linux's -ENOSYS.
const LINUX_ENOSYS: u64 = -38i64 as u64;

/// `t`, serving a call, used up the time its caller lent it: send a timeout
/// message to its fault endpoint, if it has one. Without one nothing
/// happens: it waits for the caller's next refill.
pub fn timeout(m: &mut Phys, t: u64, used_ns: u64, wait_ns: u64) {
    let th = tcb(t);
    let fe = th.slots[SLOT_FAULT].cap;
    if !fe.is(Kind::Endpoint) || fe.rights & rights::WRITE == 0 {
        return;
    }
    th.fault = [0, used_ns, wait_ns, th.regs.pc(), th.regs.sp()];
    th.flags |= tflag::FAULTING;
    th.flags2 |= tflag2::TIMED_OUT;
    let _ = send(m, t, fe.object, fe.a, false, true, true);
}

/// The last capability to an endpoint or notification goes: wake every
/// thread waiting on it with [`Error::Cancelled`]. Does a bounded amount of
/// work; returns false if more is left.
pub fn finalise_queue(k: &mut sched::Sys, object: u64, is_ep: bool) -> bool {
    for _ in 0..64 {
        let q = if is_ep {
            &mut ep(object).queue
        } else {
            &mut ntfn(object).queue
        };
        let t = q.head;
        if t == 0 {
            if is_ep {
                ep(object).state = epstate::IDLE;
            } else {
                let b = core::mem::take(&mut ntfn(object).bound);
                if b != 0 {
                    tcb(b).ntfn = 0;
                }
            }
            return true;
        }
        cancel(k, t);
        wake(k, t, Error::Cancelled);
    }
    false
}

/// The last capability to a reply object goes: its caller and the thread
/// waiting to use it are woken with [`Error::Cancelled`], and lent time
/// goes back.
pub fn finalise_reply(k: &mut sched::Sys, rep: u64) {
    let rp = reply(rep);
    let caller = core::mem::take(&mut rp.caller);
    let receiver = core::mem::take(&mut rp.receiver);
    if receiver != 0 {
        cancel(k, receiver);
        wake(k, receiver, Error::Cancelled);
    }
    if caller != 0 {
        tcb(caller).state = state::READY;
        tcb(caller).waiting = 0;
    }
    give_back(k, rep, caller);
    if caller != 0 {
        wake(k, caller, Error::Cancelled);
    }
}

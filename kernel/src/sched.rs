// SPDX-License-Identifier: GPL-3.0-only
//! The scheduler: run queues, time budgets, placement on cores, and
//! switching between threads.
//!
//! - **Budgets** (decisions 11, 53): a thread runs only on a scheduling
//!   context with time left (`alaya-sched` keeps the refill rule). When it
//!   runs out, its context waits on its core's *release list* until the
//!   next refill, and the core's timer fires then.
//! - **Priorities**: 256 levels, highest first; threads of one level share
//!   the core in slices of [`SLICE_NS`]. A thread running on a lent context
//!   (a server serving a call) runs at least at the lender's priority.
//! - **Placement** (decision 54): a context names a core type, or one core.
//!   When a thread becomes ready the kernel picks a core: the one it last
//!   ran on if idle, else any idle core it may use, else the one running
//!   the least important work below it, else the one with the shortest
//!   queue. A core with nothing to do takes waiting work from others. A
//!   running thread is never moved.
//!
//! Everything here runs under the big kernel lock. Each core has its own
//! queues; other cores change them only under the lock and then send the
//! core an interrupt so it looks again.

use core::sync::atomic::Ordering;

use alaya_abi::core_type;

use crate::arch::apic;
use crate::arch::cpu::{self, cpuid, msr, rdtsc};
use crate::arch::trap::RESCHED_VECTOR;
use crate::obj::{self, FPU_OFFSET, SLOT_SPACE, Sc, Tcb, scflag, state, tflag};
pub use crate::sync::Sys;
use crate::{fpu, percpu, time};

pub const PRIORITIES: usize = 256;
/// Time slice between threads of the same priority.
pub const SLICE_NS: u64 = 2_000_000;

/// One core's scheduling state.
#[derive(Clone, Copy)]
pub struct CoreState {
    head: [u64; PRIORITIES],
    tail: [u64; PRIORITIES],
    bitmap: [u64; 4],
    pub queued: u32,
    /// The thread running here (0: idle).
    pub current: u64,
    /// The address space loaded (0: the kernel's own).
    pub space: u64,
    /// The last program address space loaded (stays set while idle).
    pub last_space: u64,
    /// The scheduling context being charged, since `sc_start`; it runs out
    /// at `sc_end`.
    pub sc: u64,
    pub sc_start: u64,
    pub sc_end: u64,
    pub slice_end: u64,
    /// Scheduling contexts waiting for a refill, soonest first.
    pub release: u64,
    /// `alaya_abi::core_type`; 0 when the CPU has one kind of core.
    pub ctype: u8,
    pub online: bool,
}

impl CoreState {
    pub const EMPTY: CoreState = CoreState {
        head: [0; PRIORITIES],
        tail: [0; PRIORITIES],
        bitmap: [0; 4],
        queued: 0,
        current: 0,
        space: 0,
        last_space: 0,
        sc: 0,
        sc_start: 0,
        sc_end: 0,
        slice_end: 0,
        release: 0,
        ctype: 0,
        online: false,
    };

    fn best(&self) -> Option<u8> {
        (0..4)
            .rev()
            .find(|&i| self.bitmap[i] != 0)
            .map(|i| (i * 64 + 63 - self.bitmap[i].leading_zeros() as usize) as u8)
    }
}

fn tcb(t: u64) -> &'static mut Tcb {
    obj::get(t)
}

fn sc(s: u64) -> &'static mut Sc {
    obj::get(s)
}

/// Read this core's type and mark it ready for threads.
pub fn init_this_core(k: &mut Sys) {
    let me = percpu::index() as usize;
    let hybrid = cpu::max_leaf() >= 7 && cpuid(7, 0).edx & (1 << 15) != 0;
    k.cores[me].ctype = if hybrid && cpu::max_leaf() >= 0x1a {
        match cpuid(0x1a, 0).eax >> 24 {
            0x40 => core_type::PERFORMANCE as u8,
            0x20 => core_type::EFFICIENCY as u8,
            _ => 0,
        }
    } else {
        0
    };
    k.cores[me].online = true;
    if k.cores[me].ctype != 0 {
        k.hybrid = true;
    }
}

/// The priority a thread runs at: its own, or its lender's if higher.
pub fn eff_prio(t: u64) -> u8 {
    let th = tcb(t);
    let mut p = th.prio;
    if th.sc != 0 && th.sc != th.home_sc {
        let home = sc(th.sc).home;
        if home != 0 {
            p = p.max(tcb(home).prio);
        }
    }
    p
}

/// May a thread on context `s` run on `core`?
pub fn allowed(k: &Sys, s: u64, core: usize) -> bool {
    let c = sc(s);
    let cs = &k.cores[core];
    if !cs.online {
        return false;
    }
    if c.pin != 0 {
        return c.pin as usize - 1 == core;
    }
    u64::from(c.core_type) == core_type::ANY
        || !k.hybrid
        || u64::from(cs.ctype) == u64::from(c.core_type)
}

/// Can the thread run now (ready, with time, with a memory space)?
pub fn schedulable(t: u64) -> bool {
    let th = tcb(t);
    if th.state != state::READY || th.sc == 0 {
        return false;
    }
    let c = sc(th.sc);
    c.budget.is_configured()
        && c.flags & scflag::RELEASING == 0
        && th.slots[SLOT_SPACE].cap.is(alaya_abi::Kind::AddressSpace)
        && obj::space_meta(th.slots[SLOT_SPACE].cap.object).id != 0
}

fn enqueue(k: &mut Sys, core: usize, t: u64, front: bool) {
    let p = eff_prio(t);
    let cs = &mut k.cores[core];
    let th = tcb(t);
    debug_assert!(th.flags & tflag::QUEUED == 0);
    th.flags |= tflag::QUEUED;
    th.core = core as u16;
    th.qprio = p;
    let pi = p as usize;
    if cs.head[pi] == 0 {
        th.next = 0;
        th.prev = 0;
        cs.head[pi] = t;
        cs.tail[pi] = t;
    } else if front {
        th.prev = 0;
        th.next = cs.head[pi];
        tcb(cs.head[pi]).prev = t;
        cs.head[pi] = t;
    } else {
        th.next = 0;
        th.prev = cs.tail[pi];
        tcb(cs.tail[pi]).next = t;
        cs.tail[pi] = t;
    }
    cs.bitmap[pi / 64] |= 1 << (pi % 64);
    cs.queued += 1;
}

/// Take a thread out of its run queue, if it is in one.
pub fn dequeue(k: &mut Sys, t: u64) {
    let th = tcb(t);
    if th.flags & tflag::QUEUED == 0 {
        return;
    }
    let cs = &mut k.cores[th.core as usize];
    let pi = th.qprio as usize;
    if th.prev == 0 {
        cs.head[pi] = th.next;
    } else {
        tcb(th.prev).next = th.next;
    }
    if th.next == 0 {
        cs.tail[pi] = th.prev;
    } else {
        tcb(th.next).prev = th.prev;
    }
    if cs.head[pi] == 0 {
        cs.bitmap[pi / 64] &= !(1 << (pi % 64));
    }
    cs.queued -= 1;
    th.next = 0;
    th.prev = 0;
    th.flags &= !tflag::QUEUED;
}

/// Ask `core` to look at its queues again.
pub fn kick(core: usize) {
    if core != percpu::index() as usize {
        apic::send_ipi(
            percpu::get(core as u32).apic_id.load(Ordering::Relaxed),
            RESCHED_VECTOR,
        );
    }
}

/// Pick a core for a thread that just became ready (decision 54).
fn place(k: &Sys, t: u64) -> usize {
    let th = tcb(t);
    let s = th.sc;
    let last = th.core as usize;
    let cores = percpu::count() as usize;
    let p = eff_prio(t);
    if last < cores && allowed(k, s, last) && k.cores[last].current == 0 {
        return last;
    }
    // Handing over from the thread running here, which is about to stop
    // (a call, a reply): stay on this core, where the caches are warm.
    let me = percpu::index() as usize;
    let cur = k.cores[me].current;
    if allowed(k, s, me) && (cur == 0 || cur == t || !schedulable(cur)) {
        return me;
    }
    let mut best: Option<(usize, (u8, u32, bool))> = None;
    for c in 0..cores {
        if !allowed(k, s, c) {
            continue;
        }
        let cs = &k.cores[c];
        // Lower is better: idle first, then the lowest priority running
        // below ours, then the shortest queue; the last core wins ties.
        let running = if cs.current == 0 {
            0
        } else {
            let rp = eff_prio(cs.current);
            if rp < p { 1 + rp } else { 255 }
        };
        let key = (running, cs.queued, c != last);
        if best.is_none_or(|(_, b)| key < b) {
            best = Some((c, key));
        }
    }
    best.map_or(last.min(cores - 1), |(c, _)| c)
}

/// The thread became ready, or got time: queue it on a core, or put its
/// context on a release list if it has no time left.
pub fn make_ready(k: &mut Sys, t: u64) {
    let th = tcb(t);
    if th.flags & (tflag::QUEUED | tflag::CURRENT) != 0 || !schedulable(t) {
        // A running thread is looked at by its own core when it next
        // enters the kernel.
        if th.flags & tflag::CURRENT != 0 {
            kick(th.core as usize);
        }
        return;
    }
    let s = th.sc;
    if sc(s).budget.available(rdtsc()) == 0 {
        release_insert(k, s);
        return;
    }
    let core = place(k, t);
    enqueue(k, core, t, false);
    kick(core);
}

/// Put a context with no time left on a release list.
pub fn release_insert(k: &mut Sys, s: u64) {
    let c = sc(s);
    if c.flags & scflag::RELEASING != 0 {
        return;
    }
    let mut core = c.core as usize;
    if core >= percpu::count() as usize || !k.cores[core].online {
        core = percpu::index() as usize;
    }
    c.core = core as u32;
    c.flags |= scflag::RELEASING;
    let when = c.budget.ready_time();
    let cs = &mut k.cores[core];
    let mut prev = 0u64;
    let mut at = cs.release;
    while at != 0 && sc(at).budget.ready_time() <= when {
        prev = at;
        at = sc(at).next_release;
    }
    c.next_release = at;
    if prev == 0 {
        cs.release = s;
        kick(core);
    } else {
        sc(prev).next_release = s;
    }
    if let Some(t) = (c.thread != 0).then_some(c.thread) {
        dequeue(k, t);
    }
}

/// Take a context off its release list, if it is on one.
pub fn release_remove(k: &mut Sys, s: u64) {
    let c = sc(s);
    if c.flags & scflag::RELEASING == 0 {
        return;
    }
    c.flags &= !scflag::RELEASING;
    let cs = &mut k.cores[c.core as usize];
    if cs.release == s {
        cs.release = c.next_release;
    } else {
        let mut at = cs.release;
        while at != 0 {
            let n = sc(at).next_release;
            if n == s {
                sc(at).next_release = c.next_release;
                break;
            }
            at = n;
        }
    }
    c.next_release = 0;
}

/// Give back time whose refill is due, on this core.
fn release_due(k: &mut Sys, core: usize, now: u64) {
    loop {
        let s = k.cores[core].release;
        if s == 0 || sc(s).budget.ready_time() > now {
            return;
        }
        let c = sc(s);
        k.cores[core].release = c.next_release;
        c.next_release = 0;
        c.flags &= !scflag::RELEASING;
        if c.thread != 0 {
            make_ready(k, c.thread);
        }
    }
}

/// Charge the context running on this core for its time so far, and stop
/// charging it.
fn charge(k: &mut Sys, core: usize, now: u64) {
    let cs = &mut k.cores[core];
    if cs.sc == 0 {
        return;
    }
    let c = sc(cs.sc);
    let used = now.saturating_sub(cs.sc_start);
    c.budget.charge(cs.sc_start, used);
    c.used += used;
    cs.sc = 0;
}

/// If the running context's time is up, charge it, and either go on with
/// newly usable time or put it on the release list.
fn account(k: &mut Sys, core: usize, now: u64) {
    let s = k.cores[core].sc;
    if s == 0 || now < k.cores[core].sc_end {
        return;
    }
    charge(k, core, now);
    let avail = sc(s).budget.available(now);
    if avail == 0 {
        release_insert(k, s);
    } else {
        let cs = &mut k.cores[core];
        cs.sc = s;
        cs.sc_start = now;
        cs.sc_end = now.saturating_add(avail);
    }
}

/// The running thread gives up the rest of its time: to its equals if its
/// context is a round-robin one, else until its next refill.
pub fn yield_now(k: &mut Sys, core: usize) {
    let now = rdtsc();
    let cs = &mut k.cores[core];
    let s = cs.sc;
    if s == 0 {
        return;
    }
    if sc(s).budget.is_round_robin() {
        cs.slice_end = 0;
        if cs.current != 0 {
            tcb(cs.current).slice_end = 0;
        }
        return;
    }
    let start = cs.sc_start;
    let c = sc(s);
    c.used += now.saturating_sub(start);
    c.budget.yield_rest(start, now);
    cs.sc = 0;
    if c.budget.available(now) == 0 {
        release_insert(k, s);
    }
}

/// Look for a thread another core is holding in its queue that may run
/// here; take the most important one.
fn steal(k: &mut Sys, core: usize) -> u64 {
    let mut best: Option<(u8, u64)> = None;
    for c in 0..percpu::count() as usize {
        if c == core || k.cores[c].queued == 0 {
            continue;
        }
        for i in (0..4).rev() {
            let mut bits = k.cores[c].bitmap[i];
            while bits != 0 {
                let pi = i * 64 + 63 - bits.leading_zeros() as usize;
                bits &= !(1 << (pi % 64));
                if best.is_some_and(|(p, _)| p as usize >= pi) {
                    break;
                }
                let mut t = k.cores[c].head[pi];
                while t != 0 {
                    if allowed(k, tcb(t).sc, core) {
                        best = Some((pi as u8, t));
                        break;
                    }
                    t = tcb(t).next;
                }
            }
        }
    }
    match best {
        Some((_, t)) => {
            dequeue(k, t);
            k.migrations += 1;
            t
        }
        None => 0,
    }
}

/// Choose what this core runs next (0: nothing).
pub fn schedule(k: &mut Sys, core: usize) -> u64 {
    let now = rdtsc();
    release_due(k, core, now);
    account(k, core, now);
    let cur = k.cores[core].current;
    let cur_ok = cur != 0 && schedulable(cur) && allowed(k, tcb(cur).sc, core);
    let best = k.cores[core].best();
    if cur_ok {
        let p = eff_prio(cur);
        let rotate = now >= k.cores[core].slice_end && best == Some(p);
        if !rotate && best.is_none_or(|b| b <= p) {
            return cur;
        }
        enqueue(k, core, cur, !rotate);
        if rotate {
            tcb(cur).slice_end = 0;
        }
    }
    match k.cores[core].best() {
        Some(pi) => {
            let t = k.cores[core].head[pi as usize];
            dequeue(k, t);
            t
        }
        None => steal(k, core),
    }
}

/// Make `next` the thread running on this core: save the old thread's FPU
/// state, load the new one's, switch address spaces, start charging its
/// context and arm the timer.
pub fn switch_to(k: &mut Sys, core: usize, next: u64) {
    let now = rdtsc();
    let hhdm = crate::limine::hhdm_offset().unwrap();
    let cur = k.cores[core].current;
    if next != cur {
        if cur != 0 {
            let c = tcb(cur);
            fpu::save(cur + hhdm + FPU_OFFSET);
            c.flags &= !tflag::CURRENT;
        }
        if next != 0 {
            let n = tcb(next);
            if n.core as usize != core {
                k.migrations += 1;
            }
            n.flags |= tflag::CURRENT;
            n.core = core as u16;
            let area = next + hhdm + FPU_OFFSET;
            if n.flags & tflag::FPU_READY == 0 {
                fpu::init_area(area);
                n.flags |= tflag::FPU_READY;
            }
            fpu::restore(area);
            // SAFETY: user-mode segment bases; the user GS base sits in
            // KERNEL_GS_BASE until `swapgs` on the way out.
            unsafe {
                cpu::wrmsr(msr::FS_BASE, n.regs.fs_base);
                cpu::wrmsr(msr::KERNEL_GS_BASE, n.regs.gs_base);
            }
            let space = n.slots[SLOT_SPACE].cap.object;
            if space != k.cores[core].space {
                crate::vspace::load(k, core, space);
                k.cores[core].space = space;
                if space != k.cores[core].last_space {
                    // Another program's code runs here next.
                    crate::arch::protect::on_space_switch();
                    k.cores[core].last_space = space;
                }
            }
        } else if k.cores[core].space != 0 {
            crate::vspace::load_kernel();
            k.cores[core].space = 0;
        }
        k.cores[core].current = next;
        k.switches += 1;
    }
    let nsc = if next == 0 { 0 } else { tcb(next).sc };
    if nsc != k.cores[core].sc {
        charge(k, core, now);
        if nsc != 0 {
            let avail = sc(nsc).budget.available(now);
            let cs = &mut k.cores[core];
            cs.sc = nsc;
            cs.sc_start = now;
            cs.sc_end = now.saturating_add(avail);
            sc(nsc).core = core as u32;
        }
    }
    if next != 0 {
        let n = tcb(next);
        if n.slice_end <= now {
            n.slice_end = now + time::ns_to_tsc(SLICE_NS);
        }
        k.cores[core].slice_end = n.slice_end;
    }
    arm(k, core, now);
}

/// Arm this core's timer for the next thing that needs it.
pub fn arm(k: &Sys, core: usize, now: u64) {
    let cs = &k.cores[core];
    let mut d = u64::MAX;
    if cs.sc != 0 {
        d = d.min(cs.sc_end);
        if cs.current != 0 && cs.best() == Some(eff_prio(cs.current)) {
            d = d.min(cs.slice_end);
        }
    }
    if cs.release != 0 {
        d = d.min(sc(cs.release).budget.ready_time());
    }
    if d == u64::MAX {
        time::disarm();
    } else {
        time::set_deadline(d.max(now + 1));
    }
}

/// Stop `t` from running anywhere. The caller has already made it
/// unschedulable. Returns false while another core still runs it; that
/// core has been asked to switch away, and the caller retries.
pub fn evict(k: &mut Sys, t: u64) -> bool {
    dequeue(k, t);
    let th = tcb(t);
    if th.flags & tflag::CURRENT == 0 {
        return true;
    }
    let core = th.core as usize;
    if core == percpu::index() as usize {
        // It is the thread making this call: forget it here, so nothing is
        // saved into it on the way out.
        th.flags &= !tflag::CURRENT;
        k.cores[core].current = 0;
        if k.cores[core].sc == th.sc {
            charge(k, core, rdtsc());
        }
        return true;
    }
    kick(core);
    false
}

/// Stop charging context `s` anywhere. Returns false while another core
/// still runs on it (that core was asked to switch).
pub fn evict_sc(k: &mut Sys, s: u64) -> bool {
    release_remove(k, s);
    let me = percpu::index() as usize;
    let mut done = true;
    for c in 0..percpu::count() as usize {
        if k.cores[c].sc == s {
            if c == me {
                charge(k, c, rdtsc());
            } else {
                kick(c);
                done = false;
            }
        }
    }
    done
}

/// The timer fired on this core.
pub fn timer(k: &mut Sys, core: usize) {
    let now = rdtsc();
    release_due(k, core, now);
    account(k, core, now);
}

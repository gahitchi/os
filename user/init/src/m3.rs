// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 3 checks: threads on every core, time budgets, priorities,
//! messages, lent time, notifications, faults, separate address spaces,
//! and the time a call and reply take.

use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use alaya_sys::abi::{BootInfo, FAULT_LABEL, core_type, map, state};
use alaya_sys::{At, Caps, Error, Kind, Msg, MsgInfo, rights};

use crate::{expect_err, say};

/// Where thread stacks go in the first program's address space.
const STACKS: u64 = 0x1000_0000_0000;
/// A window for filling pages before they move to another program.
const SCRATCH: u64 = 0x2000_0000_0000;
/// A page shared between two programs.
const SHARED: u64 = 0x3000_0000_0000;
/// Where the second program's stack goes.
const STACK2: u64 = 0x4000_0000_0000;

/// Server labels.
const ISOLATION: u64 = 1;
const NULL: u64 = 2;
const SPIN: u64 = 3;
const ECHO: u64 = 4;

/// A slot of the server's own table where it receives a capability.
const RECV_SLOT: u64 = 4000;

pub(crate) static TICKS: AtomicU64 = AtomicU64::new(0);
pub(crate) static TICK: AtomicU64 = AtomicU64::new(0);
pub(crate) static HZ: AtomicU64 = AtomicU64::new(1);
static COUNT_L: AtomicU64 = AtomicU64::new(0);
static COUNT_H: AtomicU64 = AtomicU64::new(0);
static RESULT: AtomicU64 = AtomicU64::new(0);
static SIGNALS: AtomicU64 = AtomicU64::new(0);
static DONE: AtomicU64 = AtomicU64::new(0);
static SEEN: [AtomicU64; 64] = [const { AtomicU64::new(u64::MAX) }; 64];
/// Set after the second program was copied: it must still see 0.
static MARK: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    static __text_end: u8;
}

/// Kernel objects and stacks for the checks, from one untyped block.
pub(crate) struct Kit<'a> {
    pub info: &'a BootInfo,
    pub ut: u64,
    pub next: u64,
    pub stacks: u64,
}

impl Kit<'_> {
    pub(crate) fn slot(&mut self) -> u64 {
        let h = self.next;
        self.next += 1;
        h
    }

    pub(crate) fn at(&self, index: u64) -> At {
        At {
            table: self.info.table,
            index,
        }
    }

    pub(crate) fn make(&mut self, kind: Kind, bits: u8) -> u64 {
        let h = self.slot();
        alaya_sys::retype(self.ut, kind, bits, 1, self.at(h)).unwrap();
        h
    }

    pub(crate) fn page_at(&mut self, space: u64, at: u64, flags: u64) -> u64 {
        let p = self.make(Kind::Page, 12);
        alaya_sys::page_map(p, space, at, flags).unwrap();
        p
    }

    /// A 16 KiB stack in the first program; returns its top.
    pub(crate) fn stack(&mut self) -> u64 {
        let base = STACKS + self.stacks * 0x10000;
        self.stacks += 1;
        for i in 0..4 {
            self.page_at(self.info.space, base + 0xc000 + i * 4096, map::WRITE);
        }
        base + 0x10000
    }

    /// A thread in the first program running `entry(arg)` at `prio`, with
    /// `budget` µs every `period` µs on `ctype` cores (`pin`: core + 1).
    pub(crate) fn thread(
        &mut self,
        entry: extern "C" fn(u64) -> !,
        arg: u64,
        prio: u64,
        budget: u64,
        period: u64,
        pin: u64,
    ) -> (u64, u64) {
        let t = self.make(Kind::Thread, 12);
        let sc = self.make(Kind::SchedContext, 8);
        alaya_sys::thread_configure(t, self.info.table, self.info.space, 0).unwrap();
        alaya_sys::thread_priority(t, prio, prio).unwrap();
        let top = self.stack();
        alaya_sys::thread_write(
            t,
            entry as *const () as u64,
            alaya_sys::entry_stack(top),
            arg,
            0,
            0,
        )
        .unwrap();
        alaya_sys::sc_configure(
            self.info.sched_control,
            sc,
            budget,
            period,
            core_type::ANY,
            pin,
        )
        .unwrap();
        alaya_sys::sc_bind(sc, t).unwrap();
        alaya_sys::thread_start(t).unwrap();
        (t, sc)
    }
}

pub(crate) fn ms_to_ticks(ms: u64) -> u64 {
    ms * HZ.load(SeqCst) / 1000
}

pub(crate) fn ticks_to_us(t: u64) -> u64 {
    t * 1_000_000 / HZ.load(SeqCst)
}

/// Sleep for about `ms`: wait for the ticker thread's signals.
pub(crate) fn sleep_ms(ms: u64) {
    let end = alaya_sys::clock() + ms_to_ticks(ms);
    while alaya_sys::clock() < end {
        alaya_sys::wait(TICK.load(SeqCst)).unwrap();
    }
}

/// Wait (sleeping) until `done` holds, at most `ms`.
pub(crate) fn sleep_until(ms: u64, done: impl Fn() -> bool) -> bool {
    let end = alaya_sys::clock() + ms_to_ticks(ms);
    while !done() {
        if alaya_sys::clock() > end {
            return false;
        }
        alaya_sys::wait(TICK.load(SeqCst)).unwrap();
    }
    true
}

/// Run for `ms` of this thread's own time: gaps while it was not running
/// do not count.
pub(crate) fn work_ms(ms: u64) {
    let (target, gap) = (ms_to_ticks(ms), ms_to_ticks(1) / 4);
    let mut ran = 0;
    let mut last = alaya_sys::clock();
    while ran < target {
        let now = alaya_sys::clock();
        if now - last < gap {
            ran += now - last;
        }
        last = now;
    }
}

// ---------------------------------------------------------------------------
// Thread bodies

/// Wakes once per period of its budget and signals the first program.
pub(crate) extern "C" fn ticker(ntfn: u64) -> ! {
    loop {
        TICKS.fetch_add(1, SeqCst);
        let _ = alaya_sys::signal(ntfn, 1);
        alaya_sys::yield_now();
    }
}

/// Reports the core it runs on, then waits forever.
extern "C" fn reporter(arg: u64) -> ! {
    let (i, done) = (arg & 0xffff, arg >> 16);
    SEEN[i as usize].store(alaya_sys::this_core(), SeqCst);
    DONE.fetch_add(1, SeqCst);
    let _ = alaya_sys::signal(done, 1);
    loop {
        let _ = alaya_sys::wait(done);
    }
}

extern "C" fn spinner(_: u64) -> ! {
    loop {
        COUNT_L.fetch_add(1, SeqCst);
    }
}

/// Set while the watcher checks that an equal shares the core with it.
static SHARING: AtomicU64 = AtomicU64::new(0);

/// On each signal: spin 20 ms and count how far the low-priority spinner
/// got meanwhile. When checking sharing, spin on (up to two seconds) until
/// the spinner got a turn: on a busy host 20 ms of wall time may hold no
/// timer tick at all.
extern "C" fn watcher(arg: u64) -> ! {
    let (go, done) = (arg & 0xffff, arg >> 16);
    loop {
        let _ = alaya_sys::wait(go);
        let a = COUNT_L.load(SeqCst);
        let start = alaya_sys::clock();
        let (end, cap) = (start + ms_to_ticks(20), start + ms_to_ticks(2000));
        loop {
            let now = alaya_sys::clock();
            let shared = SHARING.load(SeqCst) == 0 || COUNT_L.load(SeqCst) != a;
            if now >= cap || (now >= end && shared) {
                break;
            }
            COUNT_H.fetch_add(1, SeqCst);
        }
        RESULT.store(COUNT_L.load(SeqCst) - a, SeqCst);
        let _ = alaya_sys::signal(done, 1);
    }
}

/// Spins with no end: a budget must hold it.
extern "C" fn hog(_: u64) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

/// A server: `arg` = endpoint | reply << 16. Runs in the first program or,
/// as a copy, in the second.
extern "C" fn server(arg: u64) -> ! {
    let (ep, rep) = (arg & 0xffff, arg >> 16);
    let caps = Caps {
        recv: RECV_SLOT,
        reply: rep,
        send: 0,
    };
    let mut answer: Option<Msg> = None;
    loop {
        let got = match answer {
            Some(a) => alaya_sys::msg_call(alaya_sys::abi::call::REPLY_RECV, ep, &a, caps),
            None => alaya_sys::msg_call(alaya_sys::abi::call::RECV, ep, &Msg::default(), caps),
        };
        let Ok(m) = got else {
            answer = None;
            continue;
        };
        if m.info.has(MsgInfo::SIGNAL) {
            SIGNALS.fetch_or(m.badge, SeqCst);
            answer = None;
            continue;
        }
        answer = Some(match m.label() {
            NULL => Msg::default(),
            ISOLATION => {
                // SAFETY: the page at SHARED is mapped in this program.
                unsafe { (SHARED as *mut u64).write_volatile(0xa1a7a) };
                Msg::new(ISOLATION, &[MARK.load(SeqCst)])
            }
            SPIN => {
                work_ms(m.words[0]);
                Msg::new(SPIN, &[])
            }
            _ => {
                let mut cap = 0;
                if m.info.has(MsgInfo::CAP) {
                    cap = 1;
                    let _ = alaya_sys::signal(RECV_SLOT, 5);
                    let _ = alaya_sys::delete(At {
                        table: 1,
                        index: RECV_SLOT,
                    });
                }
                let n = m.info.words() as usize;
                let sum: u64 = m.words[..n].iter().sum();
                Msg::new(
                    m.label() + 1,
                    &[m.badge, sum, m.info.words(), cap, alaya_sys::this_core()],
                )
            }
        });
    }
}

/// Calls the server `n` times from its own core, checking every answer.
extern "C" fn client(arg: u64) -> ! {
    let (ep, done) = (arg & 0xffff, arg >> 16);
    let me = alaya_sys::this_core();
    let mut bad = 0;
    for i in 0..500u64 {
        let m = Msg::new(ECHO, &[i, me, 1]);
        match alaya_sys::ipc_call(ep, &m, Caps::default()) {
            Ok(r) if r.label() == ECHO + 1 && r.words[1] == i + me + 1 => {}
            _ => bad += 1,
        }
    }
    RESULT.fetch_add(bad, SeqCst);
    DONE.fetch_add(1, SeqCst);
    let _ = alaya_sys::signal(done, 1);
    loop {
        let _ = alaya_sys::wait(done);
    }
}

/// Calls the server with a 30 ms job on a 10 ms budget.
extern "C" fn slow_client(arg: u64) -> ! {
    let (ep, done) = (arg & 0xffff, arg >> 16);
    let start = alaya_sys::clock();
    let r = alaya_sys::ipc_call(ep, &Msg::new(SPIN, &[30]), Caps::default());
    RESULT.store(
        if r.is_ok() {
            alaya_sys::clock() - start
        } else {
            0
        },
        SeqCst,
    );
    let _ = alaya_sys::signal(done, 1);
    loop {
        let _ = alaya_sys::wait(done);
    }
}

/// Asks the server in the second program to write the shared page.
extern "C" fn isolation_caller(ep: u64) -> ! {
    let r = alaya_sys::ipc_call(ep, &Msg::new(ISOLATION, &[]), Caps::default());
    RESULT.store(if r.is_err() { 1 } else { 2 }, SeqCst);
    loop {
        alaya_sys::yield_now();
    }
}

extern "C" fn bad_opcode(_: u64) -> ! {
    alaya_sys::crash()
}

extern "C" fn bad_read(_: u64) -> ! {
    // SAFETY: deliberately faults: nothing is mapped at 16.
    let v = unsafe { (16 as *const u64).read_volatile() };
    loop {
        core::hint::black_box(v);
    }
}

// ---------------------------------------------------------------------------

/// Wait until thread `t` is in `state`.
pub(crate) fn until_state(t: u64, s: u64) {
    assert!(
        sleep_until(2000, || alaya_sys::thread_read(t).unwrap()[3] == s),
        "thread never reached state {s}"
    );
}

/// Run the milestone 3 checks. `checks` counts passed checks.
/// Returns the next free slot.
pub fn run(info: &BootInfo, big: u64, first_free: u64, pass: &mut dyn FnMut(&str)) -> u64 {
    HZ.store(info.clock_hz, SeqCst);
    let cores = info.cores;
    let last = cores - 1;
    let mut k = Kit {
        info,
        ut: 0,
        next: first_free,
        stacks: 0,
    };
    k.ut = k.slot();
    alaya_sys::retype(big, Kind::Untyped, 23, 1, k.at(k.ut)).unwrap();
    let ctl = info.sched_control;

    // A ticker: 50 µs every 2 ms, so the first program can sleep.
    let tick = k.make(Kind::Notification, 0);
    TICK.store(tick, SeqCst);
    k.thread(ticker, tick, 254, 50, 2000, 0);
    sleep_ms(10);
    assert!(TICKS.load(SeqCst) > 0, "the ticker never ran");
    pass("threads start, and a budgeted thread wakes every period");

    // One thread pinned to each core reports where it runs.
    let done = k.make(Kind::Notification, 0);
    let n = cores.min(64);
    for i in 0..n {
        k.thread(reporter, i | done << 16, 210, 1000, 1000, i + 1);
    }
    assert!(
        sleep_until(5000, || DONE.load(SeqCst) == n),
        "only {} of {n} threads ran",
        DONE.load(SeqCst)
    );
    for i in 0..n {
        assert!(
            SEEN[i as usize].load(SeqCst) == i,
            "thread {i} ran on the wrong core"
        );
    }
    pass("a thread pinned to each core runs there");

    // A budget of 2 ms every 10 ms holds a thread that never stops.
    let (_, hog_sc) = k.thread(hog, 0, 150, 2000, 10_000, last + 1);
    // The kernel charges by the clock, so time the host takes the virtual
    // core away while the hog runs counts as the hog's. One unlucky window
    // can then look like an overrun; a real overrun shows in every one, so
    // up to three windows are measured.
    let measure = || {
        let start = alaya_sys::clock();
        let used0 = alaya_sys::sc_info(hog_sc).unwrap()[2];
        sleep_ms(200);
        let used = alaya_sys::sc_info(hog_sc).unwrap()[2] - used0;
        (used / 1000, ticks_to_us(alaya_sys::clock() - start))
    };
    let mut tries = 0;
    let (share, elapsed_us) = loop {
        let (share, elapsed_us) = measure();
        tries += 1;
        if share <= elapsed_us / 5 + 3000 || tries == 3 {
            break (share, elapsed_us);
        }
    };
    say!(
        "a 2 ms / 10 ms budget used {} µs of {} µs ({}%)",
        share,
        elapsed_us,
        share * 100 / elapsed_us.max(1)
    );
    assert!(share <= elapsed_us / 5 + 3000, "the budget was exceeded");
    assert!(
        share >= elapsed_us / 20,
        "the thread got far less than its budget"
    );
    pass("a time budget holds a thread that never stops");

    // Priorities: a higher-priority thread keeps a lower one off its core;
    // equal priorities share it.
    let go = k.make(Kind::Notification, 0);
    let fin = k.make(Kind::Notification, 0);
    let (low, _) = k.thread(spinner, 0, 50, 1000, 1000, last + 1);
    let (high, _) = k.thread(watcher, go | fin << 16, 60, 1000, 1000, last + 1);
    sleep_ms(10);
    alaya_sys::signal(go, 1).unwrap();
    alaya_sys::wait(fin).unwrap();
    assert!(
        RESULT.load(SeqCst) == 0,
        "a lower-priority thread ran {} steps while a higher one was busy",
        RESULT.load(SeqCst)
    );
    alaya_sys::thread_priority(high, 50, 50).unwrap();
    SHARING.store(1, SeqCst);
    alaya_sys::signal(go, 1).unwrap();
    alaya_sys::wait(fin).unwrap();
    assert!(
        RESULT.load(SeqCst) > 0,
        "equal priorities did not share the core"
    );
    alaya_sys::thread_stop(low).unwrap();
    alaya_sys::thread_stop(high).unwrap();
    assert!(alaya_sys::thread_read(low).unwrap()[3] == state::STOPPED);
    pass("priorities: higher runs first, equal ones share the core");

    // Messages between threads of one program.
    let ep = k.make(Kind::Endpoint, 0);
    let rep = k.make(Kind::Reply, 0);
    let (srv, _) = k.thread(server, ep | rep << 16, 180, 1000, 1000, 0);
    let badged = k.slot();
    alaya_sys::mint(k.at(badged), k.at(ep), rights::ALL, 77).unwrap();
    let r = alaya_sys::ipc_call(
        badged,
        &Msg::new(ECHO, &[1, 2, 3, 4, 5, 6]),
        Caps::default(),
    )
    .unwrap();
    assert!(
        r.label() == ECHO + 1 && r.words[..4] == [77, 21, 6, 0],
        "bad answer {r:?}"
    );
    let lonely = k.make(Kind::Endpoint, 0);
    expect_err(
        alaya_sys::try_send(lonely, &Msg::default()),
        Error::WouldBlock,
    );
    let read_only = k.slot();
    alaya_sys::copy(k.at(read_only), k.at(ep), rights::READ | rights::GRANT).unwrap();
    expect_err(
        alaya_sys::ipc_call(read_only, &Msg::default(), Caps::default()),
        Error::NoRights,
    );
    // Pass a notification to the server; it signals it back.
    let nx = k.make(Kind::Notification, 0);
    let m = Msg {
        info: MsgInfo::new(ECHO, 0).with_cap(),
        ..Msg::default()
    };
    let r = alaya_sys::ipc_call(
        ep,
        &m,
        Caps {
            send: nx,
            ..Caps::default()
        },
    )
    .unwrap();
    assert!(r.words[3] == 1, "the capability did not arrive");
    assert!(
        alaya_sys::poll(nx).unwrap() == 5,
        "the passed notification was not signalled"
    );
    pass("call and reply, badges, rights, try_send, passing a capability");

    // A bound notification wakes the server in recv.
    let nb = k.make(Kind::Notification, 0);
    alaya_sys::thread_bind(srv, nb).unwrap();
    alaya_sys::signal(nb, 0x10).unwrap();
    assert!(
        sleep_until(2000, || SIGNALS.load(SeqCst) == 0x10),
        "the bound notification did not wake the server"
    );
    let r = alaya_sys::ipc_call(ep, &Msg::new(ECHO, &[1]), Caps::default()).unwrap();
    assert!(r.words[1] == 1);
    pass("notifications: signal, poll, wait, and binding to a server");

    // Many cores call one server at once.
    let all = k.make(Kind::Notification, 0);
    DONE.store(0, SeqCst);
    RESULT.store(0, SeqCst);
    let clients = cores.min(8);
    for i in 0..clients {
        k.thread(client, ep | all << 16, 120, 1000, 1000, i + 1);
    }
    assert!(
        sleep_until(60_000, || DONE.load(SeqCst) == clients),
        "clients did not finish"
    );
    assert!(
        RESULT.load(SeqCst) == 0,
        "{} wrong answers",
        RESULT.load(SeqCst)
    );
    say!("{clients} cores made {} calls to one server", clients * 500);
    pass("calls from every core at once all get the right answer");

    // A passive server runs on its callers' time.
    let ep2 = k.make(Kind::Endpoint, 0);
    let rep2 = k.make(Kind::Reply, 0);
    let (psrv, psc) = k.thread(server, ep2 | rep2 << 16, 100, 1000, 1000, 0);
    until_state(psrv, state::RECEIVING);
    alaya_sys::sc_unbind(psc).unwrap();
    let used0 = alaya_sys::sc_info(psc).unwrap()[2];
    for i in 0..20 {
        let r = alaya_sys::ipc_call(ep2, &Msg::new(ECHO, &[i]), Caps::default()).unwrap();
        assert!(r.words[1] == i);
    }
    assert!(
        alaya_sys::sc_info(psc).unwrap()[2] == used0,
        "the passive server used its own time"
    );
    // On a 10 ms / 100 ms budget, a 30 ms job takes at least three periods.
    let slow_done = k.make(Kind::Notification, 0);
    RESULT.store(0, SeqCst);
    k.thread(slow_client, ep2 | slow_done << 16, 120, 10_000, 100_000, 0);
    alaya_sys::wait(slow_done).unwrap();
    let took = ticks_to_us(RESULT.load(SeqCst));
    say!(
        "a 30 ms job on a 10 ms / 100 ms budget took {} ms",
        took / 1000
    );
    assert!(
        took >= 190_000,
        "the server ran past its caller's budget ({took} µs)"
    );
    pass("a passive server runs on its caller's time, and waits for refills");

    // Faults: to a fault endpoint as a message, or the thread stops.
    let fep = k.make(Kind::Endpoint, 0);
    let frep = k.make(Kind::Reply, 0);
    let t = k.make(Kind::Thread, 12);
    let tsc = k.make(Kind::SchedContext, 8);
    alaya_sys::thread_configure(t, info.table, info.space, fep).unwrap();
    let top = k.stack();
    alaya_sys::thread_write(
        t,
        bad_opcode as *const () as u64,
        alaya_sys::entry_stack(top),
        0,
        0,
        0,
    )
    .unwrap();
    alaya_sys::sc_configure(ctl, tsc, 1000, 1000, core_type::ANY, 0).unwrap();
    alaya_sys::sc_bind(tsc, t).unwrap();
    alaya_sys::thread_start(t).unwrap();
    let f = alaya_sys::recv(fep, frep).unwrap();
    assert!(
        f.label() == FAULT_LABEL && f.words[0] == 6,
        "bad fault message {f:?}"
    );
    let entry = bad_opcode as *const () as u64;
    assert!(
        (entry..entry + 64).contains(&f.words[3]),
        "fault at the wrong place"
    );
    alaya_sys::thread_stop(t).unwrap();
    let (t2, _) = k.thread(bad_read, 0, 100, 1000, 1000, 0);
    until_state(t2, state::STOPPED);
    pass("faults go to a fault endpoint as messages, or stop the thread");

    // A second program in its own address space: a copy of this one.
    let space = k.make(Kind::AddressSpace, 13);
    let pool = k.make(Kind::Untyped, 16);
    expect_err(
        alaya_sys::page_map(k.make(Kind::Page, 12), space, 0x1000, 0),
        Error::InvalidArgument,
    );
    alaya_sys::space_assign(space, pool).unwrap();
    // SAFETY: a symbol from the linker script.
    let text_end = unsafe { &__text_end as *const u8 as u64 };
    let mut at = info.image_start;
    while at < info.image_end {
        let p = k.page_at(info.space, SCRATCH, map::WRITE);
        for i in (0..4096).step_by(8) {
            // SAFETY: both pages are mapped here.
            unsafe {
                ((SCRATCH + i) as *mut u64).write_volatile(((at + i) as *const u64).read_volatile())
            };
        }
        alaya_sys::page_unmap(p).unwrap();
        let flags = if at < text_end {
            map::EXECUTE
        } else {
            map::WRITE
        };
        alaya_sys::page_map(p, space, at, flags).unwrap();
        at += 4096;
    }
    MARK.store(1, SeqCst);
    for i in 0..4 {
        k.page_at(space, STACK2 + i * 4096, map::WRITE);
    }
    let shared = k.page_at(info.space, SHARED, map::WRITE);
    let shared2 = k.slot();
    alaya_sys::copy(k.at(shared2), k.at(shared), rights::ALL).unwrap();
    alaya_sys::page_map(shared2, space, SHARED, map::WRITE).unwrap();
    expect_err(
        alaya_sys::page_map(shared, space, SHARED, map::WRITE),
        Error::SlotInUse,
    );
    let table2 = k.make(Kind::Table, 12);
    let ep3 = k.make(Kind::Endpoint, 0);
    let rep3 = k.make(Kind::Reply, 0);
    let t2a = |index| At {
        table: table2,
        index,
    };
    alaya_sys::copy(t2a(1), k.at(table2), rights::ALL).unwrap();
    alaya_sys::copy(t2a(2), k.at(ep3), rights::ALL).unwrap();
    alaya_sys::copy(t2a(3), k.at(rep3), rights::ALL).unwrap();
    let x = k.make(Kind::Thread, 12);
    let xsc = k.make(Kind::SchedContext, 8);
    let xfault = k.make(Kind::Endpoint, 0);
    alaya_sys::thread_configure(x, table2, space, xfault).unwrap();
    alaya_sys::thread_priority(x, 100, 100).unwrap();
    alaya_sys::thread_write(
        x,
        server as *const () as u64,
        alaya_sys::entry_stack(STACK2 + 0x4000),
        2 | 3 << 16,
        0,
        0,
    )
    .unwrap();
    alaya_sys::sc_configure(ctl, xsc, 1000, 1000, core_type::ANY, 0).unwrap();
    alaya_sys::sc_bind(xsc, x).unwrap();
    alaya_sys::thread_start(x).unwrap();
    until_state(x, state::RECEIVING);
    alaya_sys::sc_unbind(xsc).unwrap();
    let r = alaya_sys::ipc_call(ep3, &Msg::new(ISOLATION, &[]), Caps::default()).unwrap();
    assert!(
        r.words[0] == 0,
        "the second program sees the first one's memory"
    );
    // SAFETY: the shared page is mapped here.
    let v = unsafe { (SHARED as *const u64).read_volatile() };
    assert!(v == 0xa1a7a, "the shared page does not carry data");
    pass("a second program: its own memory, a shared page, calls between them");

    // How long a call and its reply take: between two programs, and inside
    // one, on one core.
    let measure = |ep: u64| {
        for _ in 0..200 {
            alaya_sys::ipc_call(ep, &Msg::new(NULL, &[]), Caps::default()).unwrap();
        }
        let rounds = 2000;
        let start = alaya_sys::clock();
        for _ in 0..rounds {
            alaya_sys::ipc_call(ep, &Msg::new(NULL, &[]), Caps::default()).unwrap();
        }
        (alaya_sys::clock() - start) / rounds
    };
    let between = measure(ep3);
    let inside = measure(ep2);
    let ns = |c: u64| c * 1_000_000_000 / HZ.load(SeqCst);
    say!(
        "ipc: call and reply round trip: {} cycles ({} ns) between programs, {} cycles ({} ns) inside one",
        between,
        ns(between),
        inside,
        ns(inside)
    );
    pass("call and reply measured");

    // Unmapping is seen at once: the shared page goes from the second
    // program, which then faults on it when asked to write it.
    alaya_sys::delete(k.at(shared2)).unwrap();
    RESULT.store(0, SeqCst);
    k.thread(isolation_caller, ep3, 120, 1000, 1000, 0);
    let f = alaya_sys::recv(xfault, frep).unwrap();
    assert!(
        f.label() == FAULT_LABEL && f.words[0] == 14 && f.words[2] == SHARED,
        "expected a page fault on the shared page, got {f:?}"
    );
    alaya_sys::thread_stop(x).unwrap();
    pass("a deleted page is unmapped at once");

    // Take everything back: threads running, waiting and stopped on every
    // core, their budgets, endpoints, the second program and its memory.
    let [r0, v0, _, _] = alaya_sys::all_counters();
    alaya_sys::revoke(k.at(k.ut)).unwrap();
    let [r1, v1, migrations, switches] = alaya_sys::all_counters();
    expect_err(alaya_sys::thread_read(srv), Error::InvalidHandle);
    assert!(
        RESULT.load(SeqCst) == 0,
        "a thread made from revoked memory still ran"
    );
    let again = k.make(Kind::Thread, 12);
    expect_err(alaya_sys::thread_start(x), Error::InvalidHandle);
    alaya_sys::thread_configure(again, info.table, info.space, 0).unwrap();
    say!(
        "revoked {} capabilities ({} restarts); {} thread switches, {} moves between cores so far",
        v1 - v0,
        r1 - r0,
        switches,
        migrations
    );
    pass("revoking takes back running threads, address spaces and all");
    k.next
}

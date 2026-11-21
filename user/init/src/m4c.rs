// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 4 checks, part 3: frame deadlines (decisions 70 and 81),
//! timeout messages (decision 55 follow-up) and the core-type helper
//! (decision 54).

use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use alaya_sys::abi::{BootInfo, TIMEOUT_LABEL, state};
use alaya_sys::cores::{self, Cores, Work};
use alaya_sys::{Caps, Kind, Msg};

use crate::m3::{Kit, ms_to_ticks, sleep_until, until_state, work_ms};
use crate::say;

static ORDER: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static DONE: AtomicU64 = AtomicU64::new(0);
/// A notification nobody signals: finished threads sleep on it.
static PARK: AtomicU64 = AtomicU64::new(0);
/// The threads of a check wait on this gate until all of them exist, so
/// the first one started cannot finish its work before the others are
/// there. Blocking, not spinning, so the core stays free meanwhile.
static GATE: AtomicU64 = AtomicU64::new(0);
static STARTED: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
static COUNT: AtomicU64 = AtomicU64::new(0);

fn wait_for_go() {
    let _ = alaya_sys::wait(GATE.load(SeqCst));
}

/// Once every gated thread waits at the gate, let them all go.
fn release() {
    let n = COUNT.swap(0, SeqCst) as usize;
    for t in &STARTED[..n] {
        until_state(t.load(SeqCst), state::WAITING_SIGNAL);
    }
    for _ in 0..n {
        alaya_sys::signal(GATE.load(SeqCst), 1).unwrap();
    }
}

fn record(id: u64) {
    let n = DONE.fetch_add(1, SeqCst);
    if let Some(o) = ORDER.get(n as usize) {
        o.store(id, SeqCst);
    }
}

fn order(n: usize) -> [u64; 8] {
    core::array::from_fn(|i| if i < n { ORDER[i].load(SeqCst) } else { 0 })
}

fn reset() {
    COUNT.store(0, SeqCst);
    DONE.store(0, SeqCst);
    for o in &ORDER {
        o.store(0, SeqCst);
    }
}

/// `arg` = id | ms << 8 | notification << 32: work, record, then sleep.
extern "C" fn worker(arg: u64) -> ! {
    let (id, ms, n) = (arg & 0xff, (arg >> 8) & 0xffff, arg >> 32);
    wait_for_go();
    work_ms(ms);
    record(id);
    loop {
        let _ = alaya_sys::wait(n);
    }
}

/// `arg` = endpoint | reply << 16: works as long as each call asks.
extern "C" fn spin_server(arg: u64) -> ! {
    let (ep, rep) = (arg & 0xffff, arg >> 16);
    let mut got = alaya_sys::recv(ep, rep);
    loop {
        got = match got {
            Ok(m) => {
                work_ms(m.words[0]);
                alaya_sys::reply_recv(ep, rep, &Msg::new(0, &[]))
            }
            Err(_) => alaya_sys::recv(ep, rep),
        };
    }
}

/// `arg` = id | ms << 8 | endpoint << 32: one call, record, then sleep.
extern "C" fn caller(arg: u64) -> ! {
    let (id, ms, ep) = (arg & 0xff, (arg >> 8) & 0xffff, arg >> 32);
    wait_for_go();
    let _ = alaya_sys::ipc_call(ep, &Msg::new(0, &[ms]), Caps::default());
    record(id);
    loop {
        let _ = alaya_sys::wait(PARK.load(SeqCst));
    }
}

/// A thread whose context is configured and given a deadline before it
/// starts. Returns (thread, context).
#[allow(clippy::too_many_arguments)]
fn spawn(
    k: &mut Kit,
    entry: extern "C" fn(u64) -> !,
    arg: u64,
    prio: u64,
    budget_us: u64,
    period_us: u64,
    pin: u64,
    deadline: u64,
    fault: u64,
) -> (u64, u64) {
    let t = k.make(Kind::Thread, 12);
    let sc = k.make(Kind::SchedContext, 8);
    alaya_sys::thread_configure(t, k.info.table, k.info.space, fault).unwrap();
    alaya_sys::thread_priority(t, prio, prio).unwrap();
    let top = k.stack();
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
        k.info.sched_control,
        sc,
        budget_us,
        period_us,
        alaya_sys::abi::core_type::ANY,
        pin,
    )
    .unwrap();
    alaya_sys::sc_deadline(sc, deadline).unwrap();
    alaya_sys::sc_bind(sc, t).unwrap();
    alaya_sys::thread_start(t).unwrap();
    if entry as *const () != spin_server as *const () {
        let n = COUNT.fetch_add(1, SeqCst) as usize;
        STARTED[n].store(t, SeqCst);
    }
    (t, sc)
}

/// Run the checks; returns the next free slot.
pub fn run(info: &BootInfo, big: u64, first_free: u64, pass: &mut dyn FnMut(&str)) -> u64 {
    let mut k = Kit {
        info,
        ut: 0,
        next: first_free,
        // The part before keeps its stacks; start after them.
        stacks: 64,
    };
    k.ut = k.slot();
    alaya_sys::retype(big, Kind::Untyped, 22, 1, k.at(k.ut)).unwrap();
    let park = k.make(Kind::Notification, 0);
    PARK.store(park, SeqCst);
    let gate = k.make(Kind::Notification, 0);
    GATE.store(gate, SeqCst);
    // Everything runs on one core, the last.
    let pin = info.cores;
    let now = alaya_sys::clock;

    // Earliest deadline first among equals; priority always wins.
    reset();
    let t0 = now();
    let job = |id: u64| id | 30 << 8 | park << 32;
    spawn(
        &mut k,
        worker,
        job(1),
        120,
        1000,
        1000,
        pin,
        t0 + ms_to_ticks(300),
        0,
    );
    spawn(
        &mut k,
        worker,
        job(2),
        120,
        1000,
        1000,
        pin,
        t0 + ms_to_ticks(100),
        0,
    );
    spawn(
        &mut k,
        worker,
        job(3),
        120,
        1000,
        1000,
        pin,
        t0 + ms_to_ticks(200),
        0,
    );
    spawn(
        &mut k,
        worker,
        job(4),
        130,
        1000,
        1000,
        pin,
        t0 + ms_to_ticks(1000),
        0,
    );
    release();
    assert!(
        sleep_until(20_000, || DONE.load(SeqCst) == 4),
        "the workers did not finish"
    );
    let o = order(4);
    say!(
        "deadlines: four 30 ms jobs on one core finished in the order {}, {}, {}, {} (4 is more important, then soonest deadline first)",
        o[0],
        o[1],
        o[2],
        o[3]
    );
    assert!(o[..4] == [4, 2, 3, 1], "wrong order {:?}", &o[..4]);
    pass("frame deadlines: soonest deadline first among equals, priority always wins");

    // Deadlines travel with calls: a server works for the caller whose
    // deadline is sooner first.
    reset();
    let mut servers = [0u64; 2];
    for s in servers.iter_mut() {
        let ep = k.make(Kind::Endpoint, 0);
        let rep = k.make(Kind::Reply, 0);
        let (t, sc) = spawn(
            &mut k,
            spin_server,
            ep | rep << 16,
            120,
            1000,
            1000,
            pin,
            0,
            0,
        );
        until_state(t, state::RECEIVING);
        alaya_sys::sc_unbind(sc).unwrap();
        *s = ep;
    }
    let t0 = now();
    spawn(
        &mut k,
        caller,
        1 | 30 << 8 | servers[0] << 32,
        120,
        1000,
        1000,
        pin,
        t0 + ms_to_ticks(400),
        0,
    );
    spawn(
        &mut k,
        caller,
        2 | 30 << 8 | servers[1] << 32,
        120,
        1000,
        1000,
        pin,
        t0 + ms_to_ticks(100),
        0,
    );
    release();
    assert!(
        sleep_until(20_000, || DONE.load(SeqCst) == 2),
        "the callers did not finish"
    );
    let o = order(2);
    assert!(
        o[..2] == [2, 1],
        "the server did not follow its caller's deadline: {:?}",
        &o[..2]
    );
    pass("deadlines travel with calls: servers work on the soonest deadline first");

    // A missed deadline is recorded, with the thread that missed it.
    reset();
    let t0 = now();
    let (mt, msc) = spawn(
        &mut k,
        worker,
        9 | 30 << 8 | park << 32,
        120,
        1000,
        1000,
        pin,
        t0 + ms_to_ticks(5),
        0,
    );
    release();
    assert!(sleep_until(20_000, || DONE.load(SeqCst) == 1));
    let (misses, by, missed) = alaya_sys::sc_deadline(msc, 0).unwrap();
    let who = alaya_sys::inspect(k.at(mt)).unwrap().object;
    assert!(
        missed && misses == 1 && by == who,
        "miss not recorded: {misses} misses, by {by:#x}, expected {who:#x}"
    );
    say!("deadlines: a 30 ms job due in 5 ms was recorded as missed, by the thread that ran it");
    pass("a missed deadline is recorded with the part that missed it");

    // Timeout messages: a server that runs out of its caller's time tells
    // its handler, if it asked to.
    reset();
    let fault = k.make(Kind::Endpoint, 0);
    let rep = k.make(Kind::Reply, 0);
    let ep = k.make(Kind::Endpoint, 0);
    let srep = k.make(Kind::Reply, 0);
    let (st, ssc) = spawn(
        &mut k,
        spin_server,
        ep | srep << 16,
        110,
        1000,
        1000,
        pin,
        0,
        fault,
    );
    until_state(st, state::RECEIVING);
    alaya_sys::sc_unbind(ssc).unwrap();
    alaya_sys::thread_timeouts(st, true).unwrap();
    // 2 ms every 50 ms; the job takes 10 ms of the caller's time.
    spawn(
        &mut k,
        caller,
        7 | 10 << 8 | ep << 32,
        105,
        2000,
        50_000,
        pin,
        0,
        0,
    );
    release();
    let m = alaya_sys::recv(fault, rep).unwrap();
    assert!(
        m.label() == TIMEOUT_LABEL,
        "expected a timeout message, got {m:?}"
    );
    say!(
        "timeouts: the server ran out of its caller's time after {} us; its handler was told, and let it wait for the next refill",
        m.words[1] / 1000
    );
    // No more messages: from now on it just waits, as before.
    alaya_sys::thread_timeouts(st, false).unwrap();
    alaya_sys::reply(rep, &Msg::new(0, &[])).unwrap();
    assert!(
        sleep_until(20_000, || DONE.load(SeqCst) == 1),
        "the slow call never finished"
    );
    pass(
        "timeout messages: a server out of lent time tells its handler, then waits for the refill",
    );

    // The core-type helper.
    let mine = Cores::from_types(&info.core_types[..info.cores as usize]);
    let hybrid = Cores {
        performance: 6,
        efficiency: 8,
        plain: 0,
    };
    use alaya_sys::abi::core_type::{ANY, EFFICIENCY, PERFORMANCE};
    assert!(cores::choose(&hybrid, Work::Game) == PERFORMANCE);
    assert!(cores::choose(&hybrid, Work::Interactive) == PERFORMANCE);
    assert!(cores::choose(&hybrid, Work::Background) == EFFICIENCY);
    assert!(cores::choose(&hybrid, Work::System) == ANY);
    let flat = Cores {
        performance: 0,
        efficiency: 0,
        plain: 8,
    };
    assert!(
        cores::choose(&flat, Work::Game) == ANY && cores::choose(&flat, Work::Background) == ANY
    );
    let sc = k.make(Kind::SchedContext, 8);
    let chosen = cores::configure(info.sched_control, sc, 1000, 10_000, &mine, Work::Game).unwrap();
    say!(
        "cores: {} performance, {} efficiency, {} of one kind; a game's budget gets core type {}",
        mine.performance,
        mine.efficiency,
        mine.plain,
        chosen
    );
    assert!(mine.hybrid() || chosen == ANY);
    pass(
        "the core-type helper puts games on performance cores and background work on efficiency cores",
    );

    alaya_sys::revoke(k.at(k.ut)).unwrap();
    k.next
}

// SPDX-License-Identifier: GPL-3.0-only
//! The kernel fuzzer (boot flag `fuzz=<seconds>`).
//!
//! A program must never be able to crash the kernel, however it calls it.
//! So this runs threads on every core that make random system calls, as
//! fast as they can, on a sandbox: a program of their own (a copy of this
//! one in its own address space) with a capability table full of things to
//! break: memory to carve up, pages, endpoints, notifications, reply
//! objects, time budgets, other threads, each other. Their calls are random
//! but aimed: mostly real operations on real handles with odd arguments,
//! so they reach deep into the kernel instead of bouncing off at the door.
//!
//! The first program watches. A fuzzing thread that stops making progress
//! (it waits forever, faulted, or another one stopped it) is reset and
//! started again. At the end everything is taken back with one revoke
//! while the threads still run, and the kernel must still work.

use core::arch::asm;
use core::sync::atomic::{AtomicU64, Ordering::Relaxed, Ordering::SeqCst};

use alaya_sys::abi::{BootInfo, call, core_type, map, op, state};
use alaya_sys::{At, Error, Kind, rights};

use crate::say;

/// Fuzzing threads' stacks, 64 KiB apart, in the sandbox.
const STACKS: u64 = 0x5000_0000_0000;
/// A page shared with the sandbox: one progress counter per thread.
const COUNTERS: u64 = 0x6000_0000_0000;
/// Where the fuzzing threads aim their page mappings.
const WINDOW: u64 = 0x7000_0000_0000;
/// The watcher's ticker stack, in this program.
const TICKER_STACK: u64 = 0x5800_0000_0000;

const MAX_FUZZERS: u64 = 8;
const VICTIMS: u64 = 2;
/// Fuzzing threads may raise a priority up to this, well under this
/// program's, so the watcher always gets to run.
const FUZZ_PRIO: u64 = 100;

/// The sandbox table: what is in which slot.
mod slot {
    pub const TABLE: u64 = 1;
    pub const SPACE: u64 = 2;
    pub const CONTROL: u64 = 3;
    pub const MEMORY: u64 = 4;
    pub const ENDPOINTS: u64 = 5; // 4
    pub const NOTIFICATIONS: u64 = 9; // 2
    pub const REPLIES: u64 = 11; // 4
    pub const PAGES: u64 = 15; // 8
    pub const BIG_PAGE: u64 = 23;
    pub const VICTIMS: u64 = 24; // 2
    pub const VICTIM_SCS: u64 = 26; // 2
    pub const FUZZERS: u64 = 28; // up to 8
    /// Send-only endpoints of the system's helpers: the registry, the
    /// memory helper, the disk keeper.
    pub const HELPERS: u64 = 36; // 3
    /// Slots from here on are free for the threads to fill.
    pub const FREE: u64 = 39;
    pub const BITS: u8 = 8;
    pub const COUNT: u64 = 1 << BITS;
}

static TICK: AtomicU64 = AtomicU64::new(0);
static HZ: AtomicU64 = AtomicU64::new(1);

unsafe extern "C" {
    static __text_end: u8;
}

// ---------------------------------------------------------------------------
// The fuzzing threads

/// xorshift64*: small, fast, and good enough to pick calls.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// A handle: usually one that names something.
    fn handle(&mut self) -> u64 {
        match self.below(10) {
            0..=7 => self.below(slot::FREE + 40),
            8 => self.below(slot::COUNT + 50),
            _ => self.next(),
        }
    }

    /// An argument: small numbers, edges, addresses, times, or noise.
    fn value(&mut self) -> u64 {
        match self.below(16) {
            0..=4 => self.below(slot::FREE + 40),
            5 => self.below(slot::COUNT + 8),
            6 => 0,
            7 => u64::MAX,
            8 => 1 << self.below(64),
            9 => WINDOW + self.below(64) * 4096,
            10 => WINDOW + (self.below(1 << 30) & !0x1f_ffff),
            11 => self.next(),
            12 => self.below(16) * 4096,
            13 => self.below(20_000),
            14 => 0xffff_8000_0000_0000 + self.below(1 << 40),
            _ => 0x7fff_ffff_f000 - self.below(3) * 0x1000,
        }
    }
}

/// One system call with every register of the message ABI set.
#[inline(never)]
fn raw(nr: u64, rdi: u64, rsi: u64, a: [u64; 6], r14: u64, r15: u64) -> u64 {
    let err;
    // SAFETY: a system call only changes the registers named here; the
    // kernel must cope with any values in them.
    unsafe {
        asm!(
            "syscall",
            inout("rax") nr => err,
            inout("rdi") rdi => _,
            inout("rsi") rsi => _,
            inout("rdx") a[0] => _,
            inout("r10") a[1] => _,
            inout("r8") a[2] => _,
            inout("r9") a[3] => _,
            inout("r12") a[4] => _,
            inout("r13") a[5] => _,
            in("r14") r14,
            in("r15") r15,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    err
}

const OPS: [u64; 23] = [
    op::COPY,
    op::MINT,
    op::MOVE,
    op::DELETE,
    op::REVOKE,
    op::INSPECT,
    op::RETYPE,
    op::THREAD_CONFIGURE,
    op::THREAD_PRIORITY,
    op::THREAD_WRITE,
    op::THREAD_READ,
    op::THREAD_START,
    op::THREAD_STOP,
    op::THREAD_BIND,
    op::SC_BIND,
    op::SC_UNBIND,
    op::SC_INFO,
    op::SC_CONFIGURE,
    op::SPACE_ASSIGN,
    op::PAGE_MAP,
    op::PAGE_UNMAP,
    op::PAGE_ADDRESS,
    0,
];

fn one_call(r: &mut Rng) {
    let mut a = [0; 6];
    for v in &mut a {
        *v = r.value();
    }
    let h = r.handle();
    match r.below(100) {
        // Operations on capabilities.
        0..=54 => {
            let o = if r.below(20) == 0 {
                r.below(100)
            } else {
                OPS[r.below(OPS.len() as u64) as usize]
            };
            match o {
                op::RETYPE => {
                    a[0] = r.below(14);
                    a[1] = match r.below(4) {
                        0 => r.below(64),
                        1 => 12,
                        _ => r.below(16),
                    };
                    a[2] = 1 + r.below(3);
                    if r.below(4) != 0 {
                        a[3] = slot::TABLE;
                        a[4] = slot::FREE + r.below(slot::COUNT - slot::FREE);
                    }
                }
                op::COPY | op::MINT | op::MOVE if r.below(4) != 0 => {
                    a[0] = slot::FREE + r.below(slot::COUNT - slot::FREE);
                    a[1] = slot::TABLE;
                    a[2] = r.below(slot::FREE + 40);
                    a[3] = r.below(256);
                }
                op::THREAD_WRITE if r.below(2) == 0 => {
                    // Somewhere that faults, rather than into this code.
                    a[0] = WINDOW + r.below(64) * 4096;
                }
                _ => {}
            }
            let h = if (op::COPY..=op::INSPECT).contains(&o) && r.below(2) == 0 {
                slot::TABLE
            } else {
                h
            };
            let h = if (5..15).contains(&h) && r.below(8) != 0 {
                0
            } else {
                h
            };
            raw(call::CALL, h, o, a, 0, 0);
        }
        // Messages.
        55..=89 => {
            // Calls that may wait forever stall the thread until the
            // watcher resets it, so most are ones that never wait.
            let nr = if r.below(4) == 0 {
                r.below(8)
            } else {
                [call::TRY_SEND, call::SIGNAL, call::POLL][r.below(3) as usize]
            };
            let info = r.below(1 << 20);
            let r14 = if r.below(2) == 0 {
                r.handle() | (slot::FREE + r.below(slot::COUNT - slot::FREE)) << 32
            } else {
                r.value()
            };
            raw(nr, h, info, a, r14, r.handle());
        }
        // Aimed at the helpers: calls with labels and words they know,
        // or nearly.
        90..=92 => {
            let h = slot::HELPERS + r.below(3);
            let label = if r.below(4) == 0 {
                r.below(1 << 12)
            } else {
                r.below(20)
            };
            let words = r.below(7);
            let info = label << 8 | words | if r.below(4) == 0 { 1 << 4 } else { 0 };
            for (i, v) in a.iter_mut().enumerate() {
                *v = match r.below(4) {
                    0 => r.next(),
                    1 => r.below(30),
                    2 if i == 0 => 0x6b736964, // "disk"
                    _ => r.below(4096),
                };
            }
            let r14 = r.handle() | (slot::FREE + r.below(slot::COUNT - slot::FREE)) << 32;
            let nr = if r.below(4) == 0 {
                call::TRY_SEND
            } else {
                call::CALL
            };
            raw(nr, h, info, a, r14, 0);
        }
        93..=94 => {
            raw(call::YIELD, 0, 0, a, 0, 0);
        }
        // Debug calls, but never printing: a report must be refused,
        // since only the first thread may end the boot tests.
        95..=97 => {
            let which = [1, 2, 3][r.below(3) as usize];
            raw(call::DEBUG, which, r.value(), a, 0, 0);
        }
        _ => {
            let nr = if r.below(2) == 0 {
                10 + r.below(100)
            } else {
                r.next()
            };
            raw(nr, h, r.value(), a, r.value(), r.value());
        }
    }
}

/// A fuzzing thread: random calls forever, counting each one.
extern "C" fn fuzzer(idx: u64, seed: u64) -> ! {
    // SAFETY: the counters page is mapped in the sandbox.
    let counter = unsafe { &*((COUNTERS as *const AtomicU64).add((idx % 512) as usize)) };
    let mut r = Rng(seed | 1);
    loop {
        one_call(&mut r);
        counter.fetch_add(1, Relaxed);
    }
}

/// Wakes every period of its budget and signals the watcher.
extern "C" fn ticker(ntfn: u64) -> ! {
    loop {
        let _ = alaya_sys::signal(ntfn, 1);
        alaya_sys::yield_now();
    }
}

// ---------------------------------------------------------------------------
// The watcher

struct Kit {
    table: u64,
    ut: u64,
    next: u64,
}

impl Kit {
    fn slot(&mut self) -> u64 {
        let h = self.next;
        self.next += 1;
        h
    }

    fn at(&self, index: u64) -> At {
        At {
            table: self.table,
            index,
        }
    }

    fn make(&mut self, kind: Kind, bits: u8) -> u64 {
        let h = self.slot();
        alaya_sys::retype(self.ut, kind, bits, 1, self.at(h)).unwrap();
        h
    }

    fn page_at(&mut self, space: u64, at: u64, flags: u64) -> u64 {
        let p = self.make(Kind::Page, 12);
        alaya_sys::page_map(p, space, at, flags).unwrap();
        p
    }
}

fn sleep_ms(ms: u64) {
    let end = alaya_sys::rdtsc() + ms * HZ.load(SeqCst) / 1000;
    while alaya_sys::rdtsc() < end {
        alaya_sys::wait(TICK.load(SeqCst)).unwrap();
    }
}

/// After the attack: the registry still finds the disk, the keeper still
/// reads it, and the memory helper still keeps its books.
fn keeper_jobs() -> u64 {
    let ctl = crate::m4b::HELPERS[4].load(SeqCst);
    alaya_sys::ipc_call(
        ctl,
        &alaya_sys::Msg::new(alaya_programs::keeper::MORE, &[]),
        alaya_sys::Caps::default(),
    )
    .map_or(0, |m| m.words[3])
}

fn check_helpers(jobs_before: u64) {
    use alaya_programs::{keeper as kp, memory, registry, status};
    use alaya_sys::{Caps, Msg};
    let h: [u64; 6] = core::array::from_fn(|i| crate::m4b::HELPERS[i].load(SeqCst));
    let (client, reg, ctl, mem) = (h[2], h[3], h[4], h[5]);
    let call = |ep: u64, label: u64, words: &[u64]| {
        alaya_sys::ipc_call(ep, &Msg::new(label, words), Caps::default()).unwrap()
    };
    let n = registry::name("disk");
    assert!(
        call(reg, registry::LOOKUP, &n).words[0] == 0,
        "the registry lost the disk"
    );
    let s = call(ctl, kp::STATS, &[]).words;
    assert!(s[5] != kp::state::OFF, "the fuzzer switched the disk off");
    for sector in 0..8 {
        let r = call(client, kp::READ, &[sector, 0]).words;
        assert!(
            r[0] == status::OK,
            "the keeper failed a read after the fuzzer: {r:?}"
        );
    }
    let u = call(mem, memory::USAGE, &[]).words;
    assert!(
        u[1] == 2 << 20 && u[0] <= u[1],
        "the memory helper lost its books: {u:?}"
    );
    say!(
        "fuzz: the registry, the memory helper and the disk keeper still answer correctly (the keeper did {} disk jobs for the fuzzer)",
        keeper_jobs() - jobs_before - 8
    );
}

/// Run the fuzzer for `seconds`; `first_free` is the first free slot of
/// this program's table.
pub fn run(info: &BootInfo, big: u64, first_free: u64, seconds: u64) {
    HZ.store(info.tsc_hz, SeqCst);
    let mut k = Kit {
        table: info.table,
        ut: 0,
        next: first_free,
    };
    k.ut = k.slot();
    if alaya_sys::retype(big, Kind::Untyped, 23, 1, k.at(k.ut)).is_err() {
        alaya_sys::retype(big, Kind::Untyped, 22, 1, k.at(k.ut)).unwrap();
    }
    let ctl = info.sched_control;
    let fuzzers = info.cores.min(MAX_FUZZERS);
    say!(
        "fuzz: {} threads on {} cores for {seconds} s",
        fuzzers + VICTIMS,
        info.cores
    );

    // A ticker, so the watcher can sleep.
    let tick = k.make(Kind::Notification, 0);
    TICK.store(tick, SeqCst);
    for i in 0..4 {
        k.page_at(info.space, TICKER_STACK + i * 4096, map::WRITE);
    }
    let t = k.make(Kind::Thread, 12);
    let sc = k.make(Kind::SchedContext, 8);
    alaya_sys::thread_configure(t, info.table, info.space, 0).unwrap();
    alaya_sys::thread_priority(t, 254, 254).unwrap();
    alaya_sys::thread_write(
        t,
        ticker as *const () as u64,
        TICKER_STACK + 0x4000 - 8,
        tick,
        0,
        0,
    )
    .unwrap();
    alaya_sys::sc_configure(ctl, sc, 50, 2000, core_type::ANY, 0).unwrap();
    alaya_sys::sc_bind(sc, t).unwrap();
    alaya_sys::thread_start(t).unwrap();

    // The sandbox: an address space with a copy of this program.
    let space = k.make(Kind::AddressSpace, 13);
    let pool = k.make(Kind::Untyped, 18);
    alaya_sys::space_assign(space, pool).unwrap();
    // SAFETY: a symbol from the linker script.
    let text_end = unsafe { &__text_end as *const u8 as u64 };
    let scratch = STACKS - 0x1000;
    let mut at = info.image_start;
    while at < info.image_end {
        let p = k.page_at(info.space, scratch, map::WRITE);
        for i in (0..4096).step_by(8) {
            // SAFETY: both pages are mapped here.
            unsafe {
                ((scratch + i) as *mut u64).write_volatile(((at + i) as *const u64).read_volatile())
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
    let counters = k.page_at(info.space, COUNTERS, map::WRITE);
    let counters2 = k.slot();
    alaya_sys::copy(k.at(counters2), k.at(counters), rights::ALL).unwrap();
    alaya_sys::page_map(counters2, space, COUNTERS, map::WRITE).unwrap();
    let threads = fuzzers + VICTIMS;
    for i in 0..threads {
        for j in 0..4 {
            k.page_at(space, STACKS + i * 0x10000 + 0xc000 + j * 4096, map::WRITE);
        }
    }

    // The sandbox's table, full of things to break.
    let table = k.make(Kind::Table, slot::BITS);
    let sb = |index| At { table, index };
    alaya_sys::copy(sb(slot::TABLE), k.at(table), rights::ALL).unwrap();
    alaya_sys::copy(sb(slot::SPACE), k.at(space), rights::ALL).unwrap();
    alaya_sys::copy(sb(slot::CONTROL), k.at(ctl), rights::ALL).unwrap();
    let into = |k: &Kit, kind, bits, count, first| {
        alaya_sys::retype(k.ut, kind, bits, count, sb(first)).unwrap();
    };
    into(&k, Kind::Untyped, 20, 1, slot::MEMORY);
    into(&k, Kind::Endpoint, 0, 4, slot::ENDPOINTS);
    into(&k, Kind::Notification, 0, 2, slot::NOTIFICATIONS);
    into(&k, Kind::Reply, 0, 4, slot::REPLIES);
    into(&k, Kind::Page, 12, 8, slot::PAGES);
    into(&k, Kind::Page, 21, 1, slot::BIG_PAGE);
    let helpers: [u64; 3] = core::array::from_fn(|i| crate::m4b::HELPERS[i].load(SeqCst));
    for (i, &h) in helpers.iter().enumerate() {
        if h != 0 {
            alaya_sys::copy(
                sb(slot::HELPERS + i as u64),
                k.at(h),
                rights::WRITE | rights::GRANT,
            )
            .unwrap();
        }
    }

    // The threads: fuzzers pinned one per core, and two victims that start
    // the same way but which nobody restarts.
    let mut handles = [(0u64, 0u64); (MAX_FUZZERS + VICTIMS) as usize];
    let seed = alaya_sys::rdtsc();
    let reset = |t: u64, i: u64, seed: u64| -> Result<(), Error> {
        alaya_sys::thread_configure(t, table, space, 0)?;
        alaya_sys::thread_priority(t, if i < fuzzers { FUZZ_PRIO } else { 90 }, FUZZ_PRIO)?;
        alaya_sys::thread_write(
            t,
            fuzzer as *const () as u64,
            STACKS + i * 0x10000 + 0x10000 - 8,
            i,
            seed,
            0,
        )
    };
    for i in 0..threads {
        let t = k.make(Kind::Thread, 12);
        let sc = k.make(Kind::SchedContext, 8);
        let pin = if i < fuzzers { i + 1 } else { 0 };
        reset(t, i, seed ^ i << 32).unwrap();
        alaya_sys::sc_configure(ctl, sc, 1000, 1000, core_type::ANY, pin).unwrap();
        alaya_sys::sc_bind(sc, t).unwrap();
        let (dst, sc_dst) = if i < fuzzers {
            (slot::FUZZERS + i, None)
        } else {
            (
                slot::VICTIMS + i - fuzzers,
                Some(slot::VICTIM_SCS + i - fuzzers),
            )
        };
        alaya_sys::copy(sb(dst), k.at(t), rights::ALL).unwrap();
        if let Some(s) = sc_dst {
            alaya_sys::copy(sb(s), k.at(sc), rights::ALL).unwrap();
        }
        handles[i as usize] = (t, sc);
    }
    let jobs_before = if helpers[0] != 0 { keeper_jobs() } else { 0 };
    for &(t, _) in &handles[..threads as usize] {
        alaya_sys::thread_start(t).unwrap();
    }

    // Watch: reset any fuzzer that made no progress since the last look.
    // SAFETY: the counters page is mapped here.
    let count = |i: u64| unsafe { (*(COUNTERS as *const AtomicU64).add(i as usize)).load(Relaxed) };
    let mut last = [0u64; MAX_FUZZERS as usize];
    let (mut restarts, mut failed_restarts) = (0u64, 0u64);
    let start = alaya_sys::rdtsc();
    let end = start + seconds * info.tsc_hz;
    let mut report = start + 5 * info.tsc_hz;
    let mut round = 0u64;
    while alaya_sys::rdtsc() < end {
        sleep_ms(10);
        round += 1;
        for i in 0..fuzzers {
            let c = count(i);
            if c != last[i as usize] {
                last[i as usize] = c;
                continue;
            }
            let t = handles[i as usize].0;
            let _ = alaya_sys::thread_stop(t);
            let restarted =
                reset(t, i, seed ^ round << 8 ^ i).and_then(|_| alaya_sys::thread_start(t));
            match restarted {
                Ok(()) => restarts += 1,
                // Another fuzzer started it again meanwhile, say.
                Err(_) => failed_restarts += 1,
            }
        }
        if alaya_sys::rdtsc() >= report {
            report += 5 * info.tsc_hz;
            let calls: u64 = (0..threads).map(count).sum();
            say!(
                "fuzz: {} s, {calls} calls, {restarts} restarts",
                (alaya_sys::rdtsc() - start) / info.tsc_hz
            );
        }
    }

    // How the threads stand, then take everything back while they run.
    let mut states = [0u64; 8];
    for &(t, _) in &handles[..threads as usize] {
        if let Ok(r) = alaya_sys::thread_read(t) {
            states[(r[3] as usize).min(7)] += 1;
        }
    }
    let calls: u64 = (0..threads).map(count).sum();
    let [r0, v0, _, _] = alaya_sys::all_counters();
    alaya_sys::revoke(k.at(k.ut)).unwrap();
    let [r1, v1, _, _] = alaya_sys::all_counters();
    for &(t, _) in &handles[..threads as usize] {
        crate::expect_err(alaya_sys::thread_read(t), Error::InvalidHandle);
    }

    // The helpers still answer, and answer right.
    if helpers[0] != 0 {
        check_helpers(jobs_before);
    }

    // The kernel still works.
    let n = k.make(Kind::Notification, 0);
    alaya_sys::signal(n, 5).unwrap();
    assert!(
        alaya_sys::poll(n).unwrap() == 5,
        "a notification lost its bits"
    );
    alaya_sys::revoke(k.at(k.ut)).unwrap();
    say!(
        "fuzz: done: {calls} calls, {restarts} restarts ({failed_restarts} raced), \
         {} ready and {} stopped at the end; revoked {} capabilities ({} restarts)",
        states[state::READY as usize],
        states[state::STOPPED as usize],
        v1 - v0,
        r1 - r0
    );
}

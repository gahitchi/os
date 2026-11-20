// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 4 checks, part 1: the foundation additions (decision 77).
//!
//! - the processor protections the kernel reports, and UMIP really
//!   stopping a program from reading the kernel's descriptor tables;
//! - every kind of kernel object carries its format label;
//! - the big kernel lock, measured while every core makes calls.
//!
//! (SMEP and SMAP are checked by the kernel's own fault tests, `cargo
//! xtask test --fault smap` and `--fault smep`.)

use core::sync::atomic::{AtomicU64, Ordering::SeqCst};

use alaya_sys::abi::{BootInfo, FAULT_LABEL, core_type, format, protect};
use alaya_sys::{Caps, Kind, Msg};

use crate::m3::{HZ, Kit, TICK, TICKS, sleep_ms, ticker};
use crate::say;

static CALLS: AtomicU64 = AtomicU64::new(0);
static STOP: AtomicU64 = AtomicU64::new(0);

/// Reads the interrupt table's address: UMIP must make this fault.
extern "C" fn read_idt(_: u64) -> ! {
    let mut buf = [0u8; 16];
    // SAFETY: SIDT writes 10 bytes into the buffer (or faults).
    unsafe { core::arch::asm!("sidt [{}]", in(reg) buf.as_mut_ptr(), options(nostack)) };
    loop {
        core::hint::black_box(&buf);
        alaya_sys::yield_now();
    }
}

/// Answers every call with an empty message. `arg` = endpoint | reply << 16.
extern "C" fn null_server(arg: u64) -> ! {
    let (ep, rep) = (arg & 0xffff, arg >> 16);
    let caps = Caps {
        recv: 0,
        reply: rep,
        send: 0,
    };
    let mut got = alaya_sys::msg_call(alaya_sys::abi::call::RECV, ep, &Msg::default(), caps);
    loop {
        got = match got {
            Ok(_) => {
                alaya_sys::msg_call(alaya_sys::abi::call::REPLY_RECV, ep, &Msg::default(), caps)
            }
            Err(_) => alaya_sys::msg_call(alaya_sys::abi::call::RECV, ep, &Msg::default(), caps),
        };
    }
}

/// Calls its server until told to stop.
extern "C" fn null_client(ep: u64) -> ! {
    loop {
        if STOP.load(SeqCst) == 0 {
            if alaya_sys::ipc_call(ep, &Msg::default(), Caps::default()).is_ok() {
                CALLS.fetch_add(1, SeqCst);
            }
        } else {
            alaya_sys::yield_now();
        }
    }
}

/// Run the checks; returns the next free slot.
pub fn run(info: &BootInfo, big: u64, first_free: u64, pass: &mut dyn FnMut(&str)) -> u64 {
    HZ.store(info.tsc_hz, SeqCst);
    let mut k = Kit {
        info,
        ut: 0,
        next: first_free,
        stacks: 0,
    };
    k.ut = k.slot();
    alaya_sys::retype(big, Kind::Untyped, 23, 1, k.at(k.ut)).unwrap();
    let tick = k.make(Kind::Notification, 0);
    TICK.store(tick, SeqCst);
    TICKS.store(0, SeqCst);
    k.thread(ticker, tick, 254, 50, 2000, 0);

    // The protections the kernel turned on.
    let on = info.protections;
    let names = [
        (protect::SMEP, "SMEP"),
        (protect::SMAP, "SMAP"),
        (protect::UMIP, "UMIP"),
        (protect::PCID, "PCID"),
        (protect::EIBRS, "eIBRS"),
        (protect::IBPB, "IBPB"),
        (protect::VERW, "VERW"),
    ];
    let mut line = [0u8; 64];
    let mut n = 0;
    for (bit, name) in names {
        if on & bit != 0 && n + name.len() < line.len() {
            line[n] = b' ';
            line[n + 1..n + 1 + name.len()].copy_from_slice(name.as_bytes());
            n += name.len() + 1;
        }
    }
    say!(
        "protect: on:{}",
        core::str::from_utf8(&line[..n]).unwrap_or("?")
    );
    if on & protect::UMIP != 0 {
        let fault = k.make(Kind::Endpoint, 0);
        let rep = k.make(Kind::Reply, 0);
        let t = k.make(Kind::Thread, 12);
        let sc = k.make(Kind::SchedContext, 8);
        alaya_sys::thread_configure(t, info.table, info.space, fault).unwrap();
        alaya_sys::thread_priority(t, 100, 100).unwrap();
        let top = k.stack();
        alaya_sys::thread_write(t, read_idt as *const () as u64, top - 8, 0, 0, 0).unwrap();
        alaya_sys::sc_configure(info.sched_control, sc, 1000, 1000, core_type::ANY, 0).unwrap();
        alaya_sys::sc_bind(sc, t).unwrap();
        alaya_sys::thread_start(t).unwrap();
        let f = alaya_sys::recv(fault, rep).unwrap();
        assert!(
            f.label() == FAULT_LABEL && f.words[0] == 13,
            "SIDT in user mode did not fault with UMIP on: {f:?}"
        );
        alaya_sys::thread_stop(t).unwrap();
        pass("processor safety switches: a program cannot read the kernel's tables");
    } else {
        say!("protect: this CPU has no UMIP; skipping its check");
        pass("processor safety switches reported");
    }

    // Format labels.
    let kinds: [(Kind, u8); 7] = [
        (Kind::Table, 4),
        (Kind::Endpoint, 0),
        (Kind::Notification, 0),
        (Kind::Thread, 12),
        (Kind::SchedContext, 8),
        (Kind::Reply, 0),
        (Kind::AddressSpace, 13),
    ];
    for (kind, bits) in kinds {
        let h = k.make(kind, bits);
        let i = alaya_sys::inspect(k.at(h)).unwrap();
        assert!(
            i.format != 0 && i.format == format::of(kind),
            "{kind:?} carries format {} instead of {}",
            i.format,
            format::of(kind)
        );
    }
    let own = alaya_sys::inspect(k.at(info.thread)).unwrap();
    assert!(
        own.format == format::THREAD,
        "the first thread has no format label"
    );
    let page = k.make(Kind::Page, 12);
    assert!(alaya_sys::inspect(k.at(page)).unwrap().format == 0);
    pass("every kernel object carries its format label");

    // The big kernel lock under load: on every core, a client calls a
    // server as fast as it can.
    let cores = info.cores;
    for c in 0..cores {
        let ep = k.make(Kind::Endpoint, 0);
        let rep = k.make(Kind::Reply, 0);
        k.thread(null_server, ep | rep << 16, 150, 1000, 1000, c + 1);
        // Let the server reach its receive first.
        sleep_ms(2);
        k.thread(null_client, ep, 140, 1000, 1000, c + 1);
    }
    let before = alaya_sys::lock_stats();
    let start = alaya_sys::rdtsc();
    CALLS.store(0, SeqCst);
    sleep_ms(500);
    let calls = CALLS.load(SeqCst);
    let after = alaya_sys::lock_stats();
    let elapsed = alaya_sys::rdtsc() - start;
    STOP.store(1, SeqCst);
    let takes = after[0] - before[0];
    let waits = after[1] - before[1];
    let wait_ticks = after[2] - before[2];
    let held_ticks = after[3] - before[3];
    let pct = |x: u64, of: u64| x * 1000 / of.max(1);
    let waiting = pct(wait_ticks, elapsed * cores);
    let held = pct(held_ticks, elapsed);
    say!(
        "bkl: {} core(s) making calls for 500 ms: {} calls, lock taken {} times ({} had to wait)",
        cores,
        calls,
        takes,
        waits
    );
    say!(
        "bkl: the lock was held {}.{}% of the time; cores spent {}.{}% of their time waiting for it",
        held / 10,
        held % 10,
        waiting / 10,
        waiting % 10
    );
    assert!(calls > 0, "no calls were made under load");
    pass("the big kernel lock measured under load from every core");

    alaya_sys::revoke(k.at(k.ut)).unwrap();
    k.next
}

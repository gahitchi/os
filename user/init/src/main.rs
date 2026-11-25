// SPDX-License-Identifier: GPL-3.0-only
//! The first Alaya program.
//!
//! In milestone 2 it is a test: it takes the untyped memory the kernel
//! handed it and checks retype, copy, mint, move, delete and revoke from
//! user mode, including revokes big enough that the kernel interrupts and
//! restarts them, and the reuse of memory after an untyped is revoked. It
//! prints one line per check and reports the result to the kernel, which
//! prints `alaya: boot ok` when all passed.

#![no_std]
#![no_main]

use core::fmt::{self, Write};

use alaya_sys::abi::{BOOT_INFO_MAGIC, BootInfo, boot_flags, call, op};
use alaya_sys::{At, Error, Kind, rights};

mod fuzz;
mod installed;
mod m3;
mod m4;
mod m4b;
mod m4c;
mod m5;
mod m6;
mod m6b;
mod m6c;
mod m7;
mod m7b;
mod m7c;
mod m7d;
mod m7e;
mod m7f;
mod m7g;
mod m7h;
mod m8a;
mod m8b;
mod m8c;
mod m8d;
mod m8e;

alaya_sys::start_code!(main);

/// One line of output, printed in a single call.
struct Line {
    buf: [u8; 200],
    len: usize,
}

impl Write for Line {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let n = s.len().min(self.buf.len() - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

pub(crate) fn say_line(args: fmt::Arguments) {
    let mut line = Line {
        buf: [0; 200],
        len: 0,
    };
    let _ = line.write_str("init: ");
    let _ = line.write_fmt(args);
    let _ = line.write_str("\n");
    alaya_sys::print(core::str::from_utf8(&line.buf[..line.len]).unwrap_or("?\n"));
}

macro_rules! say {
    ($($arg:tt)*) => ($crate::say_line(format_args!($($arg)*)));
}
pub(crate) use say;

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    say!("FAILED: {}", info.message());
    if let Some(at) = info.location() {
        say!("  at {}:{}", at.file(), at.line());
    }
    alaya_sys::report(1);
    loop {
        alaya_sys::yield_now();
    }
}

/// Check that `r` is the error `e`.
#[track_caller]
pub(crate) fn expect_err<T: fmt::Debug>(r: Result<T, Error>, e: Error) {
    match r {
        Err(got) if got == e => {}
        other => panic!("expected {e:?}, got {other:?}"),
    }
}

/// Hands out free slots of the root table, one at a time.
struct Slots {
    table: u64,
    next: u64,
}

impl Slots {
    fn take(&mut self) -> At {
        let at = At {
            table: self.table,
            index: self.next,
        };
        self.next += 1;
        at
    }
}

extern "C" fn main(info: &'static BootInfo) -> ! {
    say!("hello from user mode");
    assert!(info.magic == BOOT_INFO_MAGIC, "bad boot information");
    // Its milliseconds were counted at a guessed 1 GHz until now (wrong on
    // ARM, whose clock runs at 62.5 MHz in QEMU).
    alaya_sys::rt::set_clock_hz(info.clock_hz);
    let root = info.table;
    let mut slots = Slots {
        table: root,
        next: info.first_free,
    };
    let mut checks = 0;
    let mut pass = |name: &str| {
        checks += 1;
        say!("ok {checks:>2} {name}");
    };

    // The boot capabilities.
    let blocks = &info.untyped[..info.untyped_count as usize];
    assert!(!blocks.is_empty(), "no untyped memory");
    let big = blocks.iter().max_by_key(|b| b.size_bits).unwrap();
    assert!(big.size_bits >= 25, "the largest block is under 32 MiB");
    let own = alaya_sys::inspect(At {
        table: root,
        index: root,
    })
    .unwrap();
    assert!(own.is(Kind::Table) && u64::from(own.size_bits) == info.table_bits);
    for b in blocks {
        let i = alaya_sys::inspect(At {
            table: root,
            index: b.handle,
        })
        .unwrap();
        assert!(i.is(Kind::Untyped) && i.object == b.base && u64::from(i.size_bits) == b.size_bits);
        assert!(i.badge_or_free == 0, "fresh untyped memory has a watermark");
    }
    let total: u64 = blocks.iter().map(|b| 1u64 << b.size_bits).sum();
    say!(
        "{} untyped blocks, {} MiB, largest {} MiB, on {} cores",
        blocks.len(),
        total >> 20,
        (1u64 << big.size_bits) >> 20,
        info.cores
    );
    pass("boot capabilities: own table and untyped memory");

    // Bad calls are refused with the right errors.
    let u = big.handle;
    let at = |index| At { table: root, index };
    expect_err(alaya_sys::inspect(at(0)), Error::InvalidHandle);
    expect_err(
        alaya_sys::inspect(at(1 << info.table_bits)),
        Error::InvalidHandle,
    );
    expect_err(
        alaya_sys::call(4095, op::INSPECT, [1, 0, 0, 0, 0, 0]),
        Error::InvalidHandle,
    );
    expect_err(alaya_sys::call(root, op::RETYPE, [0; 6]), Error::NoSuchCall);
    expect_err(
        alaya_sys::call(u, op::INSPECT, [1, 0, 0, 0, 0, 0]),
        Error::NoSuchCall,
    );
    expect_err(
        alaya_sys::copy(at(4000), at(u), rights::ALL),
        Error::WrongKind,
    );
    expect_err(
        alaya_sys::retype(u, Kind::Empty, 0, 1, at(4000)),
        Error::InvalidArgument,
    );
    expect_err(
        alaya_sys::retype(u, Kind::Page, 13, 1, at(4000)),
        Error::InvalidArgument,
    );
    expect_err(
        alaya_sys::retype(u, Kind::Endpoint, 0, 1, at(root)),
        Error::SlotInUse,
    );
    expect_err(
        alaya_sys::call(u, op::RETYPE, [Kind::Table as u64, 30, 1, root, 4000, 0]),
        Error::InvalidArgument,
    );
    let (e, _) = alaya_sys::syscall(77, 0, 0, [0; 6]);
    assert!(e == Error::NoSuchCall as u64);
    let (e, _) = alaya_sys::syscall(call::SEND, 0, 0, [0; 6]);
    assert!(e == Error::InvalidHandle as u64);
    let (e, _) = alaya_sys::syscall(call::SEND, root, 0, [0; 6]);
    assert!(e == Error::WrongKind as u64);
    pass("bad handles, kinds, sizes and calls are refused");

    // A 16 MiB untyped from the big block, a table of 1024 slots from it,
    // and 100 endpoints in that table.
    let w = slots.take();
    alaya_sys::retype(u, Kind::Untyped, 24, 1, w).unwrap();
    let wi = alaya_sys::inspect(w).unwrap();
    assert!(wi.is(Kind::Untyped) && wi.size_bits == 24 && wi.object == big.base);
    let a = slots.take();
    alaya_sys::retype(w.index, Kind::Table, 10, 1, a).unwrap();
    let ta = |index| At {
        table: a.index,
        index,
    };
    alaya_sys::retype(w.index, Kind::Endpoint, 0, 100, ta(1)).unwrap();
    for i in 1..=100 {
        assert!(alaya_sys::inspect(ta(i)).unwrap().is(Kind::Endpoint));
    }
    expect_err(alaya_sys::inspect(ta(101)), Error::InvalidHandle);
    let free = alaya_sys::inspect(w).unwrap().badge_or_free;
    assert!(free == (64 << 10) + 100 * 64, "watermark {free:#x}");
    pass("retype: untyped, a table, 100 endpoints in batches");

    // Copy, rights and badges.
    let c = slots.take();
    alaya_sys::copy(c, ta(1), rights::READ | rights::GRANT).unwrap();
    assert!(alaya_sys::inspect(c).unwrap().rights == rights::READ | rights::GRANT);
    let c2 = slots.take();
    alaya_sys::copy(c2, c, rights::ALL).unwrap();
    assert!(
        alaya_sys::inspect(c2).unwrap().rights == rights::READ | rights::GRANT,
        "rights grew"
    );
    let m = slots.take();
    alaya_sys::mint(m, ta(1), rights::ALL, 42).unwrap();
    assert!(alaya_sys::inspect(m).unwrap().badge_or_free == 42);
    expect_err(
        alaya_sys::mint(slots.take(), m, rights::ALL, 43),
        Error::InvalidArgument,
    );
    expect_err(
        alaya_sys::mint(at(4001), ta(1), rights::ALL, 0),
        Error::InvalidArgument,
    );
    let r = slots.take();
    alaya_sys::copy(r, ta(1), rights::READ).unwrap();
    expect_err(alaya_sys::copy(at(4002), r, rights::READ), Error::NoRights);
    expect_err(alaya_sys::copy(c, ta(2), rights::ALL), Error::SlotInUse);
    assert!(alaya_sys::inspect(ta(1)).unwrap().has_children());
    pass("copy and mint: rights only shrink, one badge, grant needed");

    // Move keeps the place in the tree.
    alaya_sys::move_cap(ta(200), m).unwrap();
    expect_err(alaya_sys::inspect(m), Error::InvalidHandle);
    assert!(alaya_sys::inspect(ta(200)).unwrap().badge_or_free == 42);
    alaya_sys::move_cap(m, ta(200)).unwrap();
    pass("move");

    // Revoke takes back every copy, and only those.
    alaya_sys::revoke(ta(1)).unwrap();
    for x in [c, c2, m, r] {
        expect_err(alaya_sys::inspect(x), Error::InvalidHandle);
    }
    let e1 = alaya_sys::inspect(ta(1)).unwrap();
    assert!(e1.is(Kind::Endpoint) && !e1.has_children());
    assert!(alaya_sys::inspect(ta(2)).unwrap().is(Kind::Endpoint));
    pass("revoke deletes every copy and keeps the original");

    // Delete: the middle of a chain goes, its copies stay revocable.
    let d1 = slots.take();
    let d2 = slots.take();
    alaya_sys::copy(d1, ta(3), rights::ALL).unwrap();
    alaya_sys::copy(d2, d1, rights::ALL).unwrap();
    alaya_sys::delete(d1).unwrap();
    expect_err(alaya_sys::delete(d1), Error::InvalidHandle);
    assert!(alaya_sys::inspect(ta(3)).unwrap().has_children());
    alaya_sys::revoke(ta(3)).unwrap();
    expect_err(alaya_sys::inspect(d2), Error::InvalidHandle);
    pass("delete keeps what was derived revocable");

    // A big revoke: 16383 copies of one endpoint in a table of 16384
    // slots, half of them a chain of copies of copies. The kernel stops
    // it many times and the call restarts by itself.
    let b = slots.take();
    alaya_sys::retype(w.index, Kind::Table, 14, 1, b).unwrap();
    let tb = |index| At {
        table: b.index,
        index,
    };
    let e = ta(4);
    for i in 1..(1u64 << 14) {
        let src = if i > 1 && i <= 8192 { tb(i - 1) } else { e };
        alaya_sys::copy(tb(i), src, rights::ALL).unwrap();
    }
    let (restarts0, revoked0) = alaya_sys::counters();
    alaya_sys::revoke(e).unwrap();
    let (restarts1, revoked1) = alaya_sys::counters();
    for i in 1..(1u64 << 14) {
        expect_err(alaya_sys::inspect(tb(i)), Error::InvalidHandle);
    }
    assert!(revoked1 - revoked0 == (1 << 14) - 1);
    assert!(
        restarts1 > restarts0,
        "the big revoke was never interrupted"
    );
    say!(
        "revoked {} capabilities; the kernel interrupted the revoke {} times",
        revoked1 - revoked0,
        restarts1 - restarts0
    );
    pass("a big revoke is interrupted and restarted, and completes");

    // Unreachable tables are emptied before memory is reused. An endpoint
    // from the big block has copies inside two tables made from W; the
    // last capability to one of the tables is dropped. Revoking W must
    // delete those copies, then hand W's memory back.
    let x = slots.take();
    alaya_sys::retype(u, Kind::Endpoint, 0, 1, x).unwrap();
    alaya_sys::copy(ta(300), x, rights::ALL).unwrap();
    alaya_sys::copy(tb(5), x, rights::ALL).unwrap();
    alaya_sys::delete(b).unwrap();
    let small = slots.take();
    alaya_sys::retype(w.index, Kind::Untyped, 16, 1, small).unwrap();
    alaya_sys::delete(small).unwrap();
    assert!(alaya_sys::inspect(x).unwrap().has_children());
    let old_a = alaya_sys::inspect(a).unwrap().object;
    let (restarts0, _) = alaya_sys::counters();
    alaya_sys::revoke(w).unwrap();
    let (restarts1, _) = alaya_sys::counters();
    let xi = alaya_sys::inspect(x).unwrap();
    assert!(
        !xi.has_children(),
        "a copy inside an unreachable table survived"
    );
    expect_err(alaya_sys::inspect(a), Error::InvalidHandle);
    let wi = alaya_sys::inspect(w).unwrap();
    assert!(wi.badge_or_free == 0 && wi.clean == 0, "untyped not reset");
    say!(
        "revoking the 16 MiB untyped restarted {} times",
        restarts1 - restarts0
    );
    pass("revoking untyped empties unreachable tables, then resets it");

    // The memory comes back: the same address, and the new table is empty.
    alaya_sys::retype(w.index, Kind::Table, 10, 1, a).unwrap();
    assert!(alaya_sys::inspect(a).unwrap().object == old_a);
    for i in 1..1024 {
        expect_err(alaya_sys::inspect(ta(i)), Error::InvalidHandle);
    }
    pass("revoked memory is reused, and new tables are empty");

    // Running out.
    let tiny = slots.take();
    alaya_sys::retype(w.index, Kind::Untyped, 12, 1, tiny).unwrap();
    let p = slots.take();
    alaya_sys::retype(tiny.index, Kind::Page, 12, 1, p).unwrap();
    expect_err(
        alaya_sys::retype(tiny.index, Kind::Page, 12, 1, slots.take()),
        Error::NoMemory,
    );
    expect_err(
        alaya_sys::retype(tiny.index, Kind::Untyped, 12, 1, at(4003)),
        Error::InvalidArgument,
    );
    alaya_sys::revoke(tiny).unwrap();
    alaya_sys::retype(tiny.index, Kind::Page, 12, 1, p).unwrap();
    pass("untyped memory runs out, and comes back after revoke");

    // Every big page size, from the big block.
    let p2 = slots.take();
    alaya_sys::retype(u, Kind::Page, 21, 1, p2).unwrap();
    assert!(
        alaya_sys::inspect(p2)
            .unwrap()
            .object
            .is_multiple_of(1 << 21)
    );
    pass("2 MiB pages are aligned");

    slots.next = m3::run(info, u, slots.next, &mut pass);
    slots.next = m4::run(info, u, slots.next, &mut pass);
    slots.next = m4b::run(info, u, slots.next, &mut pass);
    slots.next = m4c::run(info, u, slots.next, &mut pass);
    slots.next = m5::run(info, u, slots.next, &mut pass);
    let session = m6::run(info, u, slots.next, &mut pass);
    slots.next = session.s.k.next;
    if info.flags & boot_flags::FUZZ != 0 {
        fuzz::run(info, u, slots.next, info.fuzz_seconds);
    }

    let (restarts, revoked) = alaya_sys::counters();
    say!("all {checks} checks passed ({restarts} restarted calls, {revoked} capabilities revoked)");
    alaya_sys::report(0);
    session.serve()
}

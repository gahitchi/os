// SPDX-License-Identifier: GPL-3.0-only
//! System calls for Alaya programs: thin wrappers over the `syscall`
//! instruction, following the register layout in `alaya-abi`.
//!
//! This is the start of the SDK (decision 41). It will carry the library
//! linking exception (`LICENSE-EXCEPTION.md`) once that text is final.

#![no_std]

pub use alaya_abi::{self as abi, Error, Kind, rights};
use core::arch::asm;

/// Make a call: `nr` in rax, `a0` in rdi, `a1` in rsi, `args` in rdx, r10,
/// r8, r9, r12, r13. Returns the error code and four results.
#[inline]
pub fn syscall(nr: u64, a0: u64, a1: u64, args: [u64; 6]) -> (u64, [u64; 4]) {
    let (err, r0, r1, r2, r3);
    // SAFETY: the kernel keeps every register but rcx and r11, and the
    // ones it returns results in.
    unsafe {
        asm!(
            "syscall",
            inout("rax") nr => err,
            in("rdi") a0,
            in("rsi") a1,
            inout("rdx") args[0] => r0,
            inout("r10") args[1] => r1,
            inout("r8") args[2] => r2,
            inout("r9") args[3] => r3,
            in("r12") args[4],
            in("r13") args[5],
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    (err, [r0, r1, r2, r3])
}

fn result(r: (u64, [u64; 4])) -> Result<[u64; 4], Error> {
    match r.0 {
        0 => Ok(r.1),
        e => Err(Error::from_u64(e)),
    }
}

/// Call `op` on the capability `handle`.
pub fn call(handle: u64, op: u64, args: [u64; 6]) -> Result<[u64; 4], Error> {
    result(syscall(abi::call::CALL, handle, op, args))
}

/// Print text on the kernel's serial port (debug builds of the kernel).
pub fn print(text: &str) {
    let mut rest = text.as_bytes();
    while !rest.is_empty() {
        let n = rest.len().min(abi::debug::MAX_PRINT);
        let _ = syscall(
            abi::call::DEBUG,
            abi::debug::PRINT,
            n as u64,
            [rest.as_ptr() as u64, 0, 0, 0, 0, 0],
        );
        rest = &rest[n..];
    }
}

/// Tell the kernel how the boot tests went. Does not return on success.
pub fn report(status: u64) {
    let _ = syscall(abi::call::DEBUG, abi::debug::REPORT, status, [0; 6]);
}

/// Kernel counters: (restarted calls, capabilities deleted by revokes).
pub fn counters() -> (u64, u64) {
    let (_, r) = syscall(abi::call::DEBUG, abi::debug::COUNTERS, 0, [0; 6]);
    (r[0], r[1])
}

pub fn yield_now() {
    let _ = syscall(abi::call::YIELD, 0, 0, [0; 6]);
}

/// A slot named by a table handle and an index in that table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct At {
    pub table: u64,
    pub index: u64,
}

/// What `inspect` reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Info {
    pub kind: u8,
    pub rights: u8,
    pub size_bits: u8,
    pub flags: u8,
    /// The object's format label (`abi::format`; 0 for kinds without one).
    pub format: u8,
    pub object: u64,
    pub badge_or_free: u64,
    pub clean: u64,
}

impl Info {
    pub fn is(&self, kind: Kind) -> bool {
        self.kind == kind as u8
    }

    pub fn has_children(&self) -> bool {
        self.flags & abi::flags::HAS_CHILDREN != 0
    }
}

pub fn copy(dst: At, src: At, rights: u8) -> Result<(), Error> {
    call(
        dst.table,
        abi::op::COPY,
        [dst.index, src.table, src.index, u64::from(rights), 0, 0],
    )
    .map(|_| ())
}

pub fn mint(dst: At, src: At, rights: u8, badge: u64) -> Result<(), Error> {
    call(
        dst.table,
        abi::op::MINT,
        [dst.index, src.table, src.index, u64::from(rights), badge, 0],
    )
    .map(|_| ())
}

pub fn move_cap(dst: At, src: At) -> Result<(), Error> {
    call(
        dst.table,
        abi::op::MOVE,
        [dst.index, src.table, src.index, 0, 0, 0],
    )
    .map(|_| ())
}

pub fn delete(at: At) -> Result<(), Error> {
    call(at.table, abi::op::DELETE, [at.index, 0, 0, 0, 0, 0]).map(|_| ())
}

/// Revoke. The kernel may interrupt it many times; each time the call
/// restarts by itself, so this returns only when the revoke is complete.
pub fn revoke(at: At) -> Result<(), Error> {
    call(at.table, abi::op::REVOKE, [at.index, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn inspect(at: At) -> Result<Info, Error> {
    let r = call(at.table, abi::op::INSPECT, [at.index, 0, 0, 0, 0, 0])?;
    Ok(Info {
        kind: r[0] as u8,
        rights: (r[0] >> 8) as u8,
        size_bits: (r[0] >> 16) as u8,
        flags: (r[0] >> 24) as u8,
        format: (r[0] >> 32) as u8,
        object: r[1],
        badge_or_free: r[2],
        clean: r[3],
    })
}

/// Make `count` objects from untyped `ut` into `dst` and the slots after
/// it, as many calls as it takes.
pub fn retype(ut: u64, kind: Kind, bits: u8, count: u64, dst: At) -> Result<(), Error> {
    let mut made = 0;
    while made < count {
        let r = call(
            ut,
            abi::op::RETYPE,
            [
                kind as u64,
                u64::from(bits),
                count - made,
                dst.table,
                dst.index + made,
                0,
            ],
        )?;
        made += r[0];
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Messages

pub use abi::Info as MsgInfo;

/// A message: its info word (label, length, flags), the sender's badge (or
/// a notification's bits) and up to six data words.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Msg {
    pub info: MsgInfo,
    pub badge: u64,
    pub words: [u64; 6],
}

impl Msg {
    pub fn new(label: u64, words: &[u64]) -> Msg {
        let mut w = [0; 6];
        w[..words.len()].copy_from_slice(words);
        Msg {
            info: MsgInfo::new(label, words.len() as u64),
            badge: 0,
            words: w,
        }
    }

    pub fn label(&self) -> u64 {
        self.info.label()
    }
}

/// Capabilities a message call sends and receives.
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    /// Handle of a capability to send (with [`MsgInfo::CAP`] in the info).
    pub send: u64,
    /// Slot of the own table to receive a capability in (0: none).
    pub recv: u64,
    /// Reply object for `recv` and `reply_recv`.
    pub reply: u64,
}

/// A message call: the full register set of the message ABI.
#[inline]
pub fn msg_call(nr: u64, handle: u64, m: &Msg, caps: Caps) -> Result<Msg, Error> {
    let (err, badge, info, w0, w1, w2, w3, w4, w5);
    // SAFETY: the kernel keeps every register but rcx, r11 and the outputs.
    unsafe {
        asm!(
            "syscall",
            inout("rax") nr => err,
            inout("rdi") handle => badge,
            inout("rsi") m.info.0 => info,
            inout("rdx") m.words[0] => w0,
            inout("r10") m.words[1] => w1,
            inout("r8") m.words[2] => w2,
            inout("r9") m.words[3] => w3,
            inout("r12") m.words[4] => w4,
            inout("r13") m.words[5] => w5,
            in("r14") caps.send | caps.recv << 32,
            in("r15") caps.reply,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    match err {
        0 => Ok(Msg {
            info: MsgInfo(info),
            badge,
            words: [w0, w1, w2, w3, w4, w5],
        }),
        e => Err(Error::from_u64(e)),
    }
}

/// Send `m` on endpoint `ep` and wait for the answer.
pub fn ipc_call(ep: u64, m: &Msg, caps: Caps) -> Result<Msg, Error> {
    msg_call(abi::call::CALL, ep, m, caps)
}

/// Send and wait until the message is taken.
pub fn send(ep: u64, m: &Msg) -> Result<(), Error> {
    msg_call(abi::call::SEND, ep, m, Caps::default()).map(|_| ())
}

/// Send only if someone is waiting now.
pub fn try_send(ep: u64, m: &Msg) -> Result<(), Error> {
    msg_call(abi::call::TRY_SEND, ep, m, Caps::default()).map(|_| ())
}

/// Answer the caller held by reply object `reply`.
pub fn reply(reply: u64, m: &Msg) -> Result<(), Error> {
    msg_call(abi::call::SEND, reply, m, Caps::default()).map(|_| ())
}

/// Wait for a message on `ep` (or a signal of the bound notification).
pub fn recv(ep: u64, reply: u64) -> Result<Msg, Error> {
    msg_call(
        abi::call::RECV,
        ep,
        &Msg::default(),
        Caps {
            reply,
            ..Caps::default()
        },
    )
}

/// Answer the last caller and wait for the next message, in one call.
pub fn reply_recv(ep: u64, reply: u64, m: &Msg) -> Result<Msg, Error> {
    msg_call(
        abi::call::REPLY_RECV,
        ep,
        m,
        Caps {
            reply,
            ..Caps::default()
        },
    )
}

/// Raise `bits` (plus the capability's badge) on a notification.
pub fn signal(ntfn: u64, bits: u64) -> Result<(), Error> {
    let m = Msg {
        words: [bits, 0, 0, 0, 0, 0],
        ..Msg::default()
    };
    msg_call(abi::call::SIGNAL, ntfn, &m, Caps::default()).map(|_| ())
}

/// Wait for signals; returns the bits, cleared.
pub fn wait(ntfn: u64) -> Result<u64, Error> {
    msg_call(abi::call::WAIT, ntfn, &Msg::default(), Caps::default()).map(|m| m.badge)
}

/// Take the signals without waiting (0: none).
pub fn poll(ntfn: u64) -> Result<u64, Error> {
    msg_call(abi::call::POLL, ntfn, &Msg::default(), Caps::default()).map(|m| m.badge)
}

// ---------------------------------------------------------------------------
// Threads, time, memory

pub fn thread_configure(t: u64, table: u64, space: u64, fault: u64) -> Result<(), Error> {
    call(t, abi::op::THREAD_CONFIGURE, [table, space, fault, 0, 0, 0]).map(|_| ())
}

pub fn thread_priority(t: u64, prio: u64, mcp: u64) -> Result<(), Error> {
    call(t, abi::op::THREAD_PRIORITY, [prio, mcp, 0, 0, 0, 0]).map(|_| ())
}

/// Set a stopped thread's registers: where it starts, its stack, two
/// arguments and its thread-local storage base.
pub fn thread_write(
    t: u64,
    rip: u64,
    rsp: u64,
    arg0: u64,
    arg1: u64,
    fs: u64,
) -> Result<(), Error> {
    call(t, abi::op::THREAD_WRITE, [rip, rsp, arg0, arg1, fs, 0]).map(|_| ())
}

/// rip, rsp, rax and state ([`abi::state`]).
pub fn thread_read(t: u64) -> Result<[u64; 4], Error> {
    call(t, abi::op::THREAD_READ, [0; 6])
}

pub fn thread_start(t: u64) -> Result<(), Error> {
    call(t, abi::op::THREAD_START, [0; 6]).map(|_| ())
}

pub fn thread_stop(t: u64) -> Result<(), Error> {
    call(t, abi::op::THREAD_STOP, [0; 6]).map(|_| ())
}

pub fn thread_bind(t: u64, ntfn: u64) -> Result<(), Error> {
    call(t, abi::op::THREAD_BIND, [ntfn, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn sc_bind(sc: u64, thread: u64) -> Result<(), Error> {
    call(sc, abi::op::SC_BIND, [thread, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn sc_unbind(sc: u64) -> Result<(), Error> {
    call(sc, abi::op::SC_UNBIND, [0; 6]).map(|_| ())
}

/// Budget ns, period ns, time used ns, core it last ran on.
pub fn sc_info(sc: u64) -> Result<[u64; 4], Error> {
    call(sc, abi::op::SC_INFO, [0; 6])
}

/// Give scheduling context `sc` `budget_us` every `period_us`, on cores of
/// `core_type`, or only on core `pin - 1` if `pin` is not 0.
pub fn sc_configure(
    control: u64,
    sc: u64,
    budget_us: u64,
    period_us: u64,
    core_type: u64,
    pin: u64,
) -> Result<(), Error> {
    call(
        control,
        abi::op::SC_CONFIGURE,
        [sc, budget_us, period_us, core_type, pin, 0],
    )
    .map(|_| ())
}

pub fn space_assign(space: u64, pool: u64) -> Result<(), Error> {
    call(space, abi::op::SPACE_ASSIGN, [pool, 0, 0, 0, 0, 0]).map(|_| ())
}

pub fn page_map(page: u64, space: u64, at: u64, flags: u64) -> Result<(), Error> {
    call(page, abi::op::PAGE_MAP, [space, at, flags, 0, 0, 0]).map(|_| ())
}

pub fn page_unmap(page: u64) -> Result<(), Error> {
    call(page, abi::op::PAGE_UNMAP, [0; 6]).map(|_| ())
}

/// The core this thread runs on.
pub fn this_core() -> u64 {
    let (_, r) = syscall(abi::call::DEBUG, abi::debug::CORE, 0, [0; 6]);
    r[0]
}

/// Kernel counters: restarted calls, capabilities revoked, threads moved
/// between cores, thread switches.
pub fn all_counters() -> [u64; 4] {
    syscall(abi::call::DEBUG, abi::debug::COUNTERS, 0, [0; 6]).1
}

/// Big kernel lock use since boot: taken, waited, ticks waiting, ticks held.
pub fn lock_stats() -> [u64; 4] {
    syscall(abi::call::DEBUG, abi::debug::LOCK_STATS, 0, [0; 6]).1
}

/// The time-stamp counter.
#[inline]
pub fn rdtsc() -> u64 {
    // SAFETY: reading the TSC has no side effects.
    unsafe { core::arch::x86_64::_rdtsc() }
}

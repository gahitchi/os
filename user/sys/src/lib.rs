// SPDX-License-Identifier: GPL-3.0-only
//! System calls for Alaya programs: thin wrappers over the system call
//! instruction (`syscall` on a PC, `svc` on ARM), following the register
//! layout in `alaya-abi` and [`arch`].
//!
//! This is the start of the SDK (decision 41). It will carry the library
//! linking exception (`LICENSE-EXCEPTION.md`) once that text is final.

#![no_std]

pub use alaya_abi::{self as abi, Error, Kind, rights};

pub mod arch;
pub use arch::{clock, crash, entry_stack};

/// Make a call: the call number, a handle, a second argument and six
/// data words (registers in [`arch`]). Returns the error code and four
/// results.
#[inline]
pub fn syscall(nr: u64, a0: u64, a1: u64, args: [u64; 6]) -> (u64, [u64; 4]) {
    let (err, _, _, w) = arch::raw_call(nr, a0, a1, args, 0, 0);
    (err, [w[0], w[1], w[2], w[3]])
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
    let (err, badge, info, words) = arch::raw_call(
        nr,
        handle,
        m.info.0,
        m.words,
        caps.send | caps.recv << 32,
        caps.reply,
    );
    match err {
        0 => Ok(Msg {
            info: MsgInfo(info),
            badge,
            words,
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

/// Physical address of a page (drivers).
pub fn page_address(page: u64) -> Result<u64, Error> {
    call(page, abi::op::PAGE_ADDRESS, [0; 6]).map(|r| r[0])
}

// ---------------------------------------------------------------------------
// Devices

/// Let the devices of I/O space `io` reach `page` at device address `at`.
pub fn page_iomap(page: u64, io: u64, at: u64, flags: u64) -> Result<(), Error> {
    call(page, abi::op::PAGE_IOMAP, [io, at, flags, 0, 0, 0]).map(|_| ())
}

pub fn iospace_assign(io: u64, pool: u64) -> Result<(), Error> {
    call(io, abi::op::IOSPACE_ASSIGN, [pool, 0, 0, 0, 0, 0]).map(|_| ())
}

/// A page capability for device registers at `phys` (`bits`: 12 or 21).
pub fn device_page(control: u64, phys: u64, bits: u8, dst: At) -> Result<(), Error> {
    call(
        control,
        abi::op::DEVICE_PAGE,
        [phys, u64::from(bits), dst.table, dst.index, 0, 0],
    )
    .map(|_| ())
}

/// Attach PCI function `bdf` to I/O space `io`.
pub fn device_bind(control: u64, io: u64, bdf: u16) -> Result<(), Error> {
    call(
        control,
        abi::op::DEVICE_BIND,
        [io, u64::from(bdf), 0, 0, 0, 0],
    )
    .map(|_| ())
}

/// Device accesses the IOMMU blocked since boot.
pub fn device_faults(control: u64) -> Result<u64, Error> {
    call(control, abi::op::DEVICE_FAULTS, [0; 6]).map(|r| r[0])
}

/// Take an interrupt vector delivered to `core`; returns the vector.
pub fn irq_get(control: u64, core: u64, dst: At) -> Result<u64, Error> {
    call(
        control,
        abi::op::IRQ_GET,
        [core, dst.table, dst.index, 0, 0, 0],
    )
    .map(|r| r[0])
}

/// Deliver interrupt `irq` as `bits` on notification `ntfn` (0: stop).
pub fn irq_set(irq: u64, ntfn: u64, bits: u64) -> Result<(), Error> {
    call(irq, abi::op::IRQ_SET, [ntfn, bits, 0, 0, 0, 0]).map(|_| ())
}

/// MSI address and data for `irq`, raised by PCI function `bdf` (bus << 8
/// | device << 3 | function), and how many times it fired.
pub fn irq_msi(irq: u64, bdf: u16) -> Result<(u64, u64, u64), Error> {
    call(irq, abi::op::IRQ_MSI, [u64::from(bdf), 0, 0, 0, 0, 0]).map(|r| (r[0], r[1], r[2]))
}

/// Set the deadline of the work `sc` pays for, in clock
/// ticks (0: none). Returns deadlines missed in total, the thread object
/// that held the context when the last one passed, and whether the
/// deadline it replaces was missed.
pub fn sc_deadline(sc: u64, deadline: u64) -> Result<(u64, u64, bool), Error> {
    call(sc, abi::op::SC_DEADLINE, [deadline, 0, 0, 0, 0, 0]).map(|r| (r[0], r[1], r[2] != 0))
}

/// Ask for a timeout message (to the fault endpoint) when time lent to
/// thread `t` by a caller runs out.
pub fn thread_timeouts(t: u64, on: bool) -> Result<(), Error> {
    call(t, abi::op::THREAD_TIMEOUTS, [u64::from(on), 0, 0, 0, 0, 0]).map(|_| ())
}

pub mod cores;
pub mod elf;
pub mod heap;
pub mod program;
pub mod rt;

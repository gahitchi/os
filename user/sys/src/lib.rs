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

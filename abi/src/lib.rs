// SPDX-License-Identifier: GPL-3.0-only
//! The kernel's interface, as numbers and layouts shared by the kernel and
//! every program (`docs/design/kernel-interface.md`).
//!
//! # Calls
//!
//! A program enters the kernel with the `syscall` instruction:
//!
//! | Register | In | Out |
//! | --- | --- | --- |
//! | `rax` | call number ([`call`]) | error code ([`Error`], 0 is success) |
//! | `rdi` | handle the call acts on | |
//! | `rsi` | operation ([`op`]) | |
//! | `rdx`, `r10`, `r8`, `r9`, `r12`, `r13` | arguments 0 to 5 | results 0 to 3 in `rdx`, `r10`, `r8`, `r9` |
//!
//! `rcx` and `r11` are overwritten by the CPU; every other register is kept.
//!
//! # Handles
//!
//! Each program has one flat table of capabilities (decision 51). A handle
//! is an index into it. Handle 0 is never valid. Slots of *other* tables are
//! named by a handle to that table plus an index.

#![no_std]

/// Call numbers (`rax`). Milestone 2 has `call` (on kernel objects),
/// `yield` and `debug`; the message calls arrive in milestone 3.
pub mod call {
    pub const CALL: u64 = 0;
    pub const SEND: u64 = 1;
    pub const TRY_SEND: u64 = 2;
    pub const RECV: u64 = 3;
    pub const REPLY_RECV: u64 = 4;
    pub const SIGNAL: u64 = 5;
    pub const WAIT: u64 = 6;
    pub const POLL: u64 = 7;
    pub const YIELD: u64 = 8;
    pub const DEBUG: u64 = 9;
}

/// Operations of `debug` (`rdi`).
pub mod debug {
    /// Print `rsi` bytes at address `rdx` (at most [`MAX_PRINT`]).
    pub const PRINT: u64 = 0;
    /// Report the result of the boot tests: `rsi` = 0 for success.
    pub const REPORT: u64 = 1;
    /// Read kernel counters: returns restarts of interrupted calls in
    /// result 0, capabilities deleted by revokes in result 1.
    pub const COUNTERS: u64 = 2;
    pub const MAX_PRINT: usize = 256;
}

/// Operations of `call` (`rsi`), by the kind of object the handle names.
pub mod op {
    // On a capability table. Slots are (table handle, index) pairs; the
    // invoked table holds the destination.
    /// Copy: args = dst index, src table, src index, rights.
    pub const COPY: u64 = 1;
    /// Mint: as copy, plus arg 4 = badge (non-zero).
    pub const MINT: u64 = 2;
    /// Move: args = dst index, src table, src index.
    pub const MOVE: u64 = 3;
    /// Delete: arg 0 = index.
    pub const DELETE: u64 = 4;
    /// Revoke: arg 0 = index. Deletes everything derived from the
    /// capability; for an untyped, also takes its memory back.
    pub const REVOKE: u64 = 5;
    /// Inspect: arg 0 = index. Results: kind | rights << 8 | size bits << 16
    /// | flags << 24, object address, badge or untyped free offset, untyped
    /// clean offset.
    pub const INSPECT: u64 = 6;

    // On untyped memory.
    /// Retype: args = kind, size bits, count, dst table, dst index.
    /// Makes at most [`super::RETYPE_BATCH`] objects; result 0 is how many.
    pub const RETYPE: u64 = 16;
}

/// Most objects one retype makes. Programs loop for more.
pub const RETYPE_BATCH: u64 = 16;

/// Inspect flags.
pub mod flags {
    /// A revoke of this capability has started and not finished.
    pub const REVOKING: u8 = 1 << 0;
    /// Something was derived from this capability.
    pub const HAS_CHILDREN: u8 = 1 << 1;
}

/// Error codes (`rax` on return).
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Ok = 0,
    /// The call number or operation does not exist.
    NoSuchCall = 1,
    /// The handle or index names no slot, or an empty one.
    InvalidHandle = 2,
    /// The capability is the wrong kind for this operation.
    WrongKind = 3,
    /// The capability lacks a right the operation needs.
    NoRights = 4,
    /// The destination slot is not empty.
    SlotInUse = 5,
    /// The capability is being revoked.
    Busy = 6,
    /// The untyped memory has no room for the objects.
    NoMemory = 7,
    /// An argument is out of range.
    InvalidArgument = 8,
}

impl Error {
    pub fn from_u64(v: u64) -> Error {
        match v {
            0 => Error::Ok,
            1 => Error::NoSuchCall,
            2 => Error::InvalidHandle,
            3 => Error::WrongKind,
            4 => Error::NoRights,
            5 => Error::SlotInUse,
            6 => Error::Busy,
            7 => Error::NoMemory,
            _ => Error::InvalidArgument,
        }
    }
}

/// Rights a capability carries.
pub mod rights {
    pub const READ: u8 = 1 << 0;
    pub const WRITE: u8 = 1 << 1;
    /// May be copied or minted onward.
    pub const GRANT: u8 = 1 << 2;
    pub const ALL: u8 = READ | WRITE | GRANT;
}

/// Kinds of capability.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Empty = 0,
    Untyped = 1,
    Table = 2,
    Endpoint = 3,
    Notification = 4,
    Page = 5,
}

impl Kind {
    pub fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            0 => Kind::Empty,
            1 => Kind::Untyped,
            2 => Kind::Table,
            3 => Kind::Endpoint,
            4 => Kind::Notification,
            5 => Kind::Page,
            _ => return None,
        })
    }
}

/// Sizes, as powers of two. A table of `bits` has `2^bits` slots of
/// [`SLOT_BYTES`] bytes; slot 0 of every table is reserved.
pub const SLOT_BITS: u8 = 6;
pub const SLOT_BYTES: u64 = 1 << SLOT_BITS;
pub const TABLE_MIN_BITS: u8 = 2;
pub const TABLE_MAX_BITS: u8 = 16;
/// Endpoints and notifications take 64 bytes each.
pub const SMALL_OBJECT_BITS: u8 = 6;
/// Pages: 4 KiB, 2 MiB, 1 GiB.
pub const PAGE_BITS: [u8; 3] = [12, 21, 30];
/// Untyped memory made by retype: 4 KiB up to the parent's size.
pub const UNTYPED_MIN_BITS: u8 = 12;
pub const UNTYPED_MAX_BITS: u8 = 47;

/// Where the first program finds its boot information (read only).
pub const BOOT_INFO_ADDRESS: u64 = 0x7fff_f000_0000;
/// Top of the first program's stack.
pub const INIT_STACK_TOP: u64 = 0x7fff_e000_0000;
pub const INIT_STACK_BYTES: u64 = 64 * 1024;

pub const BOOT_INFO_MAGIC: u64 = 0x616c_6179_615f_6269; // "alaya_bi"
pub const MAX_BOOT_UNTYPED: usize = 192;

/// One block of untyped memory handed to the first program.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BootUntyped {
    pub handle: u64,
    pub base: u64,
    pub size_bits: u64,
}

/// What the kernel tells the first program at start.
#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    /// Handle of the program's own table.
    pub table: u64,
    /// The table has `2^table_bits` slots.
    pub table_bits: u64,
    /// First empty slot after the boot capabilities.
    pub first_free: u64,
    /// Online cores.
    pub cores: u64,
    pub untyped_count: u64,
    pub untyped: [BootUntyped; MAX_BOOT_UNTYPED],
}

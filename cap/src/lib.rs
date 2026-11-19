// SPDX-License-Identifier: GPL-3.0-only
//! Capabilities for the Alaya kernel: capability tables, the derivation
//! tree, untyped memory, retype and interruptible revoke.
//!
//! This crate holds the logic the TLA+ model in `formal/revoke` describes,
//! written against an abstract [`Memory`] so the same code runs in the
//! kernel (on physical memory through the direct map), in host tests, and
//! under Kani.
//!
//! # Layout
//!
//! A capability table is `2^bits` slots of 64 bytes, in memory retyped from
//! untyped memory (or, for the first program's table, taken at boot). Slot
//! 0 of every table is its header, never a capability. A [`Slot`] is a
//! [`Cap`] (32 bytes) plus four links (32 bytes) that place it in the
//! derivation tree:
//!
//! - `first`, `last`: its first and last child;
//! - `prev`, `next`: its siblings. The first child's `prev` and the last
//!   child's `next` point at the parent instead, tagged with [`UP`].
//!
//! With these links every tree change is O(1): adding a child, moving a
//! capability to another slot (its children are found through `first` and
//! `last`), and deleting one (its children take its place among its
//! siblings, so nothing derived from it is lost from its parent's subtree).
//!
//! # Rules (decision 51 and `formal/revoke/Revoke.tla`)
//!
//! - Deleting a capability never frees an object. Memory comes back only
//!   when an untyped is revoked; its watermark then goes back to zero.
//! - Copies are children of their source; objects made by retype are
//!   children of the untyped. Untyped capabilities are never copied, so
//!   each untyped has exactly one.
//! - All capabilities to one object are neighbours in the tree, so "is this
//!   the last capability to its object" is an O(1) check ([`is_final`]).
//! - Deleting the last capability to a table puts the table on the purge
//!   list ("orphans"): nobody can reach it, but it may still hold
//!   capabilities. Every untyped revoke empties the whole purge list before
//!   it resets, so no slot can live in memory that is handed out again.
//! - A revoke marks its capability [`REVOKING`]: it cannot be copied,
//!   moved, deleted or retyped from until the revoke ends. The revoke is
//!   interruptible: it does its work in steps and stops when the
//!   [`Budget`] says so; calling it again continues.

#![no_std]

#[cfg(any(test, kani))]
extern crate std;

pub use alaya_abi::{Error, Kind, rights};
use alaya_abi::{
    PAGE_BITS, RETYPE_BATCH, SLOT_BITS, SLOT_BYTES, SMALL_OBJECT_BITS, TABLE_MAX_BITS,
    TABLE_MIN_BITS, UNTYPED_MAX_BITS, UNTYPED_MIN_BITS,
};

#[cfg(kani)]
mod proofs;
#[cfg(test)]
mod tests;

/// Address of a slot: the physical address of its 64 bytes. Zero is "none".
pub type SlotRef = u64;

/// Tag on a `prev` or `next` link that points at the parent.
pub const UP: u64 = 1;

/// Internal flag: a revoke of this capability is in progress.
pub const REVOKING: u8 = alaya_abi::flags::REVOKING;

/// The kind byte of a table's header slot (slot 0).
pub const HEADER: u8 = 0xff;

/// Bytes zeroed per step of a retype.
pub const ZERO_CHUNK: u64 = 64 * 1024;

/// A capability: what it names and what it allows.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cap {
    /// [`Kind`] as a byte, or [`HEADER`].
    pub kind: u8,
    pub rights: u8,
    /// Table: log2 of its slot count. Untyped and pages: log2 of bytes.
    pub size_bits: u8,
    pub flags: u8,
    pub _reserved: u32,
    /// Physical address of the object.
    pub object: u64,
    /// Endpoint and notification: badge. Untyped: free offset (watermark).
    /// Header: next table on the purge list.
    pub a: u64,
    /// Untyped: clean offset; memory between `a` and `b` is known to be
    /// zero. Header: purge cursor.
    pub b: u64,
}

impl Cap {
    pub const EMPTY: Cap = Cap {
        kind: 0,
        rights: 0,
        size_bits: 0,
        flags: 0,
        _reserved: 0,
        object: 0,
        a: 0,
        b: 0,
    };

    pub fn kind(&self) -> Option<Kind> {
        Kind::from_u8(self.kind)
    }

    pub fn is_empty(&self) -> bool {
        self.kind == Kind::Empty as u8
    }

    /// A capability that names an object (not empty, not a header).
    pub fn is_live(&self) -> bool {
        !self.is_empty() && self.kind != HEADER
    }

    pub fn is(&self, kind: Kind) -> bool {
        self.kind == kind as u8
    }

    pub fn revoking(&self) -> bool {
        self.flags & REVOKING != 0
    }

    /// Both name the same object (rights and badges may differ).
    pub fn same_object(&self, other: &Cap) -> bool {
        self.is_live()
            && self.kind == other.kind
            && self.object == other.object
            && self.size_bits == other.size_bits
    }

    /// A new capability to a fresh object, with all rights.
    pub fn new(kind: Kind, object: u64, size_bits: u8) -> Cap {
        Cap {
            kind: kind as u8,
            rights: rights::ALL,
            size_bits,
            object,
            ..Cap::EMPTY
        }
    }
}

/// One slot of a capability table.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Slot {
    pub cap: Cap,
    pub prev: u64,
    pub next: u64,
    pub first: u64,
    pub last: u64,
}

impl Slot {
    pub const EMPTY: Slot = Slot {
        cap: Cap::EMPTY,
        prev: 0,
        next: 0,
        first: 0,
        last: 0,
    };
}

const _: () = assert!(core::mem::size_of::<Slot>() as u64 == SLOT_BYTES);
const _: () = assert!(core::mem::size_of::<Cap>() == 32);

/// Where slots live. The kernel reads physical memory through its direct
/// map; tests use an array.
pub trait Memory {
    fn read(&self, at: SlotRef) -> Slot;
    fn write(&mut self, at: SlotRef, slot: &Slot);
    /// Set `len` bytes at physical address `at` to zero.
    fn zero(&mut self, at: u64, len: u64);
}

/// Decides when an interruptible operation must stop. `step` is called
/// after each unit of work and returns true when the operation must
/// return now, to be called again later.
pub trait Budget {
    fn step(&mut self) -> bool;
}

/// A budget that never runs out.
pub struct Unlimited;

impl Budget for Unlimited {
    fn step(&mut self) -> bool {
        false
    }
}

/// How an interruptible operation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress<T> {
    Done(T),
    /// Stopped by the budget; call again to continue.
    Interrupted,
}

/// Kernel-wide capability state (under the big kernel lock).
#[derive(Clone, Copy, Debug, Default)]
pub struct Globals {
    /// First table on the purge list (address of its header), or 0.
    pub orphans: u64,
    /// Table being emptied right now, or 0.
    pub purging: u64,
    /// Capabilities deleted by revokes, ever.
    pub revoked: u64,
}

impl Globals {
    pub const fn new() -> Self {
        Globals {
            orphans: 0,
            purging: 0,
            revoked: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Links

fn is_up(link: u64) -> bool {
    link & UP != 0
}

fn target(link: u64) -> SlotRef {
    link & !UP
}

/// Make whatever `left` names point right at `to`: a sibling's `next`, or
/// the parent's `first` when `left` is the parent.
fn point_right<M: Memory>(m: &mut M, left: u64, to: u64) {
    if left == 0 {
        return;
    }
    let at = target(left);
    let mut s = m.read(at);
    if is_up(left) {
        s.first = if is_up(to) { 0 } else { to };
    } else {
        s.next = to;
    }
    m.write(at, &s);
}

/// Make whatever `right` names point left at `to`.
fn point_left<M: Memory>(m: &mut M, right: u64, to: u64) {
    if right == 0 {
        return;
    }
    let at = target(right);
    let mut s = m.read(at);
    if is_up(right) {
        s.last = if is_up(to) { 0 } else { to };
    } else {
        s.prev = to;
    }
    m.write(at, &s);
}

/// Put the empty slot `child` in as the first child of `parent`, holding
/// `cap`.
pub fn insert_child<M: Memory>(m: &mut M, parent: SlotRef, child: SlotRef, cap: Cap) {
    let mut p = m.read(parent);
    let mut c = Slot { cap, ..Slot::EMPTY };
    c.prev = parent | UP;
    if p.first == 0 {
        c.next = parent | UP;
        p.last = child;
    } else {
        c.next = p.first;
        let mut f = m.read(p.first);
        f.prev = child;
        m.write(p.first, &f);
    }
    p.first = child;
    m.write(parent, &p);
    m.write(child, &c);
}

/// Put `cap` into the empty slot `at` with no parent (boot capabilities).
pub fn insert_root<M: Memory>(m: &mut M, at: SlotRef, cap: Cap) {
    m.write(at, &Slot { cap, ..Slot::EMPTY });
}

/// Take the capability in `at` out of the tree and empty the slot. Its
/// children take its place among its siblings.
fn unlink<M: Memory>(m: &mut M, at: SlotRef) {
    let s = m.read(at);
    let (left, right) = (s.prev, s.next);
    if s.first == 0 {
        point_right(m, left, right);
        point_left(m, right, left);
    } else {
        point_right(m, left, s.first);
        let mut f = m.read(s.first);
        f.prev = left;
        m.write(s.first, &f);
        point_left(m, right, s.last);
        let mut l = m.read(s.last);
        l.next = right;
        m.write(s.last, &l);
    }
    m.write(at, &Slot::EMPTY);
}

/// The parent of the capability in `at`, if it has one. O(siblings): for
/// tests and proofs only; the kernel never needs it.
pub fn parent_of<M: Memory>(m: &M, at: SlotRef, limit: usize) -> Option<SlotRef> {
    let mut x = at;
    for _ in 0..limit {
        let s = m.read(x);
        if s.prev == 0 {
            return None;
        }
        if is_up(s.prev) {
            return Some(target(s.prev));
        }
        x = s.prev;
    }
    None
}

/// Is `at` the last capability to its object? All capabilities to one
/// object are neighbours: a copy is a child of its source, and a deleted
/// capability's children take its place. So it is enough to look at the
/// children and the two neighbours.
pub fn is_final<M: Memory>(m: &M, at: SlotRef) -> bool {
    let s = m.read(at);
    if s.cap.is(Kind::Untyped) {
        return true;
    }
    if s.first != 0 {
        // Everything derived from a non-untyped capability is a copy.
        return false;
    }
    for link in [s.prev, s.next] {
        if link != 0 && m.read(target(link)).cap.same_object(&s.cap) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Tables

/// The slot at `index` of the table at physical address `table` with
/// `2^bits` slots. Index 0 is the header and never valid.
pub fn table_slot(table: u64, bits: u8, index: u64) -> Result<SlotRef, Error> {
    if index == 0 || bits > TABLE_MAX_BITS || index >> bits != 0 {
        return Err(Error::InvalidHandle);
    }
    Ok(table + (index << SLOT_BITS))
}

/// Resolve slot `index` of the table that `table_cap` names, checking that
/// the capability is a table with `need` rights.
pub fn slot_in<M: Memory>(
    m: &M,
    table_cap: SlotRef,
    index: u64,
    need: u8,
) -> Result<SlotRef, Error> {
    let t = m.read(table_cap).cap;
    if !t.is_live() {
        return Err(Error::InvalidHandle);
    }
    if !t.is(Kind::Table) {
        return Err(Error::WrongKind);
    }
    if t.rights & need != need {
        return Err(Error::NoRights);
    }
    table_slot(t.object, t.size_bits, index)
}

/// Write the header of a new table (its memory is already zero).
pub fn init_table<M: Memory>(m: &mut M, table: u64, bits: u8) {
    let header = Slot {
        cap: Cap {
            kind: HEADER,
            size_bits: bits,
            ..Cap::EMPTY
        },
        ..Slot::EMPTY
    };
    m.write(table, &header);
}

// ---------------------------------------------------------------------------
// Operations

fn live<M: Memory>(m: &M, at: SlotRef) -> Result<Slot, Error> {
    let s = m.read(at);
    if s.cap.is_live() {
        Ok(s)
    } else {
        Err(Error::InvalidHandle)
    }
}

fn empty<M: Memory>(m: &M, at: SlotRef) -> Result<(), Error> {
    if m.read(at).cap.is_empty() {
        Ok(())
    } else {
        Err(Error::SlotInUse)
    }
}

/// Copy the capability in `src` to the empty slot `dst` as its child, with
/// at most `mask` rights; `badge` (non-zero) mints a badged endpoint or
/// notification from an unbadged one.
pub fn copy<M: Memory>(
    m: &mut M,
    src: SlotRef,
    dst: SlotRef,
    mask: u8,
    badge: Option<u64>,
) -> Result<(), Error> {
    let s = live(m, src)?;
    empty(m, dst)?;
    if src == dst {
        return Err(Error::SlotInUse);
    }
    if s.cap.is(Kind::Untyped) {
        return Err(Error::WrongKind);
    }
    if s.cap.revoking() {
        return Err(Error::Busy);
    }
    if s.cap.rights & rights::GRANT == 0 {
        return Err(Error::NoRights);
    }
    let mut cap = s.cap;
    cap.rights &= mask;
    cap.flags = 0;
    if let Some(badge) = badge {
        if !(cap.is(Kind::Endpoint) || cap.is(Kind::Notification)) {
            return Err(Error::WrongKind);
        }
        if badge == 0 || cap.a != 0 {
            return Err(Error::InvalidArgument);
        }
        cap.a = badge;
    }
    insert_child(m, src, dst, cap);
    Ok(())
}

/// Move the capability in `src` to the empty slot `dst`, keeping its place
/// in the tree.
pub fn move_cap<M: Memory>(m: &mut M, src: SlotRef, dst: SlotRef) -> Result<(), Error> {
    let s = live(m, src)?;
    empty(m, dst)?;
    if src == dst {
        return Err(Error::SlotInUse);
    }
    if s.cap.revoking() {
        return Err(Error::Busy);
    }
    m.write(dst, &s);
    point_right(m, s.prev, dst);
    point_left(m, s.next, dst);
    if s.first != 0 {
        let mut f = m.read(s.first);
        f.prev = dst | UP;
        m.write(s.first, &f);
        let mut l = m.read(s.last);
        l.next = dst | UP;
        m.write(s.last, &l);
    }
    m.write(src, &Slot::EMPTY);
    Ok(())
}

/// Delete the capability in `at`, whatever it is. If it was the last one
/// to a table, the table goes on the purge list.
fn delete_any<M: Memory>(m: &mut M, g: &mut Globals, at: SlotRef) {
    let s = m.read(at);
    let orphan = s.cap.is(Kind::Table) && is_final(m, at);
    unlink(m, at);
    if orphan {
        let table = s.cap.object;
        let mut h = m.read(table);
        h.cap.a = g.orphans;
        h.cap.b = 1;
        m.write(table, &h);
        g.orphans = table;
    }
}

/// Delete the capability in `at` (a program asked for it).
pub fn delete<M: Memory>(m: &mut M, g: &mut Globals, at: SlotRef) -> Result<(), Error> {
    let s = live(m, at)?;
    if s.cap.revoking() {
        return Err(Error::Busy);
    }
    delete_any(m, g, at);
    Ok(())
}

/// One step of emptying the purge list. Returns false when it is empty.
/// `watch` is the slot of the revoke doing the work: returns `Err` if that
/// slot was emptied.
fn purge_step<M: Memory>(m: &mut M, g: &mut Globals, watch: SlotRef) -> Result<bool, ()> {
    if g.purging == 0 {
        if g.orphans == 0 {
            return Ok(false);
        }
        g.purging = g.orphans;
        g.orphans = m.read(g.purging).cap.a;
    }
    let table = g.purging;
    let h = m.read(table);
    let index = h.cap.b;
    if index >> h.cap.size_bits != 0 {
        g.purging = 0;
        return Ok(true);
    }
    let at = table + (index << SLOT_BITS);
    if m.read(at).cap.is_live() {
        delete_any(m, g, at);
        g.revoked += 1;
    }
    // Re-read: deleting may have touched other headers, never this one
    // (the table is unreachable, so it cannot be orphaned again).
    let mut h = m.read(table);
    h.cap.b = index + 1;
    m.write(table, &h);
    if at == watch { Err(()) } else { Ok(true) }
}

/// Revoke the capability in `at`: delete everything derived from it. For
/// an untyped, then empty every unreachable table and reset its watermark,
/// so its memory can be retyped again.
///
/// Interruptible: returns [`Progress::Interrupted`] when `budget` runs out;
/// calling it again continues. If the capability itself is deleted while
/// the revoke runs (it was inside a table that became unreachable), the
/// revoke ends without resetting.
pub fn revoke<M: Memory, B: Budget>(
    m: &mut M,
    g: &mut Globals,
    at: SlotRef,
    budget: &mut B,
) -> Result<Progress<()>, Error> {
    let mut s = live(m, at)?;
    s.cap.flags |= REVOKING;
    m.write(at, &s);
    loop {
        let first = m.read(at).first;
        if first == 0 {
            break;
        }
        delete_any(m, g, first);
        g.revoked += 1;
        if budget.step() {
            return Ok(Progress::Interrupted);
        }
    }
    if m.read(at).cap.is(Kind::Untyped) {
        loop {
            match purge_step(m, g, at) {
                Ok(false) => break,
                Ok(true) => {}
                Err(()) => return Ok(Progress::Done(())),
            }
            if budget.step() {
                return Ok(Progress::Interrupted);
            }
        }
        let mut s = m.read(at);
        s.cap.a = 0;
        s.cap.b = 0;
        m.write(at, &s);
    }
    let mut s = m.read(at);
    s.cap.flags &= !REVOKING;
    m.write(at, &s);
    Ok(Progress::Done(()))
}

/// Size in bytes (as a power of two) of one object of `kind`, and the
/// `size_bits` its capability records.
pub fn object_bits(kind: Kind, bits: u8, untyped_bits: u8) -> Result<(u8, u8), Error> {
    match kind {
        Kind::Table if (TABLE_MIN_BITS..=TABLE_MAX_BITS).contains(&bits) => {
            Ok((bits + SLOT_BITS, bits))
        }
        Kind::Endpoint | Kind::Notification => Ok((SMALL_OBJECT_BITS, SMALL_OBJECT_BITS)),
        Kind::Page if PAGE_BITS.contains(&bits) => Ok((bits, bits)),
        Kind::Untyped
            if (UNTYPED_MIN_BITS..=UNTYPED_MAX_BITS).contains(&bits) && bits < untyped_bits =>
        {
            Ok((bits, bits))
        }
        _ => Err(Error::InvalidArgument),
    }
}

/// Where retype puts its objects: `count` slots of a table, from `index`.
#[derive(Clone, Copy, Debug)]
pub struct Destination {
    /// Physical address of the table.
    pub table: u64,
    pub bits: u8,
    pub index: u64,
}

/// Make up to `count` objects of `kind` from the untyped in `ut` (at most
/// [`RETYPE_BATCH`] per call), as its children, into consecutive slots of
/// `dst`. Returns how many were made.
///
/// The memory is zeroed first, a chunk per budget step; the progress is
/// kept in the untyped's clean offset, so an interrupted retype continues
/// where it stopped. New untyped memory is not zeroed: it is zeroed when
/// it is retyped in turn.
pub fn retype<M: Memory, B: Budget>(
    m: &mut M,
    ut: SlotRef,
    kind: Kind,
    bits: u8,
    count: u64,
    dst: Destination,
    budget: &mut B,
) -> Result<Progress<u64>, Error> {
    let u = live(m, ut)?.cap;
    if !u.is(Kind::Untyped) {
        return Err(Error::WrongKind);
    }
    if u.rights & rights::WRITE == 0 {
        return Err(Error::NoRights);
    }
    if u.revoking() {
        return Err(Error::Busy);
    }
    let (obj_bits, cap_bits) = object_bits(kind, bits, u.size_bits)?;
    if count == 0 {
        return Err(Error::InvalidArgument);
    }
    let size = 1u64 << obj_bits;
    let region = 1u64 << u.size_bits;
    let start = u.a.next_multiple_of(size);
    let room = if start >= region {
        0
    } else {
        (region - start) >> obj_bits
    };
    let n = count.min(RETYPE_BATCH).min(room);
    if n == 0 {
        return Err(Error::NoMemory);
    }
    for i in 0..n {
        let at = table_slot(dst.table, dst.bits, dst.index.wrapping_add(i))?;
        empty(m, at)?;
    }
    let end = start + n * size;

    if kind != Kind::Untyped {
        let mut clean = u.b;
        while clean < end {
            let len = (end - clean).min(ZERO_CHUNK);
            m.zero(u.object + clean, len);
            clean += len;
            let mut s = m.read(ut);
            s.cap.b = clean;
            m.write(ut, &s);
            if clean < end && budget.step() {
                return Ok(Progress::Interrupted);
            }
        }
    }

    for i in 0..n {
        let object = u.object + start + i * size;
        if kind == Kind::Table {
            init_table(m, object, cap_bits);
        }
        let at = dst.table + ((dst.index + i) << SLOT_BITS);
        insert_child(m, ut, at, Cap::new(kind, object, cap_bits));
    }
    let mut s = m.read(ut);
    s.cap.a = end;
    s.cap.b = s.cap.b.max(end);
    m.write(ut, &s);
    Ok(Progress::Done(n))
}

/// What `inspect` reports about a slot: the capability, and whether
/// anything was derived from it.
pub fn inspect<M: Memory>(m: &M, at: SlotRef) -> (Cap, bool) {
    let s = m.read(at);
    if s.cap.kind == HEADER {
        return (Cap::EMPTY, false);
    }
    (s.cap, s.first != 0)
}

// SPDX-License-Identifier: GPL-3.0-only
//! Address spaces (decision 56): a program's private memory map.
//!
//! An address space object is 8 KiB: its top-level page table, then its
//! bookkeeping. Before use it is given an untyped of its own (the *pool*,
//! moved into the object), and the kernel takes the lower page tables from
//! it as mappings need them. So the kernel still allocates nothing, and
//! programs only ever map pages.
//!
//! Each page capability remembers where it is mapped: the address, and the
//! address space's id with a generation number. Deleting the capability
//! unmaps the page; when an address space goes away its id's generation
//! moves on, so old page capabilities can no longer reach whatever reuses
//! its memory.
//!
//! Unmapping must also remove the page from every core's TLB. Cores that
//! have the address space loaded are sent an interrupt and flush; a core
//! waiting for the big kernel lock flushes while it waits, so this never
//! deadlocks.

use core::sync::atomic::Ordering;

use alaya_abi::{Error, Kind, map, rights};
use alaya_cap::{self as cap, INTERNAL, Memory, SlotRef};

use crate::arch::paging as pte;
use crate::caps::Phys;
use crate::mm::paging::attr;
use crate::obj::{self, SPACE_META, internal, space_meta};
use crate::syscall::USER_END;
use crate::{mm, percpu};

/// Most address spaces at once. An address space's id is also its tag in
/// the TLB (PCID on a PC, ASID on ARM), and tag 0 is the kernel's.
pub const MAX_SPACES: usize = pte::MAX_TAGS;

/// Per core: the tags whose TLB entries may be stale there, because a
/// mapping changed while that core had another address space loaded. The
/// core flushes the tag the next time it loads it.
pub struct Stale([[u64; MAX_SPACES.div_ceil(64) + 1]; crate::percpu::MAX_CPUS]);

impl Stale {
    pub const EMPTY: Stale = Stale([[0; MAX_SPACES.div_ceil(64) + 1]; crate::percpu::MAX_CPUS]);

    fn mark_all(&mut self, id: u64) {
        for core in 0..percpu::count() as usize {
            self.0[core][id as usize / 64] |= 1 << (id % 64);
        }
    }

    fn mark(&mut self, core: usize, id: u64) {
        self.0[core][id as usize / 64] |= 1 << (id % 64);
    }

    /// Take the mark of `id` on `core`: true if it must be flushed.
    fn take(&mut self, core: usize, id: u64) -> bool {
        let w = &mut self.0[core][id as usize / 64];
        let was = *w & 1 << (id % 64) != 0;
        *w &= !(1 << (id % 64));
        was
    }
}

/// Load `space` on this core (`core`), with its tag when tagged memory
/// maps are on, keeping its TLB entries unless they may be stale.
pub fn load(k: &mut crate::sched::Sys, core: usize, space: u64) {
    let id = space_meta(space).id;
    let stale = k.stale.take(core, id);
    pte::load_space(space, id, stale);
}

/// Load the kernel's own tables (tag 0) on this core.
pub fn load_kernel() {
    pte::load_kernel(mm::kernel_root());
}

#[derive(Clone, Copy)]
pub struct SpaceEntry {
    /// The address space object, or 0 when the id is free.
    pub space: u64,
    pub generation: u32,
}

impl SpaceEntry {
    pub const EMPTY: SpaceEntry = SpaceEntry {
        space: 0,
        generation: 0,
    };
}

fn table(phys: u64) -> &'static mut [u64; 512] {
    obj::get(phys)
}

/// The id a page capability records: generation and index + 1.
fn stamp(k: &crate::sched::Sys, id: u64) -> u64 {
    u64::from(k.spaces[id as usize - 1].generation) << 32 | id
}

/// The address space a stamp names, if it still exists.
fn resolve(k: &crate::sched::Sys, stamp: u64) -> Option<u64> {
    let id = (stamp & 0xffff_ffff) as usize;
    let e = k.spaces.get(id.checked_sub(1)?)?;
    (e.space != 0 && u64::from(e.generation) == stamp >> 32).then_some(e.space)
}

/// `SPACE_ASSIGN`: move the untyped in `pool` into the address space and
/// set it up.
pub fn assign(m: &mut Phys, space: u64, pool: SlotRef) -> Result<(), Error> {
    let meta = space_meta(space);
    if !meta.pool.cap.is_empty() {
        return Err(Error::SlotInUse);
    }
    let s = m.read(pool);
    if !s.cap.is(Kind::Untyped) {
        return Err(Error::WrongKind);
    }
    if s.cap.rights & rights::WRITE == 0 {
        return Err(Error::NoRights);
    }
    if s.first != 0 || s.cap.a != 0 || s.cap.revoking() {
        return Err(Error::Busy);
    }
    if meta.id == 0 {
        let id = m
            .sys
            .spaces
            .iter()
            .position(|e| e.space == 0)
            .ok_or(Error::NoMemory)?;
        m.sys.spaces[id].space = space;
        meta.id = id as u64 + 1;
        // An earlier address space with this id may have left entries in
        // some core's TLB under the same tag.
        m.sys.stale.mark_all(meta.id);
        // The kernel's half of every address space is the kernel's own.
        if pte::SHARES_KERNEL_HALF {
            let kernel = table(mm::kernel_root());
            table(space)[pte::KERNEL_HALF..].copy_from_slice(&kernel[pte::KERNEL_HALF..]);
        }
    }
    let dst = space + SPACE_META;
    cap::move_cap(m, pool, dst)?;
    let mut d = m.read(dst);
    d.cap.flags |= INTERNAL;
    d.cap._reserved = internal::SPACE_POOL;
    m.write(dst, &d);
    Ok(())
}

/// Take one zeroed page from the pool for a page table.
fn take_table(m: &mut Phys, space: u64) -> Option<u64> {
    let dst = space + SPACE_META;
    let mut s = m.read(dst);
    if !s.cap.is(Kind::Untyped) {
        return None;
    }
    let size = 1u64 << s.cap.size_bits;
    if s.cap.a + 4096 > size {
        return None;
    }
    let phys = s.cap.object + s.cap.a;
    m.zero(phys, 4096);
    s.cap.a += 4096;
    s.cap.b = s.cap.b.max(s.cap.a);
    m.write(dst, &s);
    space_meta(space).tables += 1;
    Some(phys)
}

/// Levels: 1 maps 4 KiB, 2 maps 2 MiB, 3 maps 1 GiB.
fn level_of(bits: u8) -> u32 {
    match bits {
        12 => 1,
        21 => 2,
        _ => 3,
    }
}

/// `PAGE_MAP`: map the page in `at` into `space` at `virt`.
pub fn map_page(m: &mut Phys, at: SlotRef, space: u64, virt: u64, flags: u64) -> Result<(), Error> {
    let mut s = m.read(at);
    let c = s.cap;
    if c.b != 0 {
        return Err(Error::SlotInUse);
    }
    let meta = space_meta(space);
    if meta.id == 0 {
        return Err(Error::InvalidArgument);
    }
    let size = 1u64 << c.size_bits;
    if flags & !(map::WRITE | map::EXECUTE) != 0
        || !virt.is_multiple_of(size)
        || virt == 0
        || virt.checked_add(size).is_none_or(|e| e > USER_END)
    {
        return Err(Error::InvalidArgument);
    }
    if flags & map::WRITE != 0 && c.rights & rights::WRITE == 0 {
        return Err(Error::NoRights);
    }
    if c.size_bits == 30 && !pte::gib_pages() {
        return Err(Error::InvalidArgument);
    }
    let leaf = level_of(c.size_bits);
    let mut t = space;
    let mut level = 4;
    while level > leaf {
        let i = (virt >> (12 + 9 * (level - 1))) as usize & 511;
        let e = table(t)[i];
        if !pte::present(e) {
            let new = take_table(m, space).ok_or(Error::NoMemory)?;
            table(t)[i] = pte::table(new, true);
            t = new;
        } else if pte::is_block(e) {
            return Err(Error::SlotInUse);
        } else {
            t = pte::address(e);
        }
        level -= 1;
    }
    let i = (virt >> (12 + 9 * (leaf - 1))) as usize & 511;
    if pte::present(table(t)[i]) {
        return Err(Error::SlotInUse);
    }
    let mut a = attr::USER;
    if flags & map::WRITE != 0 {
        a |= attr::WRITABLE;
    }
    if flags & map::EXECUTE == 0 {
        a |= attr::NO_EXECUTE;
    }
    if !mm::is_ram(c.object, size) {
        // Device registers (made by DEVICE_PAGE): strong uncached.
        a |= attr::DEVICE;
    }
    table(t)[i] = pte::leaf(c.object, a, leaf);
    s.cap.a = virt | flags;
    s.cap.b = stamp(m.sys, meta.id);
    m.write(at, &s);
    Ok(())
}

/// Unmap the page capability in `at`, if it is mapped.
pub fn unmap_page(m: &mut Phys, at: SlotRef) {
    let mut s = m.read(at);
    let c = s.cap;
    if c.b == 0 {
        return;
    }
    s.cap.a = 0;
    s.cap.b = 0;
    m.write(at, &s);
    if c.b & crate::iommu::IO_STAMP != 0 {
        crate::iommu::unmap(m.sys, c.b, c.a & !0xfff, c.size_bits, c.object);
        return;
    }
    let Some(space) = resolve(m.sys, c.b) else {
        return;
    };
    let virt = c.a & !0xfff;
    let leaf = level_of(c.size_bits);
    let mut t = space;
    let mut level = 4;
    while level > leaf {
        let e = table(t)[(virt >> (12 + 9 * (level - 1))) as usize & 511];
        if !pte::present(e) || pte::is_block(e) {
            return;
        }
        t = pte::address(e);
        level -= 1;
    }
    let i = (virt >> (12 + 9 * (leaf - 1))) as usize & 511;
    let e = table(t)[i];
    // Only if it still maps this page (the pool may have been taken away
    // and the tables cleared meanwhile).
    if pte::present(e) && pte::address(e) & !((1u64 << c.size_bits) - 1) == c.object {
        table(t)[i] = 0;
        flush(m.sys, space);
    }
}

/// The pool was taken away: drop every user mapping.
pub fn clear(m: &mut Phys, space: u64) {
    table(space)[..pte::KERNEL_HALF].fill(0);
    space_meta(space).tables = 0;
    flush(m.sys, space);
}

/// The last capability to the address space goes: give back its pool and
/// its id. Returns false while some core still has it loaded.
pub fn finalise(m: &mut Phys, g: &mut cap::Globals, space: u64) -> bool {
    let me = percpu::index() as usize;
    for c in 0..percpu::count() as usize {
        if m.sys.cores[c].space == space {
            if c == me && m.sys.cores[c].current == 0 {
                load_kernel();
                m.sys.cores[c].space = 0;
            } else if c != me {
                crate::sched::kick(c);
                return false;
            }
        }
    }
    let pool = space + SPACE_META;
    if m.read(pool).cap.is_live() && !cap::delete_any(m, g, pool) {
        return false;
    }
    let meta = space_meta(space);
    if meta.id != 0 {
        let e = &mut m.sys.spaces[meta.id as usize - 1];
        e.space = 0;
        e.generation = e.generation.wrapping_add(1);
        meta.id = 0;
    }
    true
}

/// Make every core with `space` loaded forget its old translations, and
/// wait until they have. Cores that hold the space's tag but have another
/// space loaded flush it when they next load it.
fn flush(k: &mut crate::sched::Sys, space: u64) {
    let me = percpu::index() as usize;
    let id = space_meta(space).id;
    for c in 0..percpu::count() as usize {
        if k.cores[c].space != space {
            if id != 0 {
                k.stale.mark(c, id);
            }
            continue;
        }
        if c == me {
            pte::flush_local();
        } else {
            let p = percpu::get(c as u32);
            p.flush.store(true, Ordering::Release);
            crate::arch::wake_core(p.hw_id.load(Ordering::Relaxed));
        }
    }
    for c in 0..percpu::count() as usize {
        if c != me && k.cores[c].space == space {
            while percpu::get(c as u32).flush.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        }
    }
}

/// Flush this core's TLB if another core asked. Called on every interrupt
/// and while waiting for the big kernel lock.
pub fn poll_flush() {
    let p = percpu::this();
    if p.flush.load(Ordering::Acquire) {
        pte::flush_local();
        p.flush.store(false, Ordering::Release);
    }
}

/// The physical address `virt` maps to in `space`, if user mode may read it.
pub fn translate(space: u64, virt: u64) -> Option<u64> {
    let mut t = space;
    let mut level = 4;
    loop {
        let e = table(t)[(virt >> (12 + 9 * (level - 1))) as usize & 511];
        if !pte::present(e) {
            return None;
        }
        if level == 1 || pte::is_block(e) {
            if !pte::user(e) {
                return None;
            }
            let size = 1u64 << (12 + 9 * (level - 1));
            return Some((pte::address(e) & !(size - 1)) + (virt & (size - 1)));
        }
        t = pte::address(e);
        level -= 1;
    }
}

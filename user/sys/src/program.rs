// SPDX-License-Identifier: GPL-3.0-only
//! Starting a program from an ELF image (the program starter's library).
//!
//! Everything the new program is made of comes from one untyped capability
//! the parent hands in: its table, address space and page tables, thread,
//! code, stack and the memory it gets for itself. So the parent ends the
//! program, and takes all of it back, with one revoke of that untyped; a
//! keeper restarts a crashed driver exactly this way.

use crate::abi::{
    PROGRAM_STACK_BYTES, PROGRAM_STACK_TOP, START_INFO_ADDRESS, StartInfo, map, rights, start,
};
use crate::elf::Elf;
use crate::{At, Error, Kind};

/// Consecutive free slots of the parent's own table.
#[derive(Clone, Copy, Debug)]
pub struct Slots {
    pub table: u64,
    pub next: u64,
    pub end: u64,
}

impl Slots {
    pub fn take(&mut self) -> Result<u64, Error> {
        if self.next >= self.end {
            return Err(Error::NoMemory);
        }
        self.next += 1;
        Ok(self.next - 1)
    }

    pub fn at(&self, index: u64) -> At {
        At {
            table: self.table,
            index,
        }
    }
}

/// The parent's side: its own table, address space, a free page-sized
/// window to fill pages through, and the slots it may use.
pub struct Parent {
    pub space: u64,
    pub scratch: u64,
    pub slots: Slots,
}

/// What to start.
pub struct Spec<'a> {
    pub name: &'a str,
    /// The ELF file, readable by the parent.
    pub image: &'a [u8],
    /// Untyped memory everything is made from (a handle of the parent).
    pub untyped: u64,
    /// Slots of the new program's table, as a power of two.
    pub table_bits: u8,
    /// Untyped memory the program gets for its own use, as a power of two
    /// (0: none).
    pub own_bits: u8,
    pub priority: u64,
    /// A configured scheduling context for its first thread.
    pub sc: u64,
    /// A configured scheduling context for its clock thread (0: none).
    pub clock_sc: u64,
    /// Endpoint its faults go to (0: none).
    pub fault: u64,
    /// Capabilities to copy into its table from `start::FIRST_GIVEN` on.
    pub given: &'a [u64],
    /// Pages of the parent to map into the program too: (handle, address,
    /// [`map`] flags). The program shares them with the parent; they go
    /// away from it when it ends.
    pub shared: &'a [(u64, u64, u64)],
    pub args: &'a [u64],
    pub tsc_hz: u64,
    pub cores: u64,
}

/// A started program, as handles of the parent.
#[derive(Clone, Copy, Debug, Default)]
pub struct Program {
    pub table: u64,
    pub thread: u64,
    pub space: u64,
}

struct Builder<'p> {
    parent: &'p mut Parent,
    table: u64,
    /// Next free slot of the new program's table.
    child_next: u64,
}

impl Builder<'_> {
    fn child(&self, index: u64) -> At {
        At {
            table: self.table,
            index,
        }
    }

    /// A fresh 4 KiB page from `ut`, filled by `fill` through the scratch
    /// window, then mapped at `at` in `space` and moved into the program's
    /// table.
    fn page(
        &mut self,
        ut: u64,
        space: u64,
        at: u64,
        flags: u64,
        fill: impl FnOnce(&mut [u8; 4096]),
    ) -> Result<(), Error> {
        let p = self.parent.slots.take()?;
        let pa = self.parent.slots.at(p);
        crate::retype(ut, Kind::Page, 12, 1, pa)?;
        crate::page_map(p, self.parent.space, self.parent.scratch, map::WRITE)?;
        // SAFETY: the page was just mapped at the scratch window, writable.
        let buf = unsafe { &mut *(self.parent.scratch as *mut [u8; 4096]) };
        fill(buf);
        crate::page_unmap(p)?;
        crate::page_map(p, space, at, flags)?;
        let dst = self.child(self.child_next);
        self.child_next += 1;
        crate::move_cap(dst, pa)?;
        self.parent.slots.next -= 1;
        Ok(())
    }
}

/// Build and start a program. The parent's slots used for it are given
/// back (all its objects end up in its own table or inside its objects).
pub fn start(parent: &mut Parent, spec: &Spec) -> Result<Program, Error> {
    let elf = Elf::parse(spec.image).ok_or(Error::InvalidArgument)?;
    let s = &mut parent.slots;
    let ut = spec.untyped;
    // The program's own memory first: it is the largest piece, and taking
    // it first wastes nothing on alignment.
    let own_ut = if spec.own_bits != 0 {
        let u = s.take()?;
        crate::retype(ut, Kind::Untyped, spec.own_bits, 1, s.at(u))?;
        Some(u)
    } else {
        None
    };
    let table = s.take()?;
    crate::retype(ut, Kind::Table, spec.table_bits, 1, s.at(table))?;
    let space = s.take()?;
    crate::retype(ut, Kind::AddressSpace, 13, 1, s.at(space))?;
    let pool = s.take()?;
    crate::retype(ut, Kind::Untyped, 16, 1, s.at(pool))?;
    crate::space_assign(space, pool)?;
    s.next -= 1; // the pool moved into the address space
    let thread = s.take()?;
    crate::retype(ut, Kind::Thread, 12, 1, s.at(thread))?;
    let prog = Program {
        table,
        thread,
        space,
    };
    let me = s.table;
    let child = |index| At { table, index };
    crate::copy(child(start::TABLE), s.at(table), rights::ALL)?;
    crate::copy(child(start::THREAD), s.at(thread), rights::ALL)?;
    crate::copy(child(start::SPACE), s.at(space), rights::ALL)?;
    let mut own = 0;
    if let Some(u) = own_ut {
        crate::move_cap(child(start::UNTYPED), s.at(u))?;
        own = u64::from(spec.own_bits);
    }
    if spec.clock_sc != 0 {
        crate::copy(child(start::CLOCK_SC), s.at(spec.clock_sc), rights::ALL)?;
    }
    for (i, &h) in spec.given.iter().enumerate() {
        crate::copy(
            child(start::FIRST_GIVEN + i as u64),
            At {
                table: me,
                index: h,
            },
            rights::ALL,
        )?;
    }

    let mut b = Builder {
        parent,
        table,
        child_next: start::FIRST_GIVEN + spec.given.len() as u64,
    };
    for seg in elf.segments() {
        let first = seg.vaddr & !0xfff;
        let end = (seg.vaddr + seg.mem_size).next_multiple_of(4096);
        let mut flags = 0;
        if seg.write {
            flags |= map::WRITE;
        }
        if seg.execute {
            flags |= map::EXECUTE;
        }
        let mut page = first;
        while page < end {
            b.page(ut, space, page, flags, |buf| {
                buf.fill(0);
                let from = page.max(seg.vaddr);
                let to = (page + 4096).min(seg.vaddr + seg.file_size as u64);
                if from < to {
                    let src = seg.offset + (from - seg.vaddr) as usize;
                    let n = (to - from) as usize;
                    let at = (from - page) as usize;
                    buf[at..at + n].copy_from_slice(&elf.bytes[src..src + n]);
                }
            })?;
            page += 4096;
        }
    }
    for &(page, at, flags) in spec.shared {
        let p = b.parent.slots.take()?;
        let pa = b.parent.slots.at(p);
        crate::copy(pa, b.parent.slots.at(page), rights::ALL)?;
        crate::page_map(p, space, at, flags)?;
        let dst = b.child(b.child_next);
        b.child_next += 1;
        crate::move_cap(dst, pa)?;
        b.parent.slots.next -= 1;
    }
    let mut at = PROGRAM_STACK_TOP - PROGRAM_STACK_BYTES;
    while at < PROGRAM_STACK_TOP {
        b.page(ut, space, at, map::WRITE, |buf| buf.fill(0))?;
        at += 4096;
    }
    let first_free_guess = b.child_next + 1;
    let mut info = StartInfo {
        magic: start::MAGIC,
        name: [0; 32],
        tsc_hz: spec.tsc_hz,
        cores: spec.cores,
        untyped_bits: own,
        first_free: first_free_guess,
        table_slots: 1 << spec.table_bits,
        priority: spec.priority,
        args: [0; start::MAX_ARGS],
    };
    let n = spec.name.len().min(31);
    info.name[..n].copy_from_slice(&spec.name.as_bytes()[..n]);
    let k = spec.args.len().min(start::MAX_ARGS);
    info.args[..k].copy_from_slice(&spec.args[..k]);
    b.page(ut, space, START_INFO_ADDRESS, 0, |buf| {
        buf.fill(0);
        // SAFETY: StartInfo is plain data and fits in a page.
        let bytes = unsafe {
            core::slice::from_raw_parts(
                &info as *const StartInfo as *const u8,
                core::mem::size_of::<StartInfo>(),
            )
        };
        buf[..bytes.len()].copy_from_slice(bytes);
    })?;
    debug_assert!(b.child_next == first_free_guess);

    crate::thread_configure(thread, table, space, spec.fault)?;
    crate::thread_priority(thread, spec.priority, spec.priority)?;
    crate::thread_write(
        thread,
        elf.entry,
        PROGRAM_STACK_TOP - 8,
        START_INFO_ADDRESS,
        0,
        0,
    )?;
    crate::sc_bind(spec.sc, thread)?;
    crate::thread_start(thread)?;
    Ok(prog)
}

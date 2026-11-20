// SPDX-License-Identifier: GPL-3.0-only
//! 4-level page tables of 512 entries, built by the kernel itself. Both
//! PCs and ARM (with 4 KiB pages) use this shape; what an entry looks like
//! is behind the processor wall (`arch::paging`).
//!
//! Mappings use the largest page that fits: 1 GiB where the CPU has it,
//! then 2 MiB, then 4 KiB, as the design asks for huge pages from the start.

use super::frames;
use super::layout::PAGE_SIZE;
use crate::arch::paging as pte;

/// The kernel's own attribute bits; `arch::paging::leaf` turns them into
/// the processor's.
pub mod attr {
    pub const WRITABLE: u64 = 1 << 0;
    /// Reachable from user mode.
    pub const USER: u64 = 1 << 1;
    pub const NO_EXECUTE: u64 = 1 << 2;
    /// The same in every memory map (the kernel's half).
    pub const GLOBAL: u64 = 1 << 3;
    /// Device registers: uncached, in order.
    pub const DEVICE: u64 = 1 << 4;
}
pub use attr::{DEVICE, GLOBAL, NO_EXECUTE, USER, WRITABLE};

const GIB: u64 = 1 << 30;
const MIB2: u64 = 2 << 20;

/// Page sizes by level: level 1 maps 4 KiB, level 2 maps 2 MiB, level 3 1 GiB.
const fn level_size(level: u32) -> u64 {
    PAGE_SIZE << (9 * (level - 1))
}

#[derive(Clone, Copy)]
pub struct AddressSpace {
    root: u64,
    hhdm: u64,
    gib_pages: bool,
    /// Bytes mapped with each page size: 4 KiB, 2 MiB, 1 GiB.
    pub mapped: [u64; 3],
}

impl AddressSpace {
    pub fn new(hhdm: u64, gib_pages: bool) -> Self {
        Self {
            root: frames::alloc_zeroed(),
            hhdm,
            gib_pages,
            mapped: [0; 3],
        }
    }

    /// An address space for a program whose top table is the zeroed page
    /// at `root`: the lower half is empty and the upper half shares the
    /// kernel's tables (`kernel_root`).
    pub fn with_root(root: u64, hhdm: u64, gib_pages: bool, kernel_root: u64) -> Self {
        let space = Self {
            root,
            hhdm,
            gib_pages,
            mapped: [0; 3],
        };
        let kernel = space.table(kernel_root);
        let entries = space.table(space.root);
        entries[pte::KERNEL_HALF..].copy_from_slice(&kernel[pte::KERNEL_HALF..]);
        space
    }

    /// The kernel's own tables, to add mappings at boot.
    pub fn kernel(root: u64, hhdm: u64) -> Self {
        Self {
            root,
            hhdm,
            gib_pages: false,
            mapped: [0; 3],
        }
    }

    /// Map the 4 KiB pages of `[virt, virt + len)` that nothing maps yet
    /// (pages inside a huge mapping count as mapped).
    pub fn map_absent(&mut self, virt: u64, phys: u64, len: u64, flags: u64) {
        let mut off = 0;
        while off < len {
            if !self.is_mapped(virt + off) {
                self.map_one(virt + off, phys + off, 1, flags);
            }
            off += PAGE_SIZE;
        }
    }

    fn is_mapped(&self, virt: u64) -> bool {
        let mut table = self.root;
        let mut level = 4;
        loop {
            let e = self.table(table)[(virt >> (12 + 9 * (level - 1))) as usize & 511];
            if !pte::present(e) {
                return false;
            }
            if level == 1 || pte::is_block(e) {
                return true;
            }
            table = pte::address(e);
            level -= 1;
        }
    }

    /// Physical address of the top-level table, for CR3.
    pub fn root(&self) -> u64 {
        self.root
    }

    fn table(&self, phys: u64) -> &'static mut [u64; 512] {
        // SAFETY: page tables are whole frames reached through the direct
        // map. The kernel's are edited only by the bootstrap core before
        // other cores use them; a program's only under the big kernel lock.
        unsafe { &mut *((phys + self.hhdm) as *mut [u64; 512]) }
    }

    /// Map `len` bytes at `virt` to `phys`. Both must be page aligned.
    /// Panics if any part is already mapped: overlapping mappings are bugs.
    pub fn map(&mut self, mut virt: u64, mut phys: u64, len: u64, flags: u64) {
        assert!(
            virt.is_multiple_of(PAGE_SIZE)
                && phys.is_multiple_of(PAGE_SIZE)
                && len.is_multiple_of(PAGE_SIZE)
        );
        let end = virt + len;
        while virt < end {
            let left = end - virt;
            let fits =
                |size: u64| virt.is_multiple_of(size) && phys.is_multiple_of(size) && left >= size;
            let level = if self.gib_pages && fits(GIB) {
                3
            } else if fits(MIB2) {
                2
            } else {
                1
            };
            self.map_one(virt, phys, level, flags);
            self.mapped[level as usize - 1] += level_size(level);
            virt += level_size(level);
            phys += level_size(level);
        }
    }

    fn map_one(&mut self, virt: u64, phys: u64, leaf_level: u32, flags: u64) {
        let mut table = self.root;
        let mut level = 4;
        while level > leaf_level {
            let index = (virt >> (12 + 9 * (level - 1))) as usize & 511;
            let entries = self.table(table);
            let entry = entries[index];
            let user = flags & USER != 0;
            if !pte::present(entry) {
                let next = frames::alloc_zeroed();
                // Upper levels allow everything; the leaf decides.
                entries[index] = pte::table(next, user);
                table = next;
            } else {
                assert!(
                    !pte::is_block(entry),
                    "mapping {virt:#x} overlaps a huge page"
                );
                entries[index] = pte::widen_table(entry, user);
                table = pte::address(entry);
            }
            level -= 1;
        }
        let index = (virt >> (12 + 9 * (leaf_level - 1))) as usize & 511;
        let entries = self.table(table);
        assert!(!pte::present(entries[index]), "{virt:#x} is already mapped");
        entries[index] = pte::leaf(phys, flags, leaf_level);
    }
}

// SPDX-License-Identifier: GPL-3.0-only
//! The PC's page-table format, and loading and flushing memory maps.
//!
//! Four levels of 512 eight-byte entries, as on ARM with 4 KiB pages, so
//! the walk itself is shared (`mm::paging`, `vspace.rs`); only what an
//! entry looks like lives here. Attributes come in as the kernel's own
//! bits ([`crate::mm::paging::attr`]) and go out as the PC's.
//!
//! Each program's memory map has a tag (PCID) when the CPU offers them, so
//! switching between programs keeps the cached translations of both.

use super::cpu;
use crate::mm::paging::attr;

const PRESENT: u64 = 1 << 0;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const WRITE_THROUGH: u64 = 1 << 3;
const CACHE_DISABLE: u64 = 1 << 4;
const HUGE: u64 = 1 << 7;
const GLOBAL: u64 = 1 << 8;
const NO_EXECUTE: u64 = 1 << 63;
/// Strong uncached (PAT entry 3), for device registers.
const UNCACHED: u64 = WRITE_THROUGH | CACHE_DISABLE;
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;

/// Tells `mov cr3` to keep the TLB entries of the new tag.
const CR3_NOFLUSH: u64 = 1 << 63;

/// Tags for program memory maps: 12 bits, and tag 0 is the kernel's.
pub const MAX_TAGS: usize = 4095;

/// A leaf entry mapping `phys` at `level` (1: 4 KiB, 2: 2 MiB, 3: 1 GiB).
pub fn leaf(phys: u64, attrs: u64, level: u32) -> u64 {
    let mut e = phys | PRESENT;
    if attrs & attr::WRITABLE != 0 {
        e |= WRITABLE;
    }
    if attrs & attr::USER != 0 {
        e |= USER;
    }
    if attrs & attr::NO_EXECUTE != 0 {
        e |= NO_EXECUTE;
    }
    if attrs & attr::GLOBAL != 0 {
        e |= GLOBAL;
    }
    if attrs & attr::DEVICE != 0 {
        e |= UNCACHED;
    }
    if level > 1 {
        e |= HUGE;
    }
    e
}

/// An entry pointing at the next-level table `phys`. On a PC, user mode
/// can only reach a page if every table on the way allows it.
pub fn table(phys: u64, user: bool) -> u64 {
    phys | PRESENT | WRITABLE | if user { USER } else { 0 }
}

/// An existing table entry, opened to user mode if `user`.
pub fn widen_table(entry: u64, user: bool) -> u64 {
    entry | if user { USER } else { 0 }
}

pub fn present(e: u64) -> bool {
    e & PRESENT != 0
}

/// A leaf above level 1 (a large page).
pub fn is_block(e: u64) -> bool {
    e & HUGE != 0
}

/// May user mode reach what the entry maps?
pub fn user(e: u64) -> bool {
    e & USER != 0
}

pub fn address(e: u64) -> u64 {
    e & ADDRESS
}

/// Can leaves sit at level 3 (1 GiB pages)?
pub fn gib_pages() -> bool {
    cpu::features().gib_pages
}

/// The index of the first kernel entry in a top-level table: the upper
/// half belongs to the kernel and is shared by every memory map.
pub const KERNEL_HALF: usize = 256;

/// Turn on the paging features the kernel relies on and load the kernel's
/// own tables, `root`. Every core, once.
pub fn enable(root: u64) {
    assert!(
        cpu::cr4() & cpu::CR4_LA57 == 0,
        "5-level paging is on, but the kernel uses 4 levels"
    );
    // SAFETY: the new tables map everything the kernel uses (its image,
    // the direct map, the stacks). NXE makes the no-execute bit valid, WP
    // makes read-only pages read-only for the kernel too, PGE keeps global
    // (kernel) pages across address-space switches.
    unsafe {
        let efer = cpu::rdmsr(cpu::msr::EFER);
        cpu::wrmsr(cpu::msr::EFER, efer | cpu::EFER_NXE);
        cpu::write_cr0(cpu::cr0() | cpu::CR0_WP);
        cpu::write_cr4(cpu::cr4() | cpu::CR4_PGE);
        cpu::write_cr3(root);
    }
}

/// Load a program's memory map `root`, tagged `tag` when tags are on;
/// `flush` drops what the core cached under that tag.
pub fn load_space(root: u64, tag: u64, flush: bool) {
    let mut cr3 = root;
    if super::protect::pcid() {
        cr3 |= tag;
        if !flush {
            cr3 |= CR3_NOFLUSH;
        }
    }
    // SAFETY: the address space maps the kernel's half too.
    unsafe { cpu::write_cr3(cr3) };
}

/// Load the kernel's own tables (tag 0).
pub fn load_kernel(root: u64) {
    let mut cr3 = root;
    if super::protect::pcid() {
        cr3 |= CR3_NOFLUSH;
    }
    // SAFETY: the kernel's own tables.
    unsafe { cpu::write_cr3(cr3) };
}

/// Load a memory map for a self-test, without tags.
pub fn load_raw(root: u64) {
    // SAFETY: the caller's tables map the kernel's half like the kernel's.
    unsafe { cpu::write_cr3(root) };
}

/// Drop this core's cached translations of the memory map it has loaded.
pub fn flush_local() {
    // SAFETY: reloading CR3 only drops cached translations.
    unsafe { cpu::write_cr3(cpu::cr3()) };
}

/// Make page-table writes visible to a device that walks them without
/// snooping the caches.
pub fn clean_for_device(virt: u64) {
    // SAFETY: flushes one cache line of mapped memory.
    unsafe { core::arch::asm!("clflush [{}]", in(reg) virt, options(nostack)) };
}

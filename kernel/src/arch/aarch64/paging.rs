// SPDX-License-Identifier: GPL-3.0-only
//! ARM's page-table format (4 KiB pages, 48-bit addresses), and loading
//! and flushing memory maps.
//!
//! The tree has the same shape as a PC's (four levels of 512 entries), so
//! the walk is shared (`mm::paging`, `vspace.rs`). ARM keeps two roots: the
//! kernel's tables in TTBR1 (the upper half, the same on every core all the
//! time) and a program's in TTBR0 (the lower half), tagged with the
//! program's address-space id (ASID).
//!
//! Attribute memory types (MAIR): index 0 is normal write-back memory,
//! index 1 device registers (nGnRE).

use super::cpu;
use crate::mm::paging::attr;

const VALID: u64 = 1 << 0;
/// In a table or level-1 (4 KiB) entry: points at a table or is a page.
const TABLE_OR_PAGE: u64 = 1 << 1;
const ATTR_NORMAL: u64 = 0 << 2;
const ATTR_DEVICE: u64 = 1 << 2;
/// AP[1]: user mode may reach it.
const AP_USER: u64 = 1 << 6;
/// AP[2]: read only.
const AP_READ_ONLY: u64 = 1 << 7;
const INNER_SHAREABLE: u64 = 3 << 8;
const ACCESSED: u64 = 1 << 10;
const NOT_GLOBAL: u64 = 1 << 11;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const ADDRESS: u64 = 0x0000_ffff_ffff_f000;

const MAIR: u64 = 0xff | 0x04 << 8;

/// Tags for program memory maps (ASIDs; 16 bits on most cores, tag 0 is
/// the kernel's).
pub const MAX_TAGS: usize = 4095;

/// A leaf entry mapping `phys` at `level` (1: 4 KiB, 2: 2 MiB, 3: 1 GiB).
pub fn leaf(phys: u64, attrs: u64, level: u32) -> u64 {
    let mut e = phys | VALID | ACCESSED;
    if level == 1 {
        e |= TABLE_OR_PAGE;
    }
    if attrs & attr::DEVICE != 0 {
        e |= ATTR_DEVICE;
    } else {
        e |= ATTR_NORMAL | INNER_SHAREABLE;
    }
    if attrs & attr::WRITABLE == 0 {
        e |= AP_READ_ONLY;
    }
    if attrs & attr::USER != 0 {
        // The kernel never runs program code (like SMEP on a PC).
        e |= AP_USER | NOT_GLOBAL | PXN;
        if attrs & attr::NO_EXECUTE != 0 {
            e |= UXN;
        }
    } else {
        // Programs never run kernel code.
        e |= UXN;
        if attrs & attr::NO_EXECUTE != 0 {
            e |= PXN;
        }
    }
    e
}

/// An entry pointing at the next-level table `phys`. ARM checks rights in
/// the leaf only.
pub fn table(phys: u64, _user: bool) -> u64 {
    phys | VALID | TABLE_OR_PAGE
}

pub fn widen_table(entry: u64, _user: bool) -> u64 {
    entry
}

pub fn present(e: u64) -> bool {
    e & VALID != 0
}

/// A leaf above level 1 (a block).
pub fn is_block(e: u64) -> bool {
    e & (VALID | TABLE_OR_PAGE) == VALID
}

pub fn user(e: u64) -> bool {
    e & AP_USER != 0
}

pub fn address(e: u64) -> u64 {
    e & ADDRESS
}

/// 1 GiB blocks exist with 4 KiB pages on every ARMv8 core.
pub fn gib_pages() -> bool {
    true
}

/// Entries of a top-level table below this index are a program's.
pub const KERNEL_HALF: usize = 256;
/// The kernel's half lives in TTBR1, so programs' tables do not carry a
/// copy of it (in TTBR0 a copy would alias the kernel at low addresses).
pub const SHARES_KERNEL_HALF: bool = false;

/// The translation settings: 48-bit addresses in both halves, 4 KiB pages,
/// write-back cached and inner shareable walks, 16-bit ASIDs when the core
/// has them, taken from TTBR0.
fn tcr() -> u64 {
    let f = cpu::features();
    let ips = sysreg!("id_aa64mmfr0_el1") & 0x7;
    let walk = 1 << 8 | 1 << 10 | 3 << 12; // IRGN0, ORGN0 WB-WA, SH0 inner
    16 | walk | 16 << 16 | walk << 16 | 2 << 30 | ips << 32 | u64::from(f.asid16) << 36
}

/// Load the kernel's tables `root` (both halves; the lower half stays
/// empty until a program runs) with the kernel's memory types. Every core,
/// once.
pub fn enable(root: u64) {
    set_sysreg!("mair_el1", MAIR);
    set_sysreg!("tcr_el1", tcr());
    cpu::isb();
    set_sysreg!("ttbr1_el1", root);
    set_sysreg!("ttbr0_el1", root);
    flush_all();
}

/// Load a program's memory map `root` with tag `tag`; `flush` drops what
/// this core cached under that tag.
pub fn load_space(root: u64, tag: u64, flush: bool) {
    cpu::dsb_ish();
    set_sysreg!("ttbr0_el1", root | tag << 48);
    cpu::isb();
    if flush {
        flush_local();
    }
}

/// Load the kernel's own tables (tag 0) in the lower half.
pub fn load_kernel(root: u64) {
    set_sysreg!("ttbr0_el1", root);
    cpu::isb();
}

/// Load a memory map for a self-test, without tags.
pub fn load_raw(root: u64) {
    cpu::dsb_ish();
    set_sysreg!("ttbr0_el1", root);
    cpu::isb();
    flush_all();
}

/// Drop this core's cached translations of the memory map it has loaded.
pub fn flush_local() {
    let asid = sysreg!("ttbr0_el1") >> 48;
    // SAFETY: drops cached translations only.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi aside1, {}",
            "dsb nsh",
            "isb",
            in(reg) asid << 48,
            options(nostack, preserves_flags)
        )
    };
}

fn flush_all() {
    // SAFETY: drops cached translations only.
    unsafe {
        core::arch::asm!(
            "dsb ishst",
            "tlbi vmalle1",
            "dsb nsh",
            "isb",
            options(nostack, preserves_flags)
        )
    };
}

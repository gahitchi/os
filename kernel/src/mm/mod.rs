// SPDX-License-Identifier: GPL-3.0-only
//! Memory: the physical memory map, the kernel's own page tables and the
//! per-core stacks.
//!
//! The bootstrap core builds one set of page tables for the kernel and
//! switches to it; every other core loads the same tables when it starts.
//! From then on nothing the bootloader mapped is relied on, except what the
//! kernel mapped again on purpose.

pub mod frames;
pub mod layout;
pub mod paging;

use core::sync::atomic::{AtomicU64, Ordering};

use layout::{PAGE_SIZE, Stack};
use paging::{AddressSpace, GLOBAL, NO_EXECUTE, UNCACHED, WRITABLE};

use crate::arch::cpu;
use crate::{kprintln, limine};

/// Physical address of the kernel's top-level page table.
static KERNEL_ROOT: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" {
    static __kernel_start: u8;
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __data_end: u8;
}

fn sym(s: &u8) -> u64 {
    s as *const u8 as u64
}

const fn align_down(x: u64) -> u64 {
    x & !(PAGE_SIZE - 1)
}

const fn align_up(x: u64) -> u64 {
    align_down(x + PAGE_SIZE - 1)
}

/// Print the physical memory map the firmware and bootloader handed over.
pub fn print_memory_map() {
    let mut usable = 0;
    for e in limine::memmap().expect("bootloader gave no memory map") {
        kprintln!(
            "mem: {:#014x}-{:#014x} {:>9} KiB  {}",
            e.base,
            e.base + e.length,
            e.length / 1024,
            limine::memmap_kind_name(e.kind)
        );
        if e.kind == limine::MEMMAP_USABLE {
            usable += e.length;
        }
    }
    kprintln!("mem: {} MiB usable", usable / (1024 * 1024));
}

/// Build the kernel's page tables and switch the bootstrap core to them.
///
/// `cpus` is how many cores need stacks; `lapic_mmio` is the physical
/// address of the xAPIC registers when x2APIC is not in use.
pub fn init(cpus: u32, lapic_mmio: Option<u64>) {
    let hhdm = limine::hhdm_offset().expect("bootloader gave no direct map");
    let image = limine::executable_address().expect("bootloader gave no kernel address");
    let features = cpu::features();
    frames::init(hhdm);
    let mut space = AddressSpace::new(hhdm, features.gib_pages);

    // The kernel image, each part with only the rights it needs.
    // SAFETY: linker-provided symbols; only their addresses are used.
    let (start, text, text_end, rodata, rodata_end, data, data_end) = unsafe {
        (
            sym(&__kernel_start),
            sym(&__text_start),
            sym(&__text_end),
            sym(&__rodata_start),
            sym(&__rodata_end),
            sym(&__data_start),
            sym(&__data_end),
        )
    };
    let phys = |virt: u64| virt - image.virtual_base + image.physical_base;
    let mut map_image = |from: u64, to: u64, flags: u64| {
        let (from, to) = (align_down(from), align_up(to));
        if to > from {
            space.map(from, phys(from), to - from, flags | GLOBAL);
        }
    };
    map_image(start, text, NO_EXECUTE); // bootloader requests: read only
    map_image(text, text_end, 0); // code: read and execute
    map_image(rodata, rodata_end, NO_EXECUTE);
    map_image(data, data_end, WRITABLE | NO_EXECUTE);

    // The direct map of physical memory, at the bootloader's offset so that
    // pointers it handed over stay valid. Device and reserved ranges are
    // left out: they are mapped one by one, uncached, when a driver needs
    // them.
    let mut pending: Option<(u64, u64)> = None;
    let mut direct = 0;
    let mut flush = |space: &mut AddressSpace, range: (u64, u64)| {
        space.map(
            range.0 + hhdm,
            range.0,
            range.1 - range.0,
            WRITABLE | NO_EXECUTE | GLOBAL,
        );
        direct += range.1 - range.0;
    };
    for e in limine::memmap().expect("no memory map") {
        if matches!(
            e.kind,
            limine::MEMMAP_RESERVED | limine::MEMMAP_BAD_MEMORY | limine::MEMMAP_FRAMEBUFFER
        ) {
            continue;
        }
        let (from, to) = (align_down(e.base), align_up(e.base + e.length));
        pending = match pending {
            Some((a, b)) if from <= b => Some((a, b.max(to))),
            Some(range) => {
                flush(&mut space, range);
                Some((from, to))
            }
            None => Some((from, to)),
        };
    }
    if let Some(range) = pending {
        flush(&mut space, range);
    }

    if let Some(mmio) = lapic_mmio {
        space.map(
            mmio + hhdm,
            mmio,
            PAGE_SIZE,
            WRITABLE | NO_EXECUTE | GLOBAL | UNCACHED,
        );
    }

    // Every core's stacks, each with an unmapped guard page below it.
    for cpu in 0..cpus {
        for stack in Stack::ALL {
            for page in (0..stack.size()).step_by(PAGE_SIZE as usize) {
                let frame = frames::alloc_zeroed();
                space.map(
                    stack.bottom(cpu) + page,
                    frame,
                    PAGE_SIZE,
                    WRITABLE | NO_EXECUTE | GLOBAL,
                );
            }
        }
    }

    KERNEL_ROOT.store(space.root(), Ordering::Release);
    switch_this_core();

    kprintln!(
        "mem: kernel page tables at {:#x}: direct map {} MiB ({} MiB in 1 GiB pages, {} MiB in 2 MiB, {} KiB in 4 KiB)",
        space.root(),
        direct / (1024 * 1024),
        space.mapped[2] / (1024 * 1024),
        space.mapped[1] / (1024 * 1024),
        space.mapped[0] / 1024,
    );
    kprintln!(
        "mem: kernel took {} KiB for page tables and stacks of {} core(s)",
        frames::taken() / 1024,
        cpus
    );
}

/// Turn on no-execute, write protection and global pages on this core and
/// load the kernel's page tables.
pub fn switch_this_core() {
    let root = KERNEL_ROOT.load(Ordering::Acquire);
    assert!(root != 0, "kernel page tables not built yet");
    assert!(
        cpu::cr4() & cpu::CR4_LA57 == 0,
        "5-level paging is on, but the kernel uses 4 levels"
    );
    // SAFETY: the new tables map everything the kernel uses: its image, the
    // direct map (bootloader structures and this core's boot stack included)
    // and the per-core stacks.
    unsafe {
        let efer = cpu::rdmsr(cpu::msr::EFER);
        cpu::wrmsr(cpu::msr::EFER, efer | cpu::EFER_NXE);
        cpu::write_cr0(cpu::cr0() | cpu::CR0_WP);
        cpu::write_cr4(cpu::cr4() | cpu::CR4_PGE);
        cpu::write_cr3(root);
    }
}

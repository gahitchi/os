// SPDX-License-Identifier: GPL-3.0-only
//! The first program: loading it, giving it all free memory as untyped
//! capabilities, and entering user mode.
//!
//! Like seL4's root task, the first program starts with:
//!
//! - its own capability table (`2^ROOT_BITS` slots), whose capability is
//!   in slot 1;
//! - one untyped capability per block of free memory, from slot 2 on;
//! - a read-only page of [`BootInfo`] at [`BOOT_INFO_ADDRESS`], whose
//!   address it also gets in `rdi`.
//!
//! Its code, stack, table and boot information come from the kernel's
//! boot allocator. Everything else is untyped memory it hands out itself.

use alaya_abi::{
    BOOT_INFO_ADDRESS, BOOT_INFO_MAGIC, BootInfo, BootUntyped, INIT_STACK_BYTES, INIT_STACK_TOP,
    Kind, MAX_BOOT_UNTYPED, SLOT_BITS, UNTYPED_MAX_BITS, UNTYPED_MIN_BITS,
};
use alaya_cap::{self as cap, Cap};

use crate::arch::cpu::{self, msr};
use crate::arch::gdt::{USER_CODE, USER_DATA};
use crate::caps::{Phys, RootTable};
use crate::mm::layout::PAGE_SIZE;
use crate::mm::paging::{AddressSpace, NO_EXECUTE, USER, WRITABLE};
use crate::mm::{self, frames};
use crate::{kprintln, limine, percpu, sync};

/// The first program's table has 4096 slots.
pub const ROOT_BITS: u8 = 12;

/// The first program, as the kernel keeps it.
#[derive(Clone, Copy)]
pub struct Init {
    pub root: RootTable,
    pub space: AddressSpace,
}

/// Load the first program from the boot module, give it its capabilities
/// and run it on this core. Never returns.
pub fn start() -> ! {
    let module = limine::modules()
        .find(|m| m.path().ends_with("/init"))
        .expect("no first program: the boot image lacks boot/init");
    let elf = module.bytes();
    let hhdm = limine::hhdm_offset().unwrap();
    let mut space = AddressSpace::new_user(hhdm, cpu::features().gib_pages, mm::kernel_root());

    let entry = load_elf(&mut space, elf, hhdm);
    for page in (INIT_STACK_TOP - INIT_STACK_BYTES..INIT_STACK_TOP).step_by(PAGE_SIZE as usize) {
        space.map(
            page,
            frames::alloc_zeroed(),
            PAGE_SIZE,
            USER | WRITABLE | NO_EXECUTE,
        );
    }
    let info_frame = frames::alloc_zeroed();
    space.map(BOOT_INFO_ADDRESS, info_frame, PAGE_SIZE, USER | NO_EXECUTE);

    // The table, then all the memory nobody took.
    let table_bytes = 1u64 << (ROOT_BITS + SLOT_BITS);
    let table = frames::alloc_contiguous(table_bytes, table_bytes);
    let mut m = Phys::new();
    cap::init_table(&mut m, table, ROOT_BITS);
    cap::insert_root(
        &mut m,
        table + (1 << SLOT_BITS),
        Cap::new(Kind::Table, table, ROOT_BITS),
    );

    // SAFETY: the frame was just allocated for this and is reached through
    // the direct map.
    let info = unsafe { &mut *((info_frame + hhdm) as *mut BootInfo) };
    info.magic = BOOT_INFO_MAGIC;
    info.table = 1;
    info.table_bits = u64::from(ROOT_BITS);
    info.cores = u64::from(percpu::count());
    let mut handle = 2u64;
    let mut total = 0u64;
    frames::freeze(|start, end| {
        let mut at = start;
        while at < end {
            // The largest aligned power-of-two block that fits.
            let align = if at == 0 { 63 } else { at.trailing_zeros() };
            let fit = 63 - (end - at).leading_zeros();
            let bits = align.min(fit).min(u32::from(UNTYPED_MAX_BITS));
            if bits < u32::from(UNTYPED_MIN_BITS) {
                break;
            }
            let n = info.untyped_count as usize;
            if n == MAX_BOOT_UNTYPED {
                kprintln!("init: more than {MAX_BOOT_UNTYPED} memory blocks; the rest is unused");
                return;
            }
            let slot = table + (handle << SLOT_BITS);
            cap::insert_root(&mut m, slot, Cap::new(Kind::Untyped, at, bits as u8));
            info.untyped[n] = BootUntyped {
                handle,
                base: at,
                size_bits: u64::from(bits),
            };
            info.untyped_count += 1;
            handle += 1;
            total += 1 << bits;
            at += 1 << bits;
        }
    });
    info.first_free = handle;
    kprintln!(
        "init: {} KiB program, {} untyped blocks, {} MiB of untyped memory",
        elf.len() / 1024,
        info.untyped_count,
        total >> 20
    );

    {
        sync::BKL.lock().init = Some(Init {
            root: RootTable {
                table,
                bits: ROOT_BITS,
            },
            space,
        });
    }
    // SAFETY: the address space maps the kernel's upper half as well.
    unsafe { cpu::write_cr3(space.root()) };
    enter(entry, INIT_STACK_TOP, BOOT_INFO_ADDRESS)
}

/// Load the program's segments and return its entry point.
fn load_elf(space: &mut AddressSpace, elf: &[u8], hhdm: u64) -> u64 {
    let u16_at = |o: usize| u16::from_le_bytes([elf[o], elf[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(elf[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(elf[o..o + 8].try_into().unwrap());
    assert!(
        elf.len() >= 64 && elf[..4] == *b"\x7fELF" && elf[4] == 2 && elf[5] == 1,
        "the first program is not a 64-bit little-endian ELF file"
    );
    assert!(u16_at(18) == 62, "the first program is not for x86-64");
    let entry = u64_at(24);
    let (phoff, phentsize, phnum) = (u64_at(32) as usize, u16_at(54) as usize, u16_at(56));
    for i in 0..phnum as usize {
        let ph = phoff + i * phentsize;
        if u32_at(ph) != 1 {
            continue; // not PT_LOAD
        }
        let pflags = u32_at(ph + 4);
        let (offset, vaddr, filesz, memsz) = (
            u64_at(ph + 8) as usize,
            u64_at(ph + 16),
            u64_at(ph + 32) as usize,
            u64_at(ph + 40),
        );
        assert!(
            vaddr >= PAGE_SIZE
                && vaddr + memsz <= crate::syscall::USER_END
                && filesz as u64 <= memsz,
            "the first program has a segment outside user space"
        );
        let mut flags = USER;
        if pflags & 2 != 0 {
            flags |= WRITABLE;
        }
        if pflags & 1 == 0 {
            flags |= NO_EXECUTE;
        }
        let first = vaddr & !(PAGE_SIZE - 1);
        let mut page = first;
        while page < vaddr + memsz {
            let frame = frames::alloc_zeroed();
            space.map(page, frame, PAGE_SIZE, flags);
            // Copy the part of the file that lands in this page.
            let from = page.max(vaddr);
            let to = (page + PAGE_SIZE).min(vaddr + filesz as u64);
            if from < to {
                let src = &elf[offset + (from - vaddr) as usize..offset + (to - vaddr) as usize];
                // SAFETY: the frame is fresh and reached through the direct map.
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        src.as_ptr(),
                        (frame + hhdm + (from - page)) as *mut u8,
                        src.len(),
                    )
                };
            }
            page += PAGE_SIZE;
        }
    }
    entry
}

/// Drop to user mode at `rip` with stack `rsp` and `rdi` = `arg`.
fn enter(rip: u64, rsp: u64, arg: u64) -> ! {
    // The program's GS base starts at zero; `swapgs` below parks the
    // kernel's in KERNEL_GS_BASE until the next entry.
    // SAFETY: only this core's GS bases change.
    unsafe { cpu::wrmsr(msr::KERNEL_GS_BASE, 0) };
    // SAFETY: the frame describes user mode with interrupts on; every
    // general register is cleared so no kernel data reaches the program.
    unsafe {
        core::arch::asm!(
            "swapgs",
            "push {ss}",
            "push {rsp}",
            "push {rflags}",
            "push {cs}",
            "push {rip}",
            "xor eax, eax",
            "xor ebx, ebx",
            "xor ecx, ecx",
            "xor edx, edx",
            "xor esi, esi",
            "xor ebp, ebp",
            "xor r8d, r8d",
            "xor r9d, r9d",
            "xor r10d, r10d",
            "xor r11d, r11d",
            "xor r12d, r12d",
            "xor r13d, r13d",
            "xor r14d, r14d",
            "xor r15d, r15d",
            "iretq",
            ss = in(reg) u64::from(USER_DATA),
            rsp = in(reg) rsp,
            rflags = in(reg) cpu::RFLAGS_IF | 2,
            cs = in(reg) u64::from(USER_CODE),
            rip = in(reg) rip,
            in("rdi") arg,
            options(noreturn),
        )
    }
}

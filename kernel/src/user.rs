// SPDX-License-Identifier: GPL-3.0-only
//! The first program: loading it, giving it all free memory as untyped
//! capabilities, and starting it as the first thread.
//!
//! Like seL4's root task, the first program starts with:
//!
//! - its own capability table (`2^ROOT_BITS` slots), whose capability is
//!   in slot 1;
//! - its own thread (slot 2), scheduling context (slot 3: a round-robin
//!   one, any core), address space (slot 4, with 1 MiB of page-table
//!   memory) and the scheduling control capability (slot 5);
//! - the interrupt control (slot 6) and device control (slot 7)
//!   capabilities, which make it the device manager;
//! - one untyped capability per block of free memory, from slot 8 on;
//! - the other programs of the boot image, mapped read only at
//!   [`BOOT_MODULES_ADDRESS`] and listed in the boot information;
//! - a read-only [`BootInfo`] at [`BOOT_INFO_ADDRESS`], whose address it
//!   also gets in `rdi`.
//!
//! Its code, stack, table, objects and boot information come from the
//! kernel's boot allocator. Everything else is untyped memory it hands out
//! itself.

use alaya_abi::{
    BOOT_INFO_ADDRESS, BOOT_INFO_BYTES, BOOT_INFO_MAGIC, BOOT_MODULES_ADDRESS, BootInfo,
    BootModule, BootUntyped, INIT_STACK_BYTES, INIT_STACK_TOP, Kind, MAX_BOOT_MODULES,
    MAX_BOOT_UNTYPED, MAX_CORES, MAX_PRIORITY, SLOT_BITS, SPACE_BITS, THREAD_BITS,
    UNTYPED_MAX_BITS, UNTYPED_MIN_BITS, boot_flags,
};
use alaya_cap::{self as cap, Cap, INTERNAL, Memory};

use crate::arch;
use crate::caps::Phys;
use crate::mm::layout::PAGE_SIZE;
use crate::mm::paging::{AddressSpace, NO_EXECUTE, USER, WRITABLE};
use crate::mm::{self, frames};
use crate::obj::{self, SLOT_SPACE, SLOT_TABLE, SPACE_META, Sc, Tcb, internal, state};
use crate::{kprintln, limine, percpu, sched, sync, time};

/// The first program's table has 4096 slots.
pub const ROOT_BITS: u8 = 12;

/// Page-table memory for the first program's address space.
const POOL_BITS: u8 = 20;

/// Priority of the first program's thread. It may give out any priority.
const INIT_PRIORITY: u8 = 200;

/// Time slice of the first program's round-robin budget.
const INIT_PERIOD_NS: u64 = 10_000_000;

/// Load the first program from the boot module, give it its capabilities
/// and run it. Never returns.
pub fn start(platform: &crate::acpi::Platform) -> ! {
    let module = limine::modules()
        .find(|m| m.path().ends_with("/init"))
        .expect("no first program: the boot image lacks boot/init");
    let elf = module.bytes();
    let hhdm = limine::hhdm_offset().unwrap();

    let space_obj = frames::alloc_contiguous(1 << SPACE_BITS, 1 << SPACE_BITS);
    let mut space = AddressSpace::with_root(
        space_obj,
        hhdm,
        arch::paging::gib_pages(),
        mm::kernel_root(),
    );
    let (entry, image_start, image_end) = load_elf(&mut space, elf, hhdm);
    for page in (INIT_STACK_TOP - INIT_STACK_BYTES..INIT_STACK_TOP).step_by(PAGE_SIZE as usize) {
        space.map(
            page,
            frames::alloc_zeroed(),
            PAGE_SIZE,
            USER | WRITABLE | NO_EXECUTE,
        );
    }
    // The other programs of the boot image, read only, for the first
    // program to start.
    let mut modules = heapless_modules();
    let mut at = BOOT_MODULES_ADDRESS;
    let mut n = 0;
    for f in limine::modules().filter(|m| !m.path().ends_with("/init")) {
        if n == MAX_BOOT_MODULES {
            kprintln!(
                "init: more than {MAX_BOOT_MODULES} programs in the boot image; the rest are left out"
            );
            break;
        }
        let bytes = f.bytes();
        let phys = bytes.as_ptr() as u64 - hhdm;
        assert!(
            phys.is_multiple_of(PAGE_SIZE),
            "boot module not page aligned"
        );
        let len = (bytes.len() as u64).next_multiple_of(PAGE_SIZE);
        space.map(at, phys, len, USER | NO_EXECUTE);
        let name = f.path().rsplit('/').next().unwrap_or("?").as_bytes();
        let mut m = BootModule {
            address: at,
            size: bytes.len() as u64,
            ..BootModule::default()
        };
        let k = name.len().min(31);
        m.name[..k].copy_from_slice(&name[..k]);
        modules[n] = m;
        n += 1;
        at += len.next_multiple_of(1 << 21);
    }
    let modules = &modules[..n];
    let info_frames = frames::alloc_contiguous(BOOT_INFO_BYTES, PAGE_SIZE);
    space.map(
        BOOT_INFO_ADDRESS,
        info_frames,
        BOOT_INFO_BYTES,
        USER | NO_EXECUTE,
    );

    let thread = frames::alloc_contiguous(1 << THREAD_BITS, 1 << THREAD_BITS);
    let sc_obj = frames::alloc_zeroed();
    let pool = frames::alloc_contiguous(1 << POOL_BITS, 1 << POOL_BITS);
    let table_bytes = 1u64 << (ROOT_BITS + SLOT_BITS);
    let table = frames::alloc_contiguous(table_bytes, table_bytes);

    let mut k = sync::BKL.lock();
    let mut m = Phys::new(&mut k.sys);
    for (kind, object) in [
        (Kind::Thread, thread),
        (Kind::SchedContext, sc_obj),
        (Kind::AddressSpace, space_obj),
    ] {
        obj::stamp(kind, object);
    }
    let slot = |h: u64| table + (h << SLOT_BITS);
    cap::init_table(&mut m, table, ROOT_BITS);
    cap::insert_root(&mut m, slot(1), Cap::new(Kind::Table, table, ROOT_BITS));
    cap::insert_root(&mut m, slot(2), Cap::new(Kind::Thread, thread, THREAD_BITS));
    cap::insert_root(
        &mut m,
        slot(3),
        Cap::new(Kind::SchedContext, sc_obj, alaya_abi::SCHED_BITS),
    );
    cap::insert_root(
        &mut m,
        slot(4),
        Cap::new(Kind::AddressSpace, space_obj, SPACE_BITS),
    );
    cap::insert_root(&mut m, slot(5), Cap::new(Kind::SchedControl, 0, 0));
    cap::insert_root(&mut m, slot(6), Cap::new(Kind::IrqControl, 0, 0));
    cap::insert_root(&mut m, slot(7), Cap::new(Kind::DeviceControl, 0, 0));

    // The address space: its page-table memory and its id.
    let pool_slot = space_obj + SPACE_META;
    let mut pool_cap = Cap::new(Kind::Untyped, pool, POOL_BITS);
    pool_cap.flags = INTERNAL;
    pool_cap._reserved = internal::SPACE_POOL;
    cap::insert_root(&mut m, pool_slot, pool_cap);
    m.sys.spaces[0].space = space_obj;
    obj::space_meta(space_obj).id = 1;

    // The thread holds copies of its table and address space.
    for (i, src, tag) in [
        (SLOT_TABLE, slot(1), internal::THREAD_TABLE),
        (SLOT_SPACE, slot(4), internal::THREAD_SPACE),
    ] {
        let dst = thread + i as u64 * 64;
        cap::copy(&mut m, src, dst, alaya_abi::rights::ALL, None).unwrap();
        let mut s = m.read(dst);
        s.cap.flags |= INTERNAL;
        s.cap._reserved = tag;
        m.write(dst, &s);
    }

    let t = obj::get::<Tcb>(thread);
    t.prio = INIT_PRIORITY;
    t.mcp = MAX_PRIORITY as u8;
    t.regs = crate::arch::Regs::start(entry, INIT_STACK_TOP, BOOT_INFO_ADDRESS, 0, 0);
    t.state = state::READY;
    let period = time::ns_to_ticks(INIT_PERIOD_NS);
    let s = obj::get::<Sc>(sc_obj);
    s.budget.configure(period, period, time::now());
    s.home = thread;
    s.thread = thread;
    t.sc = sc_obj;
    t.home_sc = sc_obj;

    // SAFETY: the frames were just allocated for this and are reached
    // through the direct map.
    let info = unsafe { &mut *((info_frames + hhdm) as *mut BootInfo) };
    info.magic = BOOT_INFO_MAGIC;
    info.table = 1;
    info.table_bits = u64::from(ROOT_BITS);
    info.thread = 2;
    info.sched = 3;
    info.space = 4;
    info.sched_control = 5;
    info.cores = u64::from(percpu::count());
    info.protections = arch::protections();
    info.clock_hz = time::hz();
    info.image_start = image_start;
    info.image_end = image_end;
    for word in limine::cmdline().split_whitespace() {
        if let Some(secs) = word.strip_prefix("fuzz=") {
            info.flags |= boot_flags::FUZZ;
            info.fuzz_seconds = secs.parse().unwrap_or(10);
        } else if word == "long" {
            info.flags |= boot_flags::LONG;
        } else if word == "store-soak" {
            info.flags |= boot_flags::STORE_SOAK;
        }
    }
    for c in 0..(percpu::count() as usize).min(MAX_CORES) {
        info.core_types[c] = m.sys.cores[c].ctype;
    }
    info.irq_control = 6;
    info.device_control = 7;
    if let Some(e) = platform.ecam {
        info.ecam_base = e.base;
        info.ecam_bus_start = u64::from(e.bus_start);
        info.ecam_bus_end = u64::from(e.bus_end);
    }
    info.iommu = u64::from(m.sys.iommu.present());
    info.module_count = modules.len() as u64;
    info.modules[..modules.len()].copy_from_slice(modules);
    let mut handle = 8u64;
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
            cap::insert_root(
                &mut m,
                slot(handle),
                Cap::new(Kind::Untyped, at, bits as u8),
            );
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
    let (kind, mask, size) = arch::fpu::describe();
    kprintln!("fpu: {kind}, state components {mask:#x}, {size} bytes per thread");
    let types = &m.sys.cores[..percpu::count() as usize];
    let perf = types
        .iter()
        .filter(|c| u64::from(c.ctype) == alaya_abi::core_type::PERFORMANCE)
        .count();
    let eff = types
        .iter()
        .filter(|c| u64::from(c.ctype) == alaya_abi::core_type::EFFICIENCY)
        .count();
    if m.sys.hybrid {
        kprintln!("sched: {perf} performance and {eff} efficiency cores");
    } else {
        kprintln!("sched: {} cores of one type", types.len());
    }

    k.sys.first_thread = thread;
    k.sys.started = true;
    sched::make_ready(&mut k.sys, thread);
    crate::syscall::idle(k, percpu::index() as usize)
}

fn heapless_modules() -> [BootModule; MAX_BOOT_MODULES] {
    [BootModule::default(); MAX_BOOT_MODULES]
}

/// Load the program's segments; return its entry point and where its
/// image starts and ends.
fn load_elf(space: &mut AddressSpace, elf: &[u8], hhdm: u64) -> (u64, u64, u64) {
    let u16_at = |o: usize| u16::from_le_bytes([elf[o], elf[o + 1]]);
    let u32_at = |o: usize| u32::from_le_bytes(elf[o..o + 4].try_into().unwrap());
    let u64_at = |o: usize| u64::from_le_bytes(elf[o..o + 8].try_into().unwrap());
    assert!(
        elf.len() >= 64 && elf[..4] == *b"\x7fELF" && elf[4] == 2 && elf[5] == 1,
        "the first program is not a 64-bit little-endian ELF file"
    );
    assert!(
        u16_at(18) == arch::ELF_MACHINE,
        "the first program is not for {}",
        arch::NAME
    );
    let entry = u64_at(24);
    let (phoff, phentsize, phnum) = (u64_at(32) as usize, u16_at(54) as usize, u16_at(56));
    let (mut lowest, mut highest) = (u64::MAX, 0);
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
        lowest = lowest.min(first);
        highest = highest.max((vaddr + memsz).next_multiple_of(PAGE_SIZE));
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
    (entry, lowest, highest)
}

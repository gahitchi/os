// SPDX-License-Identifier: GPL-3.0-only
//! The Alaya kernel.
//!
//! Milestone 1: the kernel boots on every core. It builds its own page
//! tables from the memory map, catches every exception with a clear report
//! on the serial port, finds the TSC frequency and drives the TSC-deadline
//! timer, and brings up every core under the big kernel lock. Each core then
//! runs a short self-test and the bootstrap core reports them all.
//!
//! Milestone 2: capabilities and memory. The kernel hands every free byte
//! of memory to the first program as untyped capabilities and runs it in
//! user mode. The program retypes, copies, moves, deletes and revokes
//! capabilities through system calls (`caps.rs`, `syscall.rs`, and the
//! `alaya-cap` crate), and reports whether all of its checks passed.
//!
//! Milestone 3: threads, time budgets and messages. Threads run on every
//! core under time budgets with sporadic refills (`sched.rs`, the
//! `alaya-sched` crate), placed by the kernel within a core type; they
//! talk through endpoints and notifications, and passive servers run on
//! their callers' time (`ipc.rs`). Programs get their own address spaces
//! (`vspace.rs`).

#![no_std]
#![no_main]

mod acpi;
mod arch;
// The device memory guard is the processor family's own (VT-d on a PC).
use arch::iommu;
mod caps;
mod device;
mod ipc;
mod kstate;
mod limine;
mod mm;
mod obj;
mod objects;
mod percpu;
mod sched;
mod selftest;
mod smp;
mod sync;
mod syscall;
mod time;
mod user;
mod vspace;
#[macro_use]
mod serial;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};

use mm::layout::Stack;

/// Printed once the kernel has finished booting. The runners look for it.
pub const BOOT_OK_MARKER: &str = "alaya: boot ok";

/// Seconds to wait for the other cores before declaring one lost.
const CORE_TIMEOUT_S: u64 = 30;

/// Entry point, called by Limine on the bootstrap processor.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
    percpu::enter(0);
    serial::init();

    kprintln!();
    kprintln!("Alaya {} ({})", env!("CARGO_PKG_VERSION"), arch::NAME);

    if !limine::BASE_REVISION.is_supported() {
        panic!(
            "bootloader does not support Limine base revision {}",
            limine::REVISION
        );
    }
    if let Some((name, version)) = limine::bootloader_info() {
        kprintln!("boot: {name} {version}");
    }
    let cmdline = limine::cmdline();
    if !cmdline.is_empty() {
        kprintln!("boot: command line \"{cmdline}\"");
    }

    let mp = limine::mp().expect("bootloader did not report the cores");
    let mut cores = mp.cpu_count as u32;
    if cores as usize > percpu::MAX_CPUS {
        kprintln!(
            "cpu: {cores} cores, but the kernel supports {}; using those",
            percpu::MAX_CPUS
        );
        cores = percpu::MAX_CPUS as u32;
    }
    percpu::set_count(cores);
    let bsp = mp.bsp_hw_id();
    percpu::get(0).hw_id.store(bsp, Ordering::Relaxed);
    let mut next = 1;
    for info in mp.cpus() {
        if info.hw_id() != bsp && (next as usize) < percpu::MAX_CPUS {
            percpu::get(next)
                .hw_id
                .store(info.hw_id(), Ordering::Relaxed);
            next += 1;
        }
    }
    arch::boot_before_memory(mp, cores);

    mm::print_memory_map();
    mm::init(cores);
    arch::boot_after_memory();

    // Leave the bootloader's stack for this core's own guarded one.
    arch::run_on_stack(Stack::Kernel.top(0), bsp_main, 0)
}

extern "C" fn bsp_main(_: u64) -> ! {
    smp::init_this_core(0);
    kprintln!(
        "cpu 0: online, hardware id {:#x} (bootstrap)",
        percpu::get(0).hw_id.load(Ordering::Relaxed)
    );
    time::calibrate();
    time::init_this_core();
    sched::init_this_core(&mut sync::BKL.lock().sys);
    percpu::get(0).online.store(true, Ordering::Release);

    let cores = percpu::count();
    let mp = limine::mp().unwrap();
    smp::start_others(mp.cpus(), mp.bsp_hw_id());
    let online = || (0..cores).filter(|&i| percpu::get(i).online.load(Ordering::Acquire));
    time::wait_for(
        CORE_TIMEOUT_S,
        || online().count() as u32 == cores,
        || {
            let missing = (0..cores)
                .find(|&i| !percpu::get(i).online.load(Ordering::Acquire))
                .unwrap_or(0);
            panic!("cpu {missing} did not come online within {CORE_TIMEOUT_S}s")
        },
    );
    kprintln!("smp: {cores} of {cores} cores online");
    arch::report_protections();
    kstate::report();

    smp::GO.store(true, Ordering::Release);
    selftest::run_on_this_core();
    time::wait_for(
        CORE_TIMEOUT_S,
        || smp::DONE.load(Ordering::Acquire) == cores,
        || {
            panic!(
                "only {} of {cores} cores finished the self-test within {CORE_TIMEOUT_S}s",
                smp::DONE.load(Ordering::Acquire)
            )
        },
    );

    let total = {
        arch::disable_interrupts();
        sync::BKL.lock().selftest_counter
    };
    let rounds = |i| percpu::get(i).bkl_rounds.load(Ordering::Relaxed);
    let expected: u64 = (0..cores).map(rounds).sum();
    assert!(
        total == expected,
        "big kernel lock test: counter {total}, expected {expected}; the lock does not exclude"
    );
    let fewest = (0..cores).map(rounds).min().unwrap_or(0);
    kprintln!(
        "bkl: {cores} core(s) took the big kernel lock {total} times (each at least {fewest}), counter {total}: ok"
    );
    for i in 0..cores {
        let c = percpu::get(i);
        kprintln!(
            "core {i}: ready, hardware id {:#x}, {} timer, {} ticks, worst lateness {} us",
            c.hw_id.load(Ordering::Relaxed),
            time::timer_name(),
            c.timer_ticks.load(Ordering::Relaxed),
            c.timer_worst_late_ns.load(Ordering::Relaxed) / 1000
        );
    }

    selftest::fault_on_bootstrap_core();
    if selftest::fault_test().is_some() {
        // The fault happens on another core; wait for it to stop us.
        smp::idle();
    }
    // Firmware tables, and the device memory guard.
    let platform = acpi::init();
    if let Some(base) = platform.iommu {
        arch::disable_interrupts();
        let buses = platform.ecam.map_or((0, 0), |e| (e.bus_start, e.bus_end));
        iommu::init(&mut sync::BKL.lock().sys, base, buses, platform.iommu_scope);
    } else {
        kprintln!(
            "iommu: none found; devices could reach all memory ({})",
            iommu::HINT
        );
    }

    // The first program prints the boot marker once its checks pass.
    user::start(&platform)
}

const NO_CPU: u32 = u32::MAX;
pub(crate) static PANIC_CPU: AtomicU32 = AtomicU32::new(NO_CPU);

/// True once any core has started to panic.
pub fn panicking() -> bool {
    PANIC_CPU.load(Ordering::Relaxed) != NO_CPU
}

/// Claim the panic for this core and stop all other cores (an NMI on a
/// PC).
/// Returns false if another core is already panicking.
pub fn begin_panic() -> bool {
    let me = percpu::index();
    match PANIC_CPU.compare_exchange(NO_CPU, me, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(owner) => return owner == me,
    }
    for i in 0..percpu::count() {
        let c = percpu::get(i);
        if i != me && c.can_stop.load(Ordering::Acquire) {
            arch::stop_core(c.hw_id.load(Ordering::Relaxed));
        }
    }
    true
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    arch::disable_interrupts();
    if !begin_panic() {
        // Another core is reporting; stay out of its way.
        arch::halt_forever();
    }
    let mut out = serial::FaultWriter::begin();
    match info.location() {
        Some(at) => out.line(format_args!(
            "alaya: panic on cpu {}: {} (at {}:{})",
            percpu::index(),
            info.message(),
            at.file(),
            at.line()
        )),
        None => out.line(format_args!(
            "alaya: panic on cpu {}: {}",
            percpu::index(),
            info.message()
        )),
    }
    arch::halt_forever()
}

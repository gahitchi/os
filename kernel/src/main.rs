// SPDX-License-Identifier: GPL-3.0-only
//! The Alaya kernel.
//!
//! Milestone 1: the kernel boots on every core. It builds its own page
//! tables from the memory map, catches every exception with a clear report
//! on the serial port, finds the TSC frequency and drives the TSC-deadline
//! timer, and brings up every core under the big kernel lock. Each core then
//! runs a short self-test and the bootstrap core reports them all.

#![no_std]
#![no_main]

mod arch;
mod limine;
mod mm;
mod percpu;
mod selftest;
mod smp;
mod sync;
mod time;
#[macro_use]
mod serial;

use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU32, Ordering};

use arch::{apic, cpu, idt, legacy};
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

    let mut brand = [0; 48];
    let f = cpu::features();
    kprintln!("cpu: {}", cpu::brand(&mut brand));
    kprintln!(
        "cpu: x2APIC {}, TSC deadline {}, invariant TSC {}, 1 GiB pages {}{}",
        yes(f.x2apic),
        yes(f.tsc_deadline),
        yes(f.invariant_tsc),
        yes(f.gib_pages),
        if f.hypervisor {
            ", running under a hypervisor"
        } else {
            ""
        }
    );

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
    let x2apic = mp.flags & limine::MP_X2APIC != 0;
    percpu::get(0)
        .apic_id
        .store(mp.bsp_lapic_id, Ordering::Relaxed);
    let mut next = 1;
    for info in mp.cpus() {
        if info.lapic_id != mp.bsp_lapic_id && (next as usize) < percpu::MAX_CPUS {
            percpu::get(next)
                .apic_id
                .store(info.lapic_id, Ordering::Relaxed);
            next += 1;
        }
    }
    kprintln!(
        "cpu: {cores} core(s), bootstrap APIC id {}, APIC mode {}",
        mp.bsp_lapic_id,
        if x2apic { "x2APIC" } else { "xAPIC" }
    );

    mm::print_memory_map();
    let lapic_mmio = (!x2apic).then(apic::mmio_physical_base);
    mm::init(cores, lapic_mmio);
    let hhdm = limine::hhdm_offset().unwrap();
    apic::set_mode(x2apic, lapic_mmio.map_or(0, |p| p + hhdm));

    idt::init();
    legacy::disable_pic();

    // Leave the bootloader's stack for this core's own guarded one.
    let top = Stack::Kernel.top(0);
    // SAFETY: the stack is mapped and nothing on the old one is used again.
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "xor ebp, ebp",
            "call {main}",
            top = in(reg) top,
            main = sym bsp_main,
            options(noreturn),
        )
    }
}

extern "C" fn bsp_main() -> ! {
    smp::init_this_core(0);
    kprintln!("cpu 0: online, APIC id {} (bootstrap)", apic::id());
    time::calibrate();
    time::init_this_core();
    percpu::get(0).online.store(true, Ordering::Release);

    let cores = percpu::count();
    let mp = limine::mp().unwrap();
    smp::start_others(mp.cpus(), mp.bsp_lapic_id);
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
            "core {i}: ready, APIC id {}, {} timer, {} ticks, worst lateness {} us",
            c.apic_id.load(Ordering::Relaxed),
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
    kprintln!("{BOOT_OK_MARKER}");
    smp::idle()
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

const NO_CPU: u32 = u32::MAX;
static PANIC_CPU: AtomicU32 = AtomicU32::new(NO_CPU);

/// True once any core has started to panic.
pub fn panicking() -> bool {
    PANIC_CPU.load(Ordering::Relaxed) != NO_CPU
}

/// Claim the panic for this core and stop all other cores with an NMI.
/// Returns false if another core is already panicking.
pub fn begin_panic() -> bool {
    let me = percpu::index();
    match PANIC_CPU.compare_exchange(NO_CPU, me, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => {}
        Err(owner) => return owner == me,
    }
    for i in 0..percpu::count() {
        let c = percpu::get(i);
        if i != me && c.has_idt.load(Ordering::Acquire) {
            apic::send_nmi(c.apic_id.load(Ordering::Relaxed));
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

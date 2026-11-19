// SPDX-License-Identifier: GPL-3.0-only
//! The Alaya kernel.
//!
//! Milestone 0: boot through Limine, report what the bootloader handed over,
//! and print a fixed marker on the serial port so that CI and the runners can
//! tell that the kernel came up.

#![no_std]
#![no_main]

mod arch;
mod limine;
mod sync;
#[macro_use]
mod serial;

use core::panic::PanicInfo;

/// Printed once the kernel has finished booting. The runners look for it.
pub const BOOT_OK_MARKER: &str = "alaya: boot ok";

/// Entry point, called by Limine on the bootstrap processor.
#[unsafe(no_mangle)]
extern "C" fn kmain() -> ! {
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

    if let Some(mp) = limine::mp() {
        kprintln!(
            "cpu: {} core(s), bootstrap LAPIC id {}",
            mp.cpu_count,
            mp.bsp_lapic_id
        );
    }

    if let Some(mem) = limine::memory_summary() {
        kprintln!(
            "mem: {} MiB usable in {} region(s), {} map entries",
            mem.usable_bytes / (1024 * 1024),
            mem.usable_regions,
            mem.entries
        );
    }

    if let Some(offset) = limine::hhdm_offset() {
        kprintln!("mem: direct map at {offset:#x}");
    }

    kprintln!("{BOOT_OK_MARKER}");
    arch::halt_forever()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    arch::disable_interrupts();
    // The serial lock may be held by the code that panicked; write directly.
    serial::emergency_print(format_args!("alaya: panic: {info}\n"));
    arch::halt_forever()
}

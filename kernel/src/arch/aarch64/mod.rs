// SPDX-License-Identifier: GPL-3.0-only
//! The ARM side of the processor wall (ARMv8-A, 64-bit).
//!
//! Everything below this folder knows it runs on ARM: exception levels and
//! vectors, the GICv3 interrupt controller and its ITS, the generic timer,
//! ARM's page-table format and address-space ids, the PL011 serial port,
//! the SMMU. The baseline is ARMv8.0 (chips from about 2016 on, decision
//! 98); later features (PAN, 16-bit ASIDs, single-instruction atomics) are
//! used when the core has them. `arch/mod.rs` lists what the rest of the
//! kernel may use.

#[macro_use]
pub mod cpu;
pub mod console;
pub mod entry;
pub mod fpu;
pub mod gic;
pub mod iommu;
pub mod its;
pub mod paging;
pub mod protect;
pub mod selftest;
pub mod timer;
pub mod trap;

use core::sync::atomic::Ordering;

use crate::kstate::Item;
use crate::limine::MpResponse;
use crate::mm::layout::Stack;
use crate::{item, kprintln, percpu};

pub use entry::{Regs, TrapFrame};

pub const NAME: &str = "ARM64";
/// The ELF machine number of programs for this processor (AArch64).
pub const ELF_MACHINE: u16 = 183;

/// Stop taking interrupts on this core.
#[inline]
pub fn disable_interrupts() {
    // SAFETY: masks IRQs on this core only.
    unsafe { core::arch::asm!("msr daifset, #2", options(nomem, nostack)) };
}

#[inline]
pub fn enable_interrupts() {
    // SAFETY: unmasks IRQs on this core only.
    unsafe { core::arch::asm!("msr daifclr, #2", options(nomem, nostack)) };
}

#[inline]
pub fn interrupts_enabled() -> bool {
    sysreg!("daif") & 1 << 7 == 0
}

/// Sleep until the next interrupt, then take it. WFI wakes for a pending
/// interrupt even while interrupts are masked, so none can slip in between.
#[inline]
pub fn wait_for_interrupt() {
    // SAFETY: waits, then lets the pending interrupt in and masks again.
    unsafe {
        core::arch::asm!(
            "dsb sy",
            "wfi",
            "msr daifclr, #2",
            "isb",
            "msr daifset, #2",
            options(nomem, nostack)
        )
    };
}

#[inline]
pub fn spin_hint() {
    core::hint::spin_loop();
}

/// Park this core for good.
pub fn halt_forever() -> ! {
    // SAFETY: masks everything on this core.
    unsafe { core::arch::asm!("msr daifset, #0xf", options(nomem, nostack)) };
    loop {
        // SAFETY: waits for an event that never matters.
        unsafe { core::arch::asm!("wfi", options(nomem, nostack)) };
    }
}

/// Point this core's per-core register (TPIDR_EL1) at its `PerCpu`.
#[inline]
pub fn set_cpu_base(base: u64) {
    set_sysreg!("tpidr_el1", base);
}

/// The running core's number: the first field of its `PerCpu`.
#[inline]
pub fn cpu_index() -> u32 {
    let base = sysreg!("tpidr_el1");
    // SAFETY: TPIDR_EL1 points at this core's `PerCpu`, whose first field
    // is the index.
    unsafe { core::ptr::read_volatile(base as *const u32) }
}

#[inline(always)]
pub fn speculation_barrier() {
    protect::barrier();
}

/// The core is about to run another program's code. ARM cores keep branch
/// predictions apart by ASID where they are affected (Spectre v2 needs
/// firmware help, not used yet).
#[inline]
pub fn on_space_switch(_guarded: bool) {}

pub fn boot_before_memory(mp: &MpResponse, cores: u32) {
    let f = cpu::features();
    kprintln!(
        "cpu: {} (implementer {:#x}, part {:#x}), PAN {}, 16-bit ASIDs {}, LSE atomics {}, {}-bit physical addresses",
        cpu::name(&f),
        f.implementer,
        f.part,
        yes(f.pan),
        yes(f.asid16),
        yes(f.atomics),
        f.pa_bits
    );
    kprintln!(
        "cpu: {cores} core(s), bootstrap MPIDR {:#x}, running at EL{}",
        mp.bsp_hw_id(),
        (sysreg!("currentel") >> 2) & 3
    );
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// Device registers the kernel maps while building its tables: none on
/// ARM (they are found in the firmware tables afterwards).
pub fn kernel_device_pages() -> [Option<(u64, u64)>; 0] {
    []
}

/// Device registers no program may ever map: the interrupt controller, its
/// message translator and the device memory guard.
pub fn kernel_only_device_pages() -> [(u64, u64); 4] {
    [
        (gic::GICD_PHYS.load(Ordering::Relaxed), 0x1_0000),
        (
            gic::GICR_PHYS.load(Ordering::Relaxed),
            gic::GICR_LEN.load(Ordering::Relaxed),
        ),
        (its::PHYS.load(Ordering::Relaxed), 0x2_0000),
        (iommu::PHYS.load(Ordering::Relaxed), iommu::REGS_BYTES),
    ]
}

/// Once the kernel's tables exist: find the serial port and the interrupt
/// controller in the firmware tables and switch them on.
pub fn boot_after_memory() {
    if let Some(spcr) = crate::acpi::find(b"SPCR")
        && spcr.len() >= 52
    {
        let base = u64::from_le_bytes(spcr[44..52].try_into().unwrap());
        let virt = crate::mm::map_physical(base, 4096, true);
        crate::serial::SERIAL.lock().ready(virt);
        kprintln!("console: serial port at {base:#x} (from SPCR)");
    }
    let madt =
        crate::acpi::find(b"APIC").expect("the firmware has no MADT (interrupt controller table)");
    let layout = gic::parse_madt(madt);
    gic::init(&layout);
}

/// Switch to stack `top` (as SP_EL1) and call `f(arg)` there. The old
/// stack is left behind for good.
pub fn run_on_stack(top: u64, f: extern "C" fn(u64) -> !, arg: u64) -> ! {
    // SAFETY: the stack is mapped and nothing on the old one is used again.
    unsafe {
        core::arch::asm!(
            "msr spsel, #1",
            "mov sp, {top}",
            "mov x29, xzr",
            "mov x30, xzr",
            "br {f}",
            top = in(reg) top,
            f = in(reg) f,
            in("x0") arg,
            options(noreturn),
        )
    }
}

/// Set up the calling core: exception vectors, FP for user mode,
/// protections, interrupt controller. Returns the core's MPIDR.
pub fn init_this_core(index: u32) -> u64 {
    let me = percpu::get(index);
    me.kernel_stack_top
        .store(Stack::Kernel.top(index), Ordering::Relaxed);
    // The entry code finds the emergency stack here (see entry.rs).
    me.user_sp
        .store(Stack::DoubleFault.top(index), Ordering::Relaxed);
    entry::init_this_core();
    fpu::init_this_core();
    if index == 0 {
        protect::detect();
    }
    protect::init_this_core();
    gic::init_this_core(index);
    me.can_stop.store(true, Ordering::Release);
    cpu::mpidr()
}

/// All cores are alike to the kernel for now (big.LITTLE placement comes
/// with real ARM devices).
pub fn core_type() -> u8 {
    0
}

pub fn wake_core(hw_id: u64) {
    gic::send_sgi(hw_id, gic::WAKE_SGI);
}

/// Ask a core to stop now (on a panic). Unlike a PC's NMI, a core with
/// interrupts masked only stops when it next takes them.
pub fn stop_core(hw_id: u64) {
    gic::send_sgi(hw_id, gic::STOP_SGI);
}

/// The message a device with PCI requester id `rid` writes to raise device
/// interrupt `irq` on core `core`: the ITS doorbell's address (as the
/// device sees it, through the device memory guard) and the event. None
/// without an ITS.
pub fn msi_message(core: u16, _hw_id: u64, irq: u8, rid: u16) -> Option<(u64, u64)> {
    let dev = iommu::device_id(rid);
    let event = its::map(dev, irq, core)?;
    Some((iommu::doorbell(), event))
}

/// Device interrupt `irq` was given up: drop the device's writes for it.
pub fn msi_release(irq: u8) {
    its::unmap(irq);
}

pub fn protections() -> u64 {
    protect::enabled()
}

pub fn report_protections() {
    protect::report();
}

pub fn state_items() -> [Item; 6] {
    [
        item!("FPU state size", Carried, fpu::SIZE),
        item!("processor protections", Carried, protect::ON),
        item!("interrupt distributor", Carried, gic::GICD_PHYS),
        item!("interrupt redistributors", Carried, gic::GICR_PHYS),
        item!("redistributor of each core", Carried, gic::RD),
        item!("serial port registers", Carried, console::BASE),
    ]
}

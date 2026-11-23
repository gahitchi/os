// SPDX-License-Identifier: GPL-3.0-only
//! The PC side of the processor wall (x86-64).
//!
//! Everything below this folder knows it runs on a PC: the APIC, the
//! segment and interrupt tables, `syscall`, the TSC, the legacy timer and
//! serial port, the PC's page-table format, VT-d. `arch/mod.rs` lists what
//! the rest of the kernel may use.

pub mod apic;
pub mod console;
pub mod cpu;
pub mod entry;
pub mod fpu;
pub mod gdt;
pub mod idt;
pub mod iommu;
pub mod legacy;
pub mod paging;
pub mod port;
pub mod protect;
pub mod selftest;
pub mod timer;
pub mod trap;

use core::sync::atomic::Ordering;

use crate::kstate::Item;
use crate::limine::{self, MpResponse};
use crate::mm::layout::Stack;
use crate::{item, kprintln, percpu};

pub use entry::Regs;
pub use idt::TrapFrame;

pub const NAME: &str = "x86-64";
/// The ELF machine number of programs for this processor.
pub const ELF_MACHINE: u16 = 62;

/// Stop taking interrupts on this core.
#[inline]
pub fn disable_interrupts() {
    // SAFETY: `cli` only changes the interrupt flag of this core.
    unsafe { core::arch::asm!("cli", options(nomem, nostack)) };
}

#[inline]
pub fn enable_interrupts() {
    cpu::enable_interrupts();
}

#[inline]
pub fn interrupts_enabled() -> bool {
    cpu::interrupts_enabled()
}

/// Sleep until the next interrupt, with interrupts on for that moment
/// only; a wake-up cannot slip in between.
#[inline]
pub fn wait_for_interrupt() {
    cpu::enable_interrupts_and_halt();
}

/// Hint to the core that it is spinning on a lock.
#[inline]
pub fn spin_hint() {
    core::hint::spin_loop();
}

/// Park this core for good.
pub fn halt_forever() -> ! {
    disable_interrupts();
    loop {
        // SAFETY: `hlt` with interrupts off simply stops the core (an NMI
        // can still wake it, and the loop halts it again).
        unsafe { core::arch::asm!("hlt", options(nomem, nostack)) };
    }
}

/// Point this core's per-core register (GS base) at its `PerCpu`.
#[inline]
pub fn set_cpu_base(base: u64) {
    cpu::set_gs_base(base);
}

/// The running core's number: the first field of its `PerCpu`.
#[inline]
pub fn cpu_index() -> u32 {
    let i: u32;
    // SAFETY: GS points at this core's `PerCpu` (see `percpu::enter`),
    // whose first field is the index.
    unsafe {
        core::arch::asm!("mov {:e}, gs:[0]", out(reg) i, options(readonly, nostack, preserves_flags))
    };
    i
}

/// Stop speculation past a bounds check on a value a program chose.
#[inline(always)]
pub fn speculation_barrier() {
    protect::barrier();
}

/// The core is about to run another program's code.
#[inline]
pub fn on_space_switch(guarded: bool) {
    protect::on_space_switch(guarded);
}

/// What the bootstrap core does before the kernel's memory map exists:
/// describe the CPU and the cores, and choose the APIC mode.
pub fn boot_before_memory(mp: &MpResponse, cores: u32) {
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
    let x2apic = mp.flags & limine::MP_X2APIC != 0;
    apic::set_mode(x2apic, 0);
    kprintln!(
        "cpu: {cores} core(s), bootstrap APIC id {}, APIC mode {}",
        mp.bsp_hw_id(),
        if x2apic { "x2APIC" } else { "xAPIC" }
    );
}

/// Device registers the kernel itself uses and must map (physical address
/// and length): the xAPIC page when not in x2APIC mode.
pub fn kernel_device_pages() -> [Option<(u64, u64)>; 1] {
    [(!apic::is_x2apic()).then(|| (apic::mmio_physical_base(), 4096))]
}

/// Device registers no program may ever map: the local APIC's page.
pub fn kernel_only_device_pages() -> [(u64, u64); 1] {
    [(apic::mmio_physical_base() & !0xfff, 4096)]
}

/// What the bootstrap core does once the kernel's memory map is loaded.
pub fn boot_after_memory() {
    if !apic::is_x2apic() {
        let hhdm = limine::hhdm_offset().unwrap();
        apic::set_mode(false, apic::mmio_physical_base() + hhdm);
    }
    idt::init();
    legacy::disable_pic();
}

fn yes(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

/// Switch to stack `top` and call `f(arg)` there. The old stack is left
/// behind for good.
pub fn run_on_stack(top: u64, f: extern "C" fn(u64) -> !, arg: u64) -> ! {
    // SAFETY: the stack is mapped and nothing on the old one is used again.
    unsafe {
        core::arch::asm!(
            "mov rsp, {top}",
            "xor ebp, ebp",
            "call {f}",
            top = in(reg) top,
            f = in(reg) f,
            in("rdi") arg,
            options(noreturn),
        )
    }
}

/// Set up the calling core: segments, interrupt stacks, system calls, FPU,
/// interrupt table, protections, local APIC. Returns the core's hardware
/// id as the APIC reports it.
pub fn init_this_core(index: u32) -> u64 {
    gdt::load_this_core(
        index,
        [
            Stack::DoubleFault.top(index),
            Stack::Nmi.top(index),
            Stack::MachineCheck.top(index),
        ],
        Stack::Kernel.top(index),
    );
    entry::init_this_core();
    fpu::init_this_core();
    const _: () = assert!(gdt::IST_DOUBLE_FAULT == 1 && gdt::IST_NMI == 2);
    idt::load();
    if index == 0 {
        protect::detect();
    }
    protect::init_this_core();
    percpu::get(index).can_stop.store(true, Ordering::Release);
    apic::init_this_core();
    u64::from(apic::id())
}

/// The kind of this core (`alaya_abi::core_type`): performance or
/// efficiency on a hybrid Intel CPU, 0 when all cores are alike.
pub fn core_type() -> u8 {
    use alaya_abi::core_type;
    use cpu::cpuid;
    let hybrid = cpu::max_leaf() >= 7 && cpuid(7, 0).edx & (1 << 15) != 0;
    if hybrid && cpu::max_leaf() >= 0x1a {
        match cpuid(0x1a, 0).eax >> 24 {
            0x40 => core_type::PERFORMANCE as u8,
            0x20 => core_type::EFFICIENCY as u8,
            _ => 0,
        }
    } else {
        0
    }
}

/// Ask the core with hardware id `hw_id` to look at its run queue (and its
/// flush request) again.
pub fn wake_core(hw_id: u64) {
    apic::send_ipi(hw_id as u32, trap::RESCHED_VECTOR);
}

/// Stop the core with hardware id `hw_id` now, whatever it is doing (an
/// NMI). Used on a panic.
pub fn stop_core(hw_id: u64) {
    apic::send_nmi(hw_id as u32);
}

/// The message a device writes to raise device interrupt `irq` on core
/// `core` (hardware id `hw_id`): MSI address and data. A PC's local APICs
/// take the message whichever device (`_rid`) writes it.
pub fn msi_message(_core: u16, hw_id: u64, irq: u8, _rid: u16) -> Option<(u64, u64)> {
    // Fixed delivery, edge triggered, physical destination.
    Some((0xfee0_0000 | (hw_id & 0xff) << 12, u64::from(irq)))
}

/// Device interrupt `irq` was given up. Nothing to undo on a PC.
pub fn msi_release(_irq: u8) {}

/// The protections the processor offers and the kernel turned on, as
/// `alaya_abi::protect` bits.
pub fn protections() -> u64 {
    protect::enabled()
}

/// Print what the protections are, at boot.
pub fn report_protections() {
    protect::report();
}

/// Kernel-wide state behind the wall (see `kstate.rs`).
pub fn state_items() -> [Item; 11] {
    [
        item!("timer mode", Carried, timer::DEADLINE_MODE),
        item!("APIC timer ratio", Carried, timer::APIC_PER_TSC_FIXED),
        item!("APIC mode", Carried, apic::X2APIC),
        item!("APIC registers", Carried, apic::MMIO),
        item!("FPU save method", Carried, fpu::XSAVE),
        item!("FPU state components", Carried, fpu::MASK),
        item!("FPU state size", Carried, fpu::SIZE),
        item!("processor protections", Carried, protect::ON),
        item!("interrupt table", Rebuilt, idt::IDT),
        item!("segment tables", Rebuilt, gdt::GDTS),
        item!("task state segments", Rebuilt, gdt::TSSES),
    ]
}

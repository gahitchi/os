// SPDX-License-Identifier: GPL-3.0-only
//! What happens on every interrupt and exception.
//!
//! Exceptions in the kernel are bugs, so each one prints a full report on
//! the serial port (and screen) and stops the machine. The report names the
//! exception, the core, the faulting address for page faults, and whether a
//! kernel stack ran into its guard page.

use super::cpu;
use super::idt::TrapFrame;
use super::{apic, legacy};
use crate::{percpu, sync};

/// The timer interrupt of every core.
pub const TIMER_VECTOR: u8 = 0x40;
/// Sent by another core: look at the run queue again (and flush the TLB
/// if asked).
pub const RESCHED_VECTOR: u8 = 0x41;

const NAMES: [(&str, &str); 32] = [
    ("#DE", "divide error"),
    ("#DB", "debug"),
    ("NMI", "non-maskable interrupt"),
    ("#BP", "breakpoint"),
    ("#OF", "overflow"),
    ("#BR", "bound range exceeded"),
    ("#UD", "invalid opcode"),
    ("#NM", "device not available"),
    ("#DF", "double fault"),
    ("--", "coprocessor segment overrun"),
    ("#TS", "invalid TSS"),
    ("#NP", "segment not present"),
    ("#SS", "stack-segment fault"),
    ("#GP", "general protection fault"),
    ("#PF", "page fault"),
    ("--", "reserved vector 15"),
    ("#MF", "x87 floating-point error"),
    ("#AC", "alignment check"),
    ("#MC", "machine check"),
    ("#XM", "SIMD floating-point error"),
    ("#VE", "virtualization exception"),
    ("#CP", "control protection"),
    ("--", "reserved vector 22"),
    ("--", "reserved vector 23"),
    ("--", "reserved vector 24"),
    ("--", "reserved vector 25"),
    ("--", "reserved vector 26"),
    ("--", "reserved vector 27"),
    ("#HV", "hypervisor injection"),
    ("#VC", "VMM communication"),
    ("#SX", "security exception"),
    ("--", "reserved vector 31"),
];

/// Called from the assembly stubs in `idt.rs` with interrupts off.
pub extern "C" fn dispatch(frame: &mut TrapFrame) {
    let vector = frame.vector as u8;
    super::protect::on_interrupt_entry();
    crate::vspace::poll_flush();
    if frame.cs & 3 == 3 && !matches!(vector, 2 | 8 | 18) {
        return from_user(frame);
    }
    if vector == 13 {
        // A safe MSR access to an MSR this CPU does not have.
        if let Some(rip) = super::protect::fixup(frame.rip) {
            frame.rip = rip;
            frame.rax = 1;
            return;
        }
    }
    match vector {
        2 => nmi(frame),
        0..=31 => fault(frame),
        TIMER_VECTOR => {
            crate::time::interrupt();
            apic::eoi();
        }
        RESCHED_VECTOR => apic::eoi(),
        crate::device::FIRST_VECTOR..=crate::device::LAST_VECTOR => {
            // A device, while this core idled in the kernel.
            apic::eoi();
            let mut k = sync::BKL.lock();
            crate::device::interrupt(&mut k.sys, vector);
        }
        apic::SPURIOUS_VECTOR => {} // no EOI for spurious interrupts
        v if v == legacy::PIC_BASE + 7 || v == legacy::PIC_BASE + 15 => {} // PIC spurious
        _ => {
            crate::kprintln!(
                "irq: unexpected vector {:#x} on cpu {}",
                vector,
                percpu::index()
            );
            apic::eoi();
        }
    }
}

/// An interrupt or exception while a thread ran in user mode: save the
/// thread, handle it, and go back to user mode with whatever runs next.
fn from_user(frame: &mut TrapFrame) {
    let vector = frame.vector as u8;
    let is_fault = vector < 32;
    if !is_fault && vector != apic::SPURIOUS_VECTOR {
        apic::eoi();
    }
    let mut k = sync::BKL.lock();
    let core = percpu::index() as usize;
    let me = k.sys.cores[core].current;
    if me != 0 {
        crate::syscall::save(me, frame, false);
    }
    match vector {
        TIMER_VECTOR => {
            crate::time::fired();
            crate::sched::timer(&mut k.sys, core)
        }
        crate::device::FIRST_VECTOR..=crate::device::LAST_VECTOR => {
            crate::device::interrupt(&mut k.sys, vector)
        }
        0..=31 if me != 0 => {
            let addr = if vector == 14 { cpu::cr2() } else { 0 };
            let mut m = crate::caps::Phys::new(&mut k.sys);
            crate::ipc::fault(&mut m, me, u64::from(vector), frame.error, addr);
        }
        _ => {}
    }
    crate::syscall::leave(k, core, frame);
}

fn nmi(frame: &TrapFrame) {
    if crate::panicking() {
        // Another core panicked and asked everyone to stop.
        super::halt_forever();
    }
    fault(frame);
}

fn fault(frame: &TrapFrame) -> ! {
    let vector = frame.vector as usize;
    let (mnemonic, name) = NAMES[vector];
    let cpu = percpu::index();
    let cr2 = cpu::cr2();
    if !crate::begin_panic() {
        // Another core is already reporting a crash.
        super::halt_forever();
    }
    let mut out = crate::serial::FaultWriter::begin();
    out.line(format_args!(
        "alaya: fault: {name} ({mnemonic}, vector {vector}) on cpu {cpu}"
    ));

    if vector == 14 {
        let e = frame.error;
        out.line(format_args!(
            "  {} {:#018x}: {}, {} mode{}{}",
            if e & 0x10 != 0 {
                "executing at"
            } else if e & 0x2 != 0 {
                "writing to"
            } else {
                "reading from"
            },
            cr2,
            if e & 0x1 != 0 {
                "protection violation"
            } else {
                "page not present"
            },
            if e & 0x4 != 0 { "user" } else { "kernel" },
            if e & 0x8 != 0 {
                ", reserved bit set"
            } else {
                ""
            },
            if e & 0x20 != 0 {
                ", protection key"
            } else {
                ""
            },
        ));
    } else if frame.error != 0 {
        out.line(format_args!("  error code {:#x}", frame.error));
    }

    // A kernel stack overflow shows up as a page fault in a guard page, or
    // as a double fault when the CPU cannot even push the page-fault frame.
    let probe = if vector == 8 || vector == 14 {
        cr2
    } else {
        frame.rsp
    };
    if let Some((owner, kind)) = crate::mm::layout::guard_page_owner(probe) {
        out.line(format_args!(
            "  {probe:#x} is the guard page below cpu {owner}'s {kind} stack: stack overflow"
        ));
    }

    out.line(format_args!(
        "  rip {:#018x}  cs {:#06x}  rflags {:#010x}",
        frame.rip, frame.cs, frame.rflags
    ));
    out.line(format_args!(
        "  rsp {:#018x}  ss {:#06x}  cr2 {:#018x}",
        frame.rsp, frame.ss, cr2
    ));
    let regs = [
        ("rax", frame.rax),
        ("rbx", frame.rbx),
        ("rcx", frame.rcx),
        ("rdx", frame.rdx),
        ("rsi", frame.rsi),
        ("rdi", frame.rdi),
        ("rbp", frame.rbp),
        ("r8 ", frame.r8),
        ("r9 ", frame.r9),
        ("r10", frame.r10),
        ("r11", frame.r11),
        ("r12", frame.r12),
        ("r13", frame.r13),
        ("r14", frame.r14),
        ("r15", frame.r15),
    ];
    for row in regs.chunks(3) {
        match row {
            [(a, x), (b, y), (c, z)] => out.line(format_args!(
                "  {a} {x:#018x}  {b} {y:#018x}  {c} {z:#018x}"
            )),
            _ => unreachable!(),
        }
    }
    out.line(format_args!(
        "  cr0 {:#x}  cr3 {:#x}  cr4 {:#x}",
        cpu::cr0(),
        cpu::cr3(),
        cpu::cr4()
    ));
    if let Some(owner) = sync::BKL.owner() {
        out.line(format_args!("  big kernel lock held by cpu {owner}"));
    }
    drop(out);
    panic!("CPU exception {mnemonic} on cpu {cpu}");
}

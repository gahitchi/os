// SPDX-License-Identifier: GPL-3.0-only
//! What happens on every exception and interrupt on ARM.
//!
//! Faults in user mode become fault messages, numbered like a PC's
//! (page fault 14, invalid instruction 6, ...), so fault handlers and
//! tests work the same on every processor. Faults in the kernel are bugs:
//! each prints a full report on the serial port and stops the machine.

use core::sync::atomic::{AtomicBool, Ordering};

use super::entry::TrapFrame;
use super::{gic, timer};
use crate::{percpu, sync};

/// Exception classes (ESR_EL1.EC).
mod ec {
    pub const UNKNOWN: u64 = 0x00;
    pub const SVC64: u64 = 0x15;
    pub const INST_ABORT_LOWER: u64 = 0x20;
    pub const INST_ABORT_SAME: u64 = 0x21;
    pub const PC_ALIGN: u64 = 0x22;
    pub const DATA_ABORT_LOWER: u64 = 0x24;
    pub const DATA_ABORT_SAME: u64 = 0x25;
    pub const SP_ALIGN: u64 = 0x26;
    pub const BRK64: u64 = 0x3c;
}

/// Device interrupt `intid` as the shared code numbers it (`device.rs`).
fn device_vector(intid: u32) -> Option<u8> {
    let n = intid.checked_sub(gic::FIRST_LPI)?;
    let v = u8::try_from(n).ok()?;
    (crate::device::FIRST_VECTOR..=crate::device::LAST_VECTOR)
        .contains(&v)
        .then_some(v)
}

/// The PC-style fault number and error bits for a user-mode exception.
fn pc_style(esr: u64) -> (u64, u64) {
    let class = esr >> 26;
    let iss = esr & 0x1ff_ffff;
    // Fault status: 0b0011xx is a permission fault.
    let permission = (iss & 0x3c) == 0x0c;
    match class {
        ec::DATA_ABORT_LOWER => {
            let write = iss & 1 << 6 != 0;
            (14, u64::from(permission) | u64::from(write) << 1 | 1 << 2)
        }
        ec::INST_ABORT_LOWER => (14, u64::from(permission) | 1 << 2 | 1 << 4),
        ec::UNKNOWN => (6, 0),
        ec::PC_ALIGN | ec::SP_ALIGN => (17, 0),
        ec::BRK64 => (3, 0),
        _ => (13, esr),
    }
}

/// An exception or interrupt while a thread ran in user mode.
pub extern "C" fn from_user(frame: &mut TrapFrame) {
    crate::vspace::poll_flush();
    if frame.kind == 0 && frame.esr >> 26 == ec::SVC64 {
        crate::syscall::syscall_entry(frame);
        return;
    }
    let mut intid = gic::SPURIOUS;
    if frame.kind == 1 {
        intid = gic::ack();
        if intid == timer::TIMER_INTID {
            crate::time::disarm();
        }
        if intid != gic::SPURIOUS {
            gic::eoi(intid);
        }
        if intid == gic::STOP_SGI && crate::panicking() {
            super::halt_forever();
        }
    }
    let mut k = sync::BKL.lock();
    let core = percpu::index() as usize;
    let me = k.sys.cores[core].current;
    if me != 0 {
        crate::syscall::save(me, frame, false);
    }
    if frame.kind == 1 {
        if intid == timer::TIMER_INTID {
            crate::sched::timer(&mut k.sys, core);
        } else if let Some(v) = device_vector(intid) {
            crate::device::interrupt(&mut k.sys, v);
        }
    } else if me != 0 {
        let (vector, error) = if frame.kind == 0 {
            pc_style(frame.esr)
        } else {
            (18, frame.esr) // a system error: like a machine check
        };
        let mut m = crate::caps::Phys::new(&mut k.sys);
        crate::ipc::fault(&mut m, me, vector, error, frame.far);
    }
    crate::syscall::leave(k, core, frame);
}

/// An interrupt while the kernel waited for one.
pub extern "C" fn kernel_irq(_frame: &mut TrapFrame) {
    crate::vspace::poll_flush();
    let intid = gic::ack();
    if intid == gic::SPURIOUS {
        return;
    }
    if intid == timer::TIMER_INTID {
        crate::time::disarm();
    }
    gic::eoi(intid);
    if intid == timer::TIMER_INTID {
        crate::time::interrupt();
    } else if intid == gic::STOP_SGI {
        if crate::panicking() {
            super::halt_forever();
        }
    } else if let Some(v) = device_vector(intid) {
        let mut k = sync::BKL.lock();
        crate::device::interrupt(&mut k.sys, v);
    }
}

static IN_FAULT: [AtomicBool; percpu::MAX_CPUS] =
    [const { AtomicBool::new(false) }; percpu::MAX_CPUS];

fn name(class: u64) -> &'static str {
    match class {
        ec::UNKNOWN => "undefined instruction",
        ec::INST_ABORT_SAME | ec::INST_ABORT_LOWER => "instruction abort",
        ec::DATA_ABORT_SAME | ec::DATA_ABORT_LOWER => "data abort",
        ec::PC_ALIGN => "misaligned pc",
        ec::SP_ALIGN => "misaligned stack pointer",
        ec::BRK64 => "breakpoint",
        0x07 => "floating point used in the kernel",
        0x2f => "system error",
        _ => "exception",
    }
}

/// An exception in the kernel: report it and stop.
pub extern "C" fn kernel_fault(frame: &mut TrapFrame) -> ! {
    let cpu = percpu::index();
    if IN_FAULT[cpu as usize].swap(true, Ordering::Relaxed) || !crate::begin_panic() {
        // A fault while reporting one, or another core is reporting.
        super::halt_forever();
    }
    let class = frame.esr >> 26;
    let mut out = crate::serial::FaultWriter::begin();
    out.line(format_args!(
        "alaya: fault: {} (class {class:#x}, ESR {:#x}) on cpu {cpu}",
        name(class),
        frame.esr
    ));
    if matches!(class, ec::DATA_ABORT_SAME | ec::INST_ABORT_SAME) {
        let iss = frame.esr & 0x1ff_ffff;
        // Fault status code: what went wrong with the access.
        let status = match iss & 0x3f {
            0x04..=0x07 => "page not present",
            0x09..=0x0b => "access flag clear",
            0x0d..=0x0f => "protection violation",
            0x10 | 0x14..=0x17 => "no device or memory answered (external abort)",
            0x21 => "misaligned access",
            0x30 => "TLB conflict",
            _ => "other fault",
        };
        out.line(format_args!(
            "  {} {:#018x}: {status}, kernel mode",
            if class == ec::INST_ABORT_SAME {
                "executing at"
            } else if iss & 1 << 6 != 0 {
                "writing to"
            } else {
                "reading from"
            },
            frame.far,
        ));
    }
    for probe in [frame.far, frame.sp] {
        if let Some((owner, kind)) = crate::mm::layout::guard_page_owner(probe) {
            out.line(format_args!(
                "  {probe:#x} is the guard page below cpu {owner}'s {kind} stack: stack overflow"
            ));
            break;
        }
    }
    out.line(format_args!(
        "  pc {:#018x}  sp {:#018x}  pstate {:#010x}",
        frame.pc, frame.sp, frame.pstate
    ));
    for row in 0..10 {
        let i = row * 3;
        out.line(format_args!(
            "  x{:<2} {:#018x}  x{:<2} {:#018x}  x{:<2} {:#018x}",
            i,
            frame.x[i],
            i + 1,
            frame.x[i + 1],
            i + 2,
            frame.x[i + 2]
        ));
    }
    out.line(format_args!("  x30 {:#018x}", frame.x[30]));
    if let Some(owner) = sync::BKL.owner() {
        out.line(format_args!("  big kernel lock held by cpu {owner}"));
    }
    drop(out);
    panic!("CPU exception ({}) on cpu {cpu}", name(class));
}

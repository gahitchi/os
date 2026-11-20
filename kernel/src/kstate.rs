// SPDX-License-Identifier: GPL-3.0-only
//! One list of all kernel-wide state (decision 77).
//!
//! Everything the kernel knows lives either in objects made from untyped
//! memory (each carrying a format label, `obj::Header`) or in the items
//! listed here. To swap the kernel without a restart (decision 70), a new
//! kernel takes over the old one's objects and the items marked *carried*;
//! items marked *rebuilt* (descriptor tables, locks, boot handshakes) are
//! set up afresh. A new kernel-wide variable must be added to this list, and
//! [`alaya_abi::format::KERNEL_STATE`] bumped if a carried item changes.

use core::mem::size_of_val;
use core::ptr::addr_of;

use crate::arch::{apic, idt, protect};
use crate::mm::{self, frames};
use crate::{fpu, percpu, serial, smp, sync, time};

/// Whether a new kernel takes an item over or builds it again.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Handover {
    Carried,
    Rebuilt,
}

pub struct Item {
    pub name: &'static str,
    pub handover: Handover,
    pub address: u64,
    pub bytes: u64,
}

macro_rules! item {
    ($name:literal, $how:ident, $var:expr) => {
        Item {
            name: $name,
            handover: Handover::$how,
            address: addr_of!($var) as u64,
            // SAFETY: only the size is taken; nothing is read.
            #[allow(unused_unsafe)]
            bytes: size_of_val(unsafe { &*addr_of!($var) }) as u64,
        }
    };
}

/// Every kernel-wide variable.
pub fn items() -> [Item; 24] {
    [
        // Objects, cores' run queues, address-space ids, counters.
        item!("big kernel lock and its state", Carried, sync::BKL),
        item!("per-core data", Carried, percpu::CPUS),
        item!("core count", Carried, percpu::COUNT),
        item!("kernel page tables", Carried, mm::KERNEL_ROOT),
        item!("boot allocator cursor", Carried, frames::NEXT),
        item!("boot allocator total", Carried, frames::TAKEN),
        item!("direct map offset", Carried, frames::HHDM),
        item!("boot allocator frozen", Carried, frames::FROZEN),
        item!("TSC frequency", Carried, time::TSC_HZ),
        item!("timer mode", Carried, time::DEADLINE_MODE),
        item!("APIC timer ratio", Carried, time::APIC_PER_TSC_FIXED),
        item!("APIC mode", Carried, apic::X2APIC),
        item!("APIC registers", Carried, apic::MMIO),
        item!("FPU save method", Carried, fpu::XSAVE),
        item!("FPU state components", Carried, fpu::MASK),
        item!("FPU state size", Carried, fpu::SIZE),
        item!("processor protections", Carried, protect::ON),
        // Set up again by the new kernel.
        item!("interrupt table", Rebuilt, idt::IDT),
        item!("segment tables", Rebuilt, percpu::GDTS),
        item!("task state segments", Rebuilt, percpu::TSSES),
        item!("serial port lock", Rebuilt, serial::SERIAL),
        item!("core start signal", Rebuilt, smp::GO),
        item!("cores done with self-tests", Rebuilt, smp::DONE),
        item!("panic owner", Rebuilt, crate::PANIC_CPU),
    ]
}

/// Print a summary at boot.
pub fn report() {
    let list = items();
    // No two items may overlap: each is listed once.
    for (i, a) in list.iter().enumerate() {
        for b in &list[i + 1..] {
            assert!(
                a.address + a.bytes <= b.address || b.address + b.bytes <= a.address,
                "kernel state items '{}' and '{}' overlap",
                a.name,
                b.name
            );
        }
    }
    let (mut carried, mut bytes, mut rebuilt) = (0, 0, 0);
    for i in &list {
        match i.handover {
            Handover::Carried => {
                carried += 1;
                bytes += i.bytes;
            }
            Handover::Rebuilt => rebuilt += 1,
        }
    }
    crate::kprintln!(
        "state: format {}, {} kernel-wide items: {} carried over a kernel swap ({} KiB), {} rebuilt",
        alaya_abi::format::KERNEL_STATE,
        list.len(),
        carried,
        bytes / 1024,
        rebuilt
    );
}

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

use crate::mm::{self, frames};
use crate::{arch, percpu, serial, smp, sync, time};

/// Whether a new kernel takes an item over or builds it again.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Handover {
    Carried,
    Rebuilt,
}

#[derive(Clone, Copy)]
pub struct Item {
    pub name: &'static str,
    pub handover: Handover,
    pub address: u64,
    pub bytes: u64,
}

#[macro_export]
macro_rules! item {
    ($name:literal, $how:ident, $var:expr) => {{
        let at = core::ptr::addr_of!($var);
        $crate::kstate::Item {
            name: $name,
            handover: $crate::kstate::Handover::$how,
            address: at as u64,
            // SAFETY: only the size is taken; nothing is read.
            bytes: core::mem::size_of_val(unsafe { &*at }) as u64,
        }
    }};
}

/// Every kernel-wide variable outside the processor wall; the wall lists
/// its own (`arch::state_items`).
fn generic_items() -> [Item; 13] {
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
        item!("clock frequency", Carried, time::HZ),
        // Set up again by the new kernel.
        item!("serial port lock", Rebuilt, serial::SERIAL),
        item!("core start signal", Rebuilt, smp::GO),
        item!("cores done with self-tests", Rebuilt, smp::DONE),
        item!("panic owner", Rebuilt, crate::PANIC_CPU),
    ]
}

/// Every kernel-wide variable.
pub fn items() -> impl Iterator<Item = Item> + Clone {
    generic_items().into_iter().chain(arch::state_items())
}

/// Print a summary at boot.
pub fn report() {
    // No two items may overlap: each is listed once.
    for (i, a) in items().enumerate() {
        for b in items().skip(i + 1) {
            assert!(
                a.address + a.bytes <= b.address || b.address + b.bytes <= a.address,
                "kernel state items '{}' and '{}' overlap",
                a.name,
                b.name
            );
        }
    }
    let (mut carried, mut bytes, mut rebuilt) = (0, 0, 0);
    for i in items() {
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
        items().count(),
        carried,
        bytes / 1024,
        rebuilt
    );
}

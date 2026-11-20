// SPDX-License-Identifier: GPL-3.0-only
//! What drivers need from the kernel: device registers, interrupts as
//! messages, and the device memory guard.
//!
//! - **Device registers.** The device control capability (held by the
//!   device manager only) makes page capabilities for physical addresses
//!   that are not RAM. They map uncached. The kernel refuses RAM and its
//!   own devices (the interrupt controller, the IOMMU).
//! - **Interrupts.** The interrupt control capability takes a free vector
//!   and makes an interrupt capability for it. A driver points it at a
//!   notification; when the device raises the interrupt, the notification
//!   gets the driver's bits, so a driver waits for its device like for any
//!   message. Devices raise them with MSI or MSI-X (`IRQ_MSI` says what to
//!   program); old pin-based interrupts are not used.
//! - **I/O spaces** (`iommu.rs`): what a device may reach.

use alaya_abi::{Error, Kind, op, rights};
use alaya_cap::{self as cap, Cap, Memory, SlotRef};

use crate::arch;
use crate::caps::{Outcome, Phys};
use crate::sched::Sys;
use crate::{iommu, ipc, mm, percpu};

/// Vectors handed to devices.
pub const FIRST_VECTOR: u8 = 0x50;
pub const LAST_VECTOR: u8 = 0xef;

#[derive(Clone, Copy)]
pub struct IrqEntry {
    pub taken: bool,
    /// The core the interrupt is delivered to.
    pub core: u16,
    /// Notification to signal (0: none), and its bits.
    pub ntfn: u64,
    pub bits: u64,
    /// Interrupts delivered.
    pub count: u64,
}

impl IrqEntry {
    pub const EMPTY: IrqEntry = IrqEntry {
        taken: false,
        core: 0,
        ntfn: 0,
        bits: 0,
        count: 0,
    };
}

fn done(r: [u64; 4]) -> Result<Outcome, Error> {
    Ok(Outcome::Done(r))
}

/// A device raised `vector`: signal its notification.
pub fn interrupt(k: &mut Sys, vector: u8) {
    let e = &mut k.irqs[vector as usize];
    e.count += 1;
    let (n, bits) = (e.ntfn, e.bits);
    if n != 0 {
        ipc::signal(k, n, bits);
    }
}

/// A notification goes away: stop delivering interrupts to it.
pub fn forget_notification(k: &mut Sys, n: u64) {
    for e in k.irqs.iter_mut().filter(|e| e.ntfn == n) {
        e.ntfn = 0;
        e.bits = 0;
    }
}

/// The last capability to interrupt `vector` goes: free the vector.
pub fn free_vector(k: &mut Sys, vector: u64) {
    if let Some(e) = k.irqs.get_mut(vector as usize) {
        if e.taken {
            arch::msi_release(vector as u8);
        }
        *e = IrqEntry::EMPTY;
    }
}

fn arg_cap(m: &Phys, me: u64, h: u64, kind: Kind, need: u8) -> Result<Cap, Error> {
    let at = ipc::lookup(m, me, h)?;
    let c = m.read(at).cap;
    if !c.is(kind) {
        return Err(Error::WrongKind);
    }
    if c.rights & need != need {
        return Err(Error::NoRights);
    }
    Ok(c)
}

/// The empty slot `index` of the table behind handle `table`.
fn dest(m: &Phys, me: u64, table: u64, index: u64) -> Result<SlotRef, Error> {
    let t = ipc::lookup(m, me, table)?;
    let at = cap::slot_in(m, t, index, rights::WRITE)?;
    if !m.read(at).cap.is_empty() {
        return Err(Error::SlotInUse);
    }
    Ok(at)
}

/// Operations on the device control capability.
pub fn device_control(m: &mut Phys, me: u64, label: u64, a: [u64; 6]) -> Result<Outcome, Error> {
    match label {
        op::DEVICE_PAGE => {
            let (phys, bits) = (a[0], a[1]);
            if !(bits == 12 || bits == 21) || !phys.is_multiple_of(1 << bits) {
                return Err(Error::InvalidArgument);
            }
            let len = 1u64 << bits;
            if phys.checked_add(len).is_none_or(|e| e > 1 << 46)
                || mm::is_ram(phys, len)
                || overlaps_kernel_device(m.sys, phys, len)
            {
                return Err(Error::InvalidArgument);
            }
            let at = dest(m, me, a[2], a[3])?;
            cap::insert_root(m, at, Cap::new(Kind::Page, phys, bits as u8));
            done([0; 4])
        }
        op::DEVICE_BIND => {
            let s = arg_cap(m, me, a[0], Kind::IoSpace, rights::WRITE)?;
            let bdf = u16::try_from(a[1]).map_err(|_| Error::InvalidArgument)?;
            iommu::bind(m.sys, s.object, bdf)?;
            done([0; 4])
        }
        op::DEVICE_FAULTS => done([iommu::poll_faults(m.sys), 0, 0, 0]),
        _ => Err(Error::NoSuchCall),
    }
}

fn overlaps_kernel_device(k: &Sys, phys: u64, len: u64) -> bool {
    let iommu = k.iommu.present().then_some((k.iommu.physical, 4096));
    arch::kernel_only_device_pages()
        .into_iter()
        .chain(iommu)
        .any(|(p, l)| p < phys + len && phys < p + l)
}

/// Operations on the interrupt control capability.
pub fn irq_control(m: &mut Phys, me: u64, label: u64, a: [u64; 6]) -> Result<Outcome, Error> {
    if label != op::IRQ_GET {
        return Err(Error::NoSuchCall);
    }
    let core = a[0];
    if core >= u64::from(percpu::count()) {
        return Err(Error::InvalidArgument);
    }
    let at = dest(m, me, a[1], a[2])?;
    let v = (FIRST_VECTOR..=LAST_VECTOR)
        .find(|&v| !m.sys.irqs[v as usize].taken)
        .ok_or(Error::NoMemory)?;
    m.sys.irqs[v as usize] = IrqEntry {
        taken: true,
        core: core as u16,
        ..IrqEntry::EMPTY
    };
    cap::insert_root(m, at, Cap::new(Kind::Irq, u64::from(v), 0));
    done([u64::from(v), 0, 0, 0])
}

/// Operations on an interrupt capability.
pub fn irq(m: &mut Phys, me: u64, c: Cap, label: u64, a: [u64; 6]) -> Result<Outcome, Error> {
    let v = c.object as usize;
    match label {
        op::IRQ_SET => {
            if c.rights & rights::WRITE == 0 {
                return Err(Error::NoRights);
            }
            let (n, bits) = if a[0] == 0 {
                (0, 0)
            } else {
                let n = arg_cap(m, me, a[0], Kind::Notification, rights::WRITE)?;
                (n.object, a[1] | n.a)
            };
            let e = &mut m.sys.irqs[v];
            e.ntfn = n;
            e.bits = bits;
            done([0; 4])
        }
        op::IRQ_MSI => {
            let e = m.sys.irqs[v];
            let rid = u16::try_from(a[0]).map_err(|_| Error::InvalidArgument)?;
            let dest = percpu::get(u32::from(e.core))
                .hw_id
                .load(core::sync::atomic::Ordering::Relaxed);
            let (address, data) =
                arch::msi_message(e.core, dest, v as u8, rid).ok_or(Error::NoMemory)?;
            done([address, data, e.count, 0])
        }
        _ => Err(Error::NoSuchCall),
    }
}

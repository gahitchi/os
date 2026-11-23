// SPDX-License-Identifier: GPL-3.0-only
//! The parts every virtio driver of milestone 6 shares: finding the
//! register blocks in the PCI configuration page, starting the device from
//! nothing, and one small queue in one page.
//!
//! The disk driver (`blk.rs`) has its own copy of this from milestone 4,
//! because its handover format depends on it; the screen and input drivers
//! use this one.

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{Ordering, fence};

use crate::driver::{CONFIG_PAGE, bar_page};

// Device status bits.
pub const ACKNOWLEDGE: u8 = 1;
pub const DRIVER: u8 = 2;
pub const DRIVER_OK: u8 = 4;
pub const FEATURES_OK: u8 = 8;

// Feature bits.
pub const VERSION_1: u64 = 1 << 32;
pub const ACCESS_PLATFORM: u64 = 1 << 33;

// Common configuration registers.
const C_DEVICE_FEATURE_SELECT: u64 = 0x00;
const C_DEVICE_FEATURE: u64 = 0x04;
const C_DRIVER_FEATURE_SELECT: u64 = 0x08;
const C_DRIVER_FEATURE: u64 = 0x0c;
const C_MSIX_CONFIG: u64 = 0x10;
const C_STATUS: u64 = 0x14;
const C_QUEUE_SELECT: u64 = 0x16;
const C_QUEUE_SIZE: u64 = 0x18;
const C_QUEUE_MSIX: u64 = 0x1a;
const C_QUEUE_ENABLE: u64 = 0x1c;
const C_QUEUE_NOTIFY_OFF: u64 = 0x1e;
const C_QUEUE_DESC: u64 = 0x20;
const C_QUEUE_DRIVER: u64 = 0x28;
const C_QUEUE_DEVICE: u64 = 0x30;

pub const NO_VECTOR: u16 = 0xffff;

/// Descriptor flags.
pub const D_NEXT: u16 = 1;
pub const D_WRITE: u16 = 2;

/// Every queue here has this many entries, all in one 4 KiB page.
pub const QSIZE: u16 = 64;
const AVAIL: u64 = 16 * QSIZE as u64;
const USED: u64 = (AVAIL + 6 + 2 * QSIZE as u64).next_multiple_of(4);
const _: () = assert!(USED + 6 + 8 * QSIZE as u64 <= 4096);

pub fn r8(a: u64) -> u8 {
    // SAFETY: callers pass addresses inside pages this driver mapped.
    unsafe { read_volatile(a as *const u8) }
}
pub fn r16(a: u64) -> u16 {
    // SAFETY: as above.
    unsafe { read_volatile(a as *const u16) }
}
pub fn r32(a: u64) -> u32 {
    // SAFETY: as above.
    unsafe { read_volatile(a as *const u32) }
}
pub fn w8(a: u64, v: u8) {
    // SAFETY: as above.
    unsafe { write_volatile(a as *mut u8, v) }
}
pub fn w16(a: u64, v: u16) {
    // SAFETY: as above.
    unsafe { write_volatile(a as *mut u16, v) }
}
pub fn w32(a: u64, v: u32) {
    // SAFETY: as above.
    unsafe { write_volatile(a as *mut u32, v) }
}
pub fn w64(a: u64, v: u64) {
    w32(a, v as u32);
    w32(a + 4, (v >> 32) as u32);
}

/// Why starting a device failed.
pub mod why {
    pub const NO_REGS: u64 = 1;
    pub const NO_RESET: u64 = 2;
    pub const NO_VERSION_1: u64 = 3;
    pub const FEATURES: u64 = 4;
    pub const QUEUE_SIZE: u64 = 5;
    pub const MSIX: u64 = 6;
    pub const DEVICE: u64 = 7;
}

/// Where the device's register blocks are, from its PCI capabilities.
#[derive(Clone, Copy)]
pub struct Regs {
    pub common: u64,
    pub device: u64,
    pub notify: u64,
    pub notify_mult: u32,
    pub msix_cap: u64,
    pub msix_table: u64,
}

impl Regs {
    pub fn find() -> Option<Regs> {
        let cfg = CONFIG_PAGE;
        let mut r = Regs {
            common: 0,
            device: 0,
            notify: 0,
            notify_mult: 0,
            msix_cap: 0,
            msix_table: 0,
        };
        if r16(cfg + 6) & 0x10 == 0 {
            return None;
        }
        let mut at = u64::from(r8(cfg + 0x34) & !3);
        let mut guard = 0;
        while at != 0 && guard < 48 {
            guard += 1;
            let id = r8(cfg + at);
            if id == 0x09 {
                let kind = r8(cfg + at + 3);
                let bar = u64::from(r8(cfg + at + 4));
                let off = u64::from(r32(cfg + at + 8));
                let va = bar_page(bar, 0) + off;
                match kind {
                    1 => r.common = va,
                    2 => {
                        r.notify = va;
                        r.notify_mult = r32(cfg + at + 16);
                    }
                    4 => r.device = va,
                    _ => {}
                }
            } else if id == 0x11 {
                r.msix_cap = at;
                let t = r32(cfg + at + 4);
                r.msix_table = bar_page(u64::from(t & 7), 0) + u64::from(t & !7);
            }
            at = u64::from(r8(cfg + at + 1) & !3);
        }
        (r.common != 0 && r.notify != 0 && r.msix_cap != 0).then_some(r)
    }

    pub fn status(&self) -> u8 {
        r8(self.common + C_STATUS)
    }

    pub fn set_status(&self, s: u8) {
        w8(self.common + C_STATUS, s);
    }

    /// Switch on memory access and bus mastering, legacy interrupts off.
    pub fn enable_pci() {
        let cmd = CONFIG_PAGE + 4;
        w16(cmd, r16(cmd) | 0x2 | 0x4 | 0x400);
    }

    /// Reset the device and agree on features (`want` on top of version 1
    /// and the IOMMU). Leaves the device ready for queues.
    pub fn reset(&self, want: u64) -> Result<u64, u64> {
        let c = self.common;
        self.set_status(0);
        let mut spins = 0u64;
        while self.status() != 0 {
            spins += 1;
            if spins > 10_000_000 {
                return Err(why::NO_RESET);
            }
            core::hint::spin_loop();
        }
        self.set_status(ACKNOWLEDGE);
        self.set_status(ACKNOWLEDGE | DRIVER);
        w32(c + C_DEVICE_FEATURE_SELECT, 0);
        let lo = u64::from(r32(c + C_DEVICE_FEATURE));
        w32(c + C_DEVICE_FEATURE_SELECT, 1);
        let offered = lo | u64::from(r32(c + C_DEVICE_FEATURE)) << 32;
        if offered & VERSION_1 == 0 {
            return Err(why::NO_VERSION_1);
        }
        let got = offered & (VERSION_1 | ACCESS_PLATFORM | want);
        w32(c + C_DRIVER_FEATURE_SELECT, 0);
        w32(c + C_DRIVER_FEATURE, got as u32);
        w32(c + C_DRIVER_FEATURE_SELECT, 1);
        w32(c + C_DRIVER_FEATURE, (got >> 32) as u32);
        self.set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK);
        if self.status() & FEATURES_OK == 0 {
            return Err(why::FEATURES);
        }
        Ok(got)
    }

    /// Point MSI-X entry 0 at the vector the kernel gave this device
    /// (`msi`: address, data). Configuration changes raise no interrupt.
    pub fn msix(&self, msi: (u64, u64)) {
        let t = self.msix_table;
        w32(t, msi.0 as u32);
        w32(t + 4, (msi.0 >> 32) as u32);
        w32(t + 8, msi.1 as u32);
        w32(t + 12, 0);
        let mc = CONFIG_PAGE + self.msix_cap + 2;
        w16(mc, (r16(mc) | 0x8000) & !0x4000);
        w16(self.common + C_MSIX_CONFIG, NO_VECTOR);
    }

    /// Set up queue `index` in the ring page at `va` (device address
    /// `iova`), interrupting through MSI-X entry 0.
    pub fn queue(&self, index: u16, va: u64, iova: u64) -> Result<Queue, u64> {
        let c = self.common;
        w16(c + C_QUEUE_SELECT, index);
        let max = r16(c + C_QUEUE_SIZE);
        if max < QSIZE {
            return Err(why::QUEUE_SIZE);
        }
        w16(c + C_QUEUE_SIZE, QSIZE);
        w16(c + C_QUEUE_MSIX, 0);
        if r16(c + C_QUEUE_MSIX) == NO_VECTOR {
            return Err(why::MSIX);
        }
        // SAFETY: the ring page is mapped writable at `va`.
        unsafe { core::ptr::write_bytes(va as *mut u8, 0, 4096) };
        w64(c + C_QUEUE_DESC, iova);
        w64(c + C_QUEUE_DRIVER, iova + AVAIL);
        w64(c + C_QUEUE_DEVICE, iova + USED);
        w16(c + C_QUEUE_ENABLE, 1);
        let notify =
            self.notify + u64::from(r16(c + C_QUEUE_NOTIFY_OFF)) * u64::from(self.notify_mult);
        Ok(Queue {
            va,
            notify,
            index,
            avail_idx: 0,
            used_seen: 0,
        })
    }

    pub fn ready(&self) {
        self.set_status(ACKNOWLEDGE | DRIVER | FEATURES_OK | DRIVER_OK);
    }
}

/// One queue of [`QSIZE`] entries in one page.
pub struct Queue {
    va: u64,
    notify: u64,
    index: u16,
    avail_idx: u16,
    used_seen: u16,
}

impl Queue {
    /// Fill descriptor `n`.
    pub fn desc(&self, n: u16, addr: u64, len: u32, flags: u16, next: u16) {
        let a = self.va + u64::from(n) * 16;
        w64(a, addr);
        w32(a + 8, len);
        w16(a + 12, flags);
        w16(a + 14, next);
    }

    /// Offer the chain starting at descriptor `head` (not yet visible to
    /// the device until [`Queue::kick`]).
    pub fn offer(&mut self, head: u16) {
        let slot = u64::from(self.avail_idx % QSIZE);
        w16(self.va + AVAIL + 4 + 2 * slot, head);
        self.avail_idx = self.avail_idx.wrapping_add(1);
    }

    pub fn kick(&self) {
        fence(Ordering::SeqCst);
        w16(self.va + AVAIL + 2, self.avail_idx);
        fence(Ordering::SeqCst);
        w16(self.notify, self.index);
    }

    /// The next chain the device finished: (head descriptor, bytes written).
    pub fn take_used(&mut self) -> Option<(u16, u32)> {
        fence(Ordering::SeqCst);
        let idx = r16(self.va + USED + 2);
        if idx == self.used_seen {
            return None;
        }
        let e = self.va + USED + 4 + 8 * u64::from(self.used_seen % QSIZE);
        self.used_seen = self.used_seen.wrapping_add(1);
        Some((r32(e) as u16, r32(e + 4)))
    }
}

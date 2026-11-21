// SPDX-License-Identifier: GPL-3.0-only
//! The ARM debug console: a PL011 serial port, found through the firmware's
//! SPCR table.
//!
//! The port's registers are device memory the bootloader does not map, so
//! until the kernel's own page tables exist and the firmware tables are
//! read, output collects in a buffer and is printed the moment the port is
//! ready.

use core::sync::atomic::{AtomicU64, Ordering};

/// Virtual address of the port's registers (0: not ready yet).
pub(crate) static BASE: AtomicU64 = AtomicU64::new(0);

const DR: u64 = 0x00;
const FR: u64 = 0x18;
const FR_TXFF: u32 = 1 << 5;
const EARLY: usize = 16 * 1024;

pub struct Uart {
    early: [u8; EARLY],
    early_len: usize,
}

impl Uart {
    pub const fn new() -> Self {
        Uart {
            early: [0; EARLY],
            early_len: 0,
        }
    }

    /// The firmware already set the port up (speed and format).
    pub fn init(&self) {}

    pub fn write_byte(&mut self, byte: u8) {
        let base = BASE.load(Ordering::Relaxed);
        if base == 0 {
            if self.early_len < EARLY {
                self.early[self.early_len] = byte;
                self.early_len += 1;
            }
            return;
        }
        put(base, byte);
    }

    /// The port is mapped at `virt`: print what waited.
    pub fn ready(&mut self, virt: u64) {
        BASE.store(virt, Ordering::Relaxed);
        for i in 0..self.early_len {
            put(virt, self.early[i]);
        }
        self.early_len = 0;
    }
}

fn put(base: u64, byte: u8) {
    // SAFETY: the PL011 registers, mapped as device memory.
    unsafe {
        while core::ptr::read_volatile((base + FR) as *const u32) & FR_TXFF != 0 {
            super::spin_hint();
        }
        core::ptr::write_volatile((base + DR) as *mut u32, u32::from(byte));
    }
}

// SPDX-License-Identifier: GPL-3.0-only
//! The PC's debug console: the first serial port (COM1, a 16550 UART).

use super::port::{inb, outb};

const COM1: u16 = 0x3f8;

pub struct Uart {
    base: u16,
}

impl Uart {
    pub const fn new() -> Self {
        Uart { base: COM1 }
    }

    pub fn init(&self) {
        // SAFETY: standard 16550 setup sequence on the COM1 ports.
        unsafe {
            outb(self.base + 1, 0x00); // no interrupts
            outb(self.base + 3, 0x80); // set the baud divisor
            outb(self.base, 0x01); //     115200 baud
            outb(self.base + 1, 0x00);
            outb(self.base + 3, 0x03); // 8 bits, no parity, one stop bit
            outb(self.base + 2, 0xc7); // FIFO on, cleared, 14-byte threshold
            outb(self.base + 4, 0x03); // DTR and RTS
        }
    }

    pub fn write_byte(&self, byte: u8) {
        // SAFETY: reading the line status register has no side effects.
        while unsafe { inb(self.base + 5) } & 0x20 == 0 {
            super::spin_hint();
        }
        // SAFETY: the transmit holding register is empty.
        unsafe { outb(self.base, byte) };
    }
}

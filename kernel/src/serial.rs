// SPDX-License-Identifier: GPL-3.0-only
//! Debug output on the first serial port (COM1, a 16550 UART).

use core::fmt::{self, Write};

use crate::arch::port::{inb, outb};
use crate::sync::{SpinLock, SpinLockGuard};

const COM1: u16 = 0x3f8;

pub(crate) struct Uart {
    base: u16,
}

impl Uart {
    fn init(&self) {
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

    fn write_byte(&self, byte: u8) {
        // SAFETY: reading the line status register has no side effects.
        while unsafe { inb(self.base + 5) } & 0x20 == 0 {
            crate::arch::spin_hint();
        }
        // SAFETY: the transmit holding register is empty.
        unsafe { outb(self.base, byte) };
    }
}

impl Write for Uart {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                self.write_byte(b'\r');
            }
            self.write_byte(byte);
        }
        Ok(())
    }
}

pub(crate) static SERIAL: SpinLock<Uart> = SpinLock::new(Uart { base: COM1 });

pub fn init() {
    SERIAL.lock().init();
}

pub fn print(args: fmt::Arguments) {
    // Interrupts stay off while the lock is held, so an interrupt handler
    // on this core that prints cannot deadlock against it.
    let was_on = crate::arch::cpu::interrupts_enabled();
    crate::arch::disable_interrupts();
    let _ = SERIAL.lock().write_fmt(args);
    if was_on {
        crate::arch::cpu::enable_interrupts();
    }
}

/// Exclusive use of the serial port for a fault report or panic message.
///
/// Waits a little for the lock; if another core still holds it (it may have
/// been stopped mid-line), the report is printed anyway.
pub struct FaultWriter {
    guard: Option<SpinLockGuard<'static, Uart>>,
}

impl FaultWriter {
    pub fn begin() -> Self {
        let mut writer = Self {
            guard: SERIAL.lock_bounded(50_000_000),
        };
        if writer.guard.is_none() {
            // A stopped core may have been mid-line; start on a fresh one.
            writer.line(format_args!(""));
        }
        writer
    }

    pub fn line(&mut self, args: fmt::Arguments) {
        let uart = match &mut self.guard {
            Some(guard) => &mut **guard,
            // SAFETY: only used on the way to stopping the machine.
            None => unsafe { SERIAL.force_get() },
        };
        let _ = uart.write_fmt(format_args!("{args}\n"));
    }
}

#[macro_export]
macro_rules! kprint {
    ($($arg:tt)*) => ($crate::serial::print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! kprintln {
    () => ($crate::kprint!("\n"));
    ($($arg:tt)*) => ($crate::serial::print(format_args!("{}\n", format_args!($($arg)*))));
}

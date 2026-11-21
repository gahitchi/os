// SPDX-License-Identifier: GPL-3.0-only
//! Debug output on the serial port (the device itself is behind the
//! processor wall: `arch::console`).

use core::fmt::{self, Write};

use crate::arch;
use crate::arch::console::Uart;
use crate::sync::{SpinLock, SpinLockGuard};

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

pub(crate) static SERIAL: SpinLock<Uart> = SpinLock::new(Uart::new());

pub fn init() {
    SERIAL.lock().init();
}

pub fn print(args: fmt::Arguments) {
    // Interrupts stay off while the lock is held, so an interrupt handler
    // on this core that prints cannot deadlock against it.
    let was_on = arch::interrupts_enabled();
    arch::disable_interrupts();
    let _ = SERIAL.lock().write_fmt(args);
    if was_on {
        arch::enable_interrupts();
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

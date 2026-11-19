// SPDX-License-Identifier: GPL-3.0-only
//! Legacy PC I/O ports.

/// Write a byte to an I/O port.
///
/// # Safety
/// Writing to an I/O port can have any effect on the hardware behind it.
#[inline]
pub unsafe fn outb(port: u16, value: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// Read a byte from an I/O port.
///
/// # Safety
/// Reading an I/O port can have side effects on the hardware behind it.
#[inline]
pub unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        core::arch::asm!("in al, dx", out("al") value, in("dx") port, options(nomem, nostack, preserves_flags));
    }
    value
}

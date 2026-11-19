// SPDX-License-Identifier: GPL-3.0-only
//! The legacy PC timer (PIT) and interrupt controllers (8259 PIC).
//!
//! The PICs are only switched off: all interrupts go through the APICs. The
//! PIT is only used once at boot, to measure the TSC frequency when the CPU
//! does not report it.

use super::port::{inb, outb};

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xa0;
const PIC2_DATA: u16 = 0xa1;

/// First vector of the (masked) legacy PIC range; a spurious IRQ 7 or 15
/// arrives at `PIC_BASE + 7` or `PIC_BASE + 15`.
pub const PIC_BASE: u8 = 0x20;

/// Move the PICs' vectors away from the CPU exceptions and mask every line.
pub fn disable_pic() {
    // SAFETY: standard 8259 initialisation sequence, then mask everything.
    unsafe {
        outb(PIC1_COMMAND, 0x11);
        outb(PIC2_COMMAND, 0x11);
        outb(PIC1_DATA, PIC_BASE);
        outb(PIC2_DATA, PIC_BASE + 8);
        outb(PIC1_DATA, 4);
        outb(PIC2_DATA, 2);
        outb(PIC1_DATA, 1);
        outb(PIC2_DATA, 1);
        outb(PIC1_DATA, 0xff);
        outb(PIC2_DATA, 0xff);
    }
}

const PIT_HZ: u64 = 1_193_182;
const PIT_CHANNEL2: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;
const PORT_B: u16 = 0x61;

/// Busy-wait `ms` milliseconds (at most 50) on PIT channel 2, calling
/// `start` right after the count begins. Returns false if the PIT never
/// finished (no PIT, as on some recent machines).
pub fn pit_wait(ms: u64, start: impl FnOnce()) -> bool {
    let count = (PIT_HZ * ms.min(50) / 1000) as u16;
    // SAFETY: channel 2 is the speaker channel; its output is gated by port
    // 0x61 bit 0 and the speaker itself (bit 1) stays off.
    unsafe {
        let b = inb(PORT_B);
        outb(PORT_B, (b & !0x02) | 0x01);
        // Channel 2, low then high byte, mode 0 (interrupt on terminal count).
        outb(PIT_COMMAND, 0b1011_0000);
        outb(PIT_CHANNEL2, count as u8);
        outb(PIT_CHANNEL2, (count >> 8) as u8);
        start();
        // Bit 5 of port 0x61 mirrors channel 2's output, high at zero.
        let mut spins: u64 = 0;
        while inb(PORT_B) & 0x20 == 0 {
            spins += 1;
            if spins > 200_000_000 {
                return false;
            }
        }
    }
    true
}

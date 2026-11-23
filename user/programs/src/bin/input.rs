// SPDX-License-Identifier: GPL-3.0-only
//! The keyboard and mouse driver (virtio input, modern PCI). One instance
//! per device; it finds out from the device whether it is a keyboard or a
//! pointer.
//!
//! Events go into the ring in the shared page, where the client reads them
//! at its own pace. Like the other drivers it owns nothing: a new instance
//! resets the device and carries on after the last event written.

#![no_std]
#![no_main]

use core::sync::atomic::Ordering;

use alaya_programs::driver::{ARG_BDF, HEADER_PAGE, IOVA_HEADER, IOVA_RING, JOB_PAGE, RING_PAGE};
use alaya_programs::seat::*;
use alaya_programs::virtio::{D_WRITE, QSIZE, Queue, Regs, r8, r16, r32, w8, why};
use alaya_programs::{load, run, store};
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::say;

alaya_sys::entry!(main);

// Device configuration: what to read, then the answer.
const CFG_SELECT: u64 = 0;
const CFG_SUBSEL: u64 = 1;
const CFG_SIZE: u64 = 2;
const CFG_DATA: u64 = 8;
const ID_NAME: u8 = 0x01;
const EV_BITS: u8 = 0x11;

fn fail(page: &InputPage, e: u64) -> ! {
    store(&page.error, e);
    store(&page.state, run::FAILED);
    say!("cannot start the device (reason {e})");
    alaya_sys::crash()
}

/// Ask the device's configuration a question; returns the answer's size.
fn ask(regs: &Regs, select: u8, subsel: u8) -> u8 {
    w8(regs.device + CFG_SELECT, select);
    w8(regs.device + CFG_SUBSEL, subsel);
    r8(regs.device + CFG_SIZE)
}

fn kind_of(regs: &Regs) -> u64 {
    let n = ask(regs, ID_NAME, 0).min(64);
    let mut name = [0u8; 64];
    for (i, b) in name.iter_mut().enumerate().take(n as usize) {
        *b = r8(regs.device + CFG_DATA + i as u64);
    }
    let name = &name[..n as usize];
    let has = |w: &[u8]| name.windows(w.len()).any(|x| x.eq_ignore_ascii_case(w));
    if has(b"keyboard") {
        return kind::KEYBOARD;
    }
    if has(b"tablet") || has(b"mouse") {
        return kind::POINTER;
    }
    // Otherwise: a device that sends absolute positions is a pointer.
    if ask(regs, EV_BITS, ev::ABS as u8) > 0 {
        return kind::POINTER;
    }
    if ask(regs, EV_BITS, ev::KEY as u8) > 0 {
        return kind::KEYBOARD;
    }
    kind::UNKNOWN
}

fn start_device(regs: &Regs, msi: (u64, u64)) -> Result<Queue, u64> {
    regs.reset(0)?;
    regs.msix(msi);
    let mut q = regs.queue(0, RING_PAGE, IOVA_RING)?;
    // One 8-byte event buffer per entry, all offered to the device.
    for i in 0..QSIZE {
        q.desc(i, IOVA_HEADER + u64::from(i) * 8, 8, D_WRITE, 0);
        q.offer(i);
    }
    regs.ready();
    q.kick();
    Ok(q)
}

fn main(info: &'static StartInfo) -> ! {
    let g = start::FIRST_GIVEN;
    // SAFETY: the client maps the shared page here before starting us.
    let page = unsafe { &*(JOB_PAGE as *const InputPage) };
    store(&page.state, run::STARTING);
    page.starts.fetch_add(1, Ordering::AcqRel);
    let regs = Regs::find().unwrap_or_else(|| fail(page, why::NO_REGS));
    if regs.device == 0 {
        fail(page, why::NO_REGS);
    }
    let ntfn = g + G_NTFN;
    let irq = g + G_IRQ;
    let _ = alaya_sys::irq_set(irq, ntfn, IRQ_BIT);
    let msi = alaya_sys::irq_msi(irq, info.args[ARG_BDF] as u16)
        .map(|(a, d, _)| (a, d))
        .unwrap_or((0, 0));
    Regs::enable_pci();
    let k = kind_of(&regs);
    store(&page.kind, k);
    let mut q = start_device(&regs, msi).unwrap_or_else(|e| fail(page, e));
    store(&page.state, run::RUNNING);
    say!(
        "{} ready",
        match k {
            kind::KEYBOARD => "keyboard",
            kind::POINTER => "pointer",
            _ => "input device of unknown kind",
        }
    );
    let client = g + G_CLIENT;
    loop {
        let mut got = false;
        while let Some((head, _)) = q.take_used() {
            let b = HEADER_PAGE + u64::from(head % QSIZE) * 8;
            let e = pack(r16(b), r16(b + 2), r32(b + 4));
            let n = load(&page.head);
            store(&page.events[n as usize % EVENTS], e);
            store(&page.head, n + 1);
            q.offer(head);
            got = true;
        }
        if got {
            q.kick();
            let _ = alaya_sys::signal(client, INPUT_BIT);
        }
        let bits = alaya_sys::wait(ntfn).unwrap_or(0);
        if bits & CRASH_BIT != 0 {
            say!("crashing on purpose (test)");
            alaya_sys::crash();
        }
    }
}

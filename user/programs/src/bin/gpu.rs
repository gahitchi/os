// SPDX-License-Identifier: GPL-3.0-only
//! The screen driver (virtio GPU, two-dimensional part, modern PCI).
//!
//! It owns no memory the device reaches: the screen memory belongs to its
//! client and is only lent to it, so a crash loses no pixels. A new
//! instance resets the device, describes the screen memory to it again and
//! shows what is there (decisions 70, 78, 79).
//!
//! The client draws into the screen memory, notes the changed part in the
//! shared page and posts a frame; the driver copies that part to the
//! device's screen and tells it to show it, then reports the frame shown.

#![no_std]
#![no_main]

use core::sync::atomic::Ordering;

use alaya_programs::driver::{
    ARG_BDF, ARG_FLAGS, HEADER_PAGE, IOVA_HEADER, IOVA_RING, JOB_PAGE, RING_PAGE,
};
use alaya_programs::seat::*;
use alaya_programs::virtio::{D_NEXT, D_WRITE, Queue, Regs, r32, w32, w64, why};
use alaya_programs::{load, run, store};
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::{rt, say};

alaya_sys::entry!(main);

// Commands and answers.
const GET_DISPLAY_INFO: u32 = 0x0100;
const RESOURCE_CREATE_2D: u32 = 0x0101;
const SET_SCANOUT: u32 = 0x0103;
const RESOURCE_FLUSH: u32 = 0x0104;
const TRANSFER_TO_HOST_2D: u32 = 0x0105;
const ATTACH_BACKING: u32 = 0x0106;
const OK_NODATA: u32 = 0x1100;
const OK_DISPLAY_INFO: u32 = 0x1101;

/// Pixels as 32-bit words 0x00RRGGBB, which in memory are B, G, R, X.
const FORMAT_B8G8R8X8: u32 = 2;
const RESOURCE: u32 = 1;

/// The command goes at the start of the header page, the answer after it.
const CMD: u64 = HEADER_PAGE;
const RESP: u64 = HEADER_PAGE + 2048;
const IOVA_CMD: u64 = IOVA_HEADER;
const IOVA_RESP: u64 = IOVA_HEADER + 2048;

struct Gpu {
    regs: Regs,
    q: Option<Queue>,
    ntfn: u64,
    page: &'static ScreenPage,
    width: u32,
    height: u32,
    screen_bytes: u64,
    pages: u64,
}

impl Gpu {
    /// Write a command header of type `kind` and return where its body
    /// starts.
    fn header(kind: u32) -> u64 {
        // SAFETY: the header page is mapped writable here.
        unsafe { core::ptr::write_bytes(CMD as *mut u8, 0, 2048 + 512) };
        w32(CMD, kind);
        CMD + 24
    }

    /// Send the command of `len` bytes and wait for an answer of up to
    /// `resp` bytes; returns the answer type.
    fn send(&mut self, len: u32, resp: u32) -> u32 {
        let q = self.q.as_mut().expect("no queue");
        q.desc(0, IOVA_CMD, len, D_NEXT, 1);
        q.desc(1, IOVA_RESP, resp, D_WRITE, 0);
        q.offer(0);
        q.kick();
        loop {
            if q.take_used().is_some() {
                return r32(RESP);
            }
            let bits = alaya_sys::wait(self.ntfn).unwrap_or(0);
            if bits & CRASH_BIT != 0 {
                say!("crashing on purpose (test)");
                alaya_sys::crash();
            }
            // Bits other than the interrupt (a new frame) are seen again
            // by the main loop through the shared page.
        }
    }

    fn start(&mut self) -> Result<(), u64> {
        self.regs.reset(0)?;
        self.regs.msix(self.msi());
        self.q = Some(self.regs.queue(0, RING_PAGE, IOVA_RING)?);
        self.regs.ready();

        // How big is the screen?
        Self::header(GET_DISPLAY_INFO);
        if self.send(24, 24 + 16 * 24) != OK_DISPLAY_INFO {
            return Err(why::DEVICE);
        }
        let (w, h) = (r32(RESP + 24 + 8), r32(RESP + 24 + 12));
        if w == 0 || h == 0 || u64::from(w) * u64::from(h) * 4 > self.screen_bytes {
            say!("the screen is {w} by {h}, which does not fit the screen memory");
            return Err(why::DEVICE);
        }
        self.width = w;
        self.height = h;
        store(&self.page.width, u64::from(w));
        store(&self.page.height, u64::from(h));

        // One picture of that size, kept in the client's screen memory.
        let b = Self::header(RESOURCE_CREATE_2D);
        w32(b, RESOURCE);
        w32(b + 4, FORMAT_B8G8R8X8);
        w32(b + 8, w);
        w32(b + 12, h);
        if self.send(24 + 16, 24) != OK_NODATA {
            return Err(why::DEVICE);
        }
        let b = Self::header(ATTACH_BACKING);
        w32(b, RESOURCE);
        w32(b + 4, self.pages as u32);
        for i in 0..self.pages {
            let e = b + 8 + i * 16;
            w64(e, IOVA_SCREEN + i * (2 << 20));
            w32(e + 8, 2 << 20);
        }
        if self.send(24 + 8 + 16 * self.pages as u32, 24) != OK_NODATA {
            return Err(why::DEVICE);
        }
        let b = Self::header(SET_SCANOUT);
        w32(b + 8, w);
        w32(b + 12, h);
        w32(b + 16, 0);
        w32(b + 20, RESOURCE);
        if self.send(24 + 24, 24) != OK_NODATA {
            return Err(why::DEVICE);
        }
        Ok(())
    }

    fn msi(&self) -> (u64, u64) {
        alaya_sys::irq_msi(start::FIRST_GIVEN + G_IRQ, rt_bdf())
            .map(|(a, d, _)| (a, d))
            .unwrap_or((0, 0))
    }

    /// Copy the rectangle to the device's screen and show it.
    fn flush(&mut self, x: u32, y: u32, w: u32, h: u32) -> Result<(), u64> {
        let b = Self::header(TRANSFER_TO_HOST_2D);
        w32(b, x);
        w32(b + 4, y);
        w32(b + 8, w);
        w32(b + 12, h);
        w64(
            b + 16,
            (u64::from(y) * u64::from(self.width) + u64::from(x)) * 4,
        );
        w32(b + 24, RESOURCE);
        if self.send(24 + 32, 24) != OK_NODATA {
            return Err(why::DEVICE);
        }
        let b = Self::header(RESOURCE_FLUSH);
        w32(b, x);
        w32(b + 4, y);
        w32(b + 8, w);
        w32(b + 12, h);
        w32(b + 16, RESOURCE);
        if self.send(24 + 24, 24) != OK_NODATA {
            return Err(why::DEVICE);
        }
        Ok(())
    }

    /// Show the part the client marked as changed, clipped to the screen.
    fn show(&mut self) -> Result<(), u64> {
        let d = &self.page.dirty;
        let (sw, sh) = (u64::from(self.width), u64::from(self.height));
        let x = load(&d[0]).min(sw);
        let y = load(&d[1]).min(sh);
        let w = load(&d[2]).min(sw - x);
        let h = load(&d[3]).min(sh - y);
        if w > 0 && h > 0 {
            self.flush(x as u32, y as u32, w as u32, h as u32)?;
        }
        Ok(())
    }
}

static BDF: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
fn rt_bdf() -> u16 {
    BDF.load(Ordering::Relaxed) as u16
}

fn fail(page: &ScreenPage, e: u64) -> ! {
    store(&page.error, e);
    store(&page.state, run::FAILED);
    say!("cannot start the screen (reason {e})");
    alaya_sys::crash()
}

fn main(info: &'static StartInfo) -> ! {
    let g = start::FIRST_GIVEN;
    // SAFETY: the client maps the shared page here before starting us.
    let page = unsafe { &*(JOB_PAGE as *const ScreenPage) };
    let _ = info.args[ARG_FLAGS];
    BDF.store(info.args[ARG_BDF], Ordering::Relaxed);
    store(&page.state, run::STARTING);
    page.starts.fetch_add(1, Ordering::AcqRel);
    let regs = Regs::find().unwrap_or_else(|| fail(page, why::NO_REGS));
    let pages = info.args[ARG_SCREEN_PAGES].clamp(1, SCREEN_PAGES);
    let ntfn = g + G_NTFN;
    let _ = alaya_sys::irq_set(g + G_IRQ, ntfn, IRQ_BIT);
    Regs::enable_pci();
    let mut gpu = Gpu {
        regs,
        q: None,
        ntfn,
        page,
        width: 0,
        height: 0,
        screen_bytes: pages * (2 << 20),
        pages,
    };
    if let Err(e) = gpu.start() {
        fail(page, e);
    }
    // Whatever is in the screen memory now is the picture: show all of it
    // (after a crash this brings the screen back as it was).
    if let Err(e) = gpu.flush(0, 0, gpu.width, gpu.height) {
        fail(page, e);
    }
    store(&page.state, run::RUNNING);
    say!("the screen is {} by {} pixels", gpu.width, gpu.height);
    let client = g + G_CLIENT;
    let _ = alaya_sys::signal(client, SHOWN_BIT);
    loop {
        let posted = load(&page.posted);
        if posted != load(&page.shown) {
            let t0 = rt::now_us();
            if let Err(e) = gpu.show() {
                fail(page, e);
            }
            store(&page.flush_us, rt::now_us() - t0);
            store(&page.shown, posted);
            let _ = alaya_sys::signal(client, SHOWN_BIT);
            continue;
        }
        let bits = alaya_sys::wait(ntfn).unwrap_or(0);
        if bits & CRASH_BIT != 0 {
            say!("crashing on purpose (test)");
            alaya_sys::crash();
        }
    }
}

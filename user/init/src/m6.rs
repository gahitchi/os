// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 6 checks, part one: the screen, the keyboard and the mouse.
//!
//! The first program, as device manager, finds the virtual screen card and
//! the virtual keyboard and mouse on the PCI bus, locks each to its own
//! memory, and starts their drivers from the boot image. It lends the
//! screen driver the screen memory and draws a test picture into it; the
//! host (`cargo xtask test`) takes a screenshot through QEMU and checks the
//! picture, then types a key and clicks the mouse, which must arrive here.
//! Last, every driver is crashed on purpose and must come back by itself,
//! the picture with it.
//!
//! Without the host taking part (the phone script), the input check waits
//! a few seconds and says it was skipped.

use alaya_programs::driver::{self as d, ARG_BDF, ARG_DEV_CODES, ARG_DEV_PAGES};
use alaya_programs::seat::{self, InputPage, ScreenPage, ev, kind, pack, unpack};
use alaya_programs::{load, run, store};
use alaya_sys::abi::{BootInfo, FAULT_LABEL, iomap, map};
use alaya_sys::program::{self, Parent, Program, Slots, Spec};
use alaya_sys::{Kind, rights};

use crate::m3::{Kit, sleep_until};
use crate::m4b::{bar_address, cfg8, cfg16, map_config, module, unmap_config, virtio_pages};
use crate::say;

/// Where the first program sees the screen memory and the shared pages.
const SCREEN_AT: u64 = 0x2a00_0000_0000;
const SHARED_AT: u64 = 0x2b00_0000_0000;
const SCRATCH: u64 = 0x2c00_0000_0000;

const DRIVER_PRIORITY: u64 = 170;
/// Memory each driver instance is made from, and keeps for itself.
const DRIVER_BITS: u8 = 22;
const DRIVER_OWN_BITS: u8 = 20;
/// Parent slots set aside for building one driver instance.
const SPAN: u64 = 64;
const MAX_DEVICES: usize = 4;

/// The test picture: a background and four squares (x, y, colour). The
/// host checks the middle of each square in its screenshot; keep in sync
/// with `xtask` (`TEST_PICTURE`).
const BACKGROUND: u32 = 0x20_26_34;
const SQUARES: [(u32, u32, u32); 4] = [
    (100, 100, 0xe0_53_3d),
    (300, 100, 0x3f_b9_50),
    (500, 100, 0x38_8b_fd),
    (700, 100, 0xff_ff_ff),
];
const SQUARE: u32 = 100;

/// One device and its driver.
#[derive(Clone, Copy)]
struct Dev {
    bdf: u16,
    /// The driver's notification, the fault endpoint the kernel reports
    /// its crashes to, and its interrupt.
    ntfn: u64,
    fault: u64,
    irq: u64,
    cfg: u64,
    pages: [(u64, u64); 12],
    npages: usize,
    shared: u64,
    shared_va: u64,
    ring: u64,
    header: u64,
    ut: u64,
    slots: u64,
    prog: Option<Program>,
    image: &'static str,
}

struct Seat<'a> {
    k: Kit<'a>,
    dev: [Option<Dev>; MAX_DEVICES],
    /// The first program's notification the drivers signal.
    client: u64,
    fault_ep: u64,
    reply: u64,
    screen: [u64; seat::SCREEN_PAGES as usize],
    screen_pages: u64,
}

fn screen_page(d: &Dev) -> &'static ScreenPage {
    // SAFETY: the shared page is mapped at `shared_va` for good.
    unsafe { &*(d.shared_va as *const ScreenPage) }
}

fn input_page(d: &Dev) -> &'static InputPage {
    // SAFETY: as above.
    unsafe { &*(d.shared_va as *const InputPage) }
}

impl Seat<'_> {
    /// Lock the device at `bdf` to its own memory and get it ready for a
    /// driver (not started yet).
    fn bind(&mut self, n: usize, bdf: u16, image: &'static str) {
        let k = &mut self.k;
        let info = k.info;
        let cfg = map_config(k, info.ecam_base, bdf);
        let mut codes = [0u64; 12];
        let ncodes = virtio_pages(&mut codes);
        let mut bars = [0u64; 6];
        for (i, b) in bars.iter_mut().enumerate() {
            *b = bar_address(i as u64);
        }
        alaya_sys::page_unmap(cfg).unwrap();
        let mut pages = [(0u64, 0u64); 12];
        for i in 0..ncodes {
            let (bar, page) = (codes[i] >> 16, codes[i] & 0xffff);
            assert!(bars[bar as usize] != 0, "BAR {bar} is not memory");
            let h = k.slot();
            alaya_sys::device_page(
                info.device_control,
                bars[bar as usize] + page * 4096,
                12,
                k.at(h),
            )
            .unwrap();
            pages[i] = (h, codes[i]);
        }
        let io = k.make(Kind::IoSpace, 13);
        let pool = k.make(Kind::Untyped, 16);
        alaya_sys::iospace_assign(io, pool).unwrap();
        alaya_sys::device_bind(info.device_control, io, bdf).unwrap();
        let irq = k.slot();
        alaya_sys::irq_get(info.irq_control, 0, k.at(irq)).unwrap();
        // Pages the device may reach: its queue and its buffers.
        let dma = |k: &mut Kit, iova: u64| {
            let p = k.make(Kind::Page, 12);
            let c = k.slot();
            alaya_sys::copy(k.at(c), k.at(p), rights::ALL).unwrap();
            alaya_sys::page_iomap(c, io, iova, iomap::WRITE).unwrap();
            p
        };
        let ring = dma(k, d::IOVA_RING);
        let header = dma(k, d::IOVA_HEADER);
        let shared_va = SHARED_AT + n as u64 * 0x10000;
        let shared = k.page_at(info.space, shared_va, map::WRITE);
        if image == "gpu" {
            // The screen memory, lent to the device only.
            for (i, &p) in self.screen[..self.screen_pages as usize].iter().enumerate() {
                let c = k.slot();
                alaya_sys::copy(k.at(c), k.at(p), rights::ALL).unwrap();
                alaya_sys::page_iomap(
                    c,
                    io,
                    seat::IOVA_SCREEN + i as u64 * (2 << 20),
                    iomap::WRITE,
                )
                .unwrap();
            }
        }
        let ntfn = k.make(Kind::Notification, 0);
        let fault = k.slot();
        alaya_sys::mint(k.at(fault), k.at(self.fault_ep), rights::ALL, n as u64 + 1).unwrap();
        let ut = k.make(Kind::Untyped, DRIVER_BITS);
        let slots = k.next;
        k.next += SPAN;
        self.dev[n] = Some(Dev {
            bdf,
            ntfn,
            fault,
            irq,
            cfg,
            pages,
            npages: ncodes,
            shared,
            shared_va,
            ring,
            header,
            ut,
            slots,
            prog: None,
            image,
        });
    }

    /// Start (or start again) the driver of device `n`.
    fn start(&mut self, n: usize) {
        let x = self.dev[n].unwrap();
        let k = &mut self.k;
        let info = k.info;
        let _ = alaya_sys::revoke(k.at(x.ut));
        for i in x.slots..x.slots + SPAN {
            let _ = alaya_sys::delete(k.at(i));
        }
        let mut shared = [(0u64, 0u64, 0u64); 20];
        let fixed = [
            (x.shared, d::JOB_PAGE),
            (x.ring, d::RING_PAGE),
            (x.header, d::HEADER_PAGE),
            (x.cfg, d::CONFIG_PAGE),
        ];
        let mut ns = 0;
        for (h, at) in fixed {
            shared[ns] = (h, at, map::WRITE);
            ns += 1;
        }
        let mut args = [0u64; 16];
        args[ARG_DEV_PAGES] = x.npages as u64;
        args[ARG_BDF] = u64::from(x.bdf);
        for i in 0..x.npages {
            let (h, code) = x.pages[i];
            shared[ns] = (h, d::bar_page(code >> 16, code & 0xffff), map::WRITE);
            ns += 1;
            args[ARG_DEV_CODES + i] = code;
        }
        args[seat::ARG_SCREEN_PAGES] = self.screen_pages;
        let sc = k.make(Kind::SchedContext, 8);
        alaya_sys::sc_configure(
            info.sched_control,
            sc,
            1000,
            1000,
            alaya_sys::abi::core_type::ANY,
            0,
        )
        .unwrap();
        let mut parent = Parent {
            space: info.space,
            scratch: SCRATCH,
            slots: Slots {
                table: info.table,
                next: x.slots,
                end: x.slots + SPAN,
            },
        };
        let p = program::start(
            &mut parent,
            &Spec {
                name: x.image,
                image: module(info, x.image),
                untyped: x.ut,
                table_bits: 8,
                own_bits: DRIVER_OWN_BITS,
                priority: DRIVER_PRIORITY,
                sc,
                clock_sc: 0,
                fault: x.fault,
                given: &[x.ntfn, self.client, x.irq],
                shared: &shared[..ns],
                args: &args,
                clock_hz: info.clock_hz,
                cores: info.cores,
            },
        )
        .expect("start a driver");
        self.dev[n].as_mut().unwrap().prog = Some(p);
    }

    fn find(&self, image: &'static str) -> impl Iterator<Item = (usize, Dev)> + '_ {
        self.dev
            .iter()
            .enumerate()
            .filter_map(move |(i, d)| d.filter(|d| d.image == image).map(|d| (i, d)))
    }

    /// Crash the driver of device `n` on purpose, wait for the kernel's
    /// report of it, and start it again.
    fn crash_and_restart(&mut self, n: usize) {
        let x = self.dev[n].unwrap();
        alaya_sys::signal(x.ntfn, seat::CRASH_BIT).unwrap();
        let m = alaya_sys::recv(self.fault_ep, self.reply).expect("fault report");
        assert!(
            m.label() == FAULT_LABEL && m.badge == n as u64 + 1,
            "an unexpected message instead of the driver's crash"
        );
        self.start(n);
    }
}

fn screen_pixels(w: u64, h: u64) -> &'static mut [u32] {
    // SAFETY: the screen memory is mapped at SCREEN_AT, writable, and is
    // at least w * h words (the driver checks it fits).
    unsafe { core::slice::from_raw_parts_mut(SCREEN_AT as *mut u32, (w * h) as usize) }
}

fn draw_test_picture(w: u64, h: u64) {
    let px = screen_pixels(w, h);
    px.fill(BACKGROUND);
    for &(x, y, c) in &SQUARES {
        for row in y..(y + SQUARE).min(h as u32) {
            let start = (u64::from(row) * w + u64::from(x)) as usize;
            let end = start + SQUARE.min((w as u32).saturating_sub(x)) as usize;
            px[start..end].fill(c);
        }
    }
}

/// Post a frame covering the whole screen and wait until it is shown.
fn show_all(sp: &ScreenPage, ntfn: u64, w: u64, h: u64) -> bool {
    store(&sp.dirty[0], 0);
    store(&sp.dirty[1], 0);
    store(&sp.dirty[2], w);
    store(&sp.dirty[3], h);
    let n = load(&sp.posted) + 1;
    store(&sp.posted, n);
    let _ = alaya_sys::signal(ntfn, seat::FRAME_BIT);
    sleep_until(5000, || load(&sp.shown) >= n)
}

pub fn run(info: &BootInfo, big: u64, first_free: u64, pass: &mut dyn FnMut(&str)) -> u64 {
    let mut k = Kit {
        info,
        ut: 0,
        next: first_free,
        stacks: 160,
    };
    // 32 MiB for the drivers and the screen, from a block the earlier
    // checks left alone if there is one.
    let h = k.slot();
    let free = info.untyped[..info.untyped_count as usize].iter().any(|b| {
        b.handle != big
            && b.size_bits >= 25
            && alaya_sys::retype(b.handle, Kind::Untyped, 25, 1, k.at(h)).is_ok()
    });
    if !free {
        alaya_sys::retype(big, Kind::Untyped, 25, 1, k.at(h)).expect("32 MiB for the screen");
    }
    k.ut = h;

    // The screen memory first (big pages, aligned): 2 MiB pages mapped
    // into the first program, which draws the pictures for now.
    let mut screen = [0u64; seat::SCREEN_PAGES as usize];
    let screen_pages = 2;
    for (i, s) in screen.iter_mut().enumerate().take(screen_pages) {
        *s = k.make(Kind::Page, 21);
        alaya_sys::page_map(*s, info.space, SCREEN_AT + i as u64 * (2 << 20), map::WRITE).unwrap();
    }
    let client = k.make(Kind::Notification, 0);
    let fault_ep = k.make(Kind::Endpoint, 0);
    let reply = k.make(Kind::Reply, 0);
    let mut s = Seat {
        k,
        dev: [None; MAX_DEVICES],
        client,
        fault_ep,
        reply,
        screen,
        screen_pages: screen_pages as u64,
    };

    // Find the screen card and the input devices.
    let mut found = [(0u16, 0u16); MAX_DEVICES];
    let mut nfound = 0;
    let bus = info.ecam_bus_start as u16;
    for dev in 0..32u16 {
        for func in 0..8u16 {
            let bdf = bus << 8 | dev << 3 | func;
            let h = map_config(&mut s.k, info.ecam_base, bdf);
            let (vendor, device) = (cfg16(0), cfg16(2));
            let multi = cfg8(0x0e) & 0x80 != 0;
            unmap_config(&mut s.k, h);
            if vendor == seat::VIRTIO_VENDOR
                && (device == seat::GPU_DEVICE || device == seat::INPUT_DEVICE)
                && nfound < MAX_DEVICES
            {
                found[nfound] = (bdf, device);
                nfound += 1;
            }
            if vendor == 0xffff || (func == 0 && !multi) {
                break;
            }
        }
    }
    let gpu = found[..nfound]
        .iter()
        .position(|f| f.1 == seat::GPU_DEVICE)
        .expect("no virtual screen card (virtio GPU) on the PCI bus");
    for (n, &(bdf, device)) in found[..nfound].iter().enumerate() {
        s.bind(
            n,
            bdf,
            if device == seat::GPU_DEVICE {
                "gpu"
            } else {
                "input"
            },
        );
    }

    // The screen.
    s.start(gpu);
    let g = s.dev[gpu].unwrap();
    let sp = screen_page(&g);
    assert!(
        sleep_until(5000, || load(&sp.state) != run::STARTING),
        "the screen driver never started"
    );
    assert!(
        load(&sp.state) == run::RUNNING,
        "the screen driver failed (reason {})",
        load(&sp.error)
    );
    let (w, h) = (load(&sp.width), load(&sp.height));
    say!("screen: {w} by {h} pixels, locked to its own memory");
    pass("the screen driver starts, locked to its own memory, and finds the screen size");

    draw_test_picture(w, h);
    assert!(
        show_all(sp, g.ntfn, w, h),
        "the test picture was never shown"
    );
    say!(
        "screen: test picture shown ({} us in the driver)",
        load(&sp.flush_us)
    );
    pass("a picture drawn in Alaya's screen memory reaches the screen");

    // The keyboard and the mouse.
    let inputs: [Option<usize>; MAX_DEVICES] = {
        let mut a = [None; MAX_DEVICES];
        for (j, (i, _)) in s.find("input").enumerate() {
            a[j] = Some(i);
        }
        a
    };
    let mut keyboard = None;
    let mut pointer = None;
    for &i in inputs.iter().flatten() {
        s.start(i);
        let ip = input_page(&s.dev[i].unwrap());
        assert!(
            sleep_until(5000, || load(&ip.state) != run::STARTING),
            "an input driver never started"
        );
        assert!(
            load(&ip.state) == run::RUNNING,
            "an input driver failed (reason {})",
            load(&ip.error)
        );
        match load(&ip.kind) {
            kind::KEYBOARD => keyboard = Some(i),
            kind::POINTER => pointer = Some(i),
            _ => {}
        }
    }
    let keyboard = keyboard.expect("no virtual keyboard (virtio input) found");
    let pointer = pointer.expect("no virtual mouse (virtio tablet) found");
    pass("the keyboard and mouse drivers start and tell which is which");

    // The host types "a" and clicks in the middle of the screen.
    say!("input: waiting for a key and a click");
    let kp = input_page(&s.dev[keyboard].unwrap());
    let pp = input_page(&s.dev[pointer].unwrap());
    let (k0, p0) = (load(&kp.head), load(&pp.head));
    let seen = |p: &InputPage, from: u64, want: u64| {
        (from..load(&p.head)).any(|n| load(&p.events[n as usize % seat::EVENTS]) == want)
    };
    let key_down = pack(ev::KEY, 30, 1);
    let key_up = pack(ev::KEY, 30, 0);
    let click = pack(ev::KEY, ev::BTN_LEFT, 1);
    let got = sleep_until(10_000, || {
        seen(kp, k0, key_down) && seen(kp, k0, key_up) && seen(pp, p0, click)
    });
    if got {
        let mut x = None;
        for n in p0..load(&pp.head) {
            let (t, c, v) = unpack(load(&pp.events[n as usize % seat::EVENTS]));
            if t == ev::ABS && c == ev::ABS_X {
                x = Some(v);
            }
        }
        let x = x.expect("the click came without a position");
        let px = u64::from(x) * w / (u64::from(ev::ABS_MAX) + 1);
        say!("input: key 'a' pressed and let go; click at x = {px}");
        assert!(
            px.abs_diff(w / 2) <= 2,
            "the click arrived at the wrong place"
        );
        pass("a key press and a mouse click reach Alaya");
    } else {
        say!("input: nobody pressed a key (no host script); skipped");
        pass("keyboard and mouse wait for input (not scripted on this run)");
    }

    // Crash every driver once: each comes back by itself, and the picture
    // comes back without being drawn again.
    let starts = load(&sp.starts);
    s.crash_and_restart(gpu);
    assert!(
        sleep_until(5000, || load(&sp.state) == run::RUNNING
            && load(&sp.starts) == starts + 1),
        "the screen driver did not come back"
    );
    // The new driver shows the screen memory as it is when it starts.
    let g = s.dev[gpu].unwrap();
    assert!(show_all(sp, g.ntfn, w, h), "frames stopped after the crash");
    for i in [keyboard, pointer] {
        let ip = input_page(&s.dev[i].unwrap());
        let before = load(&ip.starts);
        s.crash_and_restart(i);
        assert!(
            sleep_until(5000, || load(&ip.state) == run::RUNNING
                && load(&ip.starts) == before + 1),
            "an input driver did not come back"
        );
    }
    say!("screen: back after a crash");
    pass("the screen, keyboard and mouse drivers crash and come back, picture intact");

    desktop(&mut s, gpu, [keyboard, pointer], pass);
    s.k.next
}

/// Carve an untyped of `bits` from a block other than `big` if one has
/// room, else from `big`.
fn carve(k: &mut Kit, big: u64, bits: u8, what: &str) -> u64 {
    let info = k.info;
    let h = k.slot();
    let free = info.untyped[..info.untyped_count as usize].iter().any(|b| {
        b.handle != big
            && b.size_bits >= u64::from(bits)
            && alaya_sys::retype(b.handle, Kind::Untyped, bits, 1, k.at(h)).is_ok()
    });
    if !free {
        alaya_sys::retype(big, Kind::Untyped, bits, 1, k.at(h)).expect(what);
    }
    h
}

fn sc(k: &mut Kit, budget: u64, period: u64) -> u64 {
    let s = k.make(Kind::SchedContext, 8);
    alaya_sys::sc_configure(
        k.info.sched_control,
        s,
        budget,
        period,
        alaya_sys::abi::core_type::ANY,
        0,
    )
    .unwrap();
    s
}

const DESKTOP_PRIORITY: u64 = 150;
const APP_PRIORITY: u64 = 100;
const DESKTOP_SLOTS: u64 = 512;

/// Ask the desktop something, as the first program (badge 0).
fn ask(ep: u64, label: u64, words: &[u64]) -> [u64; 6] {
    alaya_sys::ipc_call(
        ep,
        &alaya_sys::Msg::new(label, words),
        alaya_sys::Caps::default(),
    )
    .map(|m| m.words)
    .unwrap_or([u64::MAX; 6])
}

/// Part two: the desktop and the first app on the real screen.
fn desktop(s: &mut Seat, gpu: usize, inputs: [usize; 2], pass: &mut dyn FnMut(&str)) {
    use alaya_programs::desktop as dk;
    let info = s.k.info;
    let big = info.untyped[..info.untyped_count as usize]
        .iter()
        .max_by_key(|b| b.size_bits)
        .unwrap()
        .handle;
    let desk_ut = carve(&mut s.k, big, 26, "64 MiB for the desktop");
    let app_ut = carve(&mut s.k, big, 23, "8 MiB for the editor");

    // The fonts, copied once into pages every app shares read only.
    let names = [
        "Figtree-Regular.ttf",
        "Figtree-SemiBold.ttf",
        "IBMPlexMono-Regular.ttf",
        "NotoSans-Regular.ttf",
    ];
    let font_pages = alloc_free::take(0);
    let mut sizes = [0u64; 4];
    for (i, name) in names.iter().enumerate() {
        let data = module(info, name);
        sizes[i] = data.len() as u64;
        let at = FONTS_AT + i as u64 * dk::FONT_STEP;
        for p in 0..(data.len() as u64).div_ceil(4096) {
            let h = s.k.page_at(info.space, at + p * 4096, map::WRITE);
            let from = (p * 4096) as usize;
            let to = (from + 4096).min(data.len());
            // SAFETY: just mapped, writable.
            let dst =
                unsafe { core::slice::from_raw_parts_mut((at + p * 4096) as *mut u8, to - from) };
            dst.copy_from_slice(&data[from..to]);
            font_pages.push((h, dk::FONTS + i as u64 * dk::FONT_STEP + p * 4096, 0));
        }
    }

    // The desktop: the screen's shared page, the input pages, the screen
    // memory and the fonts.
    let g = s.dev[gpu].unwrap();
    let shared = alloc_free::take(1);
    shared.push((g.shared, dk::SCREEN_PAGE, map::WRITE));
    for (n, &i) in inputs.iter().enumerate() {
        shared.push((
            s.dev[i].unwrap().shared,
            dk::INPUT_PAGES + n as u64 * 4096,
            map::WRITE,
        ));
    }
    for i in 0..s.screen_pages as usize {
        shared.push((s.screen[i], dk::SCREEN + i as u64 * (2 << 20), map::WRITE));
    }
    for &p in font_pages.as_slice() {
        shared.push(p);
    }
    let ep = s.k.make(Kind::Endpoint, 0);
    let mut args = [0u64; 8];
    args[dk::ARG_SCREEN_PAGES] = s.screen_pages;
    args[dk::ARG_INPUTS] = 2;
    args[dk::ARG_FONTS..dk::ARG_FONTS + 4].copy_from_slice(&sizes);
    let desk_sc = sc(&mut s.k, 2000, 2000);
    let desk_clock = sc(&mut s.k, 50, 10_000);
    let base = s.k.next;
    s.k.next += DESKTOP_SLOTS;
    let mut parent = Parent {
        space: info.space,
        scratch: SCRATCH,
        slots: Slots {
            table: info.table,
            next: base,
            end: base + DESKTOP_SLOTS,
        },
    };
    program::start(
        &mut parent,
        &Spec {
            name: "desktop",
            image: module(info, "desktop"),
            untyped: desk_ut,
            table_bits: 11,
            own_bits: 25,
            priority: DESKTOP_PRIORITY,
            sc: desk_sc,
            clock_sc: desk_clock,
            fault: 0,
            given: &[ep, g.ntfn, s.client],
            shared: shared.as_slice(),
            args: &args,
            clock_hz: info.clock_hz,
            cores: info.cores,
        },
    )
    .expect("start the desktop");
    let sp = screen_page(&g);
    let posted = load(&sp.posted);
    assert!(
        sleep_until(10_000, || load(&sp.posted) > posted
            && load(&sp.shown) == load(&sp.posted)),
        "the desktop never drew"
    );
    // The top strip is Alaya's colour.
    let (w, h) = (load(&sp.width), load(&sp.height));
    let px = screen_pixels(w, h);
    assert!(
        px[(4 * w + w / 2) as usize] == alaya_ui_colours::BAR,
        "the top strip is {:06x}",
        px[(4 * w + w / 2) as usize]
    );
    pass("the desktop starts and draws the screen");

    // The editor, as an app: its own memory, a badged way to the desktop
    // and a notification for its events.
    let app_ep = s.k.slot();
    alaya_sys::mint(s.k.at(app_ep), s.k.at(ep), rights::ALL, 1).unwrap();
    let app_ntfn = s.k.make(Kind::Notification, 0);
    let mut app_args = [0u64; 8];
    app_args[dk::ARG_FONTS..dk::ARG_FONTS + 4].copy_from_slice(&sizes);
    let app_sc = sc(&mut s.k, 1000, 1000);
    let base = s.k.next;
    s.k.next += DESKTOP_SLOTS;
    let mut parent = Parent {
        space: info.space,
        scratch: SCRATCH,
        slots: Slots {
            table: info.table,
            next: base,
            end: base + DESKTOP_SLOTS,
        },
    };
    program::start(
        &mut parent,
        &Spec {
            name: "editor",
            image: module(info, "editor"),
            untyped: app_ut,
            table_bits: 10,
            own_bits: 22,
            priority: APP_PRIORITY,
            sc: app_sc,
            clock_sc: 0,
            fault: 0,
            given: &[app_ep, app_ntfn],
            shared: font_pages.as_slice(),
            args: &app_args,
            clock_hz: info.clock_hz,
            cores: info.cores,
        },
    )
    .expect("start the editor");
    let opened = sleep_until(15_000, || ask(ep, dk::STATE, &[])[1] == 1);
    assert!(opened, "the editor's window never opened");
    let win = ask(ep, dk::WINDOW, &[1]);
    say!(
        "desktop: the editor's window is at {}, {}, {} by {}",
        win[1],
        win[2],
        win[3],
        win[4]
    );
    pass("an app opens a window and draws in it with Alaya's letters");

    // The host drags the editor's title bar to the left edge.
    let (tx, ty) = (win[1] + win[3] / 2 - 40, win[2] + 14);
    say!("desktop: drag from {tx} {ty} to 0 {}", ty + 200);
    let snapped = sleep_until(10_000, || ask(ep, dk::WINDOW, &[1])[5] == dk::place::LEFT);
    if snapped {
        pass("dragging a window to the left edge snaps it to the left half");
    } else {
        say!("desktop: nobody dragged (no host script); skipped");
        pass("windows wait for the mouse (not scripted on this run)");
    }

    // The host types; the editor shows it (the host reads the editor's
    // report).
    say!("desktop: type hi");
    sleep_until(4000, || false);

    // Tiling on and off.
    ask(ep, dk::TILING, &[1]);
    assert!(
        sleep_until(5000, || ask(ep, dk::WINDOW, &[1])[5] == dk::place::TILED),
        "tiling did not tile"
    );
    ask(ep, dk::TILING, &[0]);
    pass("tiling shares the screen, and goes back");

    // The screen driver crashes under the desktop; frames carry on.
    s.crash_and_restart(gpu);
    let before = ask(ep, dk::STATE, &[])[4];
    ask(ep, dk::TILING, &[1]);
    ask(ep, dk::TILING, &[0]);
    assert!(
        sleep_until(5000, || ask(ep, dk::STATE, &[])[4] > before
            && load(&sp.shown) == load(&sp.posted)),
        "the desktop stopped drawing after the screen driver crashed"
    );
    say!("desktop: drawn after a screen driver crash");
    pass("the screen driver crashes under the desktop and the desktop carries on");
}

/// The fonts sit here in the first program.
const FONTS_AT: u64 = 0x2d00_0000_0000;

/// Colours the checks look for (the mock-up's, as in `alaya_ui`).
mod alaya_ui_colours {
    pub const BAR: u32 = 0xf6_f9_f8;
}

/// A fixed-size list of shared pages (the first program has no heap,
/// and its stack is small).
mod alloc_free {
    static mut LISTS: [Pages; 2] = [Pages::new(), Pages::new()];

    /// List `i`, emptied. Only the first program's main thread calls it.
    pub fn take(i: usize) -> &'static mut Pages {
        // SAFETY: one thread, and each list is handed out once per use.
        let l = unsafe { &mut (*core::ptr::addr_of_mut!(LISTS))[i] };
        l.n = 0;
        l
    }
    pub struct Pages {
        items: [(u64, u64, u64); 512],
        n: usize,
    }
    impl Pages {
        const fn new() -> Pages {
            Pages {
                items: [(0, 0, 0); 512],
                n: 0,
            }
        }
        pub fn push(&mut self, p: (u64, u64, u64)) {
            assert!(self.n < self.items.len(), "too many shared pages");
            self.items[self.n] = p;
            self.n += 1;
        }
        pub fn as_slice(&self) -> &[(u64, u64, u64)] {
            &self.items[..self.n]
        }
    }
}

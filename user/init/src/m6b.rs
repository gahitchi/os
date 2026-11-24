// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 6 checks, part two: the desktop, first setup and login, and
//! the first apps; then the first program's lasting job, looking after
//! the drivers and the apps (`Session::serve`).
//!
//! The host (`cargo xtask test`) types and drags through QEMU when the
//! checks ask it to ("keys: ...", "drag from ..."). Without a host (the
//! phone script), the first program sends the same keys and mouse moves
//! to the desktop itself, through the same path the drivers' events take.

use core::sync::atomic::Ordering::SeqCst;

use alaya_crypto::Sha256;
use alaya_programs::desktop::{self as dk, parts, role, stage};
use alaya_programs::files::{self as fs, APPS, DOCS, st};
use alaya_programs::load;
use alaya_programs::seat::ev;
use alaya_sys::abi::{BootInfo, FAULT_LABEL, map};
use alaya_sys::program::{self, Parent, Slots, Spec};
use alaya_sys::{Caps, Kind, Msg, MsgInfo, rights};

use crate::m3::sleep_until;
use crate::m4b::module;
use crate::m5::{STORE_EP, XFER_AT_PUB, XFER_PAGES};
use crate::m6::{
    BAR, FONTS_AT, HOST, SCRATCH, Seat, alloc_free, carve, input_page, sc, screen_page,
    screen_pixels,
};
use crate::say;

const DESKTOP_PRIORITY: u64 = 150;
const APP_PRIORITY: u64 = 100;
const DESKTOP_SLOTS: u64 = 512;
const APP_SLOTS: u64 = 256;
/// Fault reports of apps come with this badge plus the app's role.
const APP_FAULT: u64 = 0x100;

/// The key of the machine's own files (who can log in, the login parts).
/// The TPM keeps it from milestone 13; until then it is in the image.
const MACHINE_FILES_KEY: [u8; 32] = *b"alaya machine files key, m6 only";
/// The memory of the slow PIN step: 2^16 entries of 32 bytes (2 MiB).
const PIN_WORK: u64 = 1 << 16;
// The checks set up the PIN 2468 and try the wrong PIN 1357.

/// Ask the desktop something, as the first program (badge 0).
pub(crate) fn ask(ep: u64, label: u64, words: &[u64]) -> [u64; 6] {
    alaya_sys::ipc_call(ep, &Msg::new(label, words), Caps::default())
        .map(|m| m.words)
        .unwrap_or([u64::MAX; 6])
}

#[derive(Clone, Copy, Default)]
struct App {
    ut: u64,
    slots: u64,
    ntfn: u64,
    desktop: u64,
    store: u64,
    fault: u64,
    starts: u64,
}

/// Everything the first program keeps running after its checks.
pub(crate) struct Session<'a> {
    pub s: Seat<'a>,
    pub(crate) ep: u64,
    session_ntfn: u64,
    sizes: [u64; 4],
    apps: [App; role::COUNT as usize + 1],
    person: u64,
    /// Apps the desktop asked to start, seen while waiting.
    asked: core::cell::Cell<u64>,
    gpu: usize,
    keyboard: usize,
    pointer: usize,
}

/// The first program's own way into the store, as person `person`.
pub(crate) struct Files {
    pub(crate) ep: u64,
}

impl Files {
    pub(crate) fn buf(&self) -> &'static mut [u8] {
        let base = XFER_AT_PUB;
        // SAFETY: channel 0's transfer pages are mapped here; one request
        // at a time.
        unsafe { core::slice::from_raw_parts_mut(base as *mut u8, fs::XFER_BYTES) }
    }

    pub(crate) fn call(&self, label: u64, words: &[u64]) -> [u64; 6] {
        alaya_sys::ipc_call(self.ep, &Msg::new(label, words), Caps::default())
            .map(|m| m.words)
            .unwrap_or([u64::MAX; 6])
    }

    pub(crate) fn lookup(&self, dir: u64, name: &str) -> Option<u64> {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
        let r = self.call(fs::LOOKUP, &[dir, name.len() as u64]);
        (r[0] == st::OK).then_some(r[1])
    }

    /// The first `out.len()` bytes of a file, and its size.
    pub(crate) fn read(&self, f: u64, out: &mut [u8]) -> Option<u64> {
        let size = self.call(fs::STAT, &[f]);
        if size[0] != st::OK {
            return None;
        }
        let n = out.len().min(fs::XFER_BYTES);
        let r = self.call(fs::READ, &[f, 0, n as u64]);
        if r[0] != st::OK {
            return None;
        }
        out[..r[1] as usize].copy_from_slice(&self.buf()[..r[1] as usize]);
        Some(size[1])
    }

    pub(crate) fn root(&self, class: u64) -> u64 {
        self.call(fs::ROOT, &[class])[1]
    }
}

/// The editor's return point as the store has it: cursor and the text's
/// length.
pub(crate) fn return_point(f: &Files, buf: &mut [u8]) -> Option<(u64, usize)> {
    return_point_of(f, buf).map(|r| (r.0, r.1))
}

/// The editor's return point: cursor, the text's length, whether it is
/// saved, and the file's name (in `buf`, from `name_at` on).
pub(crate) fn return_point_of(f: &Files, buf: &mut [u8]) -> Option<(u64, usize, bool, usize)> {
    let apps = f.root(APPS);
    let r = f.lookup(apps, "editor-return")?;
    let size = f.read(r, buf)? as usize;
    let w = |i: usize| u64::from_le_bytes(buf[8 * i..8 * i + 8].try_into().unwrap());
    if size < 48 || size > buf.len() {
        return None;
    }
    let len = w(4) as usize;
    let name = w(5) as usize;
    (48 + name + len == size).then(|| (w(1), len, w(3) == 0, name))
}

impl<'a> Session<'a> {
    /// Type `spec` (see `alaya_programs::desktop::keys`): the host types it
    /// if there is one, else the first program sends the key events.
    pub(crate) fn keys(&self, spec: &str, done: impl Fn() -> bool, ms: u64) -> bool {
        say!("keys: {spec}");
        if HOST.load(SeqCst) {
            if sleep_until(ms, &done) {
                return true;
            }
            say!("the typed keys did not arrive; sending them myself");
        }
        dk::keys(spec, |code, mods| {
            let press = |c: u16, v: u32| {
                ask(
                    self.ep,
                    dk::INJECT,
                    &[u64::from(ev::KEY), u64::from(c), u64::from(v)],
                );
            };
            if mods & 1 != 0 {
                press(42, 1);
            }
            if mods & 2 != 0 {
                press(29, 1);
            }
            if mods & 4 != 0 {
                press(125, 1);
            }
            press(code, 1);
            press(code, 0);
            if mods & 4 != 0 {
                press(125, 0);
            }
            if mods & 2 != 0 {
                press(29, 0);
            }
            if mods & 1 != 0 {
                press(42, 0);
            }
        });
        sleep_until(ms, done)
    }

    /// Drag with the left button from one place to another.
    pub(crate) fn drag(&self, from: (u64, u64), to: (u64, u64), done: impl Fn() -> bool) -> bool {
        say!(
            "desktop: drag from {} {} to {} {}",
            from.0,
            from.1,
            to.0,
            to.1
        );
        // Long: a second click is not always harmless (an undo button).
        if HOST.load(SeqCst) && sleep_until(30_000, &done) {
            return true;
        }
        let (w, h) = self.size();
        let at = |x: u64, y: u64| {
            let ax = x * (u64::from(ev::ABS_MAX) + 1) / w;
            let ay = y * (u64::from(ev::ABS_MAX) + 1) / h;
            ask(
                self.ep,
                dk::INJECT,
                &[u64::from(ev::ABS), u64::from(ev::ABS_X), ax],
            );
            ask(
                self.ep,
                dk::INJECT,
                &[u64::from(ev::ABS), u64::from(ev::ABS_Y), ay],
            );
            ask(self.ep, dk::INJECT, &[u64::from(ev::SYN), 0, 0]);
        };
        let button = |down: u64| {
            ask(
                self.ep,
                dk::INJECT,
                &[u64::from(ev::KEY), u64::from(ev::BTN_LEFT), down],
            );
        };
        at(from.0, from.1);
        button(1);
        for i in 1..=10 {
            let x = (from.0 as i64 + (to.0 as i64 - from.0 as i64) * i / 10) as u64;
            let y = (from.1 as i64 + (to.1 as i64 - from.1 as i64) * i / 10) as u64;
            at(x, y);
        }
        button(0);
        sleep_until(5000, done)
    }

    fn size(&self) -> (u64, u64) {
        let sp = screen_page(&self.s.dev[self.gpu].unwrap());
        (load(&sp.width), load(&sp.height))
    }

    /// Start the apps the desktop asked for meanwhile (the dock, an undo
    /// that opens a window again); `serve` does this after the checks.
    pub(crate) fn start_asked(&mut self) -> u64 {
        let bits = self.asked_bits();
        self.asked.set(0);
        for r in 1..=role::COUNT {
            if bits & (1 << r) != 0 {
                self.start_app(r);
            }
        }
        bits
    }

    /// What the desktop asked for so far (kept for `start_asked`).
    pub(crate) fn asked_bits(&self) -> u64 {
        let bits = self.asked.get() | alaya_sys::poll(self.session_ntfn).unwrap_or(0);
        self.asked.set(bits);
        bits
    }

    pub(crate) fn session(&self) -> [u64; 6] {
        ask(self.ep, dk::SESSION, &[])
    }

    /// The window of the app with role `r`, if it has one open.
    pub(crate) fn window_of(&self, r: u64) -> Option<u64> {
        let a = ask(self.ep, dk::WINDOW_OF, &[r]);
        (a[0] == dk::OK).then_some(a[1])
    }

    /// Start (or start again) the app with role `r` for the person logged
    /// in. A copy still running is ended first and its windows closed.
    pub fn start_app(&mut self, r: u64) {
        let i = r as usize;
        let info = self.s.k.info;
        if self.apps[i].ut == 0 {
            let big = info.untyped[..info.untyped_count as usize]
                .iter()
                .max_by_key(|b| b.size_bits)
                .unwrap()
                .handle;
            let k = &mut self.s.k;
            let mut a = App {
                ut: carve(k, big, 24, "16 MiB for an app"),
                ..App::default()
            };
            a.slots = k.next;
            k.next += APP_SLOTS;
            a.ntfn = k.make(Kind::Notification, 0);
            a.desktop = k.slot();
            alaya_sys::mint(k.at(a.desktop), k.at(self.ep), rights::ALL, r).unwrap();
            a.store = k.slot();
            alaya_sys::mint(
                k.at(a.store),
                k.at(STORE_EP.load(SeqCst)),
                rights::ALL,
                fs::badge(self.person, fs::app_id(r), fs::app_channel(r)),
            )
            .unwrap();
            a.fault = k.slot();
            alaya_sys::mint(
                k.at(a.fault),
                k.at(self.s.fault_ep),
                rights::ALL,
                APP_FAULT + r,
            )
            .unwrap();
            self.apps[i] = a;
        }
        let a = self.apps[i];
        // End a copy still running, and close its windows.
        let k = &mut self.s.k;
        let _ = alaya_sys::revoke(k.at(a.ut));
        for x in a.slots..a.slots + APP_SLOTS {
            let _ = alaya_sys::delete(k.at(x));
        }
        ask(self.ep, dk::FORGET, &[r]);
        let _ = alaya_sys::poll(a.ntfn);
        let shared = alloc_free::take(2);
        for &p in alloc_free::take_ro(0).as_slice() {
            shared.push(p);
        }
        let ch = fs::app_channel(r);
        for p in 0..fs::XFER_PAGES {
            let h = XFER_PAGES[(ch * fs::XFER_PAGES + p) as usize].load(SeqCst);
            shared.push((h, dk::XFER + p * 4096, map::WRITE));
        }
        let mut args = [0u64; 8];
        args[dk::ARG_ROLE] = r;
        args[dk::ARG_CHANNEL] = ch;
        args[dk::ARG_FONTS..dk::ARG_FONTS + 4].copy_from_slice(&self.sizes);
        let app_sc = sc(k, 1000, 1000);
        let clock = sc(k, 50, 10_000);
        let mut parent = Parent {
            space: info.space,
            scratch: SCRATCH,
            slots: Slots {
                table: info.table,
                next: a.slots,
                end: a.slots + APP_SLOTS,
            },
        };
        let name = match r {
            role::EDITOR => "editor",
            role::FILES => "files",
            role::CHART => "chart",
            role::BACK => "back",
            _ => "settings",
        };
        program::start(
            &mut parent,
            &Spec {
                name,
                image: module(info, "apps"),
                untyped: a.ut,
                table_bits: 10,
                own_bits: 23,
                priority: APP_PRIORITY,
                sc: app_sc,
                clock_sc: clock,
                fault: a.fault,
                given: &[a.desktop, a.ntfn, a.store],
                shared: shared.as_slice(),
                args: &args,
                clock_hz: info.clock_hz,
                cores: info.cores,
            },
        )
        .expect("start an app");
        self.apps[i].starts += 1;
    }

    pub(crate) fn wait_window(&self, r: u64) -> u64 {
        let id = core::cell::Cell::new(None);
        let found = sleep_until(60_000, || {
            id.set(self.window_of(r));
            id.get().is_some()
        });
        assert!(found, "app {r} never opened its window");
        id.get().unwrap()
    }

    /// The first program's lasting job, after its checks: start the apps
    /// the desktop asks for, and start again any driver or app that
    /// crashes. It never returns.
    pub fn serve(mut self) -> ! {
        let info = self.s.k.info;
        alaya_sys::thread_bind(info.thread, self.session_ntfn).expect("bind");
        say!("looking after the drivers and the apps");
        loop {
            let Ok(m) = alaya_sys::recv(self.s.fault_ep, self.s.reply) else {
                continue;
            };
            if m.info.has(MsgInfo::SIGNAL) {
                if m.badge & dk::LOGGED_IN_BIT != 0 && self.person == 0 {
                    self.person = self.session()[2];
                    say!("person {} logged in; starting the text editor", self.person);
                    self.start_app(role::EDITOR);
                }
                if self.person == 0 {
                    continue;
                }
                for r in 1..=role::COUNT {
                    if m.badge & (1 << r) != 0 {
                        self.start_app(r);
                    }
                }
                continue;
            }
            if m.label() != FAULT_LABEL {
                continue;
            }
            match m.badge {
                b if (1..=4).contains(&b) && self.s.dev[b as usize - 1].is_some() => {
                    say!("a driver stopped; starting it again");
                    self.s.start(b as usize - 1);
                }
                b if b > APP_FAULT && b <= APP_FAULT + role::COUNT => {
                    say!("app {} stopped; starting it again", b - APP_FAULT);
                    self.start_app(b - APP_FAULT);
                }
                _ => {}
            }
        }
    }
}

/// Start the desktop, then the checks of first setup, login and the apps.
pub(crate) fn run<'a>(
    mut s: Seat<'a>,
    gpu: usize,
    [keyboard, pointer]: [usize; 2],
    pass: &mut dyn FnMut(&str),
) -> Session<'a> {
    let info: &'a BootInfo = s.k.info;
    let big = info.untyped[..info.untyped_count as usize]
        .iter()
        .max_by_key(|b| b.size_bits)
        .unwrap()
        .handle;
    let desk_ut = carve(&mut s.k, big, 26, "64 MiB for the desktop");

    // The fonts, copied once into pages every app shares read only.
    let names = [
        "Figtree-Regular.ttf",
        "Figtree-SemiBold.ttf",
        "IBMPlexMono-Regular.ttf",
        "NotoSans-Regular.ttf",
    ];
    let font_pages = alloc_free::take_ro(0);
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

    // The login parts: the desktop (which draws the login screen) and the
    // store (which holds the keys). Their fingerprint goes to the desktop,
    // which compares it with the last login's (decision 74).
    let mut h = Sha256::new();
    h.update(module(info, "desktop"));
    h.update(module(info, "store"));
    let fingerprint = h.finish();

    // The desktop: the screen's shared page, the input pages, the screen
    // memory, the fonts and the store's transfer pages (channel 1).
    let g = s.dev[gpu].unwrap();
    let shared = alloc_free::take(1);
    shared.push((g.shared, dk::SCREEN_PAGE, map::WRITE));
    for (n, &i) in [keyboard, pointer].iter().enumerate() {
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
    for p in 0..fs::XFER_PAGES {
        let h = XFER_PAGES[(fs::XFER_PAGES + p) as usize].load(SeqCst);
        shared.push((h, dk::XFER + p * 4096, map::WRITE));
    }
    let ep = s.k.make(Kind::Endpoint, 0);
    let session_ntfn = s.k.make(Kind::Notification, 0);
    let store_ep = STORE_EP.load(SeqCst);
    let mut given = [0u64; dk::G_STORE_PERSON as usize + fs::MAX_PEOPLE as usize];
    given[dk::G_EP as usize] = ep;
    given[dk::G_SCREEN as usize] = g.ntfn;
    given[dk::G_SEAT as usize] = s.client;
    given[dk::G_SESSION as usize] = session_ntfn;
    for (i, person) in core::iter::once(fs::MACHINE_PERSON)
        .chain(1..=fs::MAX_PEOPLE)
        .enumerate()
    {
        let h = s.k.slot();
        alaya_sys::mint(
            s.k.at(h),
            s.k.at(store_ep),
            rights::ALL,
            fs::badge(person, fs::DESKTOP_APP, 1),
        )
        .unwrap();
        given[dk::G_STORE_MACHINE as usize + i] = h;
    }
    let mut args = [0u64; 16];
    args[dk::ARG_SCREEN_PAGES] = s.screen_pages;
    args[dk::ARG_INPUTS] = 2;
    args[dk::ARG_FONTS..dk::ARG_FONTS + 4].copy_from_slice(&sizes);
    for i in 0..4 {
        args[dk::ARG_PARTS + i] =
            u64::from_le_bytes(fingerprint[8 * i..8 * i + 8].try_into().unwrap());
        args[dk::ARG_MACHINE_KEY + i] =
            u64::from_le_bytes(MACHINE_FILES_KEY[8 * i..8 * i + 8].try_into().unwrap());
    }
    args[dk::ARG_PIN_WORK] = PIN_WORK;
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
            given: &given,
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
    let (w, h) = (load(&sp.width), load(&sp.height));
    let px = screen_pixels(w, h);
    let strip = |px: &[u32]| px[(4 * w + w / 2) as usize];
    assert!(strip(px) == BAR, "the top strip is {:06x}", strip(px));
    let mut x = Session {
        s,
        ep,
        session_ntfn,
        sizes,
        apps: [App::default(); role::COUNT as usize + 1],
        person: 0,
        asked: core::cell::Cell::new(0),
        gpu,
        keyboard,
        pointer,
    };
    say!("screenshot setup");
    let first = x.session();
    assert!(
        first[1] == stage::SETUP,
        "the desktop did not start with first setup"
    );
    assert!(
        first[5] == parts::FIRST,
        "a fresh machine has no earlier login parts"
    );
    pass("the desktop starts and asks for first setup; the login parts are remembered");
    if info.flags & alaya_sys::abi::boot_flags::DESK != 0 {
        // `cargo xtask run`: the person at the keyboard sets up and logs
        // in; the first program starts the editor once they have.
        say!("desktop: it is yours now: set up a person, then use the dock");
        return x;
    }

    // ---- First setup: a name, the PIN twice, picture 3, everything on.
    let open = || ask(ep, dk::SESSION, &[])[1] == stage::OPEN;
    let setup = "Robin{tab}2468{tab}2468{tab}{right}{right}{tab}{right}{enter}";
    assert!(x.keys(setup, open, 30_000), "first setup did not finish");
    let ss = x.session();
    x.person = ss[2];
    assert!(
        x.person == 1 && ss[3] == 1,
        "setup made person {} with look {}",
        ss[2],
        ss[3]
    );
    // The desktop says it is open; the first program starts the apps.
    assert!(
        alaya_sys::poll(session_ntfn).unwrap_or(0) & dk::LOGGED_IN_BIT != 0,
        "the desktop did not say who logged in"
    );
    pass("first setup makes a person with a PIN, a picture and a look, and opens the desktop");

    // ---- The editor, as the first app.
    x.start_app(role::EDITOR);
    let id = x.wait_window(role::EDITOR);
    let win = ask(ep, dk::WINDOW, &[id]);
    say!(
        "desktop: the editor's window is at {}, {}, {} by {}",
        win[1],
        win[2],
        win[3],
        win[4]
    );
    pass("an app opens a window and draws in it with Alaya's letters");

    // The editor's title bar to the left edge.
    let (tx, ty) = (win[1] + win[3] / 2 - 40, win[2] + 14);
    let snapped = x.drag((tx, ty), (0, ty + 200), || {
        ask(ep, dk::WINDOW, &[id])[5] == dk::place::LEFT
    });
    assert!(snapped, "the window did not snap to the left half");
    pass("dragging a window to the left edge snaps it to the left half");

    // Typing reaches the editor (the host reads the editor's report).
    let mut buf = [0u8; 512];
    let me = x.files();
    let typed = |b: &mut [u8]| return_point(&me, b).is_some_and(|(_, n)| n == 30);
    x.keys("hi", || typed(&mut [0u8; 512]), 30_000);
    assert!(typed(&mut buf), "the typed keys never reached the editor");

    // Tiling on and off.
    ask(ep, dk::TILING, &[1]);
    assert!(
        sleep_until(5000, || ask(ep, dk::WINDOW, &[id])[5] == dk::place::TILED),
        "tiling did not tile"
    );
    ask(ep, dk::TILING, &[0]);
    pass("typing reaches the window in front; tiling shares the screen, and goes back");

    // The screen driver crashes under the desktop; frames carry on.
    x.s.crash_and_restart(gpu);
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

    // ---- The lock screen: the picture in the strip, a wrong PIN, the
    // right one.
    ask(ep, dk::LOCK, &[]);
    assert!(x.session()[1] == stage::LOGIN, "the desktop did not lock");
    // The middle of the picture, right of "ALAYA" in the strip.
    let picture_px = (17 * w + 91) as usize;
    let shown = || {
        let p = screen_pixels(w, h)[picture_px];
        p != BAR && p != 0
    };
    assert!(
        sleep_until(3000, shown),
        "the person's picture is not in the strip"
    );
    say!("screenshot login");
    let wrong = x.session()[4];
    let refused = || ask(ep, dk::SESSION, &[])[4] > wrong;
    assert!(
        x.keys("1357{enter}", refused, 30_000),
        "a wrong PIN was not refused"
    );
    assert!(
        x.session()[1] == stage::LOGIN,
        "a wrong PIN opened the desktop"
    );
    assert!(
        x.keys("2468{enter}", open, 30_000),
        "the right PIN did not open the desktop"
    );
    assert!(
        sleep_until(3000, || screen_pixels(w, h)[picture_px] == BAR),
        "the picture stayed in the strip after login"
    );
    say!("login: the picture showed only with the real PIN box; a wrong PIN was refused");
    pass("the real PIN box shows your picture in the strip; a wrong PIN opens nothing");

    // ---- The editor saves, is stopped in the middle of typing, and comes
    // back where it was.
    ask(ep, dk::RAISE, &[id]);
    let saved = |b: &mut [u8]| {
        let docs = me.root(DOCS);
        me.lookup(docs, "Notes.txt")
            .and_then(|f| me.read(f, b))
            .is_some_and(|n| n == 30)
    };
    assert!(
        x.keys("{ctrl+s}", || saved(&mut [0u8; 64]), 8000),
        "Ctrl+S did not save the notes"
    );
    let grew = |b: &mut [u8]| return_point(&me, b).is_some_and(|(_, n)| n == 36);
    assert!(
        x.keys("{left}{left} more ", || grew(&mut [0u8; 512]), 30_000),
        "the editor did not keep its return point"
    );
    let (cursor, len) = return_point(&me, &mut buf).unwrap();
    let starts = x.apps[role::EDITOR as usize].starts;
    // Stopped dead, as a crash would.
    x.start_app(role::EDITOR);
    assert!(x.apps[role::EDITOR as usize].starts == starts + 1);
    let id = x.wait_window(role::EDITOR);
    ask(ep, dk::RAISE, &[id]);
    // One more letter: it lands at the cursor the editor had.
    let more = |b: &mut [u8]| return_point(&me, b).is_some_and(|(_, n)| n == len + 1);
    assert!(
        x.keys("!", || more(&mut [0u8; 512]), 30_000),
        "the editor did not come back"
    );
    let (c2, _) = return_point(&me, &mut buf).unwrap();
    assert!(
        c2 == cursor + 1,
        "the cursor came back at {c2}, not {}",
        cursor + 1
    );
    // After the header and the file's name ("Notes.txt").
    let at = 48 + 9;
    let text = core::str::from_utf8(&buf[at..at + len + 1]).unwrap_or("?");
    say!(
        "editor: back with \"{}\"",
        text.rsplit('\n').next().unwrap_or("")
    );
    assert!(
        text.ends_with("Type here. more !hi"),
        "the text came back as {text}"
    );
    pass("the editor saves, and comes back after being stopped with its text and cursor");

    // ---- The file browser and the chart maker.
    x.start_app(role::FILES);
    x.wait_window(role::FILES);
    x.start_app(role::CHART);
    let chart = x.wait_window(role::CHART);
    ask(ep, dk::RAISE, &[chart]);
    let picture = || {
        let docs = me.root(DOCS);
        me.lookup(docs, "chart.ppm")
            .and_then(|f| me.read(f, &mut [0u8; 16]))
            .is_some_and(|n| n == 15 + 480 * 300 * 3)
    };
    // The chart maker reads its numbers about a second after it starts.
    sleep_until(1500, || false);
    assert!(
        x.keys("{enter}", picture, 30_000),
        "the chart maker saved no picture"
    );
    let mut head = [0u8; 16];
    let docs = me.root(DOCS);
    me.read(me.lookup(docs, "chart.ppm").unwrap(), &mut head);
    assert!(
        head.starts_with(b"P6\n480 300\n255\n"),
        "chart.ppm is not a picture"
    );
    assert!(me.lookup(docs, "numbers.txt").is_some(), "no numbers.txt");
    pass("the file browser and the chart maker open; a chart is saved as a picture");

    // ---- Loose groups: drag the editor onto the "Work" label.
    let chip = ask(ep, dk::PLACES, &[dk::places::CHIP]);
    assert!(
        chip[0] == dk::OK,
        "no group labels in the everything-on look"
    );
    let e = ask(ep, dk::WINDOW, &[id]);
    let grouped = x.drag(
        (e[1] + e[3] / 2 - 40, e[2] + 14),
        (chip[1] + chip[3] / 2, chip[2] + chip[4] / 2),
        || ask(ep, dk::GROUP, &[id])[1] == 1,
    );
    assert!(grouped, "the window is not in the group");
    assert!(
        ask(ep, dk::WINDOW, &[id])[2] == e[2],
        "a grouped window did not go back to its place"
    );
    pass("dropping a window on a group label puts it in that loose group");

    // ---- What an app cannot do: draw on the strip, or use the first
    // program's calls.
    let fake = x.s.k.slot();
    alaya_sys::mint(x.s.k.at(fake), x.s.k.at(ep), rights::ALL, role::CHART).unwrap();
    for label in [
        dk::LOCK,
        dk::INJECT,
        dk::SESSION,
        dk::STATE,
        dk::TILING,
        dk::FORGET,
    ] {
        assert!(
            ask(fake, label, &[0, 0, 0])[0] == dk::BAD_REQUEST,
            "an app could make the call {label}"
        );
    }
    for i in 1..64 {
        let r = ask(ep, dk::WINDOW, &[i]);
        assert!(
            r[0] != dk::OK || r[2] >= alaya_ui_strip(),
            "window {i} covers the strip"
        );
    }
    assert!(
        strip(screen_pixels(w, h)) == BAR,
        "something drew on the strip"
    );
    pass("apps cannot draw on the strip or use the desktop's own calls");

    say!("screenshot apps");
    // ---- Settings changes the look, and the dock follows.
    x.start_app(role::SETTINGS);
    let set = x.wait_window(role::SETTINGS);
    ask(ep, dk::RAISE, &[set]);
    let basics = || ask(ep, dk::SESSION, &[])[3] == 0;
    assert!(
        x.keys("{enter}", basics, 30_000),
        "Settings did not change the look"
    );
    assert!(
        ask(ep, dk::PLACES, &[dk::places::CHIP])[0] != dk::OK,
        "the group labels stayed in the plain look"
    );
    pass("Settings changes the look, and the desktop follows at once");

    // ---- The keyboard driver crashes while people type: it comes back.
    ask(ep, dk::RAISE, &[id]);
    x.s.crash_and_restart(keyboard);
    let ip = input_page(&x.s.dev[keyboard].unwrap());
    assert!(
        sleep_until(5000, || load(&ip.state) == alaya_programs::run::RUNNING),
        "the keyboard driver did not come back"
    );
    let n = return_point(&me, &mut buf).unwrap().1;
    let after = |b: &mut [u8]| return_point(&me, b).is_some_and(|(_, m)| m == n + 1);
    assert!(
        x.keys("?", || after(&mut [0u8; 512]), 30_000),
        "keys stopped after the keyboard driver came back"
    );
    pass("the keyboard driver crashes under the desktop; typing carries on");
    let _ = (x.pointer, x.keyboard);
    crate::m6c::run(&mut x, pass);
    crate::m7::run(&mut x, pass);
    x
}

/// The top of the area windows may use (the strip's height).
fn alaya_ui_strip() -> u64 {
    34
}

impl Session<'_> {
    /// The first program's own badge on the store, as the person logged
    /// in, on channel 0 (the checks of milestone 5 are over).
    pub(crate) fn files(&mut self) -> Files {
        let p = self.person;
        self.files_of(p)
    }

    /// The same, as person `p` (the checks of milestone 7 make another).
    pub(crate) fn files_of(&mut self, p: u64) -> Files {
        let h = self.s.k.slot();
        alaya_sys::mint(
            self.s.k.at(h),
            self.s.k.at(STORE_EP.load(SeqCst)),
            rights::ALL,
            fs::badge(p, fs::INIT_APP, 0),
        )
        .unwrap();
        Files { ep: h }
    }
}

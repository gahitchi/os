// SPDX-License-Identifier: GPL-3.0-only
//! The desktop: Alaya's compositor (milestone 6).
//!
//! It owns the screen memory and draws the screen: wallpaper, windows,
//! the dock, the top strip only it may draw on, and the pointer. Apps
//! draw into their own window pictures, which the desktop lends them, and
//! say what changed; the desktop copies the visible parts to the screen.
//! Keys go only to the window in front, the mouse only to the window
//! under it (or the one that holds it). The window rules (move, resize,
//! snap, tile) are in `alaya_ui::wm`, tested on the build machine.
//!
//! One thread does everything, woken by an app's call or by a signal: the
//! screen driver showed a frame, input arrived, or the clock ticked.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_programs::desktop::{self as dk, event, place};
use alaya_programs::seat::{self, InputPage, ScreenPage, ev, unpack};
use alaya_programs::{load, store};
use alaya_sys::abi::{StartInfo, call, map, start};
use alaya_sys::rt::{self, Mem};
use alaya_sys::{Caps, Kind, Msg, MsgInfo, rights, say};
use alaya_ui::Rect;
use alaya_ui::canvas::Canvas;
use alaya_ui::scene::{self, Desktop, DockItem, WindowView};
use alaya_ui::text::Fonts;
use alaya_ui::wm::{Out, Place, Wm};

alaya_sys::heap!();
alaya_sys::entry!(main);

/// Signal bit of the clock (the drivers use the low bits).
const CLOCK_BIT: u64 = 1 << 8;
const MAX_WINDOWS: usize = 12;
const MAX_INPUTS: usize = 4;
use alaya_programs::desktop::{PICTURE_STEP, PICTURES};

struct Win {
    id: u32,
    owner: u64,
    ntfn: u64,
    pages: [u64; dk::PAGES as usize],
    va: u64,
    title: String,
    events: VecDeque<[u64; 5]>,
    content: (i32, i32),
}

struct Desk {
    mem: Mem,
    screen: &'static ScreenPage,
    screen_ntfn: u64,
    width: i32,
    height: i32,
    inputs: [Option<(&'static InputPage, u64)>; MAX_INPUTS],
    wm: Wm,
    wins: Vec<Win>,
    next_id: u32,
    free_slots: [bool; MAX_WINDOWS],
    dirty: Rect,
    /// A frame was posted and is not shown yet.
    in_flight: bool,
    frames: u64,
    shift: bool,
    super_key: bool,
    abs: (u32, u32),
    abs_moved: bool,
    clock: String,
    fonts: Fonts<'static>,
    recv_slot: u64,
}

fn font(info: &StartInfo, i: usize) -> &'static [u8] {
    let len = info.args[dk::ARG_FONTS + i] as usize;
    // SAFETY: the first program shares each font's pages here, read only.
    unsafe { core::slice::from_raw_parts((dk::FONTS + i as u64 * dk::FONT_STEP) as *const u8, len) }
}

fn main(info: &'static StartInfo) -> ! {
    let g = start::FIRST_GIVEN;
    let mut mem = Mem::new(info);
    alaya_sys::heap::init(&mut mem, 4).expect("heap");
    // SAFETY: shared by the first program before starting us.
    let screen = unsafe { &*(dk::SCREEN_PAGE as *const ScreenPage) };
    let mut inputs = [None; MAX_INPUTS];
    for (i, slot) in inputs
        .iter_mut()
        .enumerate()
        .take((info.args[dk::ARG_INPUTS] as usize).min(MAX_INPUTS))
    {
        // SAFETY: as above.
        let p = unsafe { &*((dk::INPUT_PAGES + i as u64 * 4096) as *const InputPage) };
        *slot = Some((p, load(&p.head)));
    }
    let seat_ntfn = g + dk::G_SEAT;
    alaya_sys::thread_bind(start::THREAD, seat_ntfn).expect("bind");
    rt::start_clock(&mut mem, seat_ntfn, CLOCK_BIT, 50, info.priority).expect("clock");
    let (width, height) = (load(&screen.width) as i32, load(&screen.height) as i32);
    let fonts = Fonts::new(font(info, 0), font(info, 1), font(info, 2), font(info, 3));
    let recv_slot = mem.slot().expect("slot");
    let mut d = Desk {
        mem,
        screen,
        screen_ntfn: g + dk::G_SCREEN,
        width,
        height,
        inputs,
        wm: Wm::new(width, height),
        wins: Vec::new(),
        next_id: 1,
        free_slots: [true; MAX_WINDOWS],
        dirty: Rect::new(0, 0, width, height),
        in_flight: false,
        frames: 0,
        shift: false,
        super_key: false,
        abs: (0, 0),
        abs_moved: false,
        clock: String::from("0:00"),
        fonts,
        recv_slot,
    };
    d.tick();
    say!("drawing a {width} by {height} desktop");
    d.frame();
    d.serve(g + dk::G_EP)
}

fn screen_px(w: i32, h: i32) -> &'static mut [u32] {
    // SAFETY: the screen memory is mapped here, at least w * h pixels.
    unsafe { core::slice::from_raw_parts_mut(dk::SCREEN as *mut u32, (w * h) as usize) }
}

impl Desk {
    fn serve(&mut self, ep: u64) -> ! {
        let reply = self.mem.make(Kind::Reply, 0).expect("reply");
        loop {
            let got = alaya_sys::msg_call(
                call::RECV,
                ep,
                &Msg::default(),
                Caps {
                    reply,
                    recv: self.recv_slot,
                    send: 0,
                },
            );
            match got {
                Err(_) => {}
                Ok(m) if m.info.has(MsgInfo::SIGNAL) => self.on_signal(m.badge),
                Ok(m) => {
                    let (answer, cap) = self.on_call(&m);
                    let info = if cap != 0 {
                        MsgInfo::new(0, 6).with_cap()
                    } else {
                        MsgInfo::new(0, 6)
                    };
                    let _ = alaya_sys::msg_call(
                        call::SEND,
                        reply,
                        &Msg {
                            info,
                            badge: 0,
                            words: answer,
                        },
                        Caps {
                            send: cap,
                            ..Caps::default()
                        },
                    );
                    // The app has its own copy now; what it holds stays
                    // under the desktop's page, which can take it back.
                    if cap != 0 {
                        let _ = alaya_sys::delete(self.mem.at(cap));
                    }
                }
            }
            self.frame();
        }
    }

    fn on_signal(&mut self, bits: u64) {
        if bits & seat::SHOWN_BIT != 0 && load(&self.screen.shown) == load(&self.screen.posted) {
            self.in_flight = false;
        }
        if bits & seat::INPUT_BIT != 0 {
            self.read_input();
        }
        if bits & CLOCK_BIT != 0 {
            self.tick();
        }
        // A restarted screen driver shows the whole screen itself; a
        // frame lost in a crash is drawn again.
        if self.in_flight && load(&self.screen.shown) == load(&self.screen.posted) {
            self.in_flight = false;
        }
    }

    fn tick(&mut self) {
        let s = rt::now_ms() / 1000;
        let text = alloc::format!("{}:{:02}", s / 3600, s / 60 % 60);
        if text != self.clock {
            self.clock = text;
            self.damage(Rect::new(self.width - 120, 0, 120, alaya_ui::wm::STRIP));
        }
    }

    fn damage(&mut self, r: Rect) {
        self.dirty = self
            .dirty
            .union(r.intersect(Rect::new(0, 0, self.width, self.height)));
    }

    /// Draw what changed and hand it to the screen driver, unless a frame
    /// is still on its way.
    fn frame(&mut self) {
        if self.in_flight || self.dirty.is_empty() {
            return;
        }
        let clip = self.dirty;
        self.dirty = Rect::default();
        let (w, h) = (self.width, self.height);
        let mut cv = Canvas::new(screen_px(w, h), w, h, w).expect("screen");
        let wins = &self.wins;
        let views = |id: u32| match wins.iter().find(|x| x.id == id) {
            Some(x) => WindowView {
                title: &x.title,
                pixels: Some((
                    // SAFETY: the window's pages are mapped at `va`.
                    unsafe {
                        core::slice::from_raw_parts(
                            x.va as *const u32,
                            (dk::STRIDE * dk::MAX_H) as usize,
                        )
                    },
                    x.content.0,
                    x.content.1,
                    dk::STRIDE as i32,
                )),
                stuck: false,
            },
            None => WindowView {
                title: "",
                pixels: None,
                stuck: false,
            },
        };
        let dock: Vec<DockItem> = DOCK
            .iter()
            .map(|(name, color)| DockItem {
                name,
                color: *color,
                running: wins.iter().any(|x| x.title.starts_with(name)),
            })
            .collect();
        let desk = Desktop {
            clock: &self.clock,
            status: "",
            dock: &dock,
            show_pointer: true,
        };
        scene::draw(&mut cv, &mut self.fonts, &self.wm, &views, &desk, clip);
        let s = self.screen;
        store(&s.dirty[0], clip.x as u64);
        store(&s.dirty[1], clip.y as u64);
        store(&s.dirty[2], clip.w as u64);
        store(&s.dirty[3], clip.h as u64);
        store(&s.posted, load(&s.posted) + 1);
        self.in_flight = true;
        self.frames += 1;
        let _ = alaya_sys::signal(self.screen_ntfn, seat::FRAME_BIT);
    }

    fn read_input(&mut self) {
        for i in 0..MAX_INPUTS {
            let Some((page, mut tail)) = self.inputs[i] else {
                continue;
            };
            let head = load(&page.head);
            // Events that were overwritten before we read them are lost.
            if head - tail > seat::EVENTS as u64 {
                tail = head - seat::EVENTS as u64;
            }
            while tail < head {
                let e = unpack(load(&page.events[tail as usize % seat::EVENTS]));
                tail += 1;
                self.on_event(e);
            }
            self.inputs[i] = Some((page, tail));
        }
    }

    fn on_event(&mut self, (kind, code, value): (u16, u16, u32)) {
        match kind {
            ev::ABS => {
                let v = value.min(ev::ABS_MAX);
                if code == ev::ABS_X {
                    self.abs.0 = v;
                } else if code == ev::ABS_Y {
                    self.abs.1 = v;
                }
                self.abs_moved = true;
            }
            ev::SYN => {
                if self.abs_moved {
                    self.abs_moved = false;
                    let x = (u64::from(self.abs.0) * self.width as u64
                        / (u64::from(ev::ABS_MAX) + 1)) as i32;
                    let y = (u64::from(self.abs.1) * self.height as u64
                        / (u64::from(ev::ABS_MAX) + 1)) as i32;
                    let out = self.wm.pointer_move(x, y);
                    self.apply(out);
                }
            }
            ev::KEY if code == ev::BTN_LEFT || code == ev::BTN_RIGHT => {
                // Apply a pending move first, so the press lands where the
                // pointer is.
                self.on_event((ev::SYN, 0, 0));
                let b = if code == ev::BTN_LEFT { 1 } else { 2 };
                let down = value != 0;
                let out = self.wm.button(b, down);
                self.apply(out);
            }
            ev::KEY => self.on_key(code, value),
            _ => {}
        }
    }

    fn on_key(&mut self, code: u16, value: u32) {
        match code {
            42 | 54 => self.shift = value != 0,
            125 | 126 => self.super_key = value != 0,
            _ => {}
        }
        if self.super_key && value == 1 {
            // Window keys: Super with an arrow snaps, Super+T tiles.
            if let Some(id) = self.wm.focused() {
                let out = match code {
                    105 => self.wm.place(id, Place::Left),
                    106 => self.wm.place(id, Place::Right),
                    103 => self.wm.place(id, Place::Full),
                    108 => self.wm.place(id, Place::Free),
                    20 => {
                        let on = !self.wm.tiling;
                        self.wm.set_tiling(on)
                    }
                    _ => Vec::new(),
                };
                self.apply(out);
            }
            return;
        }
        if let Some(id) = self.wm.focused() {
            self.push(
                id,
                [
                    event::KEY,
                    u64::from(code),
                    u64::from(value),
                    u64::from(self.shift),
                    0,
                ],
            );
        }
    }

    fn push(&mut self, id: u32, e: [u64; 5]) {
        if let Some(w) = self.wins.iter_mut().find(|w| w.id == id) {
            // A window that never collects its events keeps only the
            // newest ones.
            if w.events.len() >= 256 {
                w.events.pop_front();
            }
            w.events.push_back(e);
            let _ = alaya_sys::signal(w.ntfn, dk::EVENT_BIT);
        }
    }

    fn apply(&mut self, out: Vec<Out>) {
        for o in out {
            match o {
                Out::Damage(r) => self.damage(r),
                Out::Focus(id) => {
                    for w in self.wins.iter().map(|w| w.id).collect::<Vec<_>>() {
                        self.push(w, [event::FOCUS, u64::from(w == id), 0, 0, 0]);
                    }
                }
                Out::Resized(id, w, h) => {
                    let (w, h) = (w.min(dk::MAX_W as i32), h.min(dk::MAX_H as i32));
                    if let Some(x) = self.wins.iter_mut().find(|x| x.id == id) {
                        x.content = (w, h);
                    }
                    self.push(id, [event::RESIZED, w as u64, h as u64, 0, 0]);
                    if let Some(win) = self.wm.get(id) {
                        let p = match win.place {
                            Place::Free => "free",
                            Place::Left => "snapped left",
                            Place::Right => "snapped right",
                            Place::Full => "full screen",
                            Place::Tiled => "tiled",
                        };
                        say!("window {id} is {p}, {w} by {h}");
                    }
                }
                Out::Close(id) => self.push(id, [event::CLOSE, 0, 0, 0, 0]),
                Out::Pointer {
                    id,
                    x,
                    y,
                    button,
                    down,
                    moved,
                } => {
                    let held = if down { 1 } else { 0 };
                    self.push(
                        id,
                        [
                            event::POINTER,
                            x as u64,
                            y as u64,
                            u64::from(button) | held << 8,
                            u64::from(moved),
                        ],
                    );
                }
            }
        }
    }

    fn on_call(&mut self, m: &Msg) -> ([u64; 6], u64) {
        let w = m.words;
        let owner = m.badge;
        let find = |s: &Self, id: u64| {
            s.wins
                .iter()
                .position(|x| u64::from(x.id) == id && x.owner == owner)
        };
        match m.label() {
            dk::OPEN => {
                if !m.info.has(MsgInfo::CAP) {
                    return ([dk::BAD_REQUEST, 0, 0, 0, 0, 0], 0);
                }
                let Some(slot) = self.free_slots.iter().position(|f| *f) else {
                    return ([dk::NO_MEMORY, 0, 0, 0, 0, 0], 0);
                };
                let ntfn = self.recv_slot;
                self.recv_slot = match self.mem.slot() {
                    Ok(s) => s,
                    Err(_) => return ([dk::NO_MEMORY, 0, 0, 0, 0, 0], 0),
                };
                let va = PICTURES + slot as u64 * PICTURE_STEP;
                let mut pages = [0u64; dk::PAGES as usize];
                for (i, p) in pages.iter_mut().enumerate() {
                    let Ok(h) = self.mem.make(Kind::Page, 21) else {
                        return ([dk::NO_MEMORY, 0, 0, 0, 0, 0], 0);
                    };
                    alaya_sys::page_map(h, start::SPACE, va + i as u64 * (2 << 20), map::WRITE)
                        .expect("map a picture page");
                    *p = h;
                }
                self.free_slots[slot] = false;
                let id = self.next_id;
                self.next_id += 1;
                let cw = (w[0] as i32).clamp(1, dk::MAX_W as i32);
                let ch = (w[1] as i32).clamp(1, dk::MAX_H as i32);
                self.wins.push(Win {
                    id,
                    owner,
                    ntfn,
                    pages,
                    va,
                    title: String::new(),
                    events: VecDeque::new(),
                    content: (cw, ch),
                });
                let out = self.wm.add(id, cw, ch);
                self.apply(out);
                say!("window {id} opened, {cw} by {ch}");
                ([dk::OK, u64::from(id), 0, 0, 0, 0], 0)
            }
            dk::PAGE => match find(self, w[0]) {
                Some(i) if w[1] < dk::PAGES => {
                    // Lend a copy of the page; the desktop keeps its own.
                    let c = match self.mem.slot() {
                        Ok(c) => c,
                        Err(_) => return ([dk::NO_MEMORY, 0, 0, 0, 0, 0], 0),
                    };
                    let p = self.wins[i].pages[w[1] as usize];
                    let _ = alaya_sys::delete(self.mem.at(c));
                    alaya_sys::copy(self.mem.at(c), self.mem.at(p), rights::ALL).expect("copy");
                    ([dk::OK, 0, 0, 0, 0, 0], c)
                }
                _ => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::TITLE => match find(self, w[0]) {
                Some(i) => {
                    let len = (w[1] as usize).min(32);
                    let mut b = [0u8; 32];
                    for (k, word) in w[2..6].iter().enumerate() {
                        b[k * 8..k * 8 + 8].copy_from_slice(&word.to_le_bytes());
                    }
                    let t = core::str::from_utf8(&b[..len]).unwrap_or("?");
                    self.wins[i].title = String::from(t);
                    if let Some(win) = self.wm.get(self.wins[i].id) {
                        let r = win.title_bar();
                        self.damage(r);
                    }
                    self.damage(scene::dock_rect(self.width, self.height, DOCK.len()));
                    ([dk::OK, 0, 0, 0, 0, 0], 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::DAMAGE => match find(self, w[0]) {
                Some(i) => {
                    if let Some(win) = self.wm.get(self.wins[i].id) {
                        let c = win.content();
                        let r = Rect::new(
                            c.x + w[1] as i32,
                            c.y + w[2] as i32,
                            (w[3] as i32).min(c.w),
                            (w[4] as i32).min(c.h),
                        )
                        .intersect(c);
                        self.damage(r);
                    }
                    ([dk::OK, 0, 0, 0, 0, 0], 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::EVENT => match find(self, w[0]) {
                Some(i) => match self.wins[i].events.pop_front() {
                    Some(e) => ([dk::OK, e[0], e[1], e[2], e[3], e[4]], 0),
                    None => ([dk::EMPTY, 0, 0, 0, 0, 0], 0),
                },
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::SIZE => match find(self, w[0]) {
                Some(i) => {
                    let (cw, ch) = self.wins[i].content;
                    ([dk::OK, cw as u64, ch as u64, 0, 0, 0], 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::CLOSE => match find(self, w[0]) {
                Some(i) => {
                    let x = self.wins.remove(i);
                    let out = self.wm.remove(x.id);
                    self.apply(out);
                    // Take the pages back from the app, then free them.
                    for p in x.pages {
                        let _ = alaya_sys::revoke(self.mem.at(p));
                        let _ = alaya_sys::page_unmap(p);
                        let _ = alaya_sys::delete(self.mem.at(p));
                    }
                    let _ = alaya_sys::delete(self.mem.at(x.ntfn));
                    let slot = ((x.va - PICTURES) / PICTURE_STEP) as usize;
                    self.free_slots[slot] = true;
                    self.damage(scene::dock_rect(self.width, self.height, DOCK.len()));
                    say!("window {} closed", x.id);
                    ([dk::OK, 0, 0, 0, 0, 0], 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::TILING => {
                let out = self.wm.set_tiling(w[0] != 0);
                self.apply(out);
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
            dk::STATE => (
                [
                    dk::OK,
                    self.wins.len() as u64,
                    u64::from(self.wm.focused().unwrap_or(0)),
                    u64::from(self.wm.tiling),
                    self.frames,
                    0,
                ],
                0,
            ),
            dk::WINDOW => match self.wm.get(w[0] as u32) {
                Some(win) => {
                    let f = win.frame;
                    let p = match win.place {
                        Place::Free => place::FREE,
                        Place::Left => place::LEFT,
                        Place::Right => place::RIGHT,
                        Place::Full => place::FULL,
                        Place::Tiled => place::TILED,
                    };
                    (
                        [dk::OK, f.x as u64, f.y as u64, f.w as u64, f.h as u64, p],
                        0,
                    )
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            _ => ([dk::BAD_REQUEST, 0, 0, 0, 0, 0], 0),
        }
    }
}

/// The dock: the apps of milestone 6, in the mock-up's colours.
const DOCK: [(&str, u32); 4] = [
    ("Files", 0x23_68_6b),
    ("Text editor", 0x2d_5c_9c),
    ("Chart maker", 0x9a_5b_00),
    ("Settings", 0x5b_6b_6c),
];

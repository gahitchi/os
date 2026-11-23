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
//! Before anything else it shows first setup or the login screen
//! (`alaya_ui::login`): it turns the PIN into a key with the slow PIN step
//! and opens the person's files in the store with it. While its PIN box is
//! open, the person's picture shows in the top strip, which no app can
//! draw on (decision 104). Before asking, it compares the login parts with
//! those of the last login and says what it found (decision 74).
//!
//! One thread does everything, woken by an app's call or by a signal: the
//! screen driver showed a frame, input arrived, or the clock ticked.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::Store;
use alaya_crypto::{pin_key, sha256};
use alaya_programs::desktop::{self as dk, event, parts, place, role, stage};
use alaya_programs::files::{self as fs, APPS, DOCS};
use alaya_programs::seat::{self, InputPage, ScreenPage, ev, unpack};
use alaya_programs::{load, store};
use alaya_sys::abi::{StartInfo, call, map, start};
use alaya_sys::rt::{self, Mem};
use alaya_sys::{Caps, Kind, Msg, MsgInfo, rights, say};
use alaya_ui::Rect;
use alaya_ui::canvas::Canvas;
use alaya_ui::login::{self, Login, Look, NewPerson, Person, Setup};
use alaya_ui::scene::{self, Desktop, DockItem, GroupChip, WindowView};
use alaya_ui::text::Fonts;
use alaya_ui::widgets::{Key, us_key};
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

/// What the screen shows.
enum Stage {
    Setup(Setup),
    Login(Login),
    /// The desktop of the person logged in (an index in `people`).
    Open(usize),
}

struct Desk {
    mem: Mem,
    stage: Stage,
    people: Vec<Person>,
    /// The person whose desktop is behind the login screen, if any.
    session: Option<usize>,
    look: Look,
    /// Fingerprints of the keys that opened each person's files in this
    /// boot (the store keeps them open; a lock screen checks against
    /// these).
    opened: Vec<(u64, [u8; 32])>,
    wrong_pins: u64,
    parts: u64,
    parts_now: [u8; 32],
    machine: Store,
    machine_docs: u64,
    machine_key: [u8; 32],
    pin_work: usize,
    session_ntfn: u64,
    ctrl: bool,
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
    alaya_sys::heap::init(&mut mem, 6).expect("heap");
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
    let word_bytes = |at: usize| {
        let mut b = [0u8; 32];
        for i in 0..4 {
            b[8 * i..8 * i + 8].copy_from_slice(&info.args[at + i].to_le_bytes());
        }
        b
    };
    let machine_key = word_bytes(dk::ARG_MACHINE_KEY);
    let parts_now = word_bytes(dk::ARG_PARTS);
    // The machine's own files: who can log in, and the login parts.
    let machine = Store::new(g + dk::G_STORE_MACHINE);
    if machine.unlock(&machine_key) == Err(fs::st::NOT_FOUND) {
        machine
            .add_person(&machine_key)
            .expect("the machine's own files");
        machine.save().expect("save");
    }
    let machine_docs = machine.root(DOCS).expect("the machine's documents");
    let people: Vec<Person> = machine
        .lookup(machine_docs, "people")
        .and_then(|f| machine.read_all(f))
        .map(|b| b.chunks(64).filter_map(login::decode_person).collect())
        .unwrap_or_default();
    let last = machine
        .lookup(machine_docs, "login-parts")
        .and_then(|f| machine.read_all(f))
        .ok();
    let parts = match last {
        None => parts::FIRST,
        Some(b) if b[..] == parts_now[..] => parts::SAME,
        Some(_) => parts::CHANGED,
    };
    let stage = if people.is_empty() {
        say!("first setup: nobody can log in yet");
        Stage::Setup(Setup::new())
    } else {
        Stage::Login(Login::new(people.clone(), notice(parts)))
    };
    let mut d = Desk {
        mem,
        stage,
        people,
        session: None,
        look: Look::Basics,
        opened: Vec::new(),
        wrong_pins: 0,
        parts,
        parts_now,
        machine,
        machine_docs,
        machine_key,
        pin_work: (info.args[dk::ARG_PIN_WORK] as usize).max(1),
        session_ntfn: g + dk::G_SESSION,
        ctrl: false,
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
    say!(
        "drawing a {width} by {height} desktop; login parts: {}",
        match parts {
            parts::FIRST => "first login, remembered from now on",
            parts::SAME => "the same as at the last login",
            _ => "CHANGED since the last login",
        }
    );
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
        let wm = &self.wm;
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
                group: wm
                    .get(id)
                    .and_then(|w| w.group)
                    .and_then(|g| GROUPS.get(g as usize - 1))
                    .map(|&(n, c)| (n, c)),
            },
            None => WindowView {
                title: "",
                pixels: None,
                stuck: false,
                group: None,
            },
        };
        let dock: Vec<DockItem> = dock_items(self.look)
            .map(|(name, color, r)| DockItem {
                name,
                color,
                running: wins.iter().any(|x| x.owner == r),
            })
            .collect();
        let groups: Vec<GroupChip> = match self.look {
            Look::Basics => Vec::new(),
            Look::Everything => GROUPS
                .iter()
                .enumerate()
                .map(|(i, &(name, color))| GroupChip {
                    name,
                    color,
                    count: wm
                        .wins
                        .iter()
                        .filter(|w| w.group == Some(i as u32 + 1))
                        .count(),
                })
                .collect(),
        };
        let locked = !matches!(self.stage, Stage::Open(_));
        let trusted = match &self.stage {
            Stage::Login(l) => l.picture(),
            _ => None,
        };
        let status = match self.stage {
            Stage::Setup(_) => "First setup",
            _ => "",
        };
        let desk = Desktop {
            clock: &self.clock,
            status,
            dock: &dock,
            groups: &groups,
            show_pointer: false,
            locked,
            trusted,
        };
        scene::draw(&mut cv, &mut self.fonts, &self.wm, &views, &desk, clip);
        let old = cv.set_clip(clip);
        match &self.stage {
            Stage::Setup(f) => f.draw(&mut cv, &mut self.fonts),
            Stage::Login(f) => f.draw(&mut cv, &mut self.fonts),
            Stage::Open(_) => {}
        }
        let (px, py) = self.wm.pointer;
        scene::draw_pointer(&mut cv, px, py);
        cv.set_clip(old);
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
                    if self.locked() {
                        // Nothing behind the login screen sees the mouse.
                        let (ox, oy) = self.wm.pointer;
                        self.wm.pointer = (x, y);
                        self.damage(scene::pointer_rect(ox, oy));
                        self.damage(scene::pointer_rect(x, y));
                    } else {
                        let out = self.wm.pointer_move(x, y);
                        self.apply(out);
                    }
                }
            }
            ev::KEY if code == ev::BTN_LEFT || code == ev::BTN_RIGHT => {
                // Apply a pending move first, so the press lands where the
                // pointer is.
                self.on_event((ev::SYN, 0, 0));
                let b = if code == ev::BTN_LEFT { 1 } else { 2 };
                let down = value != 0;
                let (x, y) = self.wm.pointer;
                if self.locked() {
                    if down && b == 1 {
                        self.form_click(x, y);
                    }
                    return;
                }
                if down && b == 1 && self.shell_click(x, y) {
                    return;
                }
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
            29 | 97 => self.ctrl = value != 0,
            125 | 126 => self.super_key = value != 0,
            _ => {}
        }
        if self.locked() {
            // Keys go to the login screen, and to nothing else.
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.form_key(k);
            }
            return;
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
                    u64::from(self.ctrl),
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
                Out::Grouped(id, g) => {
                    let name = GROUPS.get(g as usize - 1).map_or("?", |x| x.0);
                    say!("window {id} is in the group {name}");
                    self.damage_all();
                }
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

    fn locked(&self) -> bool {
        !matches!(self.stage, Stage::Open(_))
    }

    fn damage_all(&mut self) {
        self.damage(Rect::new(0, 0, self.width, self.height));
    }

    fn form_key(&mut self, k: Key) {
        let shift = self.shift;
        let (w, h) = (self.width, self.height);
        match &mut self.stage {
            Stage::Setup(f) => {
                if let Some(p) = f.key(k, shift) {
                    self.finish_setup(p);
                }
            }
            Stage::Login(f) => {
                if let Some(a) = f.key(k) {
                    self.try_login(a.person, &a.pin);
                }
            }
            Stage::Open(_) => return,
        }
        self.damage(login::panel(w, h).grow(24));
    }

    fn form_click(&mut self, x: i32, y: i32) {
        let (w, h) = (self.width, self.height);
        match &mut self.stage {
            Stage::Setup(f) => {
                if let Some(p) = f.click(x, y, w, h) {
                    self.finish_setup(p);
                }
            }
            Stage::Login(f) => f.click(x, y, w, h),
            Stage::Open(_) => return,
        }
        // The picture in the strip follows the person chosen.
        self.damage_all();
    }

    /// The slow PIN step, for person `id`.
    fn key_for(&mut self, id: u64, pin: &str) -> [u8; 32] {
        // Show "Checking your PIN" while it runs.
        self.frame();
        let mut salt = [0u8; 48];
        salt[..32].copy_from_slice(&sha256(&self.machine_key));
        salt[32..40].copy_from_slice(&id.to_le_bytes());
        salt[40..].copy_from_slice(b"pin salt");
        let mut work = alloc::vec![[0u8; 32]; self.pin_work];
        let t0 = rt::now_ms();
        let key = pin_key(pin.as_bytes(), &salt, &mut work);
        say!(
            "the PIN step took {} ms and {} KiB",
            rt::now_ms() - t0,
            self.pin_work * 32 / 1024
        );
        key
    }

    fn person_store(&self, id: u64) -> Store {
        Store::new(start::FIRST_GIVEN + dk::G_STORE_PERSON + id - 1)
    }

    fn finish_setup(&mut self, p: NewPerson) {
        let id = self.people.len() as u64 + 1;
        if id > fs::MAX_PEOPLE {
            return;
        }
        let key = self.key_for(id, &p.pin);
        let store = self.person_store(id);
        store.add_person(&key).expect("add the person");
        // Their folders are made by the next save.
        store.save().expect("save");
        let person = Person {
            id,
            name: p.name,
            picture: p.picture,
        };
        self.people.push(person);
        let mut all = Vec::new();
        for x in &self.people {
            all.extend_from_slice(&login::encode_person(x));
        }
        let m = self.machine;
        m.open(self.machine_docs, "people")
            .and_then(|f| m.write_all(f, &all))
            .expect("write who can log in");
        if let Ok(apps) = store.root(APPS)
            && let Ok(f) = store.open(apps, "look")
        {
            let _ = store.write_all(f, &[p.look.code()]);
        }
        say!(
            "first setup done: person {id}, picture {}, look: {}",
            p.picture,
            p.look.name()
        );
        self.opened.push((id, sha256(&key)));
        self.open_session(self.people.len() - 1);
    }

    fn try_login(&mut self, index: usize, pin: &str) {
        let Some(id) = self.people.get(index).map(|p| p.id) else {
            return;
        };
        let key = self.key_for(id, pin);
        let fp = sha256(&key);
        let ok = match self.opened.iter().find(|(p, _)| *p == id) {
            // Open already in this boot: the store would not check again.
            Some((_, known)) => alaya_crypto::same(known, &fp),
            None => match self.person_store(id).unlock(&key) {
                Ok(()) => {
                    self.opened.push((id, fp));
                    true
                }
                Err(e) => {
                    if e != fs::st::WRONG_KEY {
                        say!("the store could not open person {id}'s files: {e}");
                    }
                    false
                }
            },
        };
        if ok {
            say!("person {id} logged in");
            self.open_session(index);
        } else {
            self.wrong_pins += 1;
            say!("a wrong PIN for person {id} ({} so far)", self.wrong_pins);
            if let Stage::Login(f) = &mut self.stage {
                f.refused();
            }
        }
    }

    fn open_session(&mut self, index: usize) {
        // The login parts as they are now are what the next login compares
        // with.
        let m = self.machine;
        let _ = m
            .open(self.machine_docs, "login-parts")
            .and_then(|f| m.write_all(f, &self.parts_now));
        let _ = m.save();
        let first = self.session.is_none();
        self.session = Some(index);
        self.stage = Stage::Open(index);
        self.read_look();
        if first {
            let _ = alaya_sys::signal(self.session_ntfn, dk::LOGGED_IN_BIT);
        }
        self.damage_all();
    }

    fn lock(&mut self) {
        // Let go of whatever the mouse held.
        let (x, y) = self.wm.pointer;
        let out = self.wm.button(1, false);
        self.apply(out);
        let out = self.wm.pointer_move(x, y);
        self.apply(out);
        let mut l = Login::new(self.people.clone(), notice(self.parts));
        if let Some(i) = self.session {
            l.chosen = i;
        }
        self.stage = Stage::Login(l);
        self.damage_all();
        say!("locked");
    }

    fn read_look(&mut self) {
        let Some(i) = self.session else { return };
        let s = self.person_store(self.people[i].id);
        let code = s
            .root(APPS)
            .and_then(|a| s.lookup(a, "look"))
            .and_then(|f| s.read_all(f))
            .ok()
            .and_then(|b| b.first().copied())
            .unwrap_or(0);
        let look = Look::from_code(code);
        if look != self.look {
            self.look = look;
            say!("the look is {}", look.name());
        }
        self.wm.groups = match look {
            Look::Basics => Vec::new(),
            Look::Everything => (0..GROUPS.len())
                .map(|g| {
                    (
                        g as u32 + 1,
                        scene::group_chip(
                            self.width,
                            self.height,
                            dock_items(look).count(),
                            GROUPS.len(),
                            g,
                        ),
                    )
                })
                .collect(),
        };
        self.damage_all();
    }

    /// A click on the dock or a group label; true if it was one.
    fn shell_click(&mut self, x: i32, y: i32) -> bool {
        let n = dock_items(self.look).count();
        for (i, (name, _, r)) in dock_items(self.look).enumerate() {
            if scene::dock_item(self.width, self.height, n, i).contains(x, y) {
                match self.wins.iter().rev().find(|w| w.owner == r).map(|w| w.id) {
                    Some(id) => {
                        let mut out = Vec::new();
                        self.wm.raise(id, &mut out);
                        self.apply(out);
                    }
                    None => {
                        // Ask the first program to start it.
                        say!("starting {name}");
                        let _ = alaya_sys::signal(self.session_ntfn, 1 << r);
                    }
                }
                return true;
            }
        }
        if let Some(&(g, _)) = self.wm.groups.iter().find(|(_, r)| r.contains(x, y)) {
            let out = self.wm.raise_group(g);
            self.apply(out);
            return true;
        }
        false
    }

    fn forget(&mut self, owner: u64) {
        let ids: Vec<u32> = self
            .wins
            .iter()
            .filter(|w| w.owner == owner)
            .map(|w| w.id)
            .collect();
        for id in ids {
            self.close_window(id);
        }
    }

    fn close_window(&mut self, id: u32) {
        let Some(i) = self.wins.iter().position(|w| w.id == id) else {
            return;
        };
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
        self.damage_all();
        say!("window {} closed", x.id);
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
                    let n = dock_items(self.look).count();
                    self.damage(scene::dock_rect(self.width, self.height, n));
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
                    let id = self.wins[i].id;
                    self.close_window(id);
                    ([dk::OK, 0, 0, 0, 0, 0], 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::LOOK_CHANGED => {
                self.read_look();
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
            // Everything below is for the first program only.
            _ if owner != 0 => ([dk::BAD_REQUEST, 0, 0, 0, 0, 0], 0),
            dk::LOCK => {
                if self.session.is_some() {
                    self.lock();
                }
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
            dk::INJECT => {
                self.on_event((w[0] as u16, w[1] as u16, w[2] as u32));
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
            dk::FORGET => {
                self.forget(w[0]);
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
            dk::SESSION => {
                let (st, person) = match self.stage {
                    Stage::Setup(_) => (stage::SETUP, 0),
                    Stage::Login(_) => (stage::LOGIN, 0),
                    Stage::Open(i) => (stage::OPEN, self.people[i].id),
                };
                (
                    [
                        dk::OK,
                        st,
                        person,
                        u64::from(self.look.code()),
                        self.wrong_pins,
                        self.parts,
                    ],
                    0,
                )
            }
            dk::GROUP => match self.wm.get(w[0] as u32) {
                Some(win) => ([dk::OK, u64::from(win.group.unwrap_or(0)), 0, 0, 0, 0], 0),
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::PLACES => {
                let n = dock_items(self.look).count();
                let r = match w[0] {
                    i if i < dk::places::CHIP && (i as usize) < n => {
                        Some(scene::dock_item(self.width, self.height, n, i as usize))
                    }
                    i if i >= dk::places::CHIP => self
                        .wm
                        .groups
                        .get((i - dk::places::CHIP) as usize)
                        .map(|g| g.1),
                    _ => None,
                };
                match r {
                    Some(r) => (
                        [dk::OK, r.x as u64, r.y as u64, r.w as u64, r.h as u64, 0],
                        0,
                    ),
                    None => ([dk::BAD_REQUEST, 0, 0, 0, 0, 0], 0),
                }
            }
            dk::APP => match self.wins.iter().find(|x| u64::from(x.id) == w[0]) {
                Some(x) => ([dk::OK, x.owner, 0, 0, 0, 0], 0),
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::WINDOW_OF => match self.wins.iter().rev().find(|x| x.owner == w[0]) {
                Some(x) => ([dk::OK, u64::from(x.id), 0, 0, 0, 0], 0),
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::RAISE => {
                let mut out = Vec::new();
                self.wm.raise(w[0] as u32, &mut out);
                self.apply(out);
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
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

/// The dock: the apps of milestone 6, in the mock-up's colours, with
/// their roles.
const DOCK: [(&str, u32, u64); 4] = [
    ("Files", 0x23_68_6b, role::FILES),
    ("Text editor", 0x2d_5c_9c, role::EDITOR),
    ("Chart maker", 0x9a_5b_00, role::CHART),
    ("Settings", 0x5b_6b_6c, role::SETTINGS),
];

/// What the dock holds in each look.
fn dock_items(look: Look) -> impl Iterator<Item = (&'static str, u32, u64)> {
    DOCK.iter()
        .copied()
        .filter(move |d| look == Look::Everything || d.2 != role::CHART)
}

/// The loose groups (decision 85): labels only, numbered from 1.
const GROUPS: [(&str, u32); 2] = [("Work", 0x23_68_6b), ("Home", 0x9a_5b_00)];

/// What the login screen says about the login parts (decision 74).
fn notice(parts: u64) -> String {
    String::from(match parts {
        parts::FIRST => {
            "Checked by Alaya, not yet by the chip: this is the first login, so Alaya \
             remembers the login screen from now on."
        }
        parts::SAME => {
            "Checked by Alaya, not yet by the chip: the login screen is the same as at \
             your last login."
        }
        _ => {
            "Careful: the login screen changed since your last login. Alaya checked \
             this itself, not yet with the chip. An update does this; if you did not \
             update, do not type your PIN."
        }
    })
}

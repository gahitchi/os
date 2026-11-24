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

use alaya_apps::store::{self as st, Entry, Lasts, Store, Undone, label};
use alaya_crypto::{pin_key, sha256};
use alaya_programs::desktop::{self as dk, event, parts, place, role, stage};
use alaya_programs::files::{self as fs, APPS, DOCS, act};
use alaya_programs::seat::{self, InputPage, ScreenPage, ev, unpack};
use alaya_programs::{load, store};
use alaya_sys::abi::{StartInfo, call, map, start};
use alaya_sys::rt::{self, Mem};
use alaya_sys::{Caps, Kind, Msg, MsgInfo, rights, say};
use alaya_ui::Rect;
use alaya_ui::canvas::Canvas;
use alaya_ui::login::{self, Login, Look, NewPerson, Person, Setup};
use alaya_ui::scene::{self, Desktop, DockItem, GroupChip, Picker, Question, WindowView};
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

/// An undo that waits for the person's answer: something else changed
/// the same files since (rule B6).
enum Asked {
    Action(u64),
    Change(u64, u64),
}

struct Ask {
    what: Asked,
    title: String,
    body: String,
}

/// The Open or Save window an app asked for (dk::PICK): Alaya draws it,
/// and the app gets only what the person picks, for as long as the
/// person says (milestone 7, step 2).
struct Pick {
    win: u32,
    role: u64,
    save: bool,
    write: bool,
    files: Vec<Entry>,
    chosen: usize,
    name: String,
    /// Index into [`Pick::choices`].
    lasts: usize,
}

impl Pick {
    /// How long the app may keep it: the choices, in order. First: until
    /// the person takes it back (Settings shows it), so an app opens
    /// again where it was; Tab picks less.
    fn choices(&self) -> Vec<Lasts> {
        let mut c = alloc::vec![Lasts::Live, Lasts::UntilClosed, Lasts::Until(0)];
        if !self.save && !self.write {
            c.push(Lasts::Pinned(0));
        }
        c
    }
}

/// The search box (Alaya key + F, or the strip's Search button): the
/// person's documents by name and text, and what they used to say.
/// Opening a result hands that one file to the text editor.
struct Search {
    words: String,
    found: Vec<st::Found>,
    chosen: usize,
}

/// How many results the search box shows, and how many kept moments it
/// looks back through.
const SEARCH_MOST: u64 = 40;
const SEARCH_MOMENTS: u64 = 48;

/// How long a note stays in the strip.
const NOTE_MS: u64 = 8000;

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
    /// Each slot's picture pages, kept when its window closes and used
    /// again (wiped first) by the next window there: memory is not given
    /// back to where it came from, so making new ones each time runs out.
    slot_pages: [[u64; dk::PAGES as usize]; MAX_WINDOWS],
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
    /// What the strip says (an undo was done), and until when.
    note: String,
    note_until: u64,
    /// An undo waiting for an answer.
    ask: Option<Ask>,
    /// An Open or Save window is up.
    pick: Option<Pick>,
    /// The search box is up.
    search: Option<Search>,
    /// A file handed over from the search box, for an app that is still
    /// starting: (role, the event its window gets).
    hand: Option<(u64, [u64; 5])>,
    /// The loose group each app's window is in (by role), to put it back.
    groups_of: Vec<(u64, u32)>,
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
        slot_pages: [[0; dk::PAGES as usize]; MAX_WINDOWS],
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
        note: String::new(),
        note_until: 0,
        ask: None,
        pick: None,
        search: None,
        hand: None,
        groups_of: Vec::new(),
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
        if !self.note.is_empty() && rt::now_ms() >= self.note_until {
            self.note.clear();
            self.damage(Rect::new(0, 0, self.width, alaya_ui::wm::STRIP));
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
            _ => &self.note,
        };
        let question = self.ask.as_ref().map(|a| Question {
            title: &a.title,
            body: &a.body,
            yes: "Undo anyway",
            no: "Keep as it is",
        });
        let pick_rows: Vec<String> = self
            .pick
            .as_ref()
            .map(|p| {
                p.files
                    .iter()
                    .map(|e| alloc::format!("{}   ·   {} bytes", e.name, e.size))
                    .collect()
            })
            .unwrap_or_default();
        let pick_refs: Vec<&str> = pick_rows.iter().map(|r| r.as_str()).collect();
        let pick_title = self.pick.as_ref().map(|p| {
            alloc::format!(
                "{} wants to {} a document",
                capital(st::app_name(fs::app_id(p.role))),
                if p.save { "save" } else { "open" }
            )
        });
        let pick_foot = self
            .pick
            .as_ref()
            .map(|p| alloc::format!("Keeps it: {}", p.choices()[p.lasts].words()));
        let search_rows: Vec<String> = self
            .search
            .as_ref()
            .map(|q| q.found.iter().map(found_row).collect())
            .unwrap_or_default();
        let search_refs: Vec<&str> = search_rows.iter().map(|r| r.as_str()).collect();
        let picker = match (&self.pick, &self.search) {
            (Some(p), _) => Some(Picker {
                title: pick_title.as_deref().unwrap_or(""),
                hint: "Only what you pick is handed over. Tab: how long it keeps it.",
                rows: &pick_refs,
                chosen: p.chosen,
                mark: !p.save,
                field: p.save.then_some(("Name", p.name.as_str())),
                foot: pick_foot.as_deref().unwrap_or(""),
                empty: "No documents yet.",
                ok: if p.save { "Save here" } else { "Hand it over" },
            }),
            (None, Some(q)) => Some(Picker {
                title: "Search your documents",
                hint: "Names, words, and what files used to say. Only what you open is handed over.",
                rows: &search_refs,
                chosen: q.chosen,
                mark: true,
                field: Some(("Find", q.words.as_str())),
                foot: "Enter opens it in the text editor. Older versions open read only.",
                empty: if q.words.trim().is_empty() {
                    "Type what to look for."
                } else {
                    "Nothing found, now or in the past."
                },
                ok: "Open",
            }),
            _ => None,
        };
        let desk = Desktop {
            clock: &self.clock,
            status,
            dock: &dock,
            groups: &groups,
            show_pointer: false,
            locked,
            trusted,
            buttons: true,
            question: question.as_ref(),
            picker: picker.as_ref(),
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
        if self.pick.is_some() {
            // The Open or Save window takes every key while it is up.
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.pick_key(k);
            }
            return;
        }
        if self.search.is_some() {
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.search_key(k);
            }
            return;
        }
        if self.ask.is_some() {
            // A question is asked: Enter answers yes, Escape no; nothing
            // else reaches the apps meanwhile.
            if value == 1 && (code == 28 || code == 1) {
                self.answer(code == 28);
            }
            return;
        }
        if self.super_key && value == 1 && code == 33 {
            // Alaya key + F: search.
            self.search_start();
            return;
        }
        if self.super_key && value == 1 && code == 44 {
            // Alaya key + Z: the one undo (decision 109).
            self.undo_last();
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
                Out::Close(id) => {
                    // The person closed it: on the undo list, so the one
                    // undo opens it again at its return point.
                    if let Some(w) = self.wins.iter().find(|w| w.id == id) {
                        let mut l = alloc::vec![label::WINDOW, w.owner as u8];
                        l.extend(w.title.bytes().take(fs::LABEL - 2));
                        self.record(act::WINDOW, &l);
                    }
                    self.push(id, [event::CLOSE, 0, 0, 0, 0]);
                }
                Out::Grouped(id, g) => {
                    let name = GROUPS.get(g as usize - 1).map_or("?", |x| x.0);
                    say!("window {id} is in the group {name}");
                    if let Some(w) = self.wins.iter().find(|w| w.id == id) {
                        let role = w.owner;
                        let old = self
                            .groups_of
                            .iter()
                            .find(|x| x.0 == role)
                            .map_or(0, |x| x.1);
                        let mut l = alloc::vec![label::GROUP, role as u8, old as u8, g as u8];
                        l.extend(w.title.bytes().take(fs::LABEL - 4));
                        self.set_group_of(role, g);
                        self.record(act::GROUP, &l);
                    }
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
        if let Some(p) = &mut self.pick {
            let (ok, no) = scene::picker_buttons(self.width, self.height);
            let first = scene::picker_first(p.chosen);
            if ok.contains(x, y) {
                self.pick_done(true);
            } else if no.contains(x, y) {
                self.pick_done(false);
            } else if let Some(i) = scene::picker_row_at(self.width, self.height, x, y, first)
                && i < p.files.len()
            {
                p.chosen = i;
                if p.save {
                    p.name = p.files[i].name.clone();
                }
                self.damage_all();
            }
            return true;
        }
        if let Some(q) = &mut self.search {
            let (ok, no) = scene::picker_buttons(self.width, self.height);
            let first = scene::picker_first(q.chosen);
            if ok.contains(x, y) {
                self.search_done(true);
            } else if no.contains(x, y) {
                self.search_done(false);
            } else if let Some(i) = scene::picker_row_at(self.width, self.height, x, y, first)
                && i < q.found.len()
            {
                q.chosen = i;
                self.damage_all();
            }
            return true;
        }
        if self.ask.is_some() {
            // Only the question's buttons answer while it is asked.
            let (yes, no) = scene::question_buttons(self.width, self.height);
            if yes.contains(x, y) {
                self.answer(true);
            } else if no.contains(x, y) {
                self.answer(false);
            }
            return true;
        }
        let [undo, happened, find] = scene::strip_buttons();
        if find.contains(x, y) {
            self.search_start();
            return true;
        }
        if undo.contains(x, y) {
            self.undo_last();
            return true;
        }
        if happened.contains(x, y) {
            self.show_app(dk::role::BACK, "What happened?");
            return true;
        }
        let n = dock_items(self.look).count();
        for (i, (name, _, r)) in dock_items(self.look).enumerate() {
            if scene::dock_item(self.width, self.height, n, i).contains(x, y) {
                self.show_app(r, name);
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

    /// Bring the app with role `r` to the front, or start it.
    fn show_app(&mut self, r: u64, name: &str) {
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
    }

    /// The store of the person whose desktop is open.
    fn mine(&self) -> Option<Store> {
        match self.stage {
            Stage::Open(i) => Some(self.person_store(self.people[i].id)),
            _ => None,
        }
    }

    /// Put something the desktop did on the undo list.
    fn record(&mut self, kind: u64, l: &[u8]) {
        if let Some(s) = self.mine()
            && let Err(e) = s.record_action(kind, l)
        {
            say!("the undo list did not take it: {e}");
        }
    }

    fn set_group_of(&mut self, role: u64, g: u32) {
        self.groups_of.retain(|x| x.0 != role);
        if g != 0 {
            self.groups_of.push((role, g));
        }
    }

    fn say_note(&mut self, text: String) {
        say!("strip: {text}");
        self.note = text;
        self.note_until = rt::now_ms() + NOTE_MS;
        self.damage(Rect::new(0, 0, self.width, alaya_ui::wm::STRIP));
    }

    /// The one undo: the person's last action, anywhere (decision 110).
    fn undo_last(&mut self) {
        let Some(s) = self.mine() else { return };
        match s.last_action() {
            Ok(Some(a)) => self.undo_action(&s, a),
            Ok(None) => self.say_note(String::from("Nothing to undo")),
            Err(e) => say!("the undo list could not be read: {e}"),
        }
    }

    /// Undo one action from the list (an undo of an undo is a redo).
    fn undo_action(&mut self, s: &Store, a: st::Action) {
        let about = st::describe(&a.label);
        // The look lives in the person's app files, which the store does
        // not put back: the desktop does, and checks first that nothing
        // changed it since (rule B6). For a redo, it must still be the old
        // look.
        let expect = if a.kind == act::UNDO {
            a.label[1]
        } else {
            a.label[2]
        };
        if a.label[0] == label::LOOK && self.look_code(s) != expect {
            self.ask_about(
                Asked::Action(a.no),
                &about,
                alloc::vec![String::from("the look")],
            );
            return;
        }
        let r = s.undo(a.no, false);
        self.undone(Asked::Action(a.no), r, &about);
    }

    /// "What happened?" asks for an undo (dk::UNDO).
    fn undo_for(&mut self, by: u64, n: u64, app: u64) -> u64 {
        let Some(s) = self.mine() else {
            return dk::BAD_REQUEST;
        };
        if self.ask.is_some() {
            return dk::BAD_REQUEST;
        }
        match by {
            dk::undo_by::LAST => self.undo_last(),
            dk::undo_by::ACTION => {
                let found = s
                    .actions(n + 1, 1)
                    .ok()
                    .and_then(|l| l.first().copied())
                    .filter(|a| a.no == n && !a.undone);
                match found {
                    Some(a) => self.undo_action(&s, a),
                    None => return dk::BAD_REQUEST,
                }
            }
            dk::undo_by::CHANGE => {
                let about = alloc::format!("what {} changed", st::app_name(app));
                let r = s.undo_change(n, app, false);
                self.undone(Asked::Change(n, app), r, &about);
            }
            _ => return dk::BAD_REQUEST,
        }
        dk::OK
    }

    /// What an undo did: put windows back, or ask.
    fn undone(&mut self, what: Asked, r: Result<Undone, u64>, about: &str) {
        let Some(s) = self.mine() else { return };
        match r {
            Ok(Undone::Done {
                kind,
                label: l,
                files,
            }) => {
                let redo = kind == act::UNDO;
                // Desktop actions: the store marked them; the desktop puts
                // the window back (or, for a redo, does it again).
                match (l[0], redo) {
                    (label::WINDOW, false) => {
                        let role = u64::from(l[1]);
                        if !self.wins.iter().any(|w| w.owner == role) {
                            let _ = alaya_sys::signal(self.session_ntfn, 1 << role);
                        }
                    }
                    (label::WINDOW, true) => {
                        let role = u64::from(l[1]);
                        let ids: Vec<u32> = self
                            .wins
                            .iter()
                            .filter(|w| w.owner == role)
                            .map(|w| w.id)
                            .collect();
                        for id in ids {
                            self.push(id, [event::CLOSE, 0, 0, 0, 0]);
                        }
                    }
                    (label::LOOK, _) => {
                        let code = if redo { l[2] } else { l[1] };
                        if let Ok(apps) = s.root(APPS)
                            && let Ok(f) = s.open(apps, "look")
                        {
                            let _ = s.write_all(f, &[code]);
                            let _ = s.save();
                        }
                    }
                    (label::GROUP, _) => {
                        let role = u64::from(l[1]);
                        let g = u32::from(if redo { l[3] } else { l[2] });
                        if let Some(id) = self
                            .wins
                            .iter()
                            .rev()
                            .find(|w| w.owner == role)
                            .map(|w| w.id)
                        {
                            self.wm.set_group(id, (g != 0).then_some(g));
                        }
                        self.set_group_of(role, g);
                        self.damage_all();
                    }
                    _ => {}
                }
                // A setting may have gone back.
                self.read_look();
                let what = if l[0] == 0 {
                    String::from(about)
                } else {
                    st::describe(&l)
                };
                let text = alloc::format!(
                    "{} {}{}",
                    if redo { "Redone:" } else { "Undone:" },
                    what,
                    match files.len() {
                        0 => String::new(),
                        1 => String::from(" (1 file back)"),
                        n => alloc::format!(" ({n} files back)"),
                    }
                );
                self.say_note(text);
            }
            Ok(Undone::Conflicts(files)) => {
                let mut names: Vec<String> = files
                    .iter()
                    .take(3)
                    .map(|&f| s.name_of(f, 0).unwrap_or_else(|_| String::from("a file")))
                    .collect();
                let more = files.len().saturating_sub(3);
                if more > 0 {
                    names.push(alloc::format!("{more} more"));
                }
                self.ask_about(what, about, names);
            }
            Err(e) => {
                say!("undo failed: {e}");
                self.say_note(String::from("That could not be undone"));
            }
        }
    }

    /// Something else changed it since: show what, and ask (rule B6).
    fn ask_about(&mut self, what: Asked, about: &str, names: Vec<String>) {
        let body = alloc::format!(
            "Since then, something else changed {}. Undo anyway puts it back as it \
             was before; the newer version stays in the timeline.",
            names.join(", "),
        );
        say!("asked: undo anyway? {} changed since", names.join(", "));
        self.ask = Some(Ask {
            what,
            title: alloc::format!("Undo \"{about}\"?"),
            body,
        });
        self.damage_all();
    }

    /// The look's code in the person's app files now.
    fn look_code(&self, s: &Store) -> u8 {
        s.root(APPS)
            .and_then(|a| s.lookup(a, "look"))
            .and_then(|f| s.read_all(f))
            .ok()
            .and_then(|b| b.first().copied())
            .unwrap_or(0)
    }

    /// The answer to the question.
    fn answer(&mut self, yes: bool) {
        let Some(a) = self.ask.take() else { return };
        self.damage_all();
        let Some(s) = self.mine() else { return };
        if !yes {
            self.say_note(String::from("Kept as it is: nothing was undone"));
            return;
        }
        let about = a
            .title
            .trim_start_matches("Undo \"")
            .trim_end_matches("\"?");
        let about = String::from(about);
        match a.what {
            Asked::Action(no) => {
                let r = s.undo(no, true);
                self.undone(Asked::Action(no), r, &about);
            }
            Asked::Change(cn, app) => {
                let r = s.undo_change(cn, app, true);
                self.undone(Asked::Change(cn, app), r, &about);
            }
        }
    }

    /// An app asks for the Open or Save window (dk::PICK).
    fn pick_start(&mut self, owner: u64, w: &[u64; 6]) -> u64 {
        let Some(s) = self.mine() else {
            return dk::BAD_REQUEST;
        };
        let Some(win) = self
            .wins
            .iter()
            .find(|x| u64::from(x.id) == w[0] && x.owner == owner)
            .map(|x| x.id)
        else {
            return dk::NO_WINDOW;
        };
        if self.pick.is_some() || self.ask.is_some() || owner == 0 {
            return dk::BAD_REQUEST;
        }
        let docs = s.root(DOCS).unwrap_or(0);
        let mut files: Vec<Entry> = s
            .list(docs)
            .unwrap_or_default()
            .into_iter()
            .filter(|e| !e.is_dir)
            .collect();
        files.sort_by(|a, b| a.name.cmp(&b.name));
        let save = w[1] != 0;
        let mut p = Pick {
            win,
            role: owner,
            save,
            write: save || w[2] != 0,
            files,
            chosen: 0,
            name: String::from(if save { "Untitled.txt" } else { "" }),
            lasts: 0,
        };
        // The app may suggest how long; the person can change it.
        if let Some(i) = p
            .choices()
            .iter()
            .position(|c| c.code().0 == w[3] && w[3] != 0)
        {
            p.lasts = i;
        }
        say!(
            "{} window for {} ({} documents)",
            if save { "save" } else { "open" },
            st::app_name(fs::app_id(owner)),
            p.files.len()
        );
        self.pick = Some(p);
        self.damage_all();
        dk::OK
    }

    fn pick_key(&mut self, k: Key) {
        let Some(p) = &mut self.pick else { return };
        match k {
            Key::Enter => return self.pick_done(true),
            Key::Escape => return self.pick_done(false),
            Key::Tab => p.lasts = (p.lasts + 1) % p.choices().len(),
            Key::Up => p.chosen = p.chosen.saturating_sub(1),
            Key::Down => p.chosen = (p.chosen + 1).min(p.files.len().saturating_sub(1)),
            Key::Home => p.chosen = 0,
            Key::End => p.chosen = p.files.len().saturating_sub(1),
            Key::Backspace if p.save => {
                p.name.pop();
            }
            Key::Char(c) if p.save && p.name.len() < 40 && c != '/' => p.name.push(c),
            _ => {}
        }
        self.damage_all();
    }

    /// Hand over what was picked (or nothing), and tell the app.
    fn pick_done(&mut self, ok: bool) {
        let Some(p) = self.pick.take() else { return };
        self.damage_all();
        let app = fs::app_id(p.role);
        let picked = if !ok {
            None
        } else {
            self.mine().and_then(|s| {
                let lasts = match p.choices()[p.lasts] {
                    Lasts::Until(_) => Lasts::Until(rt::now_ms() / 1000 + 3600),
                    Lasts::Pinned(_) => Lasts::Pinned(p.files.get(p.chosen)?.version),
                    l => l,
                };
                let f = if p.save {
                    let docs = s.root(DOCS).ok()?;
                    let name = p.name.trim();
                    if name.is_empty() {
                        return None;
                    }
                    // Saving over a document of the same name is the
                    // person's choice here: it keeps its history.
                    s.open(docs, name).ok()?
                } else {
                    p.files.get(p.chosen)?.file
                };
                match s.grant(app, f, p.write, lasts) {
                    Ok(()) => {
                        say!(
                            "handed {} to {}, {}{}",
                            s.name_of(f, 0).unwrap_or_default(),
                            st::app_name(app),
                            lasts.words(),
                            if p.write { ", may change it" } else { "" }
                        );
                        Some(f)
                    }
                    Err(e) => {
                        say!("could not hand it over: {e}");
                        None
                    }
                }
            })
        };
        let (kind, _) = p.choices()[p.lasts].code();
        self.push(
            p.win,
            [
                event::PICKED,
                picked.unwrap_or(0),
                p.save as u64,
                p.write as u64,
                kind,
            ],
        );
    }

    fn search_start(&mut self) {
        if self.locked() || self.mine().is_none() || self.pick.is_some() || self.ask.is_some() {
            return;
        }
        if self.search.is_none() {
            say!("search opened");
            self.search = Some(Search {
                words: String::new(),
                found: Vec::new(),
                chosen: 0,
            });
        }
        self.damage_all();
    }

    fn search_key(&mut self, k: Key) {
        let Some(q) = &mut self.search else { return };
        let mut again = false;
        match k {
            Key::Enter => return self.search_done(true),
            Key::Escape => return self.search_done(false),
            Key::Up => q.chosen = q.chosen.saturating_sub(1),
            Key::Down => q.chosen = (q.chosen + 1).min(q.found.len().saturating_sub(1)),
            Key::Backspace => again = q.words.pop().is_some(),
            Key::Char(c) if q.words.len() < 64 && (c.is_ascii_graphic() || c == ' ') => {
                q.words.push(c);
                again = true;
            }
            _ => {}
        }
        if again {
            // Look again as the person types: the store looks through the
            // files themselves, there is no index to keep.
            let words = q.words.clone();
            let found = match self.mine() {
                Some(s) if !words.trim().is_empty() => s
                    .search(&words, SEARCH_MOST, SEARCH_MOMENTS)
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            if let Some(q) = &mut self.search {
                q.found = found;
                q.chosen = 0;
            }
        }
        self.damage_all();
    }

    /// Open the chosen result: hand that one file to the text editor
    /// (now and live, or the older version, read only).
    fn search_done(&mut self, ok: bool) {
        let Some(q) = self.search.take() else { return };
        self.damage_all();
        let Some(f) = q.found.get(q.chosen).filter(|_| ok) else {
            return;
        };
        let Some(s) = self.mine() else { return };
        let app = fs::app_id(role::EDITOR);
        let (lasts, write) = match f.past {
            Some((cn, _)) => (Lasts::Pinned(cn), false),
            None => (Lasts::Live, true),
        };
        if let Err(e) = s.grant(app, f.file, write, lasts) {
            say!("could not hand it over: {e}");
            return;
        }
        say!(
            "search for \"{}\": handed {} to {}, {}",
            q.words.trim(),
            f.name,
            st::app_name(app),
            lasts.words()
        );
        let e = [event::PICKED, f.file, 0, write as u64, lasts.code().0];
        match self
            .wins
            .iter()
            .rev()
            .find(|w| w.owner == role::EDITOR)
            .map(|w| w.id)
        {
            Some(id) => {
                self.push(id, e);
                let mut out = Vec::new();
                self.wm.raise(id, &mut out);
                self.apply(out);
            }
            None => {
                self.hand = Some((role::EDITOR, e));
                self.show_app(role::EDITOR, "Text editor");
            }
        }
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
        if self.pick.as_ref().is_some_and(|p| p.win == x.id) {
            self.pick = None;
        }
        // The app's last window: what it had until it closed goes back.
        if x.owner != 0
            && !self.wins.iter().any(|w| w.owner == x.owner)
            && let Some(s) = self.mine()
            && let Ok(n) = s.app_closed(fs::app_id(x.owner))
            && n > 0
        {
            say!(
                "{} closed: {n} hand-overs taken back",
                st::app_name(fs::app_id(x.owner))
            );
        }
        let out = self.wm.remove(x.id);
        self.apply(out);
        // Take the pages back from the app (every copy it was lent); the
        // desktop keeps them for the slot's next window.
        for p in x.pages {
            let _ = alaya_sys::revoke(self.mem.at(p));
        }
        let _ = alaya_sys::delete(self.mem.at(x.ntfn));
        let slot = ((x.va - PICTURES) / PICTURE_STEP) as usize;
        self.slot_pages[slot] = x.pages;
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
                let mut pages = self.slot_pages[slot];
                if pages[0] != 0 {
                    // Used before: wipe what the last window drew, so no
                    // app sees another's picture.
                    // SAFETY: the slot's pages are mapped at `va`.
                    unsafe {
                        core::ptr::write_bytes(va as *mut u8, 0, (dk::PAGES * (2 << 20)) as usize)
                    };
                } else {
                    for (i, p) in pages.iter_mut().enumerate() {
                        let Ok(h) = self.mem.make(Kind::Page, 21) else {
                            return ([dk::NO_MEMORY, 0, 0, 0, 0, 0], 0);
                        };
                        alaya_sys::page_map(h, start::SPACE, va + i as u64 * (2 << 20), map::WRITE)
                            .expect("map a picture page");
                        *p = h;
                    }
                    self.slot_pages[slot] = pages;
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
                if let Some((r, e)) = self.hand
                    && r == owner
                {
                    self.hand = None;
                    self.push(id, e);
                }
                ([dk::OK, u64::from(id), 0, 0, 0, 0], 0)
            }
            dk::PICK => ([self.pick_start(owner, &w), 0, 0, 0, 0, 0], 0),
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
            dk::UNDO if owner == role::BACK || owner == 0 => {
                let r = self.undo_for(w[0], w[1], w[2]);
                ([r, u64::from(self.ask.is_some()), 0, 0, 0, 0], 0)
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
                let (yes, no) = scene::question_buttons(self.width, self.height);
                let r = match w[0] {
                    dk::places::UNDO => Some(scene::strip_buttons()[0]),
                    dk::places::HAPPENED => Some(scene::strip_buttons()[1]),
                    dk::places::YES => self.ask.as_ref().map(|_| yes),
                    dk::places::NO => self.ask.as_ref().map(|_| no),
                    i if i < dk::places::CHIP && (i as usize) < n => {
                        Some(scene::dock_item(self.width, self.height, n, i as usize))
                    }
                    i if (dk::places::CHIP..dk::places::UNDO).contains(&i) => self
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
                    u64::from(self.ask.is_some())
                        | (self.note.len().min(1 << 20) as u64) << 1
                        | u64::from(self.pick.is_some()) << 62
                        | (self
                            .search
                            .as_ref()
                            .map_or(0, |q| 1 + q.found.len().min(255))
                            as u64)
                            << 52,
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

/// "the text editor" → "The text editor".
fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// One search result, as a row.
fn found_row(f: &st::Found) -> String {
    let what = match (&f.at, f.line.trim()) {
        (None, _) => String::from("its name"),
        (Some(_), l) => alloc::format!("\u{201c}{l}\u{201d}"),
    };
    match f.past {
        None => alloc::format!("{}   ·   {what}", f.name),
        Some((_, t)) => alloc::format!(
            "{}   ·   {what}   ·   {} {}:{:02}",
            f.name,
            if f.gone {
                "removed since; was there at"
            } else {
                "it said this at"
            },
            t / 3600 % 24,
            t / 60 % 60
        ),
    }
}

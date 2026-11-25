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
use alaya_ui::wm::{Out, Place, Screen, Wm};

#[path = "desktop/game.rs"]
mod game;
#[path = "desktop/groups.rs"]
mod groups;
#[path = "desktop/inbox.rs"]
mod inbox;
#[path = "desktop/install.rs"]
mod install;
#[path = "desktop/reader.rs"]
mod reader;
#[path = "desktop/typing.rs"]
mod typing;
use groups::GroupScreen;

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
    /// Live handoff (milestone 8, step 3): continue this document in
    /// another app, at this place.
    Handoff(Handoff),
}

struct Ask {
    what: Asked,
    title: String,
    body: String,
    yes: &'static str,
    no: &'static str,
}

/// A live handoff the person is asked about, or the last one done (for
/// the checks): from role, to role, the file, the place, write.
#[derive(Clone, Copy)]
struct Handoff {
    from: u64,
    to: u64,
    file: u64,
    place: u64,
    write: bool,
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
    /// An answer window (milestone 8, step 3): the app asks one question
    /// from Alaya's fixed list ([`fs::question`]) about the document
    /// picked, and gets only the answer. 0: an ordinary Open window.
    answer: u64,
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

/// A job with rules (milestone 7, step 3): an app asked for one, and
/// Alaya, not the app, shows the person the rules before it starts, and
/// what Alaya's own checks found when it is done.
struct JobScreen {
    win: u32,
    role: u64,
    chosen: usize,
    stage: JobStage,
}

enum JobStage {
    /// Which one folder may it change?
    Folder(Vec<Entry>),
    /// The rules, with what Alaya enforces, checks, or can only wish for.
    Rules {
        folder: Entry,
        checklist: Option<(u64, usize)>,
        keep_same: Vec<u64>,
        private: usize,
    },
    /// What the checks found; `conflicts`: files the person changed too.
    Result {
        report: st::JobReport,
        conflicts: Vec<u64>,
    },
}

/// The checklist a job's folder may hold: its lines are checked by
/// Alaya before the work is kept.
const CHECKLIST: &str = "checklist.txt";
/// The checks every job gets (`alaya_store::jobs::check`): nothing lost,
/// nothing emptied; the checklist when there is one.
const JOB_CHECKS: u64 = 4 | 8;
const CHECK_LIST: u64 = 16;

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
    /// The whole picture (all screens side by side); `width` and `height`
    /// are the main screen's (milestone 7b).
    full: (i32, i32),
    /// The screens' changes seen so far.
    layout: u64,
    /// The scale the person chose for each screen (by its output number;
    /// 0: Alaya picks from its size), from their app files ("scales").
    scales: [u16; seat::MAX_SCREENS],
    /// High contrast and the screen reader (`dk::seeing`), the person's.
    seeing: u64,
    /// The screen reader's state.
    reader: reader::Reader,
    /// Where the walk through Alaya's own parts is (Alaya key + Tab).
    ring: Option<usize>,
    /// The input method (milestone 7b, step 3).
    typing: typing::Typing,
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
    /// A job's rules screen or result is up.
    job: Option<JobScreen>,
    /// A file handed over from the search box, for an app that is still
    /// starting: (role, the event its window gets).
    hand: Option<(u64, [u64; 5])>,
    /// The loose group each app's window is in (by role), to put it back.
    groups_of: Vec<(u64, u32)>,
    /// The person's groups and tasks (milestone 7a).
    groups: Vec<st::GroupRow>,
    /// The group screen is up.
    gscreen: Option<GroupScreen>,
    /// A file waiting to go into the group being named.
    gchoose_after: u64,
    /// The group the person is working in (milestone 7a, step 2).
    current_group: Option<u32>,
    /// The decision inbox (milestone 7a, step 3), the row chosen while it
    /// is up, how many "now" things showed at once, and the person's
    /// rules (apps always let through).
    inbox: Vec<inbox::Item>,
    inbox_at: Option<usize>,
    now_shown: u64,
    rules: Vec<u64>,
    /// One-time hints already shown (a bit each), kept with the app files.
    hints: u64,
    /// The person's installed apps, by place (milestone 8).
    installed: [install::Place; role::INSTALLED_MAX as usize],
    /// The install sheet is up.
    install: Option<install::Install>,
    /// The last removal, for the checks: role, left, taken back, made.
    removed: Option<(u64, u64, u64, u64)>,
    /// When the first program started each installed place's app (ms),
    /// until its window opens: the Experience Guardian's clock.
    starting: [u64; role::INSTALLED_MAX as usize],
    /// The last rehearsal: (documents read, changed, side copy gone).
    rehearsed: Option<(u64, u64, bool)>,
    /// The version each installed place's running copy was started at.
    running_version: [u32; role::INSTALLED_MAX as usize],
    /// Game mode is on (`game.rs`), the roles it froze (bit r), and the
    /// install offers waiting for it to end.
    game: bool,
    game_frozen: u64,
    held_offers: Vec<u64>,
    /// Installed apps running with no window open (bit r).
    windowless: u64,
    /// How many held things came forward at the last GROUP_GO.
    came_forward: u64,
    /// The last live handoff (for the checks).
    handed: Option<Handoff>,
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
    let (width, height) = main_screen(screen);
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
        look: Look::Everyday,
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
        full: (width, height),
        layout: load(&screen.layout),
        scales: [0; seat::MAX_SCREENS],
        seeing: 0,
        reader: reader::Reader::default(),
        ring: None,
        typing: typing::Typing::default(),
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
        job: None,
        hand: None,
        groups_of: Vec::new(),
        groups: Vec::new(),
        gscreen: None,
        gchoose_after: 0,
        current_group: None,
        inbox: Vec::new(),
        inbox_at: None,
        now_shown: 0,
        rules: Vec::new(),
        hints: 0,
        installed: Default::default(),
        install: None,
        removed: None,
        starting: [0; role::INSTALLED_MAX as usize],
        rehearsed: None,
        running_version: [0; role::INSTALLED_MAX as usize],
        game: false,
        game_frozen: 0,
        held_offers: Vec::new(),
        windowless: 0,
        came_forward: 0,
        handed: None,
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
    d.read_screens();
    d.frame();
    d.serve(g + dk::G_EP)
}

/// The screens the driver shows, main one first.
fn screens_of(page: &ScreenPage) -> Vec<Screen> {
    let n = (load(&page.screens) as usize).min(seat::MAX_SCREENS);
    (0..n)
        .map(|i| {
            let r = &page.rects[i];
            Screen {
                id: load(&r[4]) as u32,
                rect: Rect::new(
                    load(&r[0]) as i32,
                    load(&r[1]) as i32,
                    load(&r[2]) as i32,
                    load(&r[3]) as i32,
                ),
                scale: 100,
            }
        })
        .collect()
}

/// The main screen's size (the whole picture when the driver says
/// nothing of screens).
/// The scale Alaya picks for a screen nobody chose one for. The card
/// does not say how big the screen is in centimetres, so its pixels
/// decide: a screen this wide is almost always a dense one.
fn auto_scale(r: Rect) -> i32 {
    match r.w.max(r.h) {
        3200.. => 200,
        2500.. => 150,
        _ => 100,
    }
}

fn main_screen(page: &ScreenPage) -> (i32, i32) {
    match screens_of(page).first() {
        Some(s) => (s.rect.w, s.rect.h),
        None => (load(&page.width) as i32, load(&page.height) as i32),
    }
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
            self.reader_poll();
            self.game_check();
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
        if bits & seat::SCREENS_BIT != 0 || load(&self.screen.layout) != self.layout {
            self.read_screens();
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
            .union(r.intersect(Rect::new(0, 0, self.full.0, self.full.1)));
    }

    /// The screens changed (or this is the start): lay the windows out
    /// on them again and draw everything.
    fn read_screens(&mut self) {
        self.layout = load(&self.screen.layout);
        let mut list = screens_of(self.screen);
        if list.is_empty() {
            return;
        }
        for sc in &mut list {
            sc.scale = self.scale_for(sc);
        }
        self.full = (
            load(&self.screen.width) as i32,
            load(&self.screen.height) as i32,
        );
        let main = list[0].rect;
        (self.width, self.height) = (main.w, main.h);
        say!(
            "screens: {} ({})",
            list.len(),
            list.iter()
                .map(|s| alloc::format!("{} by {}", s.rect.w, s.rect.h))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let out = self.wm.set_screens(list);
        self.apply(out);
        self.place_chips();
        // A frame on its way was for the old picture.
        self.in_flight = false;
        self.damage_all();
    }

    /// The scale screen `sc` is drawn at: the person's, or Alaya's pick.
    fn scale_for(&self, sc: &Screen) -> i32 {
        match self.scales.get(sc.id as usize) {
            Some(&p) if p != 0 => i32::from(p),
            _ => auto_scale(sc.rect),
        }
    }

    /// High contrast or the screen reader changed: kept with the
    /// person's app files, and every window draws again.
    fn set_seeing(&mut self, bits: u64) {
        if bits == self.seeing {
            return;
        }
        if let Some(s) = self.mine()
            && let Ok(a) = s.root(APPS)
            && let Ok(f) = s.open(a, "seeing")
        {
            let _ = s.write_all(f, &[bits as u8]);
        }
        self.seeing_now(bits);
    }

    fn seeing_now(&mut self, bits: u64) {
        let was = core::mem::replace(&mut self.seeing, bits);
        if was == bits {
            return;
        }
        say!(
            "high contrast {}, screen reader {}",
            if bits & dk::seeing::CONTRAST != 0 {
                "on"
            } else {
                "off"
            },
            if bits & dk::seeing::READER != 0 {
                "on"
            } else {
                "off"
            }
        );
        if (was ^ bits) & dk::seeing::READER != 0 {
            self.reader_changed();
        }
        let all: Vec<(u32, (i32, i32))> = self.wins.iter().map(|w| (w.id, w.content)).collect();
        for (id, (w, h)) in all {
            let scale = self.wm.get(id).map_or(100, |x| x.scale);
            self.push(id, [event::RESIZED, w as u64, h as u64, scale as u64, bits]);
        }
        self.damage_all();
    }

    /// Read the person's scales again and draw each screen at its own.
    fn load_scales(&mut self) {
        self.scales = [0; seat::MAX_SCREENS];
        if let Some(s) = self.mine()
            && let Ok(b) = s
                .root(APPS)
                .and_then(|a| s.lookup(a, "scales"))
                .and_then(|f| s.read_all(f))
        {
            for (i, c) in b.chunks_exact(2).take(seat::MAX_SCREENS).enumerate() {
                let p = u16::from_le_bytes([c[0], c[1]]);
                if dk::SCALES.contains(&u64::from(p)) {
                    self.scales[i] = p;
                }
            }
        }
        self.apply_scales();
    }

    fn apply_scales(&mut self) {
        for i in 0..self.wm.screens.len() {
            let sc = self.wm.screens[i];
            let want = self.scale_for(&sc);
            if want != sc.scale {
                let out = self.wm.set_scale(i, want);
                self.apply(out);
                say!("screen {} is drawn at {want}%", sc.id);
            }
        }
    }

    /// SCALE: the window's screen, at `pct` (0: Alaya picks; MAX: ask).
    fn scale_request(&mut self, id: u32, pct: u64) -> [u64; 6] {
        let Some(i) = self.wm.get(id).map(|w| self.wm.screen_of(w.frame)) else {
            return [dk::NO_WINDOW, 0, 0, 0, 0, 0];
        };
        let out_id = self.wm.screens[i].id as usize;
        if pct != u64::MAX {
            say!(
                "window {id} at {:?} asks for {pct}% on screen {i} (output {out_id})",
                self.wm.get(id).map(|w| w.frame)
            );
            if !dk::SCALES.contains(&pct) || out_id >= seat::MAX_SCREENS {
                return [dk::BAD_REQUEST, 0, 0, 0, 0, 0];
            }
            self.scales[out_id] = pct as u16;
            if let Some(s) = self.mine()
                && let Ok(a) = s.root(APPS)
                && let Ok(f) = s.open(a, "scales")
            {
                let b: Vec<u8> = self.scales.iter().flat_map(|p| p.to_le_bytes()).collect();
                let _ = s.write_all(f, &b);
            }
            self.apply_scales();
            self.damage_all();
        }
        let sc = self.wm.screens[i];
        [
            dk::OK,
            sc.scale as u64,
            u64::from(self.scales.get(out_id).copied().unwrap_or(0)),
            0,
            0,
            0,
        ]
    }

    /// Draw what changed and hand it to the screen driver, unless a frame
    /// is still on its way.
    fn frame(&mut self) {
        if self.in_flight || self.dirty.is_empty() {
            return;
        }
        let clip = self.dirty;
        self.dirty = Rect::default();
        let (w, h) = self.full;
        let contrast = self.seeing & dk::seeing::CONTRAST != 0;
        let mut cv = Canvas::new(screen_px(w, h), w, h, w)
            .expect("screen")
            .with_contrast(contrast);
        let screen_rects: Vec<Rect> = self.wm.screens.iter().map(|s| s.rect).collect();
        let labels: Vec<(u32, String, u32)> = self
            .groups
            .iter()
            .filter_map(|g| self.group_label(g.id).map(|(n, c)| (g.id, n, c)))
            .collect();
        let paused_roles: Vec<u64> = (1..=role::COUNT).filter(|&r| self.paused_role(r)).collect();
        let paused = |r: u64| paused_roles.contains(&r);
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
                stuck: paused(x.owner).then_some(
                    "Paused with its task: it uses nothing. Alaya key + G, then P, goes on.",
                ),
                group: wm
                    .get(id)
                    .and_then(|w| w.group)
                    .and_then(|g| labels.iter().find(|l| l.0 == g))
                    .map(|l| (l.1.as_str(), l.2)),
            },
            None => WindowView {
                title: "",
                pixels: None,
                stuck: None,
                group: None,
            },
        };
        let dock_list = self.dock();
        let dock: Vec<DockItem> = dock_list
            .iter()
            .map(|(name, color, r)| DockItem {
                name,
                color: *color,
                running: wins.iter().any(|x| x.owner == *r),
            })
            .collect();
        let groups: Vec<GroupChip> = if self.look.groups() {
            labels
                .iter()
                .take(groups::CHIPS)
                .map(|(id, name, color)| GroupChip {
                    name,
                    color: *color,
                    count: wm.wins.iter().filter(|w| w.group == Some(*id)).count(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let locked = !matches!(self.stage, Stage::Open(_));
        let trusted = match &self.stage {
            Stage::Login(l) => l.picture(),
            _ => None,
        };
        let held = self.held_words();
        let status = match self.stage {
            Stage::Setup(_) => "First setup",
            _ if self.note.is_empty() => held.as_deref().unwrap_or(""),
            _ => &self.note,
        };
        let question = self.ask.as_ref().map(|a| Question {
            title: &a.title,
            body: &a.body,
            yes: a.yes,
            no: a.no,
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
            if p.answer != 0 {
                return alloc::format!(
                    "{} asks for {} of a document",
                    capital(st::app_name(fs::app_id(p.role))),
                    st::question_words(p.answer)
                );
            }
            alloc::format!(
                "{} wants to {} a document",
                capital(st::app_name(fs::app_id(p.role))),
                if p.save { "save" } else { "open" }
            )
        });
        let pick_foot = self.pick.as_ref().map(|p| {
            if p.answer != 0 {
                alloc::format!(
                    "It gets one number, never the document · at most {} a day · shown in What happened?",
                    fs::ANSWERS_PER_DAY
                )
            } else {
                alloc::format!("Keeps it: {}", p.choices()[p.lasts].words())
            }
        });
        let search_rows: Vec<String> = self
            .search
            .as_ref()
            .map(|q| q.found.iter().map(found_row).collect())
            .unwrap_or_default();
        let search_refs: Vec<&str> = search_rows.iter().map(|r| r.as_str()).collect();
        let job_view = match (&self.gscreen, self.inbox_at) {
            _ if self.sheet_up() => self.install.as_ref().map(|x| self.install_view(x)),
            (_, Some(at)) => Some(self.inbox_view(at)),
            (Some(g), _) => Some(self.group_view(g)),
            _ => self.job.as_ref().map(|j| self.job_view(j)),
        };
        let job_refs: Vec<&str> = job_view
            .as_ref()
            .map(|v| v.rows.iter().map(|r| r.as_str()).collect())
            .unwrap_or_default();
        let picker = match (&self.pick, &self.search) {
            _ if job_view.is_some() && self.pick.is_none() && self.search.is_none() => {
                let v = job_view.as_ref().unwrap();
                Some(Picker {
                    title: &v.title,
                    hint: &v.hint,
                    rows: &job_refs,
                    chosen: match (&self.gscreen, self.inbox_at) {
                        _ if self.sheet_up() => self.install.as_ref().map_or(0, |x| x.chosen),
                        (_, Some(at)) => at,
                        (Some(g), _) => g.chosen,
                        _ => self.job.as_ref().map_or(0, |j| j.chosen),
                    },
                    mark: v.mark,
                    field: v.field.as_ref().map(|(l, f)| (*l, f.as_str())),
                    foot: &v.foot,
                    empty: v.empty,
                    ok: v.ok,
                    no: v.no,
                })
            }
            (Some(p), _) => Some(Picker {
                title: pick_title.as_deref().unwrap_or(""),
                hint: if p.answer != 0 {
                    "Only the answer is handed over, never the document."
                } else {
                    "Only what you pick is handed over. Tab: how long it keeps it."
                },
                rows: &pick_refs,
                chosen: p.chosen,
                mark: !p.save,
                field: p.save.then_some(("Name", p.name.as_str())),
                foot: pick_foot.as_deref().unwrap_or(""),
                empty: "No documents yet.",
                ok: if p.answer != 0 {
                    "Give the answer"
                } else if p.save {
                    "Save here"
                } else {
                    "Hand it over"
                },
                no: "Cancel",
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
                no: "Cancel",
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
            screens: &screen_rects,
        };
        scene::draw(&mut cv, &mut self.fonts, &self.wm, &views, &desk, clip);
        // The login and setup screens: on the main screen.
        let mut cv = Canvas::new(screen_px(w, h), self.width, self.height, w).expect("screen");
        let old = cv.set_clip(clip);
        match &self.stage {
            Stage::Setup(f) => f.draw(&mut cv, &mut self.fonts),
            Stage::Login(f) => f.draw(&mut cv, &mut self.fonts),
            Stage::Open(_) => {}
        }
        cv.set_clip(old);
        let mut cv = Canvas::new(screen_px(w, h), w, h, w)
            .expect("screen")
            .with_contrast(contrast);
        let old = cv.set_clip(clip);
        if let Some(r) = self.ring_rect() {
            // The walk's ring: thick and always seen.
            cv.round_frame(r, 9, alaya_ui::widgets::theme::ACCENT);
            cv.round_frame(r.grow(1), 10, alaya_ui::widgets::theme::ACCENT);
            cv.round_frame(r.grow(2), 11, alaya_ui::widgets::theme::ACCENT_INK);
        }
        if self.reader_on() && matches!(self.stage, Stage::Open(_)) {
            let r = self.caption_rect();
            cv.fill(r, alaya_ui::widgets::theme::INK);
            let text = if self.reader.before.is_empty() {
                self.reader.said.clone()
            } else {
                alloc::format!("{}   ·   {}", self.reader.said, self.reader.before)
            };
            let (asc, lh) = self.fonts.metrics(alaya_ui::text::Face::Strong, 14);
            self.fonts.draw(
                &mut cv,
                alaya_ui::text::Face::Strong,
                14,
                12,
                r.y + (r.h - lh) / 2 + asc,
                &text,
                alaya_ui::widgets::theme::ACCENT_INK,
            );
        }
        if matches!(self.stage, Stage::Open(_)) {
            self.draw_typing(&mut cv);
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
                if down
                    && b == 2
                    && self.gscreen.is_none()
                    && let Some(g) = self.chip_at(x, y)
                {
                    // A right click on a group's label: its screen.
                    self.group_open(g);
                    return;
                }
                if down && b == 1 && (self.picker_click(x, y) || self.osk_click(x, y)) {
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
            100 => self.typing.altgr = value != 0,
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
        if self.super_key && value == 1 && code == 19 && self.rehearsing().is_some() {
            // Alaya key + R: end the rehearsal and see what it did.
            self.install_key(Key::Enter);
            return;
        }
        if self.sheet_up() {
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.install_key(k);
            }
            return;
        }
        if self.inbox_at.is_some() {
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.inbox_key(k);
            }
            return;
        }
        if self.gscreen.is_some() {
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.group_key(k);
            }
            return;
        }
        if self.job.is_some() {
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.job_key(k);
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
        if self.typing.picker.is_some() {
            // The emoji picker takes the keys while it is open.
            if value != 0 {
                self.picker_key(code);
            }
            return;
        }
        if self.super_key && value == 1 && code == 57 {
            // Alaya key + Space: the next way of typing.
            self.next_mode();
            return;
        }
        if self.super_key && value == 1 && code == 52 {
            // Alaya key + .: the emoji picker.
            self.picker_open();
            return;
        }
        if self.super_key && value == 1 && code == 37 {
            // Alaya key + K: the on-screen keyboard.
            self.osk_toggle();
            return;
        }
        if self.super_key && value == 1 && code == 28 {
            // Alaya key + Enter: the screen reader on or off.
            let b = self.seeing ^ dk::seeing::READER;
            self.set_seeing(b);
            return;
        }
        if self.super_key && value == 1 && code == 46 {
            // Alaya key + C: high contrast on or off.
            let b = self.seeing ^ dk::seeing::CONTRAST;
            self.set_seeing(b);
            return;
        }
        if self.super_key && value == 1 && code == 15 {
            // Alaya key + Tab: walk Alaya's own parts.
            self.walk(self.shift);
            return;
        }
        if self.ring.is_some() {
            // The walk takes the keys while it is on.
            if value != 0
                && let Some(k) = us_key(code, self.shift)
            {
                self.walk_key(k);
            }
            return;
        }
        if self.super_key && value == 1 && code == 33 {
            // Alaya key + F: search.
            self.search_start();
            return;
        }
        if self.super_key && value == 1 && code == 23 {
            // Alaya key + I: the inbox, everything waiting.
            self.inbox_open();
            return;
        }
        if self.super_key && value == 1 && code == 34 {
            // Alaya key + G: the group screen (milestone 7a).
            self.group_open(0);
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
                    // With Shift: to the screen before, or the next one.
                    105 if self.shift => self.wm.to_screen(id, -1),
                    106 if self.shift => self.wm.to_screen(id, 1),
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
            // Text goes through the input method; other keys (and Ctrl
            // shortcuts, and every release) go to the window as keys.
            if value != 0
                && !self.ctrl
                && !self.paused_window(id)
                && self.type_key(code, self.shift)
            {
                return;
            }
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

    fn paused_window(&self, id: u32) -> bool {
        self.wins
            .iter()
            .find(|w| w.id == id)
            .is_some_and(|w| self.paused_role(w.owner))
    }

    fn push(&mut self, id: u32, e: [u64; 5]) {
        // A paused task's apps are frozen: keys and the pointer wait.
        if (e[0] == event::KEY || e[0] == event::POINTER || e[0] == event::TEXT)
            && self
                .wins
                .iter()
                .find(|w| w.id == id)
                .is_some_and(|w| self.paused_role(w.owner))
        {
            return;
        }
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
                    if self.ring.is_none() {
                        self.reader_window(id);
                    }
                }
                Out::Resized(id, w, h) => {
                    let (w, h) = (w.min(dk::MAX_W as i32), h.min(dk::MAX_H as i32));
                    if let Some(x) = self.wins.iter_mut().find(|x| x.id == id) {
                        x.content = (w, h);
                    }
                    let scale = self.wm.get(id).map_or(100, |x| x.scale);
                    let seeing = self.seeing;
                    self.push(
                        id,
                        [event::RESIZED, w as u64, h as u64, scale as u64, seeing],
                    );
                    if let Some(win) = self.wm.get(id) {
                        let p = match win.place {
                            Place::Free => "free",
                            Place::Left => "snapped left",
                            Place::Right => "snapped right",
                            Place::Full => "full screen",
                            Place::Tiled => "tiled",
                        };
                        if win.scale == 100 {
                            say!("window {id} is {p}, {w} by {h}");
                        } else {
                            say!("window {id} is {p}, {w} by {h} at {}%", win.scale);
                        }
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
                    let name = self
                        .group_row(g)
                        .map_or(String::from("?"), |x| x.name.clone());
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
                        self.wall_for(role, g);
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
        self.damage(Rect::new(0, 0, self.full.0, self.full.1));
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
        self.load_rules();
        if first {
            let _ = alaya_sys::signal(self.session_ntfn, dk::LOGGED_IN_BIT);
        }
        self.damage_all();
    }

    fn lock(&mut self) {
        // Locking leaves the group the person was in (a return point).
        if let Some(g) = self.current_group.take() {
            self.group_leave_now(g);
        }
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
        self.load_groups();
        self.load_scales();
        self.load_typing();
        self.load_installed();
        let seeing = s
            .root(APPS)
            .and_then(|a| s.lookup(a, "seeing"))
            .and_then(|f| s.read_all(f))
            .ok()
            .and_then(|b| b.first().copied())
            .map_or(0, u64::from);
        self.seeing_now(seeing);
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
        if let Some(inst) = self.install.as_ref().filter(|_| self.sheet_up()) {
            let (ok, no) = scene::picker_buttons(self.width, self.height);
            let first = scene::picker_first(inst.chosen);
            let removes = matches!(inst.stage, install::Stage::Installed(_));
            if ok.contains(x, y) {
                self.install_key(Key::Enter);
            } else if no.contains(x, y) {
                self.install_key(if removes { Key::Delete } else { Key::Escape });
            } else if let Some(i) = scene::picker_row_at(self.width, self.height, x, y, first) {
                if let Some(inst) = &mut self.install {
                    inst.chosen = i;
                }
                self.install_key(Key::Char(' '));
            }
            return true;
        }
        if let Some(at) = self.inbox_at {
            let (ok, no) = scene::picker_buttons(self.width, self.height);
            let first = scene::picker_first(at);
            if ok.contains(x, y) {
                self.inbox_key(Key::Enter);
            } else if no.contains(x, y) {
                self.inbox_key(Key::Escape);
            } else if let Some(i) = scene::picker_row_at(self.width, self.height, x, y, first) {
                self.inbox_at = Some(i.min(self.inbox.len().saturating_sub(1)));
                self.damage_all();
            }
            return true;
        }
        if self.gscreen.is_some() {
            self.group_click(x, y);
            return true;
        }
        if let Some(j) = &mut self.job {
            let (ok, no) = scene::picker_buttons(self.width, self.height);
            let first = scene::picker_first(j.chosen);
            if ok.contains(x, y) {
                // A click on "Keep anyway" is the deliberate choice.
                self.job_ok(true);
            } else if no.contains(x, y) {
                self.job_key(Key::Escape);
            } else if let Some(i) = scene::picker_row_at(self.width, self.height, x, y, first)
                && let JobStage::Folder(l) = &j.stage
                && i < l.len()
            {
                j.chosen = i;
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
        let items = self.dock();
        for (i, (name, _, r)) in items.iter().enumerate() {
            if scene::dock_item(self.width, self.height, items.len(), i).contains(x, y) {
                self.show_app(*r, name);
                return true;
            }
        }
        if let Some(&(g, _)) = self.wm.groups.iter().find(|(_, r)| r.contains(x, y)) {
            // Into a group: back where you left it, with what changed.
            self.group_go(g);
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
            return;
        }
        self.hint_once(
            inbox::HINT_UNDO,
            "Alaya kept the way it was. Changed your mind? Alaya key + Z",
        );
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

    /// "Make again": the app that made `result` makes it again from its
    /// sources as they are now. Only apps that can are asked (the chart
    /// maker); the person sees why not otherwise.
    fn make_again(&mut self, result: u64) -> u64 {
        let Some(s) = self.mine() else {
            return dk::BAD_REQUEST;
        };
        let Ok(m) = s.mark(result) else {
            return dk::BAD_REQUEST;
        };
        let r = m.maker.wrapping_sub(fs::app_id(0));
        let name = s.name_of(result, 0).unwrap_or_default();
        if r != role::CHART {
            self.say_note(alloc::format!(
                "{} cannot make {name} again by itself: open it and save again",
                capital(st::app_name(m.maker))
            ));
            return dk::BAD_REQUEST;
        }
        self.say_note(alloc::format!(
            "{} makes {name} again from what it is made from now",
            capital(st::app_name(m.maker))
        ));
        let e = [event::MAKE_AGAIN, result, 0, 0, 0];
        match self.wins.iter().rev().find(|w| w.owner == r).map(|w| w.id) {
            Some(id) => self.push(id, e),
            None => {
                self.hand = Some((r, e));
                self.show_app(r, "the chart maker");
            }
        }
        dk::OK
    }

    /// "Run it again": open `app` again on exactly what it had read by
    /// commit `cn` (rule B2). A copy still running closes first, so what
    /// it had until then goes back.
    fn run_again(&mut self, app: u64, cn: u64) -> (u64, u64, u64) {
        let r = app.wrapping_sub(fs::app_id(0));
        if !(1..=role::COUNT).contains(&r) || r == role::BACK || self.mine().is_none() {
            return (dk::BAD_REQUEST, 0, 0);
        }
        self.forget(r);
        let Some(s) = self.mine() else {
            return (dk::BAD_REQUEST, 0, 0);
        };
        match s.run_again(app, cn) {
            Ok((n, exact)) => {
                self.say_note(alloc::format!(
                    "Running {} again on {n} file{} as it read {}{}",
                    st::app_name(app),
                    if n == 1 { "" } else { "s" },
                    if n == 1 { "it" } else { "them" },
                    if exact == n {
                        ""
                    } else {
                        " (some from the nearest kept moment)"
                    }
                ));
                let _ = alaya_sys::signal(self.session_ntfn, 1 << r);
                (dk::OK, n, exact)
            }
            Err(_) => (dk::BAD_REQUEST, 0, 0),
        }
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
                    (label::INSTALL, _) => self.undo_install(&l, redo),
                    (label::REMOVE, _) => self.undo_install(&l, !redo),
                    (label::UPDATE, _) => self.undo_update(&l),
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
                        self.wall_for(role, g);
                        self.damage_all();
                    }
                    _ => {
                        self.group_undone(&l, redo);
                    }
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
            yes: "Undo anyway",
            no: "Keep as it is",
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
        if let Asked::Handoff(h) = a.what {
            return self.handoff_done(&s, h, yes);
        }
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
            Asked::Handoff(_) => {}
        }
    }

    /// An app asks to hand the document it has open to another app, at
    /// its place (dk::HANDOFF): Alaya asks the person first, since the
    /// other app gets the document too.
    fn handoff_start(&mut self, owner: u64, w: &[u64; 6]) -> u64 {
        let Some(s) = self.mine() else {
            return dk::BAD_REQUEST;
        };
        let (to, file, place) = (w[1], w[2], w[3]);
        let can_take = to != owner
            && (to == role::EDITOR || to == role::CHART || role::installed(to))
            && (!role::installed(to) || self.allowed(to, alaya_bundle::promise::FILES));
        if owner == 0
            || !can_take
            || self.ask.is_some()
            || self.pick.is_some()
            || !self
                .wins
                .iter()
                .any(|x| u64::from(x.id) == w[0] && x.owner == owner)
        {
            return dk::BAD_REQUEST;
        }
        // Only a document the app itself holds.
        let Some(g) = s
            .grants(fs::app_id(owner))
            .unwrap_or_default()
            .into_iter()
            .find(|g| g.file == file && !matches!(g.lasts, Lasts::Pinned(_)))
        else {
            return dk::NOT_ALLOWED;
        };
        let name = s.name_of(file, 0).unwrap_or_default();
        let (a, b) = (
            capital(st::app_name(fs::app_id(owner))),
            st::app_name(fs::app_id(to)),
        );
        say!("asked: hand {name} from {a} to {b} at line {}", place + 1);
        self.ask = Some(Ask {
            what: Asked::Handoff(Handoff {
                from: owner,
                to,
                file,
                place,
                write: g.write,
            }),
            title: alloc::format!("Continue {name} in {b}?"),
            body: alloc::format!(
                "{a} hands {name} to {b}, at line {}, so you go on where you are. \
                 {b} gets this document until it closes, and nothing else.",
                place + 1
            ),
            yes: "Continue there",
            no: "Stay here",
        });
        self.damage_all();
        dk::OK
    }

    fn handoff_done(&mut self, s: &Store, h: Handoff, yes: bool) {
        if !yes {
            self.say_note(String::from("Not handed over: you stay where you are"));
            return;
        }
        // An app that already holds it for good keeps that.
        let holds = s
            .grants(fs::app_id(h.to))
            .unwrap_or_default()
            .iter()
            .any(|g| g.file == h.file && g.lasts == Lasts::Live && (g.write || !h.write));
        if !holds
            && s.grant(fs::app_id(h.to), h.file, h.write, Lasts::UntilClosed)
                .is_err()
        {
            self.say_note(String::from("It could not be handed over"));
            return;
        }
        let e = [event::HANDOFF, h.file, h.place, h.from, h.write as u64];
        match self
            .wins
            .iter()
            .rev()
            .find(|w| w.owner == h.to)
            .map(|w| w.id)
        {
            Some(id) => {
                self.push(id, e);
                let mut out = Vec::new();
                self.wm.raise(id, &mut out);
                self.apply(out);
            }
            None => {
                self.hand = Some((h.to, e));
                self.show_app(h.to, st::app_name(fs::app_id(h.to)));
            }
        }
        say!(
            "handed off: {} goes on in {} at line {}",
            s.name_of(h.file, 0).unwrap_or_default(),
            st::app_name(fs::app_id(h.to)),
            h.place + 1
        );
        self.handed = Some(h);
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
        let save = w[1] == 1;
        let answer = if w[1] == 2 { w[2] } else { 0 };
        if w[1] > 2 || (w[1] == 2 && !fs::question_ok(answer)) {
            return dk::BAD_REQUEST;
        }
        let mut p = Pick {
            win,
            role: owner,
            save,
            write: save || (answer == 0 && w[2] != 0),
            files,
            chosen: 0,
            name: String::from(if save { "Untitled.txt" } else { "" }),
            lasts: 0,
            answer,
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
            if save {
                "save"
            } else if answer != 0 {
                "answer"
            } else {
                "open"
            },
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
            Key::Tab if p.answer == 0 => p.lasts = (p.lasts + 1) % p.choices().len(),
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
                if p.answer != 0 {
                    let f = p.files.get(p.chosen)?.file;
                    return match s.grant_answer(app, f, p.answer, fs::ANSWERS_PER_DAY) {
                        Ok(()) => {
                            say!(
                                "{} may ask {} of {}",
                                st::app_name(app),
                                st::question_words(p.answer),
                                s.name_of(f, 0).unwrap_or_default()
                            );
                            Some(f)
                        }
                        Err(e) => {
                            say!("could not give the answer grant: {e}");
                            None
                        }
                    };
                }
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

    /// An app asks for a job, or says its job is done (dk::JOB).
    fn job_request(&mut self, owner: u64, w: &[u64; 6]) -> u64 {
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
        if self.pick.is_some() || self.ask.is_some() || self.job.is_some() || owner == 0 {
            return dk::BAD_REQUEST;
        }
        let app = fs::app_id(owner);
        let stage = match w[1] {
            0 => {
                let docs = s.root(DOCS).unwrap_or(0);
                let mut l: Vec<Entry> = s
                    .list(docs)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|e| e.is_dir)
                    .collect();
                l.sort_by(|a, b| a.name.cmp(&b.name));
                say!(
                    "{} asks for a job; {} folders to choose from",
                    st::app_name(app),
                    l.len()
                );
                JobStage::Folder(l)
            }
            1 => match s.job_check(app) {
                Ok(report) => {
                    say!(
                        "{} says its job is done; Alaya's checks: {}",
                        st::app_name(app),
                        job_summary(&report)
                    );
                    JobStage::Result {
                        report,
                        conflicts: Vec::new(),
                    }
                }
                Err(_) => return dk::BAD_REQUEST,
            },
            _ => return dk::BAD_REQUEST,
        };
        self.job = Some(JobScreen {
            win,
            role: owner,
            chosen: 0,
            stage,
        });
        self.damage_all();
        dk::OK
    }

    /// For STATE: 0 no job screen, 1 choosing the folder, 2 the rules,
    /// 3 the result (passed), 4 held, 5 conflicts to settle.
    fn job_state(&self) -> u64 {
        match &self.job {
            None => 0,
            Some(j) => match &j.stage {
                JobStage::Folder(_) => 1,
                JobStage::Rules { .. } => 2,
                JobStage::Result { conflicts, .. } if !conflicts.is_empty() => 5,
                JobStage::Result { report, .. } if report.passed => 3,
                JobStage::Result { .. } => 4,
            },
        }
    }

    fn job_key(&mut self, k: Key) {
        let Some(j) = &mut self.job else { return };
        match (&j.stage, k) {
            (JobStage::Folder(l), Key::Up) => {
                j.chosen = j.chosen.saturating_sub(1).min(l.len().saturating_sub(1))
            }
            (JobStage::Folder(l), Key::Down) => {
                j.chosen = (j.chosen + 1).min(l.len().saturating_sub(1))
            }
            (JobStage::Rules { .. }, Key::Backspace) => {
                // Back to the folders.
                let (win, role) = (j.win, j.role);
                self.job = None;
                self.job_request(role, &[u64::from(win), 0, 0, 0, 0, 0]);
            }
            (JobStage::Folder(_) | JobStage::Rules { .. }, Key::Escape) => {
                let (win, app) = (j.win, fs::app_id(j.role));
                self.job = None;
                say!("job for {}: you said no", st::app_name(app));
                self.push(win, [event::JOB, dk::job_event::REFUSED, 0, 0, 0]);
            }
            (JobStage::Result { .. }, Key::Escape) => {
                // Later: the work waits on the side.
                self.job = None;
                say!("job result put aside for later");
            }
            (JobStage::Result { .. }, Key::Char('t' | 'T')) => {
                let (win, app) = (j.win, fs::app_id(j.role));
                self.job = None;
                if let Some(s) = self.mine()
                    && s.job_discard(app).is_ok()
                {
                    say!("job of {}: its work was thrown away", st::app_name(app));
                    self.push(win, [event::JOB, dk::job_event::THROWN, 0, 0, 0]);
                }
            }
            (JobStage::Result { .. }, Key::Char('k' | 'K')) => return self.job_ok(true),
            (_, Key::Enter) => return self.job_ok(false),
            _ => {}
        }
        self.damage_all();
    }

    /// The screen's main button: next, start, keep. `deliberate`: the
    /// person clicked it or pressed K; only that keeps work whose checks
    /// failed (rule R5), Enter never does.
    fn job_ok(&mut self, deliberate: bool) {
        let Some(j) = self.job.take() else { return };
        self.damage_all();
        let Some(s) = self.mine() else { return };
        let app = fs::app_id(j.role);
        match j.stage {
            JobStage::Folder(l) => {
                let Some(folder) = l.get(j.chosen).cloned() else {
                    self.job = Some(JobScreen {
                        stage: JobStage::Folder(l),
                        ..j
                    });
                    return;
                };
                // What must stay unchanged: the checklist, and the
                // person's private files (which it does not even see).
                let inside = s.list(folder.file).unwrap_or_default();
                let checklist = inside
                    .iter()
                    .find(|e| !e.is_dir && e.name == CHECKLIST)
                    .map(|e| {
                        let b = s.read_all(e.file).unwrap_or_default();
                        let lines = b
                            .split(|&c| c == b'\n')
                            .filter(|l| {
                                let t = l.trim_ascii();
                                !t.is_empty() && t[0] != b'#'
                            })
                            .count();
                        (e.file, lines)
                    });
                let mut keep_same: Vec<u64> = checklist.iter().map(|c| c.0).collect();
                let mut private = 0;
                for e in &inside {
                    if !e.is_dir && s.private(e.file, None).unwrap_or(false) {
                        private += 1;
                        if keep_same.len() < 8 && !keep_same.contains(&e.file) {
                            keep_same.push(e.file);
                        }
                    }
                }
                say!(
                    "job for {}: the rules for {} ({} must stay unchanged{})",
                    st::app_name(app),
                    folder.name,
                    keep_same.len(),
                    if checklist.is_some() {
                        ", with a checklist"
                    } else {
                        ""
                    }
                );
                self.job = Some(JobScreen {
                    chosen: 0,
                    stage: JobStage::Rules {
                        folder,
                        checklist,
                        keep_same,
                        private,
                    },
                    ..j
                });
            }
            JobStage::Rules {
                folder,
                checklist,
                keep_same,
                ..
            } => {
                let checks = JOB_CHECKS | if checklist.is_some() { CHECK_LIST } else { 0 };
                let label = alloc::format!("job: {}", folder.name);
                match s.job_start(
                    app,
                    folder.file,
                    checks,
                    checklist.map_or(0, |c| c.0),
                    &keep_same,
                    label.as_bytes(),
                ) {
                    Ok(_) => {
                        say!(
                            "job for {} started on a side copy of {}",
                            st::app_name(app),
                            folder.name
                        );
                        self.push(
                            j.win,
                            [event::JOB, dk::job_event::STARTED, folder.file, 0, 0],
                        );
                    }
                    Err(e) => {
                        say!("could not start the job: {e}");
                        self.push(j.win, [event::JOB, dk::job_event::REFUSED, 0, 0, 0]);
                    }
                }
            }
            JobStage::Result { report, conflicts } => {
                if !report.passed && !deliberate {
                    // Enter never keeps held work.
                    self.job = Some(JobScreen {
                        stage: JobStage::Result { report, conflicts },
                        ..j
                    });
                    return;
                }
                match s.job_keep(app, !report.passed, &conflicts) {
                    Ok((k, files)) if k == fs::job_kept::KEPT => {
                        say!(
                            "job of {}: kept{} ({} files)",
                            st::app_name(app),
                            if report.passed {
                                ""
                            } else {
                                " anyway, on record"
                            },
                            files.len()
                        );
                        self.push(
                            j.win,
                            [event::JOB, dk::job_event::KEPT, files.len() as u64, 0, 0],
                        );
                    }
                    Ok((k, files)) if k == fs::job_kept::CONFLICTS => {
                        say!(
                            "job of {}: you changed {} of the same files; asking",
                            st::app_name(app),
                            files.len()
                        );
                        self.job = Some(JobScreen {
                            stage: JobStage::Result {
                                report,
                                conflicts: files,
                            },
                            ..j
                        });
                    }
                    _ => {
                        // Held after all (something changed meanwhile):
                        // look again.
                        if let Ok(report) = s.job_check(app) {
                            self.job = Some(JobScreen {
                                stage: JobStage::Result {
                                    report,
                                    conflicts: Vec::new(),
                                },
                                ..j
                            });
                        }
                    }
                }
            }
        }
    }

    /// What the job screen shows.
    fn job_view(&self, j: &JobScreen) -> JobView {
        let app = capital(st::app_name(fs::app_id(j.role)));
        let s = self.mine();
        let name = |f: u64| {
            s.as_ref()
                .and_then(|s| s.name_of(f, 0).ok())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| String::from("a new file"))
        };
        match &j.stage {
            JobStage::Folder(l) => JobView {
                title: alloc::format!("{app} asks for a job"),
                hint: String::from(
                    "Choose the one folder it may change. Nothing else of yours is there for it.",
                ),
                rows: l
                    .iter()
                    .map(|e| alloc::format!("{}   ·   folder", e.name))
                    .collect(),
                mark: true,
                foot: String::from("Next: you see the rules before it starts."),
                ok: "Next",
                no: "Cancel",
                field: None,
                empty: "No folders in your documents yet: make one in the file browser.",
            },
            JobStage::Rules {
                folder,
                checklist,
                keep_same,
                private,
            } => {
                let mut same = Vec::new();
                if checklist.is_some() {
                    same.push(String::from(CHECKLIST));
                }
                if *private > 0 {
                    same.push(alloc::format!(
                        "{private} private file{}",
                        if *private == 1 { "" } else { "s" }
                    ));
                }
                let mut rows = alloc::vec![
                    alloc::format!("May change: files in {}   ·   enforced", folder.name),
                    alloc::format!(
                        "Must stay unchanged: {}   ·   enforced",
                        if same.is_empty() {
                            String::from("nothing named")
                        } else {
                            same.join(", ")
                        }
                    ),
                    String::from("Your private files: invisible to it   ·   enforced"),
                    String::from("Everything else of yours: not there for it   ·   enforced"),
                    String::from(
                        "Before keeping: nothing lost, nothing emptied   ·   checked by Alaya"
                    ),
                ];
                if let Some((_, n)) = checklist {
                    rows.push(alloc::format!(
                        "Before keeping: the {n} lines of {CHECKLIST}, as now   ·   checked by Alaya"
                    ));
                }
                rows.push(String::from(
                    "Internet: none (Alaya has no network yet)   ·   enforced",
                ));
                rows.push(String::from(
                    "\"Do a good job\"   ·   a wish: Alaya can't check this",
                ));
                let _ = keep_same;
                JobView {
                    title: alloc::format!("{app}: the rules of this job"),
                    hint: String::from(
                        "It works on a side copy. Nothing is kept before you see the result.",
                    ),
                    rows,
                    mark: false,
                    foot: String::from(
                        "Enforced: it cannot. Checked: Alaya looks itself, when it is done.",
                    ),
                    ok: "Start",
                    no: "Cancel",
                    field: None,
                    empty: "No folders in your documents yet: make one in the file browser.",
                }
            }
            JobStage::Result { report, conflicts } => {
                let mut rows = Vec::new();
                for &(check, line, ok, f) in &report.checks {
                    let what = match check {
                        1 => String::from("Nothing outside the folder changed"),
                        2 => String::from("What must stay unchanged did"),
                        4 => String::from("Nothing lost"),
                        8 => String::from("Nothing emptied"),
                        _ => alloc::format!("{CHECKLIST}, line {line}"),
                    };
                    rows.push(if ok {
                        alloc::format!("Passed: {what}")
                    } else if f != 0 {
                        alloc::format!("Did not pass: {what} ({})", name(f))
                    } else {
                        alloc::format!("Did not pass: {what}")
                    });
                }
                for &f in &report.stale {
                    rows.push(alloc::format!(
                        "Needs a look: it read {}, which you changed since",
                        name(f)
                    ));
                }
                if !conflicts.is_empty() {
                    let names: Vec<String> = conflicts.iter().map(|&f| name(f)).collect();
                    JobView {
                        title: alloc::format!("{app}: you changed some of the same files"),
                        hint: alloc::format!(
                            "Yours stays for: {}. The rest of its work is kept.",
                            names.join(", ")
                        ),
                        rows,
                        mark: false,
                        foot: String::from("Nothing is ever overwritten without asking (rule B6)."),
                        ok: "Keep, mine wins",
                        no: "Later",
                        field: None,
                        empty: "No folders in your documents yet: make one in the file browser.",
                    }
                } else if report.passed {
                    JobView {
                        title: alloc::format!("{app} is done: every check passed"),
                        hint: String::from(
                            "Checked by Alaya on the side copy, not by the app. Passing is not proof it is right.",
                        ),
                        rows,
                        mark: false,
                        foot: String::from(
                            "Keep: one step; What happened? can take it back. T throws it away.",
                        ),
                        ok: "Keep",
                        no: "Later",
                        field: None,
                        empty: "No folders in your documents yet: make one in the file browser.",
                    }
                } else {
                    JobView {
                        title: alloc::format!("{app} is done: held, some checks did not pass"),
                        hint: String::from(
                            "Nothing reached your files. Look at the reasons below.",
                        ),
                        rows,
                        mark: false,
                        foot: String::from(
                            "Keep anyway (K or click) is your call, on record. T throws it away.",
                        ),
                        ok: "Keep anyway",
                        no: "Later",
                        field: None,
                        empty: "No folders in your documents yet: make one in the file browser.",
                    }
                }
            }
        }
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
        // Asking for a job ends with the window; a result stays (the work
        // waits on the side for the person).
        if self
            .job
            .as_ref()
            .is_some_and(|j| j.win == x.id && !matches!(j.stage, JobStage::Result { .. }))
        {
            self.job = None;
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
                self.windowless &= !(1 << owner);
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
                        // From the pictures' own block, else the desktop's.
                        let pics = start::FIRST_GIVEN + dk::G_PICTURES;
                        let Ok(h) = self.mem.slot() else {
                            return ([dk::NO_MEMORY, 0, 0, 0, 0, 0], 0);
                        };
                        if alaya_sys::retype(pics, Kind::Page, 21, 1, self.mem.at(h)).is_err()
                            && alaya_sys::retype(self.mem.ut, Kind::Page, 21, 1, self.mem.at(h))
                                .is_err()
                        {
                            return ([dk::NO_MEMORY, 0, 0, 0, 0, 0], 0);
                        }
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
                if role::installed(owner) {
                    self.started(owner - role::INSTALLED);
                }
                if let Some((r, e)) = self.hand
                    && r == owner
                {
                    self.hand = None;
                    self.push(id, e);
                }
                ([dk::OK, u64::from(id), 0, 0, 0, 0], 0)
            }
            dk::PICK => ([self.pick_start(owner, &w), 0, 0, 0, 0, 0], 0),
            dk::HANDOFF if owner != 0 => ([self.handoff_start(owner, &w), 0, 0, 0, 0, 0], 0),
            dk::HANDOFF => (
                self.handed.map_or([dk::BAD_REQUEST, 0, 0, 0, 0, 0], |h| {
                    [dk::OK, h.from, h.to, h.file, h.place, h.write as u64]
                }),
                0,
            ),
            dk::JOB => ([self.job_request(owner, &w), 0, 0, 0, 0, 0], 0),
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
                    let n = self.dock().len();
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
                    let scale = self.wm.get(self.wins[i].id).map_or(100, |x| x.scale);
                    (
                        [dk::OK, cw as u64, ch as u64, scale as u64, self.seeing, 0],
                        0,
                    )
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::CLOSE => match find(self, w[0]) {
                Some(i) => {
                    let id = self.wins[i].id;
                    self.close_window(id);
                    // No window, no processor time: an installed app
                    // keeps running only with the background line.
                    if role::installed(owner)
                        && !self.wins.iter().any(|w| w.owner == owner)
                        && !self.allowed(owner, alaya_bundle::promise::BACKGROUND)
                    {
                        say!("its last window closed: app {owner} ends (no background time)");
                        let _ = alaya_sys::signal(self.session_ntfn, 1 << (dk::END_SHIFT + owner));
                    } else if role::installed(owner) && !self.wins.iter().any(|w| w.owner == owner)
                    {
                        // Running with no window: game mode freezes it.
                        self.windowless |= 1 << owner;
                    }
                    ([dk::OK, 0, 0, 0, 0, 0], 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::SEEING if owner != 0 => match find(self, w[0]) {
                Some(_) => {
                    if w[1] != u64::MAX {
                        self.set_seeing(w[1] & (dk::seeing::CONTRAST | dk::seeing::READER));
                    }
                    ([dk::OK, self.seeing, 0, 0, 0, 0], 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::TYPING if owner == 0 => (self.typing_request(w[1]), 0),
            dk::TYPED if owner == 0 => (self.typed(), 0),
            dk::OFFER_INSTALL if owner == 0 || owner == role::FILES => {
                ([self.offer_install(w[0]), 0, 0, 0, 0, 0], 0)
            }
            dk::INSTALLED if owner == 0 => (self.installed_words(w[0]), 0),
            dk::TRIAL if owner == 0 => (self.trial_words(w[0]), 0),
            dk::GAME if owner == 0 => (self.game_words(), 0),
            dk::BACKGROUND if owner != 0 => {
                let ok = self.may(owner, alaya_bundle::promise::BACKGROUND);
                (
                    [if ok { dk::OK } else { dk::NOT_ALLOWED }, 0, 0, 0, 0, 0],
                    0,
                )
            }
            dk::STARTING if owner == 0 && role::installed(w[0]) => {
                let i = (w[0] - role::INSTALLED) as usize;
                self.starting[i] = alaya_sys::rt::now_ms();
                self.running_version[i] = w[1] as u32;
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
            dk::TYPING => match find(self, w[0]) {
                Some(_) => (self.typing_request(w[1]), 0),
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::SCALE => match find(self, w[0]) {
                Some(i) => {
                    let id = self.wins[i].id;
                    (self.scale_request(id, w[1]), 0)
                }
                None => ([dk::NO_WINDOW, 0, 0, 0, 0, 0], 0),
            },
            dk::LOOK_CHANGED => {
                self.read_look();
                ([dk::OK, 0, 0, 0, 0, 0], 0)
            }
            // Any app may tell the person something; how soon it shows is
            // Alaya's choice, never the app's (decision inbox).
            // An installed app only when its promise says so and the
            // person left the line ticked; a refused ask is noted.
            dk::NOTIFY if !self.may(owner, alaya_bundle::promise::NOTIFY) => {
                ([dk::NOT_ALLOWED, 0, 0, 0, 0, 0], 0)
            }
            dk::NOTIFY if owner != 0 => (
                [
                    self.post(owner, w[0] & 0xff, inbox::text_of(w[0] >> 8, &w[1..5])),
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
                0,
            ),
            dk::NOW if owner == 0 => (
                [
                    self.post(0, w[0] & 0xff, inbox::text_of(w[0] >> 8, &w[1..5])),
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
                0,
            ),
            dk::INBOX if owner == 0 => (self.inbox_counts(), 0),
            dk::GROUP_FILE if owner == role::FILES || owner == 0 => {
                ([self.group_choose(w[0]), 0, 0, 0, 0, 0], 0)
            }
            dk::GROUP_GO if owner == 0 => {
                self.came_forward = 0;
                let r = self.group_go(w[0] as u32);
                ([r, self.came_forward, 0, 0, 0, 0], 0)
            }
            dk::BRIEF_LINE if owner == 0 => self.brief_line(w[0] as usize),
            dk::READER if owner == 0 => {
                let mut b = [0u8; 16];
                let n = self.reader.said.len().min(16);
                b[..n].copy_from_slice(&self.reader.said.as_bytes()[..n]);
                let word = |k: usize| u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
                (
                    [
                        dk::OK,
                        self.reader.spoken,
                        self.reader.role,
                        self.reader.files,
                        word(0),
                        word(1),
                    ],
                    0,
                )
            }
            dk::SEEING if owner == 0 => {
                if w[1] != u64::MAX {
                    self.set_seeing(w[1] & (dk::seeing::CONTRAST | dk::seeing::READER));
                }
                ([dk::OK, self.seeing, 0, 0, 0, 0], 0)
            }
            dk::GROUP_SCREEN if owner == 0 => ([self.group_open(w[0] as u32), 0, 0, 0, 0, 0], 0),
            dk::MAKE_AGAIN if owner == role::FILES || owner == 0 => {
                ([self.make_again(w[0]), 0, 0, 0, 0, 0], 0)
            }
            dk::RUN_AGAIN if owner == role::BACK || owner == 0 => {
                let (st, n, exact) = self.run_again(w[0], w[1]);
                ([st, n, exact, 0, 0, 0], 0)
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
                let n = self.dock().len();
                let (yes, no) = scene::question_buttons(self.width, self.height);
                let r = match w[0] {
                    k if k >= dk::places::KEY => self.typing_place(k),
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
                        | self.job_state() << 48
                        | self.group_state() << 40
                        | u64::from(self.inbox_at.is_some()) << 44
                        | (self.rules.len().min(7) as u64) << 45
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

/// What the dock holds of Alaya's own apps in each look.
fn dock_items(look: Look) -> impl Iterator<Item = (&'static str, u32, u64)> {
    DOCK.iter().copied().filter(move |d| match look {
        Look::Everyday => d.2 != role::CHART,
        Look::Play => d.2 == role::FILES || d.2 == role::SETTINGS,
        Look::Everything | Look::Create | Look::Build => true,
    })
}

impl Desk {
    /// The dock: Alaya's own apps for the look, then the apps the person
    /// installed and approved (milestone 8).
    pub(crate) fn dock(&self) -> Vec<(String, u32, u64)> {
        let mut v: Vec<(String, u32, u64)> = dock_items(self.look)
            .map(|(n, c, r)| (String::from(n), c, r))
            .collect();
        v.extend(self.installed_dock());
        v
    }
}

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

/// What the job screen shows (see `job_view`).
struct JobView {
    title: String,
    hint: String,
    rows: Vec<String>,
    mark: bool,
    foot: String,
    ok: &'static str,
    no: &'static str,
    /// What is typed (naming a group).
    field: Option<(&'static str, String)>,
    /// What shows when there are no rows.
    empty: &'static str,
}

/// Alaya's checks of a job, in a few words (the log).
fn job_summary(r: &st::JobReport) -> String {
    let failed = r.checks.iter().filter(|c| !c.2).count();
    alloc::format!(
        "{} of {} passed, {} read files changed since{}",
        r.checks.len() - failed,
        r.checks.len(),
        r.stale.len(),
        if r.passed { "; all passed" } else { "; held" }
    )
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

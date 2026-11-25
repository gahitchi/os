// SPDX-License-Identifier: GPL-3.0-only
//! What an app needs to open a window on the desktop and draw in it with
//! the toolkit: the window's picture, its events, and Alaya's letters.

#![no_std]

extern crate alloc;

pub mod c;
pub mod libc;
pub mod store;

use alaya_sys::abi::{StartInfo, map, start};
use alaya_sys::rt::Mem;
use alaya_sys::{Caps, Msg, MsgInfo};
use alaya_ui::Rect;
use alaya_ui::canvas::Canvas;
use alaya_ui::text::Fonts;
use alaya_ui::widgets::{Key, Node, Pointer, encode_tree, us_key};

use alaya_programs::desktop::{self as dk, event};

/// Where the window's picture is mapped in the app.
const PICTURE: u64 = dk::PICTURES;

/// The fonts the first program shares with every app.
pub fn fonts(info: &StartInfo) -> Fonts<'static> {
    let f = |i: usize| {
        let len = info.args[dk::ARG_FONTS + i] as usize;
        // SAFETY: shared read only at this place before the app starts.
        unsafe {
            core::slice::from_raw_parts((dk::FONTS + i as u64 * dk::FONT_STEP) as *const u8, len)
        }
    };
    Fonts::new(f(0), f(1), f(2), f(3))
}

/// Something that happened to the window, already turned into meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    /// A letter typed with Ctrl held (Ctrl+S: save).
    Shortcut(char),
    /// Pointer at (x, y) in the content; button 0 for a move.
    Pointer {
        x: i32,
        y: i32,
        button: u8,
        down: bool,
    },
    Resized(i32, i32),
    Close,
    Focus(bool),
    /// The Open or Save window closed: the document handed over (None:
    /// the person cancelled), and whether it was a save.
    /// `pinned`: one older version, read only (a search found it there).
    Picked {
        file: Option<u64>,
        save: bool,
        pinned: bool,
    },
    /// Live handoff: go on with this document at this place (a line,
    /// from 0); it came from the app with role `from`.
    HandedOff {
        file: u64,
        place: u64,
        from: u64,
    },
    /// About the job it asked for (`dk::job_event`), with the folder it
    /// may change (started) or how many files were kept.
    Job {
        what: u64,
        arg: u64,
    },
    /// Make this result again from its sources as they are now.
    MakeAgain(u64),
    /// Show this moment (a commit) on the timeline.
    Moment(u64),
}

pub struct Window {
    ep: u64,
    pub ntfn: u64,
    pub id: u64,
    /// The size in points: what the app draws in.
    pub width: i32,
    pub height: i32,
    /// Pixels per 100 points on the window's screen (milestone 7b).
    pub scale: i32,
    /// The size in pixels.
    pw: i32,
    ph: i32,
    /// The person asked for high contrast: the canvas draws in it.
    pub contrast: bool,
    /// What was last handed to the screen reader (a checksum).
    told: u64,
    /// Pointer state for the toolkit (in points).
    pub pointer: Pointer,
}

fn call(ep: u64, label: u64, words: &[u64]) -> [u64; 6] {
    alaya_sys::ipc_call(ep, &Msg::new(label, words), Caps::default())
        .map(|m| m.words)
        .unwrap_or([u64::MAX; 6])
}

impl Window {
    /// Open a window with content `w` by `h` and the title `title`.
    pub fn open(mem: &mut Mem, w: i32, h: i32, title: &str) -> Option<Window> {
        let g = start::FIRST_GIVEN;
        let ep = g + dk::APP_G_DESKTOP;
        let ntfn = g + dk::APP_G_NTFN;
        let m = Msg {
            info: MsgInfo::new(dk::OPEN, 2).with_cap(),
            badge: 0,
            words: [w as u64, h as u64, 0, 0, 0, 0],
        };
        let r = alaya_sys::ipc_call(
            ep,
            &m,
            Caps {
                send: ntfn,
                ..Caps::default()
            },
        )
        .ok()?;
        if r.words[0] != dk::OK {
            return None;
        }
        let id = r.words[1];
        for i in 0..dk::PAGES {
            let slot = mem.slot().ok()?;
            let r = alaya_sys::ipc_call(
                ep,
                &Msg::new(dk::PAGE, &[id, i]),
                Caps {
                    recv: slot,
                    ..Caps::default()
                },
            )
            .ok()?;
            if r.words[0] != dk::OK {
                return None;
            }
            alaya_sys::page_map(slot, start::SPACE, PICTURE + i * (2 << 20), map::WRITE).ok()?;
        }
        let mut win = Window {
            ep,
            ntfn,
            id,
            width: w,
            height: h,
            scale: 100,
            pw: w,
            ph: h,
            contrast: false,
            told: 0,
            pointer: Pointer::default(),
        };
        win.set_title(title);
        let s = call(ep, dk::SIZE, &[id]);
        if s[0] == dk::OK {
            win.sized(s[1], s[2], s[3], s[4]);
        }
        Some(win)
    }

    /// The desktop says the size in pixels and the scale.
    fn sized(&mut self, w: u64, h: u64, scale: u64, seeing: u64) {
        self.contrast = seeing & dk::seeing::CONTRAST != 0;
        self.scale = if (50..=400).contains(&scale) {
            scale as i32
        } else {
            100
        };
        (self.pw, self.ph) = (w as i32, h as i32);
        self.width = self.pw * 100 / self.scale;
        self.height = self.ph * 100 / self.scale;
    }

    /// Pixels to points.
    fn pt(&self, v: i32) -> i32 {
        v * 100 / self.scale
    }

    /// How big things are drawn on this window's screen (percent), and
    /// what the person chose there (0: Alaya picks).
    pub fn screen_scale(&self) -> (u64, u64) {
        let r = call(self.ep, dk::SCALE, &[self.id, u64::MAX]);
        if r[0] == dk::OK {
            (r[1], r[2])
        } else {
            (100, 0)
        }
    }

    /// The keyboard layout and the way of typing (`dk::TYPING`): set
    /// the layout (or `u64::MAX` to ask) → (layout, mode).
    pub fn typing(&self, layout: u64) -> (u64, u64) {
        let r = call(self.ep, dk::TYPING, &[self.id, layout]);
        if r[0] == dk::OK { (r[1], r[2]) } else { (0, 0) }
    }

    /// High contrast and the screen reader, as the person has them.
    pub fn seeing(&self) -> u64 {
        let r = call(self.ep, dk::SEEING, &[self.id, u64::MAX]);
        if r[0] == dk::OK { r[1] } else { 0 }
    }

    /// Turn high contrast or the screen reader on or off (Settings).
    pub fn set_seeing(&self, bits: u64) -> bool {
        call(self.ep, dk::SEEING, &[self.id, bits])[0] == dk::OK
    }

    /// Choose how big things are drawn on this window's screen (one of
    /// [`dk::SCALES`]); the window hears of it as a resize.
    pub fn set_screen_scale(&self, pct: u64) -> bool {
        call(self.ep, dk::SCALE, &[self.id, pct])[0] == dk::OK
    }

    /// Tell the person something (32 bytes at most). `kind` is only a
    /// wish (`dk::inbox`): Alaya decides how soon it shows.
    pub fn notify(&self, kind: u64, text: &str) -> bool {
        let mut b = [0u8; 32];
        let mut n = text.len().min(32);
        while !text.is_char_boundary(n) {
            n -= 1;
        }
        b[..n].copy_from_slice(&text.as_bytes()[..n]);
        let w = |k: usize| u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
        call(
            self.ep,
            dk::NOTIFY,
            &[kind | (n as u64) << 8, w(0), w(1), w(2), w(3)],
        )[0] == dk::OK
    }

    /// Ask to keep running when the last window closes (`dk::BACKGROUND`).
    pub fn background(&self) -> bool {
        call(self.ep, dk::BACKGROUND, &[])[0] == dk::OK
    }

    pub fn set_title(&self, title: &str) {
        let mut b = [0u8; 32];
        // Cut at a character boundary.
        let mut n = title.len().min(32);
        while !title.is_char_boundary(n) {
            n -= 1;
        }
        b[..n].copy_from_slice(&title.as_bytes()[..n]);
        let w = |k: usize| u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
        call(
            self.ep,
            dk::TITLE,
            &[self.id, n as u64, w(0), w(1), w(2), w(3)],
        );
    }

    /// The window's picture, as a canvas of its current size.
    pub fn canvas(&self) -> Canvas<'static> {
        // SAFETY: the two picture pages are mapped at PICTURE.
        let px = unsafe {
            core::slice::from_raw_parts_mut(PICTURE as *mut u32, (dk::STRIDE * dk::MAX_H) as usize)
        };
        Canvas::new(px, self.pw, self.ph, dk::STRIDE as i32)
            .expect("picture")
            .scaled(self.scale)
            .with_contrast(self.contrast)
    }

    /// Hand the screen reader what the window shows (the toolkit's
    /// tree of this pass); only a change is written.
    pub fn tell(&mut self, tree: &[Node]) {
        let b = encode_tree(tree, (dk::TREE_MAX - 8) as usize);
        let sum = b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
            (h ^ u64::from(x)).wrapping_mul(0x100_0000_01b3)
        });
        if sum == self.told {
            return;
        }
        self.told = sum;
        let at = (PICTURE + dk::TREE_AT) as *mut u8;
        // SAFETY: the tree's place is in the mapped picture pages, after
        // the pixels; the desktop only reads it.
        unsafe {
            let count = at as *mut u32;
            let n = count.read_volatile().wrapping_add(1);
            (at.add(4) as *mut u32).write_volatile(b.len() as u32);
            core::ptr::copy_nonoverlapping(b.as_ptr(), at.add(8), b.len());
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
            count.write_volatile(n);
        }
    }

    /// Tell the desktop a part changed (in points).
    pub fn damage(&self, r: Rect) {
        let s = |v: i32| (v * self.scale).div_euclid(100);
        let (x, y) = (s(r.x), s(r.y));
        // Rounded out, so the last pixel row of the part is in.
        let (w, h) = (s(r.x + r.w) - x + 1, s(r.y + r.h) - y + 1);
        call(
            self.ep,
            dk::DAMAGE,
            &[self.id, x as u64, y as u64, w as u64, h as u64],
        );
    }

    pub fn damage_all(&self) {
        self.damage(Rect::new(0, 0, self.width, self.height));
    }

    pub fn close(&self) {
        call(self.ep, dk::CLOSE, &[self.id]);
    }

    /// Ask Alaya for the Open window (or the Save window): the person
    /// picks, and the app gets that document and nothing else, as a
    /// [`Event::Picked`]. `write`: the app wants to change it.
    pub fn pick(&self, save: bool, write: bool) -> bool {
        call(self.ep, dk::PICK, &[self.id, save as u64, write as u64, 0])[0] == dk::OK
    }

    /// Ask Alaya for an answer window (milestone 8, step 3): the person
    /// picks a document, and the app may then ask `question` (from
    /// [`alaya_programs::files::question`]) about it with
    /// [`store::Store::answer`], getting one number and never the file.
    /// The picked document comes back as an [`Event::Picked`].
    pub fn ask_answer(&self, question: u64) -> bool {
        call(self.ep, dk::PICK, &[self.id, 2, question, 0])[0] == dk::OK
    }

    /// Hand the document this app holds to the app with role `to`, at
    /// `place` (a line, from 0): live handoff. Alaya asks the person
    /// first; the other app gets an [`Event::HandedOff`].
    pub fn hand_off(&self, to: u64, file: u64, place: u64) -> bool {
        call(self.ep, dk::HANDOFF, &[self.id, to, file, place])[0] == dk::OK
    }

    /// Ask for a job with rules (`done` false), or say the job is done
    /// (true): Alaya shows the person the rules, or what its checks
    /// found. The answer comes later as an [`Event::Job`].
    pub fn ask_job(&self, done: bool) -> bool {
        call(self.ep, dk::JOB, &[self.id, done as u64])[0] == dk::OK
    }

    /// The next event, if one is waiting. Keeps size and pointer state.
    pub fn next_event(&mut self) -> Option<Event> {
        loop {
            let r = call(self.ep, dk::EVENT, &[self.id]);
            if r[0] != dk::OK {
                return None;
            }
            let e = match r[1] {
                event::KEY => {
                    // Presses and repeats; releases mean nothing to apps.
                    if r[3] == 0 {
                        continue;
                    }
                    match us_key(r[2] as u16, r[4] != 0) {
                        // Ctrl with a letter; Ctrl+Shift+V alone keeps its capital
                        // (paste a plain copy).
                        Some(Key::Char(c)) if r[5] != 0 => {
                            Event::Shortcut(if c == 'V' { c } else { c.to_ascii_lowercase() })
                        }
                        Some(k) => Event::Key(k),
                        None => continue,
                    }
                }
                event::POINTER => {
                    let (x, y) = (self.pt(r[2] as i32), self.pt(r[3] as i32));
                    let button = (r[4] & 0xff) as u8;
                    let down = r[4] >> 8 != 0;
                    let p = &mut self.pointer;
                    p.x = x;
                    p.y = y;
                    if button != 0 {
                        if down {
                            p.down = true;
                            p.down_x = x;
                            p.down_y = y;
                        } else {
                            p.down = false;
                            p.released = true;
                        }
                    }
                    Event::Pointer { x, y, button, down }
                }
                event::RESIZED => {
                    self.sized(r[2], r[3], r[4], r[5]);
                    Event::Resized(self.width, self.height)
                }
                event::CLOSE => Event::Close,
                event::FOCUS => Event::Focus(r[2] != 0),
                event::PICKED => Event::Picked {
                    file: (r[2] != 0).then_some(r[2]),
                    save: r[3] != 0,
                    pinned: r[5] == 1,
                },
                event::JOB => Event::Job {
                    what: r[2],
                    arg: r[3],
                },
                event::MAKE_AGAIN => Event::MakeAgain(r[2]),
                event::MOMENT => Event::Moment(r[2]),
                event::HANDOFF => Event::HandedOff {
                    file: r[2],
                    place: r[3],
                    from: r[4],
                },
                event::TEXT => match char::from_u32(r[2] as u32) {
                    Some(c) if !c.is_control() => Event::Key(Key::Char(c)),
                    _ => continue,
                },
                _ => continue,
            };
            return Some(e);
        }
    }

    /// Wait until events arrive.
    pub fn wait(&self) {
        let _ = alaya_sys::wait(self.ntfn);
    }
}

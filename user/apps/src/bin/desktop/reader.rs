// SPDX-License-Identifier: GPL-3.0-only
//! The screen reader, and Alaya's own parts by keyboard (milestone 7b,
//! step 2; decisions 133 and 134).
//!
//! **The reader** says what has the keyboard: the window in front (its
//! title, and how many of the person's files its app can see, so the
//! privacy picture reaches everyone), then the piece in it that has the
//! keyboard, its role and its state. Apps hand their toolkit's tree to
//! the desktop in their picture pages (`dk::TREE_AT`); the reader reads
//! it there when it changes. Until Alaya has sound (milestone 11) it
//! shows its words in a caption bar at the top of the main screen, and
//! in the log. Alaya key + Enter turns it on or off.
//!
//! **The walk**: Alaya key + Tab goes through the strip's buttons, the
//! dock and the group labels; Tab or the arrows move on, Enter presses,
//! Escape goes back to the window in front. Alaya key + C turns high
//! contrast on or off.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_programs::desktop::{self as dk, reader_role as rr, role};
use alaya_programs::files as fs;
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::scene;
use alaya_ui::widgets::{Key, Role, decode_tree, say_focused};

use super::{Desk, capital, dock_items};

/// The caption bar's height.
pub(crate) const CAPTION_H: i32 = 30;

#[derive(Default)]
pub(crate) struct Reader {
    /// How many times it spoke.
    pub(crate) spoken: u64,
    /// What it said last, and before that (the bar shows both).
    pub(crate) said: String,
    pub(crate) before: String,
    /// The role of what it said last (`dk::reader_role`).
    pub(crate) role: u64,
    /// How many files the window's app can see (`u64::MAX`: all, as part
    /// of Alaya).
    pub(crate) files: u64,
    /// The window and the change count of its tree last read.
    seen: (u32, u32),
}

/// One stop of the walk through Alaya's own parts.
struct Stop {
    rect: Rect,
    name: String,
    what: &'static str,
}

impl Desk {
    pub(crate) fn reader_on(&self) -> bool {
        self.seeing & dk::seeing::READER != 0
    }

    /// Where the caption bar is: the top of the main screen, under the
    /// strip.
    pub(crate) fn caption_rect(&self) -> Rect {
        Rect::new(0, alaya_ui::wm::STRIP, self.width, CAPTION_H)
    }

    fn speak(&mut self, role: u64, text: String) {
        if !self.reader_on() || text == self.reader.said {
            return;
        }
        say!("reader: {text}");
        self.reader.before = core::mem::replace(&mut self.reader.said, text);
        self.reader.role = role;
        self.reader.spoken += 1;
        let r = self.caption_rect();
        self.damage(r);
    }

    /// Something Alaya's own parts say (the emoji picker's choice).
    pub(crate) fn reader_say(&mut self, text: String) {
        self.speak(rr::SHELL, text);
    }

    /// The reader was turned on or off.
    pub(crate) fn reader_changed(&mut self) {
        self.reader.seen = (0, 0);
        self.reader.said.clear();
        self.reader.before.clear();
        if self.reader_on() {
            self.speak(rr::SHELL, String::from("Screen reader on."));
            if let Some(id) = self.wm.focused() {
                self.reader_window(id);
            }
        }
        let r = self.caption_rect();
        self.damage(r);
    }

    /// A window came to the front: its title, and what its app can see.
    pub(crate) fn reader_window(&mut self, id: u32) {
        if !self.reader_on() {
            return;
        }
        let Some(w) = self.wins.iter().find(|w| w.id == id) else {
            return;
        };
        let (title, owner) = (w.title.clone(), w.owner);
        let app = fs::app_id(owner);
        let carrier = [role::FILES, role::SETTINGS, role::BACK].contains(&owner);
        let files = if carrier {
            u64::MAX
        } else {
            self.mine()
                .and_then(|s| s.grants(app).ok())
                .map_or(0, |g| g.len() as u64)
        };
        self.reader.files = files;
        let who = capital(fs::app_label(app));
        let sees = match files {
            u64::MAX => format!("{who} is part of Alaya and can see all your files."),
            0 => format!("{who} can see none of your files."),
            1 => format!("{who} can see 1 of your files."),
            n => format!("{who} can see {n} of your files."),
        };
        self.reader.seen = (id, u32::MAX);
        self.speak(rr::WINDOW, format!("{title}, window. {sees}"));
    }

    /// Read the tree of the window in front when it changed, and say
    /// what has the keyboard now.
    pub(crate) fn reader_poll(&mut self) {
        if !self.reader_on() || self.ring.is_some() {
            return;
        }
        let Some(id) = self.wm.focused() else { return };
        let Some(w) = self.wins.iter().find(|w| w.id == id) else {
            return;
        };
        let at = (w.va + dk::TREE_AT) as *const u8;
        // SAFETY: the window's picture pages are mapped at `va`, and the
        // tree's place is inside them; the app may be writing it, so what
        // is read is checked, and read again when the count moves.
        let (count, bytes) = unsafe {
            let count = (at as *const u32).read_volatile();
            if (id, count) == self.reader.seen {
                return;
            }
            let len =
                ((at.add(4) as *const u32).read_volatile() as usize).min(dk::TREE_MAX as usize - 8);
            let mut b = alloc::vec![0u8; len];
            core::ptr::copy_nonoverlapping(at.add(8), b.as_mut_ptr(), len);
            (count, b)
        };
        let first = self.reader.seen.0 != id;
        self.reader.seen = (id, count);
        if first {
            self.reader_window(id);
            self.reader.seen = (id, count);
        }
        if let Some((role, text)) = say_focused(&decode_tree(&bytes)) {
            let code = match role {
                Role::Label => rr::LABEL,
                Role::Button => rr::BUTTON,
                Role::TextBox => rr::TEXT_BOX,
                Role::List => rr::LIST,
                Role::ListItem => rr::LIST_ITEM,
                Role::Group => rr::GROUP,
            };
            self.speak(code, text);
        }
    }

    /// Alaya's own parts the walk goes through, in order.
    fn stops(&self) -> Vec<Stop> {
        let mut v = Vec::new();
        for (r, n) in scene::strip_buttons().iter().zip(scene::STRIP_BUTTONS) {
            v.push(Stop {
                rect: *r,
                name: String::from(n),
                what: "button on the strip",
            });
        }
        let n = dock_items(self.look).count();
        for (i, (name, _, _)) in dock_items(self.look).enumerate() {
            v.push(Stop {
                rect: scene::dock_item(self.width, self.height, n, i),
                name: capital(name),
                what: "in the dock",
            });
        }
        for &(g, r) in &self.wm.groups {
            v.push(Stop {
                rect: r,
                name: self.group_label(g).map_or(String::from("a group"), |l| l.0),
                what: "group label",
            });
        }
        v
    }

    /// Where the walk is, to draw its ring.
    pub(crate) fn ring_rect(&self) -> Option<Rect> {
        self.ring
            .and_then(|i| self.stops().get(i).map(|s| s.rect.grow(3)))
    }

    /// Alaya key + Tab (with Shift: backwards).
    pub(crate) fn walk(&mut self, back: bool) {
        let n = self.stops().len();
        if n == 0 {
            return;
        }
        let at = match (self.ring, back) {
            (None, false) => Some(0),
            (None, true) => Some(n - 1),
            (Some(i), false) if i + 1 < n => Some(i + 1),
            (Some(i), true) if i > 0 => Some(i - 1),
            // Past the end: back to the window in front.
            _ => None,
        };
        self.ring_to(at);
    }

    fn ring_to(&mut self, at: Option<usize>) {
        if let Some(r) = self.ring_rect() {
            self.damage(r.grow(2));
        }
        self.ring = at;
        match at.and_then(|i| self.stops().into_iter().nth(i)) {
            Some(s) => {
                let r = s.rect.grow(5);
                self.damage(r);
                say!("walk: {}, {}", s.name, s.what);
                self.speak(rr::SHELL, format!("{}, {}", s.name, s.what));
            }
            None => {
                self.ring = None;
                say!("walk: back to the window");
                self.reader.seen = (0, 0);
                if let Some(id) = self.wm.focused() {
                    self.reader_window(id);
                }
            }
        }
    }

    /// A key while the walk is on; true if it used it.
    pub(crate) fn walk_key(&mut self, k: Key) -> bool {
        let Some(i) = self.ring else { return false };
        match k {
            Key::Tab if self.shift => self.walk(true),
            Key::Tab | Key::Right | Key::Down => self.walk(false),
            Key::Left | Key::Up => self.walk(true),
            Key::Escape => self.ring_to(None),
            Key::Enter => {
                if let Some(s) = self.stops().get(i) {
                    let (x, y) = (s.rect.x + s.rect.w / 2, s.rect.y + s.rect.h / 2);
                    self.ring_to(None);
                    self.shell_click(x, y);
                }
            }
            _ => {}
        }
        true
    }
}

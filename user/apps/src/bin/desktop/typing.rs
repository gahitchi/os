// SPDX-License-Identifier: GPL-3.0-only
//! The input-method server (milestone 7b, step 3; decision 50): between
//! the keyboard and the apps. Key presses for the window in front become
//! text here (`alaya_ui::input`): the person's layout, dead keys,
//! Compose, Japanese and Chinese. Apps get text as `event::TEXT`, and
//! the keys that are not text (Enter, arrows, Ctrl shortcuts) as before.
//!
//! Alaya key + Space switches Latin, Japanese (kana) and Chinese
//! (pinyin); what is being made shows in a small box at the bottom of the
//! window in front. Alaya key + . opens the emoji picker, Alaya key + K
//! the on-screen keyboard (for touch screens; clicks type its keys).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_programs::desktop::{self as dk, event};
use alaya_programs::files::APPS;
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::canvas::Canvas;
use alaya_ui::input::{EMOJI, Ime, Layout, Mode, Out, Sym, code, sym};
use alaya_ui::text::Face;
use alaya_ui::widgets::theme;

use super::Desk;

/// The emoji picker's columns.
const COLS: usize = 8;
const CELL: i32 = 48;

/// The on-screen keyboard's rows, by key code.
const ROWS: [&[u16]; 5] = [
    &[41, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14],
    &[16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 43],
    &[30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 28],
    &[42, 86, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53],
    &[57],
];
const KEY_W: i32 = 50;
const KEY_H: i32 = 44;

/// What the input method keeps.
pub(crate) struct Typing {
    pub(crate) ime: Ime,
    pub(crate) altgr: bool,
    /// The emoji picker: the chosen cell, while open.
    pub(crate) picker: Option<usize>,
    /// The on-screen keyboard is shown, and its Shift.
    pub(crate) osk: bool,
    osk_shift: bool,
    /// What was handed to apps: how many characters, and the last ones
    /// (for checks).
    pub(crate) count: u64,
    pub(crate) last: String,
}

impl Default for Typing {
    fn default() -> Self {
        Typing {
            ime: Ime::new(Layout::Us),
            altgr: false,
            picker: None,
            osk: false,
            osk_shift: false,
            count: 0,
            last: String::new(),
        }
    }
}

impl Desk {
    /// Hand typed text to the window in front, one character at a time.
    fn give_text(&mut self, s: &str) {
        let Some(id) = self.wm.focused() else { return };
        for c in s.chars() {
            self.push(id, [event::TEXT, u64::from(c), 0, 0, 0]);
        }
        self.typing.count += s.chars().count() as u64;
        self.typing.last.push_str(s);
        while self.typing.last.len() > 32 {
            let c = self.typing.last.chars().next().unwrap();
            self.typing.last.drain(..c.len_utf8());
        }
        say!("typed {s}");
    }

    /// A key press for the window in front, through the input method.
    /// False: not text, the window gets the key itself.
    pub(crate) fn type_key(&mut self, code: u16, shift: bool) -> bool {
        let before = self.typing.ime.preedit();
        let out = self.typing.ime.key(code, shift, self.typing.altgr);
        let after = self.typing.ime.preedit();
        if before != after {
            let r = self.preedit_rect();
            self.damage(r);
        }
        match out {
            Out::Commit(s) => {
                self.give_text(&s);
                true
            }
            Out::Held => true,
            Out::Pass => false,
        }
    }

    /// Alaya key + Space: the next input mode.
    pub(crate) fn next_mode(&mut self) {
        if let Some(left) = self.typing.ime.next_mode() {
            self.give_text(&left);
        }
        let m = self.typing.ime.mode;
        say!("typing: {}", m.name());
        self.say_note(format!("Typing: {}. Alaya key + Space switches.", m.name()));
        let r = self.preedit_rect();
        self.damage(r);
    }

    /// The person's layout, from their app files.
    pub(crate) fn load_typing(&mut self) {
        let code = self
            .mine()
            .and_then(|s| {
                s.root(APPS)
                    .and_then(|a| s.lookup(a, "typing"))
                    .and_then(|f| s.read_all(f))
                    .ok()
            })
            .and_then(|b| b.first().copied())
            .unwrap_or(0);
        self.typing.ime.layout = Layout::from_code(code);
    }

    /// TYPING: set (or ask) the layout.
    pub(crate) fn typing_request(&mut self, layout: u64) -> [u64; 6] {
        if layout != u64::MAX {
            if layout as usize >= Layout::ALL.len() {
                return [dk::BAD_REQUEST, 0, 0, 0, 0, 0];
            }
            let l = Layout::from_code(layout as u8);
            self.typing.ime.layout = l;
            if let Some(s) = self.mine()
                && let Ok(a) = s.root(APPS)
                && let Ok(f) = s.open(a, "typing")
            {
                let _ = s.write_all(f, &[l.code()]);
            }
            say!("the keyboard is {}", l.name());
        }
        [
            dk::OK,
            u64::from(self.typing.ime.layout.code()),
            self.typing.ime.mode.code(),
            0,
            0,
            0,
        ]
    }

    /// TYPED, for checks.
    pub(crate) fn typed(&self) -> [u64; 6] {
        let mut b = [0u8; 32];
        let t = self.typing.last.as_bytes();
        b[..t.len()].copy_from_slice(t);
        let w = |k: usize| u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
        [dk::OK, self.typing.count, w(0), w(1), w(2), w(3)]
    }

    /// Where what is being made shows: the bottom left of the window in
    /// front (or the main screen).
    pub(crate) fn preedit_rect(&self) -> Rect {
        let c = self
            .wm
            .focused()
            .and_then(|id| self.wm.get(id))
            .map_or(Rect::new(0, 0, self.width, self.height), |w| w.content());
        Rect::new(c.x + 8, c.bottom() - 52, (c.w - 16).clamp(0, 520), 44)
    }

    // ---- The emoji picker.

    fn picker_rect(&self) -> Rect {
        let rows = EMOJI.len().div_ceil(COLS) as i32;
        let (w, h) = (COLS as i32 * CELL + 24, rows * CELL + 64);
        Rect::new((self.width - w) / 2, (self.height - h) / 2, w, h)
    }

    fn cell(&self, i: usize) -> Rect {
        let p = self.picker_rect();
        Rect::new(
            p.x + 12 + (i % COLS) as i32 * CELL,
            p.y + 12 + (i / COLS) as i32 * CELL,
            CELL,
            CELL,
        )
    }

    pub(crate) fn picker_open(&mut self) {
        self.typing.picker = Some(0);
        say!("emoji picker");
        let r = self.picker_rect();
        self.damage(r);
        self.picker_said();
    }

    fn picker_said(&mut self) {
        if let Some(i) = self.typing.picker {
            let name = EMOJI[i].1;
            self.reader_say(format!("{name}, emoji {} of {}", i + 1, EMOJI.len()));
        }
    }

    /// A key while the picker is open.
    pub(crate) fn picker_key(&mut self, code: u16) {
        let Some(i) = self.typing.picker else { return };
        let n = EMOJI.len();
        let to = match code {
            105 => Some((i + n - 1) % n),
            106 => Some((i + 1) % n),
            103 => Some((i + n - COLS) % n),
            108 => Some((i + COLS) % n),
            code::ENTER => {
                self.picker_pick(i);
                return;
            }
            code::ESC => {
                self.picker_close();
                return;
            }
            _ => None,
        };
        if let Some(t) = to {
            self.typing.picker = Some(t);
            let r = self.picker_rect();
            self.damage(r);
            self.picker_said();
        }
    }

    fn picker_pick(&mut self, i: usize) {
        self.picker_close();
        let mut s = String::new();
        s.push(EMOJI[i].0);
        self.give_text(&s);
    }

    fn picker_close(&mut self) {
        self.typing.picker = None;
        let r = self.picker_rect();
        self.damage(r.grow(4));
    }

    /// A click while the picker is open: a cell picks, outside closes.
    pub(crate) fn picker_click(&mut self, x: i32, y: i32) -> bool {
        if self.typing.picker.is_none() {
            return false;
        }
        match (0..EMOJI.len()).find(|&i| self.cell(i).contains(x, y)) {
            Some(i) => self.picker_pick(i),
            None if !self.picker_rect().contains(x, y) => self.picker_close(),
            None => {}
        }
        true
    }

    // ---- The on-screen keyboard.

    fn osk_rect(&self) -> Rect {
        let w = 14 * (KEY_W + 4) + 16;
        let h = ROWS.len() as i32 * (KEY_H + 4) + 16;
        Rect::new((self.width - w) / 2, self.height - h - 72, w, h)
    }

    /// Each key's place and code.
    fn osk_keys(&self) -> Vec<(Rect, u16)> {
        let o = self.osk_rect();
        let mut v = Vec::new();
        for (r, row) in ROWS.iter().enumerate() {
            let mut x = o.x + 8 + r as i32 * 12;
            for &k in row.iter() {
                let w = match k {
                    57 => 8 * (KEY_W + 4),
                    14 | 28 | 42 => KEY_W + 30,
                    _ => KEY_W,
                };
                let y = o.y + 8 + r as i32 * (KEY_H + 4);
                let x0 = if k == 57 { o.x + 3 * (KEY_W + 4) } else { x };
                v.push((Rect::new(x0, y, w, KEY_H), k));
                x += w + 4;
            }
        }
        v
    }

    pub(crate) fn osk_toggle(&mut self) {
        self.typing.osk = !self.typing.osk;
        say!(
            "on-screen keyboard {}",
            if self.typing.osk { "shown" } else { "hidden" }
        );
        let r = self.osk_rect();
        self.damage(r.grow(4));
    }

    /// A click on the on-screen keyboard: its key, typed.
    pub(crate) fn osk_click(&mut self, x: i32, y: i32) -> bool {
        if !self.typing.osk || !self.osk_rect().contains(x, y) {
            return false;
        }
        let Some((_, k)) = self.osk_keys().into_iter().find(|(r, _)| r.contains(x, y)) else {
            return true;
        };
        if k == 42 {
            self.typing.osk_shift = !self.typing.osk_shift;
        } else {
            let shift = core::mem::take(&mut self.typing.osk_shift);
            if !self.type_key(k, shift)
                && let Some(id) = self.wm.focused()
            {
                // Not text (Backspace, Enter): the key itself.
                self.push(id, [event::KEY, u64::from(k), 1, u64::from(shift), 0]);
                self.push(id, [event::KEY, u64::from(k), 0, u64::from(shift), 0]);
            }
        }
        let r = self.osk_rect();
        self.damage(r);
        true
    }

    /// PLACES for the on-screen keyboard's keys and the picker's cells.
    pub(crate) fn typing_place(&self, what: u64) -> Option<Rect> {
        if (dk::places::KEY..dk::places::EMOJI).contains(&what) && self.typing.osk {
            let k = (what - dk::places::KEY) as u16;
            return self.osk_keys().into_iter().find(|x| x.1 == k).map(|x| x.0);
        }
        if what >= dk::places::EMOJI && self.typing.picker.is_some() {
            let i = (what - dk::places::EMOJI) as usize;
            return (i < EMOJI.len()).then(|| self.cell(i));
        }
        None
    }

    /// Draw what the input method shows: what is being made, the picker,
    /// the on-screen keyboard.
    pub(crate) fn draw_typing(&mut self, cv: &mut Canvas) {
        // Places first; then the fonts, which drawing changes.
        let pre = self.typing.ime.preedit();
        let pre_rect = self.preedit_rect();
        let picker = self.typing.picker.map(|c| {
            let cells: Vec<Rect> = (0..EMOJI.len()).map(|i| self.cell(i)).collect();
            (c, self.picker_rect(), cells)
        });
        let osk = self.typing.osk.then(|| (self.osk_rect(), self.osk_keys()));
        let layout = self.typing.ime.layout;
        let osk_shift = self.typing.osk_shift;
        let mode = self.typing.ime.mode;
        let candidates = self.typing.ime.candidates.clone();
        let fonts = &mut self.fonts;
        if !pre.is_empty() {
            let r = pre_rect;
            cv.shadow(r, 8, 8);
            cv.round_rect(r, 8, theme::WIN);
            cv.round_frame(r, 8, theme::ACCENT);
            let (asc, _) = fonts.metrics(Face::Strong, 16);
            let y = r.y + 8 + asc;
            let mut x = fonts.draw(cv, Face::Strong, 16, r.x + 12, y, &pre, theme::INK) + 16;
            for (i, c) in candidates.iter().take(9).enumerate() {
                let t = format!("{} {}", i + 1, c);
                x = fonts.draw(cv, Face::Text, 15, x, y, &t, theme::PAST) + 14;
            }
            let hint = match mode {
                Mode::Japanese => "Enter or Space: keep",
                Mode::Chinese if candidates.is_empty() => "Space: choose",
                _ => "",
            };
            fonts.draw(cv, Face::Text, 12, x, y, hint, theme::MUTED);
        }
        if let Some((chosen, p, cells)) = picker {
            cv.shadow(p, 12, 14);
            cv.round_rect(p, 12, theme::WIN);
            for (i, (e, _)) in EMOJI.iter().enumerate() {
                let c = cells[i];
                if i == chosen {
                    cv.round_rect(c.grow(-2), 8, theme::ACCENT_SOFT);
                    cv.round_frame(c.grow(-2), 8, theme::ACCENT);
                }
                let s = String::from(*e);
                let (asc, lh) = fonts.metrics(Face::Text, 28);
                let w = fonts.width(Face::Text, 28, &s);
                let (x, y) = (c.x + (c.w - w) / 2, c.y + (c.h - lh) / 2 + asc);
                fonts.draw(cv, Face::Text, 28, x, y, &s, theme::INK);
            }
            let name = format!(
                "{}   ·   arrows choose, Enter types it, Escape closes",
                EMOJI[chosen].1
            );
            fonts.draw(
                cv,
                Face::Text,
                13,
                p.x + 14,
                p.bottom() - 20,
                &name,
                theme::MUTED,
            );
        }
        if let Some((o, keys)) = osk {
            cv.shadow(o, 12, 14);
            cv.round_rect(o, 12, theme::CHROME);
            for (r, k) in keys {
                let held = k == 42 && osk_shift;
                cv.round_rect(r, 7, if held { theme::ACCENT_SOFT } else { theme::WIN });
                cv.round_frame(r, 7, theme::LINE);
                let label = match k {
                    14 => String::from("⌫"),
                    28 => String::from("Enter"),
                    42 => String::from("Shift"),
                    57 => String::from(layout.name()),
                    _ => match sym(layout, k, osk_shift, false) {
                        Some(Sym::Char(c)) => String::from(c),
                        Some(Sym::Dead(m)) => String::from(alaya_ui::input::spacing(m)),
                        None => String::new(),
                    },
                };
                let (asc, lh) = fonts.metrics(Face::Text, 16);
                let w = fonts.width(Face::Text, 16, &label);
                let (x, y) = (r.x + (r.w - w) / 2, r.y + (r.h - lh) / 2 + asc);
                fonts.draw(cv, Face::Text, 16, x, y, &label, theme::INK);
            }
        }
    }
}

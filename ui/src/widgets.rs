// SPDX-License-Identifier: GPL-3.0-only
//! The pieces apps are built from: labels, buttons, text boxes and lists,
//! drawn in Alaya's look (the mock-up of decision 86).
//!
//! An app describes its window again each time it draws ("immediate
//! mode"); while it does, the toolkit writes down every piece with what
//! it is and what it is called. That list is the accessibility tree
//! (decision 42): a screen reader (milestone 7b) reads it, and tests use
//! it to find and press buttons by name instead of by pixel.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Rect;
use crate::canvas::{Canvas, Color};
use crate::text::{Face, Fonts};

/// Colours of the light look, from the mock-up.
pub mod theme {
    use crate::canvas::Color;
    pub const PAGE: Color = 0x000e_1516;
    pub const WALL_TOP: Color = 0x2b_5d_60;
    pub const WALL_BOTTOM: Color = 0x14_29_2b;
    pub const BAR: Color = 0xf6_f9_f8;
    pub const BAR_INK: Color = 0x1d_2a_2b;
    pub const WIN: Color = 0xfb_fc_fc;
    pub const INK: Color = 0x1d_2a_2b;
    pub const CHROME: Color = 0xee_f2_f1;
    pub const LINE: Color = 0xcf_d8_d6;
    pub const MUTED: Color = 0x5b_6b_6c;
    pub const ACCENT: Color = 0x23_68_6b;
    pub const ACCENT_INK: Color = 0xff_ff_ff;
    pub const ACCENT_SOFT: Color = 0xd3_e7_e6;
    pub const OK: Color = 0x2f_7d_4a;
    pub const OK_BG: Color = 0xdc_ef_e2;
    pub const WARN: Color = 0x9a_5b_00;
    pub const WARN_BG: Color = 0xfb_e9_c8;
    pub const PAST: Color = 0x2d_5c_9c;
    pub const PAST_BG: Color = 0xdb_e6_f6;
    pub const UNKNOWN: Color = 0x5f_66_68;
    pub const UNKNOWN_BG: Color = 0xe4_e7_e8;
    pub const NOW: Color = 0xb4_23_18;
}

/// What a piece of the window is, for the accessibility tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Label,
    Button,
    TextBox,
    List,
    ListItem,
    Group,
}

/// One entry of the accessibility tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub role: Role,
    pub name: String,
    pub rect: Rect,
    /// For text boxes: the text; for list items: "selected" or empty.
    pub value: String,
    pub focused: bool,
}

/// Keys the toolkit understands, already turned into meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Tab,
    Escape,
}

/// Editable text with a cursor (a byte index at a character boundary).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TextEdit {
    pub text: String,
    pub cursor: usize,
    /// The first line shown (multi-line boxes).
    pub scroll: usize,
}

impl TextEdit {
    pub fn new(s: &str) -> TextEdit {
        TextEdit {
            text: String::from(s),
            cursor: s.len(),
            scroll: 0,
        }
    }

    fn prev(&self, i: usize) -> usize {
        self.text[..i].char_indices().last().map_or(0, |(p, _)| p)
    }

    fn next(&self, i: usize) -> usize {
        self.text[i..]
            .chars()
            .next()
            .map_or(self.text.len(), |c| i + c.len_utf8())
    }

    fn line_start(&self, i: usize) -> usize {
        self.text[..i].rfind('\n').map_or(0, |p| p + 1)
    }

    fn line_end(&self, i: usize) -> usize {
        self.text[i..].find('\n').map_or(self.text.len(), |p| i + p)
    }

    /// The cursor's line and column (in characters).
    pub fn position(&self) -> (usize, usize) {
        let line = self.text[..self.cursor].matches('\n').count();
        let col = self.text[self.line_start(self.cursor)..self.cursor]
            .chars()
            .count();
        (line, col)
    }

    /// Put the cursor at a line and column, as near as the text allows.
    pub fn set_position(&mut self, line: usize, col: usize) {
        let mut at = 0;
        for _ in 0..line {
            match self.text[at..].find('\n') {
                Some(p) => at += p + 1,
                None => break,
            }
        }
        let end = self.line_end(at);
        let mut c = at;
        for _ in 0..col {
            if c >= end {
                break;
            }
            c = self.next(c);
        }
        self.cursor = c;
    }

    /// Apply a key; returns true if the text changed.
    pub fn key(&mut self, k: Key, multiline: bool) -> bool {
        self.cursor = self.cursor.min(self.text.len());
        match k {
            Key::Char(c) if !c.is_control() => {
                self.text.insert(self.cursor, c);
                self.cursor += c.len_utf8();
                true
            }
            Key::Enter if multiline => {
                self.text.insert(self.cursor, '\n');
                self.cursor += 1;
                true
            }
            Key::Backspace if self.cursor > 0 => {
                let p = self.prev(self.cursor);
                self.text.replace_range(p..self.cursor, "");
                self.cursor = p;
                true
            }
            Key::Delete if self.cursor < self.text.len() => {
                let n = self.next(self.cursor);
                self.text.replace_range(self.cursor..n, "");
                true
            }
            Key::Left => {
                self.cursor = self.prev(self.cursor);
                false
            }
            Key::Right => {
                self.cursor = self.next(self.cursor);
                false
            }
            Key::Home => {
                self.cursor = self.line_start(self.cursor);
                false
            }
            Key::End => {
                self.cursor = self.line_end(self.cursor);
                false
            }
            Key::Up | Key::Down if multiline => {
                let (l, c) = self.position();
                let l = if k == Key::Up {
                    l.saturating_sub(1)
                } else {
                    l + 1
                };
                self.set_position(l, c);
                false
            }
            _ => false,
        }
    }
}

/// Pointer state the app passes in each time it draws.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pointer {
    pub x: i32,
    pub y: i32,
    pub down: bool,
    /// The button went up this frame (a click ends here).
    pub released: bool,
    /// Where it went down, for clicks.
    pub down_x: i32,
    pub down_y: i32,
}

/// One drawing pass of a window.
pub struct Ui<'c, 'p, 'f, 'd> {
    pub cv: &'c mut Canvas<'p>,
    pub fonts: &'f mut Fonts<'d>,
    pub pointer: Pointer,
    /// The accessibility tree of this pass.
    pub tree: Vec<Node>,
    /// Which focusable piece has the keyboard (by order of appearance).
    pub focus: usize,
    focusables: usize,
    /// A key for the focused piece, used up by it.
    pub key: Option<Key>,
    /// A button pressed by name (accessibility, tests).
    pub press_by_name: Option<String>,
}

pub const TEXT_PX: u32 = 14;

impl<'c, 'p, 'f, 'd> Ui<'c, 'p, 'f, 'd> {
    pub fn new(cv: &'c mut Canvas<'p>, fonts: &'f mut Fonts<'d>, pointer: Pointer) -> Self {
        Ui {
            cv,
            fonts,
            pointer,
            tree: Vec::new(),
            focus: 0,
            focusables: 0,
            key: None,
            press_by_name: None,
        }
    }

    fn node(&mut self, role: Role, name: &str, rect: Rect, value: &str, focused: bool) {
        self.tree.push(Node {
            role,
            name: String::from(name),
            rect,
            value: String::from(value),
            focused,
        });
    }

    fn take_focus(&mut self) -> bool {
        let me = self.focusables;
        self.focusables += 1;
        me == self.focus
    }

    fn clicked(&self, r: Rect) -> bool {
        let p = self.pointer;
        p.released && r.contains(p.x, p.y) && r.contains(p.down_x, p.down_y)
    }

    /// Text in `face` with its top-left at (`x`, `y`).
    pub fn text(&mut self, x: i32, y: i32, face: Face, px: u32, s: &str, c: Color) -> Rect {
        let (asc, lh) = self.fonts.metrics(face, px);
        let end = self.fonts.draw(self.cv, face, px, x, y + asc, s, c);
        Rect::new(x, y, end - x, lh)
    }

    pub fn label(&mut self, x: i32, y: i32, s: &str) -> Rect {
        let r = self.text(x, y, Face::Text, TEXT_PX, s, theme::INK);
        self.node(Role::Label, s, r, "", false);
        r
    }

    /// A button; returns true when it was clicked (or pressed by name, or
    /// Enter while it has the keyboard).
    pub fn button(&mut self, x: i32, y: i32, name: &str, primary: bool) -> (bool, Rect) {
        let tw = self.fonts.width(Face::Strong, TEXT_PX, name);
        let r = Rect::new(x, y, tw + 24, 30);
        let focused = self.take_focus();
        let hover = r.contains(self.pointer.x, self.pointer.y);
        let (bg, ink, line) = if primary {
            (theme::ACCENT, theme::ACCENT_INK, theme::ACCENT)
        } else {
            (
                if hover {
                    theme::ACCENT_SOFT
                } else {
                    theme::CHROME
                },
                theme::INK,
                theme::LINE,
            )
        };
        self.cv.round_rect(r, 7, bg);
        self.cv
            .round_frame(r, 7, if focused { theme::ACCENT } else { line });
        let (asc, lh) = self.fonts.metrics(Face::Strong, TEXT_PX);
        let ty = y + (30 - lh) / 2 + asc;
        self.fonts
            .draw(self.cv, Face::Strong, TEXT_PX, x + 12, ty, name, ink);
        self.node(Role::Button, name, r, "", focused);
        let mut hit = self.clicked(r);
        if focused && self.key == Some(Key::Enter) {
            self.key = None;
            hit = true;
        }
        if self.press_by_name.as_deref() == Some(name) {
            self.press_by_name = None;
            hit = true;
        }
        (hit, r)
    }

    /// A text box `w` by `h` (more than one line high: multi-line). Keys
    /// go to it while it has the keyboard; returns true if the text
    /// changed. `mono` draws it in the code font.
    pub fn text_box(&mut self, r: Rect, name: &str, edit: &mut TextEdit, mono: bool) -> bool {
        let face = if mono { Face::Mono } else { Face::Text };
        let (asc, lh) = self.fonts.metrics(face, TEXT_PX);
        let multiline = r.h > lh + 16;
        let mut focused = self.take_focus();
        if self.clicked(r) {
            self.focus = self.focusables - 1;
            focused = true;
            // Put the cursor where the click was.
            let line = ((self.pointer.y - r.y - 8) / lh.max(1)).max(0) as usize + edit.scroll;
            let mut col = 0;
            let ls = {
                let mut at = 0;
                for _ in 0..line {
                    match edit.text[at..].find('\n') {
                        Some(p) => at += p + 1,
                        None => break,
                    }
                }
                at
            };
            let mut x = r.x + 10;
            for ch in edit.text[ls..].chars() {
                if ch == '\n' {
                    break;
                }
                let w = self.fonts.advance(face, TEXT_PX, ch);
                if x + w / 2 > self.pointer.x {
                    break;
                }
                x += w;
                col += 1;
            }
            edit.set_position(line, col);
        }
        let mut changed = false;
        if let Some(k) = self.key
            && focused
            && k != Key::Tab
            && k != Key::Escape
        {
            self.key = None;
            changed = edit.key(k, multiline);
        }
        self.cv.round_rect(r, 6, theme::WIN);
        self.cv
            .round_frame(r, 6, if focused { theme::ACCENT } else { theme::LINE });
        let old = self.cv.set_clip(r.grow(-2));
        let visible = ((r.h - 16) / lh.max(1)).max(1) as usize;
        let (cl, _) = edit.position();
        if cl < edit.scroll {
            edit.scroll = cl;
        } else if cl >= edit.scroll + visible {
            edit.scroll = cl + 1 - visible;
        }
        let mut y = r.y + 8;
        let mut at = 0;
        for (n, line) in edit.text.split('\n').enumerate() {
            let start = at;
            at += line.len() + 1;
            if n < edit.scroll {
                continue;
            }
            if n >= edit.scroll + visible {
                break;
            }
            self.fonts
                .draw(self.cv, face, TEXT_PX, r.x + 10, y + asc, line, theme::INK);
            if focused && edit.cursor >= start && edit.cursor <= start + line.len() {
                let cx = r.x
                    + 10
                    + self
                        .fonts
                        .width(face, TEXT_PX, &line[..edit.cursor - start]);
                self.cv.fill(Rect::new(cx, y, 2, lh), theme::ACCENT);
            }
            y += lh;
        }
        self.cv.set_clip(old);
        self.node(Role::TextBox, name, r, &edit.text, focused);
        changed
    }

    /// A list of rows; returns the row clicked. `marks` draws a small
    /// coloured tag at the right of each row (text, ink, background).
    pub fn list(
        &mut self,
        r: Rect,
        name: &str,
        rows: &[&str],
        selected: Option<usize>,
        marks: &[Option<(&str, Color, Color)>],
    ) -> Option<usize> {
        let focused = self.take_focus();
        self.cv.round_rect(r, 6, theme::WIN);
        self.cv
            .round_frame(r, 6, if focused { theme::ACCENT } else { theme::LINE });
        self.node(Role::List, name, r, "", focused);
        let old = self.cv.set_clip(r.grow(-1));
        let row_h = 30;
        let mut hit = None;
        for (i, &row) in rows.iter().enumerate() {
            let rr = Rect::new(r.x + 4, r.y + 4 + i as i32 * row_h, r.w - 8, row_h);
            if rr.y > r.bottom() {
                break;
            }
            let sel = selected == Some(i);
            if sel {
                self.cv.round_rect(rr, 5, theme::ACCENT_SOFT);
            }
            let (asc, lh) = self.fonts.metrics(Face::Text, TEXT_PX);
            self.fonts.draw(
                self.cv,
                Face::Text,
                TEXT_PX,
                rr.x + 8,
                rr.y + (row_h - lh) / 2 + asc,
                row,
                theme::INK,
            );
            let mut value = String::from(if sel { "selected" } else { "" });
            if let Some(Some((t, ink, bg))) = marks.get(i) {
                let tw = self.fonts.width(Face::Strong, 12, t);
                let m = Rect::new(rr.right() - tw - 20, rr.y + 6, tw + 14, 18);
                self.cv.round_rect(m, 9, *bg);
                let (a2, l2) = self.fonts.metrics(Face::Strong, 12);
                self.fonts.draw(
                    self.cv,
                    Face::Strong,
                    12,
                    m.x + 7,
                    m.y + (18 - l2) / 2 + a2,
                    t,
                    *ink,
                );
                if !value.is_empty() {
                    value.push_str(", ");
                }
                value.push_str(t);
            }
            self.tree.push(Node {
                role: Role::ListItem,
                name: String::from(row),
                rect: rr,
                value,
                focused: false,
            });
            if self.clicked(rr) || self.press_by_name.as_deref() == Some(row) {
                if self.press_by_name.as_deref() == Some(row) {
                    self.press_by_name = None;
                }
                hit = Some(i);
            }
        }
        self.cv.set_clip(old);
        if focused {
            match self.key {
                Some(Key::Down) => {
                    self.key = None;
                    hit = Some(selected.map_or(0, |s| (s + 1).min(rows.len().saturating_sub(1))));
                }
                Some(Key::Up) => {
                    self.key = None;
                    hit = Some(selected.map_or(0, |s| s.saturating_sub(1)));
                }
                _ => {}
            }
        }
        hit
    }

    /// Tab moves the keyboard to the next piece; call after drawing.
    pub fn finish(&mut self) -> usize {
        if self.key == Some(Key::Tab) && self.focusables > 0 {
            self.key = None;
            self.focus = (self.focus + 1) % self.focusables;
        }
        self.focus
    }
}

/// US English keyboard (decision 106): a key code (the numbers virtio
/// input sends) to a key, with or without Shift.
pub fn us_key(code: u16, shift: bool) -> Option<Key> {
    const LOWER: &[u8; 58] =
        b"\0\x1b1234567890-=\x08\tqwertyuiop[]\n\0asdfghjkl;'`\0\\zxcvbnm,./\0*\0 ";
    const UPPER: &[u8; 58] =
        b"\0\x1b!@#$%^&*()_+\x08\tQWERTYUIOP{}\n\0ASDFGHJKL:\"~\0|ZXCVBNM<>?\0*\0 ";
    Some(match code {
        1 => Key::Escape,
        14 => Key::Backspace,
        15 => Key::Tab,
        28 | 96 => Key::Enter,
        102 => Key::Home,
        103 => Key::Up,
        105 => Key::Left,
        106 => Key::Right,
        107 => Key::End,
        108 => Key::Down,
        111 => Key::Delete,
        c if (c as usize) < LOWER.len() => {
            let b = if shift { UPPER } else { LOWER }[c as usize];
            if b == 0 || b < 0x20 {
                return None;
            }
            Key::Char(b as char)
        }
        _ => return None,
    })
}

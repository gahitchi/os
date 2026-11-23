// SPDX-License-Identifier: GPL-3.0-only
//! The first setup and the login screen (milestone 6b), which only the
//! desktop draws.
//!
//! First setup asks for a name, a PIN (twice), a picture and what Alaya
//! will be used for (decision 90: the look). The login screen has one tile
//! per person (decision 30) and a PIN box. While the real PIN box is open,
//! the person's picture shows in the top strip, which no app can draw on
//! and no app ever sees (decision 104): a fake login screen drawn by an
//! app cannot show it.
//!
//! Only the logic and the drawing are here, so the keys, the clicks and
//! what each screen asks for are tested on the build machine. The desktop
//! turns the PIN into a key and asks the store.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Rect;
use crate::canvas::{Canvas, Color, mix};
use crate::text::{Face, Fonts};
use crate::widgets::{Key, TextEdit, theme};

/// The pictures a person can pick: a shape in a colour. Small and plain
/// for now; a person's own image comes with the photo tools.
pub const PICTURES: [(Shape, Color); 8] = [
    (Shape::Circle, 0x23_68_6b),
    (Shape::Diamond, 0xb4_23_18),
    (Shape::Square, 0x9a_5b_00),
    (Shape::Ring, 0x2d_5c_9c),
    (Shape::Diamond, 0x2f_7d_4a),
    (Shape::Circle, 0x7a_3e_9d),
    (Shape::Ring, 0xc2_4e_8a),
    (Shape::Square, 0x1d_2a_2b),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Circle,
    Square,
    Diamond,
    Ring,
}

/// Draw picture `i` filling `r` (a square looks best).
pub fn draw_picture(cv: &mut Canvas, r: Rect, i: usize) {
    let (shape, c) = PICTURES[i % PICTURES.len()];
    let s = r.w.min(r.h);
    let r = Rect::new(r.x + (r.w - s) / 2, r.y + (r.h - s) / 2, s, s);
    match shape {
        Shape::Circle => cv.round_rect(r, s / 2, c),
        Shape::Square => cv.round_rect(r.grow(-s / 10), s / 8, c),
        Shape::Ring => {
            cv.round_rect(r, s / 2, c);
            let hole = r.grow(-s / 4);
            cv.round_rect(hole, hole.w / 2, mix(c, 0xff_ff_ff, 230));
        }
        Shape::Diamond => {
            let (cx, cy, half) = (r.x + s / 2, r.y + s / 2, s / 2);
            for dy in -half..=half {
                let w = half - dy.abs();
                cv.fill(Rect::new(cx - w, cy + dy, 2 * w + 1, 1), c);
            }
        }
    }
}

/// What Alaya will be used for (decision 90, first version): two looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// Files, the text editor and settings; nothing else on screen.
    Basics,
    /// Every app in the dock, and the loose group labels.
    Everything,
}

impl Look {
    pub fn code(self) -> u8 {
        match self {
            Look::Basics => 0,
            Look::Everything => 1,
        }
    }

    pub fn from_code(c: u8) -> Look {
        if c == 1 {
            Look::Everything
        } else {
            Look::Basics
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Look::Basics => "Just the basics",
            Look::Everything => "Everything on",
        }
    }

    pub fn about(self) -> &'static str {
        match self {
            Look::Basics => "Files, writing and settings.",
            Look::Everything => "All the apps, and window groups.",
        }
    }
}

pub const PIN_MIN: usize = 4;
pub const PIN_MAX: usize = 32;
pub const NAME_MAX: usize = 40;

/// The panel in the middle of the screen both screens draw in.
pub fn panel(screen_w: i32, screen_h: i32) -> Rect {
    let (w, h) = (520, 520);
    Rect::new(
        (screen_w - w) / 2,
        crate::wm::STRIP + (screen_h - crate::wm::STRIP - h) / 2,
        w,
        h,
    )
}

/// What first setup hands the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewPerson {
    pub name: String,
    pub pin: String,
    pub picture: usize,
    pub look: Look,
}

/// First setup. Tab and Shift+Tab (or a click) move between the parts;
/// Enter anywhere finishes.
#[derive(Clone, Debug)]
pub struct Setup {
    pub name: TextEdit,
    pub pin: TextEdit,
    pub again: TextEdit,
    pub picture: usize,
    pub look: Look,
    /// 0 name, 1 PIN, 2 PIN again, 3 picture, 4 look, 5 the Start button.
    pub focus: usize,
    pub error: Option<&'static str>,
}

const SETUP_PARTS: usize = 6;

struct SetupLayout {
    name: Rect,
    pin: Rect,
    again: Rect,
    pictures: [Rect; 8],
    looks: [Rect; 2],
    start: Rect,
}

impl Default for Setup {
    fn default() -> Self {
        Setup::new()
    }
}

impl Setup {
    pub fn new() -> Setup {
        Setup {
            name: TextEdit::default(),
            pin: TextEdit::default(),
            again: TextEdit::default(),
            picture: 0,
            look: Look::Basics,
            focus: 0,
            error: None,
        }
    }

    fn layout(p: Rect) -> SetupLayout {
        let x = p.x + 32;
        let w = p.w - 64;
        let mut pictures = [Rect::default(); 8];
        for (i, r) in pictures.iter_mut().enumerate() {
            *r = Rect::new(x + i as i32 * 58, p.y + 306, 48, 48);
        }
        SetupLayout {
            name: Rect::new(x, p.y + 116, w, 36),
            pin: Rect::new(x, p.y + 186, (w - 16) / 2, 36),
            again: Rect::new(x + (w + 16) / 2, p.y + 186, (w - 16) / 2, 36),
            pictures,
            looks: [
                Rect::new(x, p.y + 386, (w - 16) / 2, 64),
                Rect::new(x + (w + 16) / 2, p.y + 386, (w - 16) / 2, 64),
            ],
            start: Rect::new(p.right() - 32 - 120, p.bottom() - 52, 120, 34),
        }
    }

    /// Check what was typed; the person to make, or why not.
    fn finish(&mut self) -> Option<NewPerson> {
        let name = self.name.text.trim();
        let pin = &self.pin.text;
        self.error = if name.is_empty() {
            self.focus = 0;
            Some("Please type a name.")
        } else if pin.chars().count() < PIN_MIN {
            self.focus = 1;
            Some("The PIN needs at least 4 characters.")
        } else if *pin != self.again.text {
            self.focus = 2;
            self.again.text.clear();
            self.again.cursor = 0;
            Some("The two PINs are not the same. Please type it again.")
        } else {
            None
        };
        if self.error.is_some() {
            return None;
        }
        Some(NewPerson {
            name: String::from(name),
            pin: pin.clone(),
            picture: self.picture,
            look: self.look,
        })
    }

    /// A key; returns the new person when Enter finishes setup.
    pub fn key(&mut self, k: Key, shift: bool) -> Option<NewPerson> {
        match k {
            Key::Tab => {
                self.focus = if shift {
                    (self.focus + SETUP_PARTS - 1) % SETUP_PARTS
                } else {
                    (self.focus + 1) % SETUP_PARTS
                };
                None
            }
            Key::Enter => self.finish(),
            Key::Left | Key::Right if self.focus == 3 => {
                let n = PICTURES.len();
                self.picture = if k == Key::Right {
                    (self.picture + 1) % n
                } else {
                    (self.picture + n - 1) % n
                };
                None
            }
            Key::Left | Key::Right if self.focus == 4 => {
                self.look = if k == Key::Right {
                    Look::Everything
                } else {
                    Look::Basics
                };
                None
            }
            _ => {
                let (edit, max) = match self.focus {
                    0 => (&mut self.name, NAME_MAX),
                    1 => (&mut self.pin, PIN_MAX),
                    2 => (&mut self.again, PIN_MAX),
                    _ => return None,
                };
                if matches!(k, Key::Char(_)) && edit.text.chars().count() >= max {
                    return None;
                }
                edit.key(k, false);
                None
            }
        }
    }

    /// A click at (`x`, `y`) on a screen `w` by `h`.
    pub fn click(&mut self, x: i32, y: i32, w: i32, h: i32) -> Option<NewPerson> {
        let l = Self::layout(panel(w, h));
        if l.name.contains(x, y) {
            self.focus = 0;
        } else if l.pin.contains(x, y) {
            self.focus = 1;
        } else if l.again.contains(x, y) {
            self.focus = 2;
        } else if let Some(i) = l.pictures.iter().position(|r| r.contains(x, y)) {
            self.focus = 3;
            self.picture = i;
        } else if let Some(i) = l.looks.iter().position(|r| r.contains(x, y)) {
            self.focus = 4;
            self.look = if i == 0 {
                Look::Basics
            } else {
                Look::Everything
            };
        } else if l.start.contains(x, y) {
            return self.finish();
        }
        None
    }

    pub fn draw(&self, cv: &mut Canvas, fonts: &mut Fonts) {
        let p = panel(cv.width, cv.height);
        let l = Self::layout(p);
        card(cv, p);
        let x = p.x + 32;
        heading(cv, fonts, x, p.y + 28, "Welcome to Alaya");
        small(
            cv,
            fonts,
            x,
            p.y + 60,
            "A few questions, then your desktop.",
            theme::MUTED,
        );
        small(cv, fonts, x, l.name.y - 22, "Your name", theme::INK);
        field(cv, fonts, l.name, &self.name, self.focus == 0, false);
        small(cv, fonts, x, l.pin.y - 22, "A PIN", theme::INK);
        small(
            cv,
            fonts,
            l.again.x,
            l.again.y - 22,
            "The PIN again",
            theme::INK,
        );
        field(cv, fonts, l.pin, &self.pin, self.focus == 1, true);
        field(cv, fonts, l.again, &self.again, self.focus == 2, true);
        small(
            cv,
            fonts,
            x,
            l.pictures[0].y - 46,
            "Pick your picture. Alaya shows it at the top of the screen",
            theme::INK,
        );
        small(
            cv,
            fonts,
            x,
            l.pictures[0].y - 28,
            "only when its real PIN box asks for your PIN.",
            theme::MUTED,
        );
        for (i, r) in l.pictures.iter().enumerate() {
            if i == self.picture {
                cv.round_rect(r.grow(4), 10, theme::ACCENT_SOFT);
                let ring = if self.focus == 3 {
                    theme::ACCENT
                } else {
                    theme::LINE
                };
                cv.round_frame(r.grow(4), 10, ring);
            }
            draw_picture(cv, r.grow(-6), i);
        }
        small(
            cv,
            fonts,
            x,
            l.looks[0].y - 24,
            "What will you use Alaya for? You can change it in Settings.",
            theme::INK,
        );
        for (i, look) in [Look::Basics, Look::Everything].iter().enumerate() {
            let r = l.looks[i];
            let on = *look == self.look;
            cv.round_rect(r, 8, if on { theme::ACCENT_SOFT } else { theme::WIN });
            let ring = if on && self.focus == 4 {
                theme::ACCENT
            } else {
                theme::LINE
            };
            cv.round_frame(r, 8, ring);
            let (asc, _) = fonts.metrics(Face::Strong, 14);
            fonts.draw(
                cv,
                Face::Strong,
                14,
                r.x + 12,
                r.y + 10 + asc,
                look.name(),
                theme::INK,
            );
            let old = cv.set_clip(cv.clip().intersect(r.grow(-4)));
            let (asc, _) = fonts.metrics(Face::Text, 12);
            fonts.draw(
                cv,
                Face::Text,
                12,
                r.x + 12,
                r.y + 36 + asc,
                look.about(),
                theme::MUTED,
            );
            cv.set_clip(old);
        }
        if let Some(e) = self.error {
            small(cv, fonts, x, l.start.y + 8, e, theme::NOW);
        }
        button(cv, fonts, l.start, "Start", self.focus == 5);
    }
}

/// One person on the login screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Person {
    pub id: u64,
    pub name: String,
    pub picture: usize,
}

/// What the login screen asks the desktop to try.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attempt {
    pub person: usize,
    pub pin: String,
}

/// The login screen: a tile per person and the PIN box.
#[derive(Clone, Debug)]
pub struct Login {
    pub people: Vec<Person>,
    pub chosen: usize,
    pub pin: TextEdit,
    pub error: Option<&'static str>,
    /// What Alaya checked before asking (decision 74).
    pub notice: String,
    /// The PIN is being checked.
    pub busy: bool,
}

impl Login {
    pub fn new(people: Vec<Person>, notice: String) -> Login {
        Login {
            people,
            chosen: 0,
            pin: TextEdit::default(),
            error: None,
            notice,
            busy: false,
        }
    }

    /// The person whose PIN box is open: their picture goes in the strip.
    pub fn picture(&self) -> Option<usize> {
        self.people.get(self.chosen).map(|p| p.picture)
    }

    fn tiles(&self, p: Rect) -> Vec<Rect> {
        let n = self.people.len().max(1) as i32;
        let (tw, gap) = (96, 16);
        let total = n * tw + (n - 1) * gap;
        (0..n)
            .map(|i| Rect::new(p.x + (p.w - total) / 2 + i * (tw + gap), p.y + 96, tw, 112))
            .collect()
    }

    fn pin_box(p: Rect) -> Rect {
        Rect::new(p.x + 110, p.y + 270, p.w - 220, 40)
    }

    pub fn key(&mut self, k: Key) -> Option<Attempt> {
        if self.busy {
            return None;
        }
        match k {
            Key::Enter if !self.pin.text.is_empty() && !self.people.is_empty() => {
                self.busy = true;
                self.error = None;
                Some(Attempt {
                    person: self.chosen,
                    pin: core::mem::take(&mut self.pin.text),
                })
            }
            Key::Left | Key::Right if self.people.len() > 1 => {
                let n = self.people.len();
                self.chosen = if k == Key::Right {
                    (self.chosen + 1) % n
                } else {
                    (self.chosen + n - 1) % n
                };
                self.pin = TextEdit::default();
                self.error = None;
                None
            }
            Key::Escape => {
                self.pin = TextEdit::default();
                None
            }
            Key::Char(_) if self.pin.text.chars().count() >= PIN_MAX => None,
            Key::Char(_) | Key::Backspace => {
                self.pin.key(k, false);
                self.pin.cursor = self.pin.text.len();
                self.error = None;
                None
            }
            _ => None,
        }
    }

    pub fn click(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let p = panel(w, h);
        if let Some(i) = self.tiles(p).iter().position(|r| r.contains(x, y))
            && i != self.chosen
            && !self.busy
        {
            self.chosen = i;
            self.pin = TextEdit::default();
            self.error = None;
        }
    }

    /// The PIN was wrong: ask again.
    pub fn refused(&mut self) {
        self.busy = false;
        self.error = Some("That PIN is not right. Please try again.");
    }

    pub fn draw(&self, cv: &mut Canvas, fonts: &mut Fonts) {
        let p = panel(cv.width, cv.height);
        card(cv, p);
        heading(cv, fonts, p.x + 32, p.y + 28, "Who is using Alaya?");
        for (i, (r, person)) in self.tiles(p).iter().zip(&self.people).enumerate() {
            let on = i == self.chosen;
            cv.round_rect(*r, 12, if on { theme::ACCENT_SOFT } else { theme::WIN });
            cv.round_frame(*r, 12, if on { theme::ACCENT } else { theme::LINE });
            // An initial, not the picture: the picture is only for the
            // strip, so a screenshot of this screen does not give it away.
            let c = Rect::new(r.x + (r.w - 52) / 2, r.y + 14, 52, 52);
            cv.round_rect(c, 26, theme::ACCENT);
            let initial: String = person.name.chars().take(1).collect();
            let tw = fonts.width(Face::Strong, 22, &initial);
            let (asc, lh) = fonts.metrics(Face::Strong, 22);
            fonts.draw(
                cv,
                Face::Strong,
                22,
                c.x + (c.w - tw) / 2,
                c.y + (c.h - lh) / 2 + asc,
                &initial,
                theme::ACCENT_INK,
            );
            let old = cv.set_clip(cv.clip().intersect(r.grow(-4)));
            let nw = fonts.width(Face::Text, 13, &person.name);
            let (asc, _) = fonts.metrics(Face::Text, 13);
            fonts.draw(
                cv,
                Face::Text,
                13,
                r.x + (r.w - nw).max(8) / 2,
                r.y + 80 + asc,
                &person.name,
                theme::INK,
            );
            cv.set_clip(old);
        }
        let b = Self::pin_box(p);
        small(cv, fonts, b.x, b.y - 24, "Your PIN", theme::INK);
        field(cv, fonts, b, &self.pin, !self.busy, true);
        let (msg, ink) = match (self.busy, self.error) {
            (true, _) => ("Checking your PIN...", theme::MUTED),
            (false, Some(e)) => (e, theme::NOW),
            (false, None) => (
                "Check that your picture is at the top of the screen.",
                theme::MUTED,
            ),
        };
        small(cv, fonts, b.x, b.bottom() + 12, msg, ink);
        // What Alaya checked before asking.
        let n = Rect::new(p.x + 24, p.bottom() - 92, p.w - 48, 68);
        cv.round_rect(n, 8, theme::PAST_BG);
        let old = cv.set_clip(cv.clip().intersect(n.grow(-6)));
        let lines = fonts.wrap(Face::Text, 12, &self.notice, n.w - 24);
        let (asc, lh) = fonts.metrics(Face::Text, 12);
        for (i, (a, z)) in lines.iter().take(3).enumerate() {
            fonts.draw(
                cv,
                Face::Text,
                12,
                n.x + 12,
                n.y + 10 + asc + i as i32 * lh,
                &self.notice[*a..*z],
                theme::PAST,
            );
        }
        cv.set_clip(old);
    }
}

fn card(cv: &mut Canvas, p: Rect) {
    cv.shadow(p, 16, 18);
    cv.round_rect(p, 16, theme::WIN);
    cv.round_frame(p, 16, theme::LINE);
}

fn heading(cv: &mut Canvas, fonts: &mut Fonts, x: i32, y: i32, s: &str) {
    let (asc, _) = fonts.metrics(Face::Strong, 22);
    fonts.draw(cv, Face::Strong, 22, x, y + asc, s, theme::INK);
}

fn small(cv: &mut Canvas, fonts: &mut Fonts, x: i32, y: i32, s: &str, c: Color) {
    let (asc, _) = fonts.metrics(Face::Text, 13);
    fonts.draw(cv, Face::Text, 13, x, y + asc, s, c);
}

/// A one-line box; `secret` shows dots instead of the text.
fn field(cv: &mut Canvas, fonts: &mut Fonts, r: Rect, e: &TextEdit, focused: bool, secret: bool) {
    cv.round_rect(r, 6, theme::WIN);
    cv.round_frame(r, 6, if focused { theme::ACCENT } else { theme::LINE });
    let old = cv.set_clip(cv.clip().intersect(r.grow(-2)));
    let (asc, lh) = fonts.metrics(Face::Text, 14);
    let ty = r.y + (r.h - lh) / 2;
    let end = if secret {
        let n = e.text.chars().count() as i32;
        for i in 0..n {
            cv.round_rect(
                Rect::new(r.x + 12 + i * 14, r.y + r.h / 2 - 4, 8, 8),
                4,
                theme::INK,
            );
        }
        r.x + 12 + n * 14
    } else {
        fonts.draw(cv, Face::Text, 14, r.x + 10, ty + asc, &e.text, theme::INK)
    };
    if focused {
        cv.fill(Rect::new(end + 1, ty, 2, lh), theme::ACCENT);
    }
    cv.set_clip(old);
}

fn button(cv: &mut Canvas, fonts: &mut Fonts, r: Rect, s: &str, focused: bool) {
    cv.round_rect(r, 8, theme::ACCENT);
    if focused {
        cv.round_frame(r.grow(2), 10, theme::ACCENT);
    }
    let tw = fonts.width(Face::Strong, 14, s);
    let (asc, lh) = fonts.metrics(Face::Strong, 14);
    fonts.draw(
        cv,
        Face::Strong,
        14,
        r.x + (r.w - tw) / 2,
        r.y + (r.h - lh) / 2 + asc,
        s,
        theme::ACCENT_INK,
    );
}

/// A person's record in the machine's own files: id, picture, name.
pub fn encode_person(p: &Person) -> [u8; 64] {
    let mut b = [0u8; 64];
    b[..8].copy_from_slice(&p.id.to_le_bytes());
    b[8] = p.picture as u8;
    let mut n = p.name.len().min(NAME_MAX * 4).min(54);
    while !p.name.is_char_boundary(n) {
        n -= 1;
    }
    b[9] = n as u8;
    b[10..10 + n].copy_from_slice(&p.name.as_bytes()[..n]);
    b
}

pub fn decode_person(b: &[u8]) -> Option<Person> {
    if b.len() < 64 {
        return None;
    }
    let id = u64::from_le_bytes(b[..8].try_into().ok()?);
    let n = (b[9] as usize).min(54);
    let name = core::str::from_utf8(&b[10..10 + n]).ok()?;
    (id != 0).then(|| Person {
        id,
        name: String::from(name),
        picture: b[8] as usize % PICTURES.len(),
    })
}

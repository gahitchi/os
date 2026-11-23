// SPDX-License-Identifier: GPL-3.0-only
//! How the compositor draws the screen: the wallpaper, the windows with
//! their frames and shadows, the snap preview, the dock, the top strip that
//! only Alaya draws, and the pointer. Only the part that changed is drawn
//! again (the clip), so moving the mouse costs a few small rectangles.

use crate::Rect;
use crate::canvas::{Canvas, Color, mix};
use crate::text::{Face, Fonts};
use crate::widgets::theme;
use crate::wm::{STRIP, TITLE, Wm};

/// What the compositor knows about one window besides its place.
pub struct WindowView<'a> {
    pub title: &'a str,
    /// The app's picture: pixels, width, height, stride.
    pub pixels: Option<(&'a [u32], i32, i32, i32)>,
    /// The app stopped answering (drawn dimmed, with a note).
    pub stuck: bool,
    /// The loose group it is in: its label and colour.
    pub group: Option<(&'a str, Color)>,
}

/// One item in the dock.
pub struct DockItem<'a> {
    pub name: &'a str,
    pub color: Color,
    pub running: bool,
}

/// A loose group's label, left of the dock.
pub struct GroupChip<'a> {
    pub name: &'a str,
    pub color: Color,
    pub count: usize,
}

pub struct Desktop<'a> {
    pub clock: &'a str,
    /// The text in the middle of the strip (what Alaya wants you to know).
    pub status: &'a str,
    pub dock: &'a [DockItem<'a>],
    pub groups: &'a [GroupChip<'a>],
    pub show_pointer: bool,
    /// The login screen is up: no windows, no dock.
    pub locked: bool,
    /// The real PIN box is open: this person's picture shows in the strip.
    pub trusted: Option<usize>,
    /// The going-back buttons in the strip ("Undo", "What happened?").
    pub buttons: bool,
    /// A question only Alaya asks (drawn over the windows).
    pub question: Option<&'a Question<'a>>,
}

/// A question Alaya asks before it does something (rule B6: something
/// else changed it since; undo anyway?).
pub struct Question<'a> {
    pub title: &'a str,
    pub body: &'a str,
    pub yes: &'a str,
    pub no: &'a str,
}

/// The strip's buttons: Undo, and What happened?
pub fn strip_buttons() -> [Rect; 2] {
    [Rect::new(96, 5, 68, 24), Rect::new(172, 5, 128, 24)]
}

/// The names on the strip's buttons.
pub const STRIP_BUTTONS: [&str; 2] = ["Undo", "What happened?"];

/// Where a question is drawn on a screen `w` by `h`, and its two buttons
/// (yes, no).
pub fn question_rect(w: i32, h: i32) -> Rect {
    Rect::new((w - 460) / 2, (h - 210) / 2, 460, 210)
}

pub fn question_buttons(w: i32, h: i32) -> (Rect, Rect) {
    let q = question_rect(w, h);
    (
        Rect::new(q.right() - 330, q.bottom() - 52, 150, 32),
        Rect::new(q.right() - 168, q.bottom() - 52, 150, 32),
    )
}

fn draw_question(cv: &mut Canvas, fonts: &mut Fonts, q: &Question, sw: i32, sh: i32) {
    let r = question_rect(sw, sh);
    cv.shadow(r, 12, 16);
    cv.round_rect(r, 12, theme::WIN);
    cv.round_frame(r, 12, theme::LINE);
    let (asc, _) = fonts.metrics(Face::Strong, 16);
    fonts.draw(
        cv,
        Face::Strong,
        16,
        r.x + 22,
        r.y + 20 + asc,
        q.title,
        theme::INK,
    );
    let (a2, l2) = fonts.metrics(Face::Text, 13);
    let lines = fonts.wrap(Face::Text, 13, q.body, r.w - 44);
    for (i, (a, b)) in lines.iter().take(5).enumerate() {
        fonts.draw(
            cv,
            Face::Text,
            13,
            r.x + 22,
            r.y + 54 + i as i32 * (l2 + 2) + a2,
            q.body[*a..*b].trim_end(),
            theme::MUTED,
        );
    }
    let (yes, no) = question_buttons(sw, sh);
    for (b, name, primary) in [(yes, q.yes, true), (no, q.no, false)] {
        cv.round_rect(
            b,
            7,
            if primary {
                theme::ACCENT
            } else {
                theme::CHROME
            },
        );
        cv.round_frame(b, 7, if primary { theme::ACCENT } else { theme::LINE });
        let tw = fonts.width(Face::Strong, 13, name);
        let (a3, l3) = fonts.metrics(Face::Strong, 13);
        fonts.draw(
            cv,
            Face::Strong,
            13,
            b.x + (b.w - tw) / 2,
            b.y + (b.h - l3) / 2 + a3,
            name,
            if primary {
                theme::ACCENT_INK
            } else {
                theme::INK
            },
        );
    }
}

/// The group labels' rectangles, left of the dock.
pub fn group_chip(w: i32, h: i32, items: usize, groups: usize, i: usize) -> Rect {
    let d = dock_rect(w, h, items);
    let cw = 92;
    let x = d.x - 16 - (groups - i) as i32 * (cw + 8);
    Rect::new(x, d.y + 8, cw, 32)
}

/// The dock's rectangle on a screen `w` by `h`, and each item's.
pub fn dock_rect(w: i32, h: i32, items: usize) -> Rect {
    let iw = 40 + 8;
    let dw = items as i32 * iw + 16;
    Rect::new((w - dw) / 2, h - 56, dw, 48)
}

pub fn dock_item(w: i32, h: i32, items: usize, i: usize) -> Rect {
    let d = dock_rect(w, h, items);
    Rect::new(d.x + 8 + i as i32 * 48 + 4, d.y + 4, 40, 40)
}

/// The arrow pointer, as rows of (start, length) of its outline and fill.
const POINTER: [&str; 19] = [
    "X",
    "XX",
    "X.X",
    "X..X",
    "X...X",
    "X....X",
    "X.....X",
    "X......X",
    "X.......X",
    "X........X",
    "X.........X",
    "X......XXXXX",
    "X...X..X",
    "X..XX..X",
    "X.X  X..X",
    "XX   X..X",
    "X     X..X",
    "      X..X",
    "       XX",
];

pub fn pointer_rect(x: i32, y: i32) -> Rect {
    Rect::new(x, y, 13, 20)
}

pub fn draw_pointer(cv: &mut Canvas, x: i32, y: i32) {
    for (dy, row) in POINTER.iter().enumerate() {
        for (dx, ch) in row.bytes().enumerate() {
            let c = match ch {
                b'X' => 0x10_18_18,
                b'.' => 0xff_ff_ff,
                _ => continue,
            };
            cv.blend(x + dx as i32, y + dy as i32, c, 255);
        }
    }
}

fn draw_window(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    w: &crate::wm::Window,
    v: &WindowView,
    front: bool,
) {
    let f = w.frame;
    cv.shadow(f, 10, if front { 14 } else { 8 });
    cv.round_rect(f, 10, theme::WIN);
    // Title bar: rounded at the top only.
    let old = cv.set_clip(cv.clip().intersect(Rect::new(f.x, f.y, f.w, TITLE)));
    cv.round_rect(Rect::new(f.x, f.y, f.w, TITLE + 12), 10, theme::CHROME);
    cv.set_clip(old);
    cv.fill(Rect::new(f.x, f.y + TITLE - 1, f.w, 1), theme::LINE);
    let (asc, lh) = fonts.metrics(Face::Strong, 13);
    let close = w.close_button();
    let old = cv.set_clip(
        cv.clip()
            .intersect(Rect::new(f.x + 12, f.y, close.x - f.x - 20, TITLE)),
    );
    let mut tx = f.x + 12;
    if let Some((name, c)) = v.group {
        // The group's label in front of the title.
        let gw = fonts.width(Face::Strong, 11, name) + 14;
        let g = Rect::new(tx, f.y + (TITLE - 18) / 2, gw, 18);
        cv.round_rect(g, 9, c);
        let (a2, l2) = fonts.metrics(Face::Strong, 11);
        fonts.draw(
            cv,
            Face::Strong,
            11,
            g.x + 7,
            g.y + (18 - l2) / 2 + a2,
            name,
            0xff_ff_ff,
        );
        tx += gw + 8;
    }
    fonts.draw(
        cv,
        Face::Strong,
        13,
        tx,
        f.y + (TITLE - lh) / 2 + asc,
        v.title,
        theme::INK,
    );
    cv.set_clip(old);
    // The close button: a soft circle with a cross.
    cv.round_rect(close, CLOSE_R, mix(theme::CHROME, theme::LINE, 140));
    let (cx, cy) = (close.x + close.w / 2, close.y + close.h / 2);
    for i in -4..=4 {
        for t in 0..2 {
            cv.blend(cx + i, cy + i + t - 1 + 1, theme::MUTED, 220);
            cv.blend(cx + i, cy - i + t - 1 + 1, theme::MUTED, 220);
        }
    }
    // The app's picture, inside the content.
    let c = w.content();
    let old = cv.set_clip(cv.clip().intersect(c));
    match v.pixels {
        Some((px, pw, ph, stride)) => cv.blit(px, pw, ph, stride, c.x, c.y),
        None => cv.fill(c, theme::WIN),
    }
    if v.stuck {
        cv.fill_alpha(c, 0xff_ff_ff, 140);
        let (asc, _) = fonts.metrics(Face::Text, 14);
        fonts.draw(
            cv,
            Face::Text,
            14,
            c.x + 16,
            c.y + 16 + asc,
            "Not answering. Alaya is restarting it.",
            theme::MUTED,
        );
    }
    cv.set_clip(old);
    cv.round_frame(f, 10, theme::LINE);
    if front {
        cv.round_frame(f.grow(1), 11, theme::ACCENT);
        cv.round_frame(f.grow(2), 12, theme::ACCENT);
    }
}

const CLOSE_R: i32 = 11;

/// Draw everything inside `clip`.
pub fn draw<'a>(
    cv: &mut Canvas,
    fonts: &mut Fonts,
    wm: &Wm,
    views: &dyn Fn(u32) -> WindowView<'a>,
    desk: &Desktop,
    clip: Rect,
) {
    let (sw, sh) = (cv.width, cv.height);
    let old = cv.set_clip(clip);
    cv.gradient(
        Rect::new(0, STRIP, sw, sh - STRIP),
        theme::WALL_TOP,
        theme::WALL_BOTTOM,
    );
    let front = wm.focused();
    for w in wm.wins.iter().filter(|_| !desk.locked) {
        if !w.frame.grow(crate::wm::SHADOW).intersect(clip).is_empty() {
            draw_window(cv, fonts, w, &views(w.id), Some(w.id) == front);
        }
    }
    if let Some(p) = wm.preview.filter(|_| !desk.locked) {
        let p = p.grow(-6);
        cv.round_rect_alpha(p, 12, theme::ACCENT_SOFT, 90);
        cv.round_frame(p, 12, theme::ACCENT_SOFT);
    }
    // The group labels.
    for (i, g) in desk.groups.iter().enumerate().filter(|_| !desk.locked) {
        let r = group_chip(sw, sh, desk.dock.len(), desk.groups.len(), i);
        if r.intersect(clip).is_empty() {
            continue;
        }
        cv.round_rect_alpha(r, 16, theme::BAR, 225);
        cv.round_rect(Rect::new(r.x + 10, r.y + 11, 10, 10), 5, g.color);
        let (asc, lh) = fonts.metrics(Face::Strong, 12);
        fonts.draw(
            cv,
            Face::Strong,
            12,
            r.x + 26,
            r.y + (r.h - lh) / 2 + asc,
            g.name,
            theme::INK,
        );
        if g.count > 0 {
            let n = alloc::format!("{}", g.count);
            let nw = fonts.width(Face::Text, 12, &n);
            fonts.draw(
                cv,
                Face::Text,
                12,
                r.right() - nw - 10,
                r.y + (r.h - lh) / 2 + asc,
                &n,
                theme::MUTED,
            );
        }
    }
    // The dock.
    if !desk.dock.is_empty() && !desk.locked {
        let d = dock_rect(sw, sh, desk.dock.len());
        if !d.intersect(clip).is_empty() {
            cv.round_rect_alpha(d, 14, theme::BAR, 225);
            for (i, item) in desk.dock.iter().enumerate() {
                let r = dock_item(sw, sh, desk.dock.len(), i);
                cv.round_rect(r, 10, item.color);
                let initial: &str = item.name.get(..1).unwrap_or("?");
                let tw = fonts.width(Face::Strong, 18, initial);
                let (asc, lh) = fonts.metrics(Face::Strong, 18);
                fonts.draw(
                    cv,
                    Face::Strong,
                    18,
                    r.x + (r.w - tw) / 2,
                    r.y + (r.h - lh) / 2 + asc,
                    initial,
                    0xff_ff_ff,
                );
                if item.running {
                    cv.round_rect(
                        Rect::new(r.x + r.w / 2 - 3, r.bottom() + 1, 6, 3),
                        1,
                        theme::ACCENT,
                    );
                }
            }
        }
    }
    // The strip at the top: only Alaya draws here.
    cv.fill(Rect::new(0, 0, sw, STRIP), theme::BAR);
    cv.fill(Rect::new(0, STRIP - 1, sw, 1), theme::LINE);
    let (asc, lh) = fonts.metrics(Face::Strong, 14);
    let ty = (STRIP - lh) / 2 + asc;
    fonts.draw(cv, Face::Strong, 14, 14, ty, "ALAYA", theme::BAR_INK);
    if let Some(pic) = desk.trusted {
        // Only the real PIN box shows the person's picture here.
        let x = 14 + fonts.width(Face::Strong, 14, "ALAYA") + 18;
        let r = Rect::new(x, (STRIP - 24) / 2, 24, 24);
        cv.round_rect(r.grow(2), 6, theme::WIN);
        crate::login::draw_picture(cv, r, pic);
        fonts.draw(
            cv,
            Face::Text,
            13,
            r.right() + 10,
            ty,
            "Your picture: this PIN box is Alaya's own",
            theme::BAR_INK,
        );
    }
    if desk.buttons && !desk.locked {
        for (r, name) in strip_buttons().iter().zip(STRIP_BUTTONS) {
            cv.round_rect(*r, 6, theme::CHROME);
            cv.round_frame(*r, 6, theme::LINE);
            let tw = fonts.width(Face::Strong, 12, name);
            let (a2, l2) = fonts.metrics(Face::Strong, 12);
            fonts.draw(
                cv,
                Face::Strong,
                12,
                r.x + (r.w - tw) / 2,
                r.y + (r.h - l2) / 2 + a2,
                name,
                theme::BAR_INK,
            );
        }
    }
    let cw = fonts.width(Face::Text, 14, desk.clock);
    fonts.draw(
        cv,
        Face::Text,
        14,
        sw - cw - 14,
        ty,
        desk.clock,
        theme::BAR_INK,
    );
    if !desk.status.is_empty() {
        let w = fonts.width(Face::Text, 13, desk.status);
        let (a2, l2) = fonts.metrics(Face::Text, 13);
        fonts.draw(
            cv,
            Face::Text,
            13,
            (sw - w) / 2,
            (STRIP - l2) / 2 + a2,
            desk.status,
            theme::MUTED,
        );
    }
    if let Some(q) = desk.question.filter(|_| !desk.locked) {
        draw_question(cv, fonts, q, sw, sh);
    }
    if desk.show_pointer {
        let (px, py) = wm.pointer;
        draw_pointer(cv, px, py);
    }
    cv.set_clip(old);
}

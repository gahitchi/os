// SPDX-License-Identifier: GPL-3.0-only
//! Host tests of the toolkit: fonts, drawing, window rules, text editing.

use std::vec;
use std::vec::Vec;

use crate::Rect;
use crate::canvas::Canvas;
use crate::font::Font;
use crate::text::{Face, Fonts};
use crate::widgets::{Key, Pointer, Role, TextEdit, Ui, us_key};
use crate::wm::{Hit, Out, Place, STRIP, TITLE, Wm};

const FIGTREE: &[u8] = include_bytes!("../../fonts/Figtree-Regular.ttf");
const FIGTREE_SEMI: &[u8] = include_bytes!("../../fonts/Figtree-SemiBold.ttf");
const PLEX: &[u8] = include_bytes!("../../fonts/IBMPlexMono-Regular.ttf");
const NOTO: &[u8] = include_bytes!("../../fonts/NotoSans-Regular.ttf");

fn fonts() -> Fonts<'static> {
    Fonts::new(FIGTREE, FIGTREE_SEMI, PLEX, NOTO)
}

#[test]
fn all_four_fonts_parse() {
    for f in [FIGTREE, FIGTREE_SEMI, PLEX, NOTO] {
        let font = Font::parse(f).expect("font parses");
        assert!(font.units_per_em >= 1000);
        assert!(font.glyph('A').is_some());
        assert!(font.ascender > 0 && font.descender < 0);
    }
}

#[test]
fn fallback_covers_greek_and_cyrillic() {
    let f = fonts();
    for c in ['Ω', 'Ж', 'ă', 'ß', 'é'] {
        assert!(f.has(Face::Text, c), "no glyph for {c}");
    }
    // Something no font has.
    assert!(!f.has(Face::Text, '\u{10FFFD}'));
}

#[test]
fn letters_have_solid_strokes_and_holes() {
    let font = Font::parse(FIGTREE).unwrap();
    let o = font.rasterize(font.glyph('O').unwrap(), 48.0);
    assert!(o.width > 20 && o.height > 25);
    // The middle of an O is empty; its left stroke is solid.
    let at = |x: usize, y: usize| o.coverage[y * o.width + x];
    assert_eq!(at(o.width / 2, o.height / 2), 0);
    let row = o.height / 2;
    let max = (0..o.width / 3).map(|x| at(x, row)).max().unwrap();
    assert!(max >= 250, "the left stroke of O is not solid ({max})");
    // Coverage is never more than full anywhere.
    let l = font.rasterize(font.glyph('l').unwrap(), 30.0);
    assert!(l.coverage.contains(&255));
}

#[test]
fn composite_glyphs_draw() {
    // Accented letters are usually built from two glyphs.
    let font = Font::parse(NOTO).unwrap();
    let e = font.rasterize(font.glyph('é').unwrap(), 40.0);
    let plain = font.rasterize(font.glyph('e').unwrap(), 40.0);
    assert!(e.height > plain.height, "the accent is missing");
}

#[test]
fn broken_fonts_never_crash() {
    let mut seed = 0x1234_5678u64;
    let mut rnd = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    assert!(Font::parse(&[]).is_none());
    assert!(Font::parse(b"OTTO and nothing else").is_none());
    for cut in [12, 100, 1000, 20_000, FIGTREE.len() - 1] {
        if let Some(f) = Font::parse(&FIGTREE[..cut]) {
            for c in "Alaya Ω".chars() {
                if let Some(g) = f.glyph(c) {
                    f.rasterize(g, 20.0);
                }
            }
        }
    }
    // Flip random bytes in the outline area and draw everything.
    for _ in 0..200 {
        let mut bad = FIGTREE.to_vec();
        for _ in 0..50 {
            let i = (rnd() as usize) % bad.len();
            bad[i] = rnd() as u8;
        }
        if let Some(f) = Font::parse(&bad) {
            for c in "Alaya 0123".chars() {
                if let Some(g) = f.glyph(c) {
                    f.rasterize(g, 16.0);
                }
            }
        }
    }
}

#[test]
fn text_measures_and_wraps() {
    let f = fonts();
    let w = f.width(Face::Text, 14, "Alaya");
    assert!(w > 30 && w < 60, "width {w}");
    // Mono: every character is as wide as every other.
    assert_eq!(
        f.width(Face::Mono, 14, "iiii"),
        f.width(Face::Mono, 14, "MMMM")
    );
    let s = "the quick brown fox jumps over the lazy dog";
    let lines = f.wrap(Face::Text, 14, s, 120);
    assert!(lines.len() >= 3);
    for (a, b) in &lines {
        assert!(f.width(Face::Text, 14, s[*a..*b].trim_end()) <= 120);
    }
    let rebuilt: std::string::String = lines.iter().map(|(a, b)| &s[*a..*b]).collect();
    assert_eq!(rebuilt, s);
}

#[test]
fn drawing_stays_inside_the_picture() {
    let mut px = vec![0u32; 100 * 50];
    let mut cv = Canvas::new(&mut px, 100, 50, 100).unwrap();
    cv.fill(Rect::new(-50, -50, 500, 500), 0xff0000);
    cv.round_rect(Rect::new(90, 40, 100, 100), 20, 0x00ff00);
    cv.shadow(Rect::new(-10, -10, 200, 200), 10, 8);
    let mut f = fonts();
    f.draw(&mut cv, Face::Text, 40, 80, 45, "Alaya Ω ✓", 0xffffff);
    assert!(Canvas::new(&mut px, 100, 51, 100).is_none());
}

#[test]
fn round_corners_are_soft() {
    let mut px = vec![0u32; 60 * 60];
    let mut cv = Canvas::new(&mut px, 60, 60, 60).unwrap();
    cv.round_rect(Rect::new(0, 0, 60, 60), 20, 0xffffff);
    assert_eq!(cv.get(0, 0), Some(0));
    assert_eq!(cv.get(30, 30), Some(0xffffff));
    assert_eq!(cv.get(30, 0), Some(0xffffff));
    // Somewhere on the corner's curve a pixel is part covered.
    let partial = (0..20)
        .flat_map(|y| (0..20).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let c = cv.get(x, y).unwrap() & 0xff;
            c > 0 && c < 255
        })
        .count();
    // The curve crosses about 30 pixels of the corner.
    assert!(partial > 15, "only {partial} soft pixels");
}

fn damage(out: &[Out]) -> bool {
    out.iter().any(|o| matches!(o, Out::Damage(_)))
}

#[test]
fn new_windows_go_in_front_and_get_the_keyboard() {
    let mut wm = Wm::new(1280, 800);
    let out = wm.add(1, 400, 300);
    assert!(out.contains(&Out::Focus(1)) && damage(&out));
    wm.add(2, 400, 300);
    assert_eq!(wm.focused(), Some(2));
    let w1 = wm.get(1).unwrap().frame;
    assert!(w1.y >= STRIP);
    assert_eq!(wm.get(1).unwrap().content().h, 300);
    // Clicking window 1's visible title bar raises it.
    wm.pointer_move(w1.x + 10, w1.y + 10);
    let out = wm.button(1, true);
    assert!(out.contains(&Out::Focus(1)));
    assert_eq!(wm.focused(), Some(1));
    wm.button(1, false);
}

#[test]
fn dragging_the_title_moves_the_window() {
    let mut wm = Wm::new(1280, 800);
    wm.add(1, 400, 300);
    let f = wm.get(1).unwrap().frame;
    wm.pointer_move(f.x + 50, f.y + 10);
    wm.button(1, true);
    wm.pointer_move(f.x + 250, f.y + 110);
    wm.button(1, false);
    let g = wm.get(1).unwrap().frame;
    assert_eq!((g.x, g.y, g.w, g.h), (f.x + 200, f.y + 100, f.w, f.h));
    assert_eq!(wm.get(1).unwrap().place, Place::Free);
}

#[test]
fn edges_snap_and_snapped_windows_come_loose() {
    let mut wm = Wm::new(1280, 800);
    wm.add(1, 400, 300);
    let f = wm.get(1).unwrap().frame;
    // Drag to the left edge: a preview, then the left half.
    wm.pointer_move(f.x + 50, f.y + 10);
    wm.button(1, true);
    wm.pointer_move(0, 400);
    assert_eq!(wm.preview, Some(Rect::new(0, STRIP, 640, 800 - STRIP)));
    let out = wm.button(1, false);
    let w = wm.get(1).unwrap();
    assert_eq!(w.place, Place::Left);
    assert_eq!(w.frame, Rect::new(0, STRIP, 640, 800 - STRIP));
    assert!(out.contains(&Out::Resized(1, 640, 800 - STRIP - TITLE)));
    assert_eq!(wm.preview, None);
    // Drag it away: back to its own size.
    wm.pointer_move(300, STRIP + 10);
    wm.button(1, true);
    wm.pointer_move(500, 300);
    wm.button(1, false);
    let w = wm.get(1).unwrap();
    assert_eq!(w.place, Place::Free);
    assert_eq!((w.frame.w, w.frame.h), (f.w, f.h));
    // Top edge fills the screen, right edge the right half.
    let before = w.frame;
    wm.pointer_move(w.frame.x + 20, w.frame.y + 10);
    wm.button(1, true);
    wm.pointer_move(700, 0);
    wm.button(1, false);
    assert_eq!(wm.get(1).unwrap().place, Place::Full);
    assert_eq!(wm.get(1).unwrap().frame, wm.area);
    let out = wm.place(1, Place::Right);
    assert!(damage(&out));
    assert_eq!(
        wm.get(1).unwrap().frame,
        Rect::new(640, STRIP, 640, 800 - STRIP)
    );
    wm.place(1, Place::Free);
    assert_eq!(wm.get(1).unwrap().frame, before);
}

#[test]
fn resizing_keeps_a_minimum_size() {
    let mut wm = Wm::new(1280, 800);
    wm.add(1, 400, 300);
    let f = wm.get(1).unwrap().frame;
    // Grab the bottom-right corner, just outside the window.
    wm.pointer_move(f.right() + 2, f.bottom() + 2);
    assert!(matches!(
        wm.hit(f.right() + 2, f.bottom() + 2),
        Hit::Edge(1, _)
    ));
    wm.button(1, true);
    wm.pointer_move(f.right() + 102, f.bottom() + 52);
    wm.button(1, false);
    let g = wm.get(1).unwrap().frame;
    assert_eq!((g.w, g.h), (f.w + 100, f.h + 50));
    // Shrink far past the minimum.
    wm.pointer_move(g.right() + 2, g.bottom() + 2);
    wm.button(1, true);
    wm.pointer_move(0, STRIP + 1);
    wm.button(1, false);
    let h = wm.get(1).unwrap().frame;
    assert_eq!((h.w, h.h), (crate::wm::MIN_W, crate::wm::MIN_H + TITLE));
}

#[test]
fn tiling_shares_the_screen_without_overlap() {
    let mut wm = Wm::new(1280, 800);
    for id in 1..=5 {
        wm.add(id, 300, 200);
    }
    wm.set_tiling(true);
    let frames: Vec<Rect> = wm.wins.iter().map(|w| w.frame).collect();
    for (i, a) in frames.iter().enumerate() {
        assert!(a.intersect(wm.area) == *a, "a tile leaves the screen");
        for b in &frames[i + 1..] {
            assert!(a.intersect(*b).is_empty(), "tiles overlap: {a:?} {b:?}");
        }
    }
    // A sixth window joins the tiles; closing one closes the gap.
    wm.add(6, 300, 200);
    assert!(wm.wins.iter().all(|w| w.place == Place::Tiled));
    wm.remove(3);
    let used: i64 = wm
        .wins
        .iter()
        .map(|w| i64::from(w.frame.w) * i64::from(w.frame.h))
        .sum();
    let area = i64::from(wm.area.w) * i64::from(wm.area.h);
    assert!(used * 100 / area > 80, "tiles leave too much empty");
    // Dragging one tile onto another swaps them.
    let (a, b) = (wm.get(1).unwrap().frame, wm.get(2).unwrap().frame);
    wm.pointer_move(a.x + 40, a.y + 10);
    wm.button(1, true);
    wm.pointer_move(b.x + 40, b.y + 60);
    wm.button(1, false);
    assert_eq!(wm.get(1).unwrap().frame, b);
    assert_eq!(wm.get(2).unwrap().frame, a);
    // Back to floating: every window where it was.
    wm.set_tiling(false);
    assert!(
        wm.wins
            .iter()
            .all(|w| w.place == Place::Free && w.frame == w.free)
    );
}

#[test]
fn clicks_in_the_content_go_to_the_app() {
    let mut wm = Wm::new(1280, 800);
    wm.add(7, 400, 300);
    let c = wm.get(7).unwrap().content();
    wm.pointer_move(c.x + 30, c.y + 40);
    let out = wm.button(1, true);
    assert!(out.contains(&Out::Pointer {
        id: 7,
        x: 30,
        y: 40,
        button: 1,
        down: true,
        moved: false
    }));
    // Moves while held go to it too, even outside the window.
    let out = wm.pointer_move(c.x - 100, c.y + 40);
    assert!(
        out.iter()
            .any(|o| matches!(o, Out::Pointer { id: 7, x: -100, .. }))
    );
    let out = wm.button(1, false);
    assert!(out.iter().any(|o| matches!(
        o,
        Out::Pointer {
            id: 7,
            down: false,
            button: 1,
            ..
        }
    )));
    // The close button asks the app.
    let x = wm.get(7).unwrap().close_button();
    wm.pointer_move(x.x + 5, x.y + 5);
    assert!(wm.button(1, true).contains(&Out::Close(7)));
}

#[test]
fn text_editing() {
    let mut e = TextEdit::new("");
    for c in "héllo".chars() {
        e.key(Key::Char(c), true);
    }
    e.key(Key::Enter, true);
    e.key(Key::Char('w'), true);
    assert_eq!(e.text, "héllo\nw");
    assert_eq!(e.position(), (1, 1));
    e.key(Key::Up, true);
    assert_eq!(e.position(), (0, 1));
    e.key(Key::Right, true);
    e.key(Key::Backspace, true);
    assert_eq!(e.text, "hllo\nw");
    e.key(Key::End, true);
    e.key(Key::Delete, true);
    assert_eq!(e.text, "hllow");
    e.key(Key::Home, true);
    assert_eq!(e.cursor, 0);
    e.key(Key::Backspace, true);
    assert_eq!(e.text, "hllow");
    // One-line boxes ignore Enter.
    assert!(!e.key(Key::Enter, false));
}

#[test]
fn us_keyboard() {
    assert_eq!(us_key(30, false), Some(Key::Char('a')));
    assert_eq!(us_key(30, true), Some(Key::Char('A')));
    assert_eq!(us_key(2, true), Some(Key::Char('!')));
    assert_eq!(us_key(57, false), Some(Key::Char(' ')));
    assert_eq!(us_key(28, false), Some(Key::Enter));
    assert_eq!(us_key(14, false), Some(Key::Backspace));
    assert_eq!(us_key(42, false), None); // Shift itself
    assert_eq!(us_key(400, false), None);
}

#[test]
fn widgets_write_the_accessibility_tree_and_press_by_name() {
    let mut px = vec![0u32; 400 * 300];
    let mut cv = Canvas::new(&mut px, 400, 300, 400).unwrap();
    let mut f = fonts();
    let mut edit = TextEdit::new("12");
    let mut ui = Ui::new(&mut cv, &mut f, Pointer::default());
    ui.press_by_name = Some("Save".into());
    ui.label(10, 10, "Numbers");
    ui.text_box(Rect::new(10, 40, 200, 120), "Numbers", &mut edit, true);
    let (saved, _) = ui.button(10, 200, "Save", true);
    let (other, _) = ui.button(100, 200, "Close", false);
    ui.list(
        Rect::new(220, 10, 170, 200),
        "Files",
        &["a.txt", "b.txt"],
        Some(1),
        &[],
    );
    assert!(saved && !other);
    let roles: Vec<Role> = ui.tree.iter().map(|n| n.role).collect();
    assert_eq!(
        roles,
        [
            Role::Label,
            Role::TextBox,
            Role::Button,
            Role::Button,
            Role::List,
            Role::ListItem,
            Role::ListItem
        ]
    );
    assert_eq!(ui.tree[1].value, "12");
    assert_eq!(ui.tree[6].value, "selected");
    // A real click: down and up on the button.
    let mut ui = Ui::new(
        &mut cv,
        &mut f,
        Pointer {
            x: 110,
            y: 210,
            down: false,
            released: true,
            down_x: 110,
            down_y: 210,
        },
    );
    let (a, _) = ui.button(10, 200, "Save", true);
    let (b, _) = ui.button(100, 200, "Close", false);
    assert!(!a && b);
}

/// Draws a whole desktop and saves it as `target/desktop.ppm`, for a
/// person to look at (`cargo test -p alaya-ui -- --ignored picture`).
#[test]
#[ignore]
fn picture_of_the_desktop() {
    use crate::scene::{self, Desktop, DockItem, WindowView};
    let (w, h) = (1280, 800);
    let mut f = fonts();
    // An app's window content, drawn with the toolkit.
    let (aw, ah) = (460, 300);
    let mut app = vec![0u32; (aw * ah) as usize];
    {
        let mut cv = Canvas::new(&mut app, aw, ah, aw).unwrap();
        cv.fill(Rect::new(0, 0, aw, ah), crate::widgets::theme::WIN);
        let mut ui = Ui::new(&mut cv, &mut f, Pointer::default());
        ui.label(16, 12, "numbers.txt — the sales of each month");
        let mut e = TextEdit::new("January 12\nFebruary 19\nMarch 7\nΩ Ж ăîșț ß");
        ui.text_box(Rect::new(16, 40, 280, 150), "Numbers", &mut e, true);
        ui.button(16, 210, "Save", true);
        ui.button(90, 210, "Make a chart", false);
        let marks = [
            Some((
                "Up to date",
                crate::widgets::theme::OK,
                crate::widgets::theme::OK_BG,
            )),
            Some((
                "Needs a look",
                crate::widgets::theme::WARN,
                crate::widgets::theme::WARN_BG,
            )),
        ];
        ui.list(
            Rect::new(310, 40, 140, 150),
            "Files",
            &["chart.png", "report"],
            Some(0),
            &marks,
        );
    }
    let app: &'static [u32] = std::boxed::Box::leak(app.into_boxed_slice());
    let mut wm = Wm::new(w, h);
    wm.add(1, 520, 340);
    wm.add(2, aw, ah);
    wm.pointer_move(700, 420);
    let mut px = vec![0u32; (w * h) as usize];
    let mut cv = Canvas::new(&mut px, w, h, w).unwrap();
    let views = |id: u32| WindowView {
        title: if id == 1 {
            "Files"
        } else {
            "Text editor — numbers.txt"
        },
        pixels: if id == 2 {
            Some((app, aw, ah, aw))
        } else {
            None
        },
        stuck: false,
    };
    let dock = [
        DockItem {
            name: "Files",
            color: 0x23_68_6b,
            running: true,
        },
        DockItem {
            name: "Editor",
            color: 0x2d_5c_9c,
            running: true,
        },
        DockItem {
            name: "Chart",
            color: 0x9a_5b_00,
            running: false,
        },
        DockItem {
            name: "Settings",
            color: 0x5b_6b_6c,
            running: false,
        },
    ];
    let desk = Desktop {
        clock: "10:42",
        status: "",
        dock: &dock,
        show_pointer: true,
    };
    scene::draw(&mut cv, &mut f, &wm, &views, &desk, Rect::new(0, 0, w, h));
    let mut out = std::format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in &px {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../target/desktop.ppm"),
        out,
    )
    .unwrap();
}

// SPDX-License-Identifier: GPL-3.0-only
//! Host tests of the toolkit: fonts, drawing, window rules, text editing.

use std::vec;
use std::vec::Vec;

use crate::Rect;
use crate::canvas::Canvas;
use crate::font::Font;
use crate::login::{Attempt, Login, Look, NewPerson, Person, Setup, decode_person, encode_person};
use crate::text::{Face, Fonts};
use crate::widgets::{Key, Pointer, Role, TextEdit, Ui, us_key};
use crate::wm::{Hit, Out, Place, STRIP, Screen, TITLE, Wm};

const FIGTREE: &[u8] = include_bytes!("../../fonts/Figtree-Regular.ttf");
const FIGTREE_SEMI: &[u8] = include_bytes!("../../fonts/Figtree-SemiBold.ttf");
const PLEX: &[u8] = include_bytes!("../../fonts/IBMPlexMono-Regular.ttf");
const NOTO: &[u8] = include_bytes!("../../fonts/NotoSans-Alaya.ttf");

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
    use crate::scene::{self, Desktop, DockItem, GroupChip, WindowView};
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
        stuck: None,
        group: (id == 2).then_some(("Work", 0x23_68_6b)),
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
        groups: &[
            GroupChip {
                name: "Work",
                color: 0x23_68_6b,
                count: 1,
            },
            GroupChip {
                name: "Home",
                color: 0x9a_5b_00,
                count: 0,
            },
        ],
        show_pointer: true,
        locked: false,
        trusted: None,
        buttons: true,
        question: None,
        picker: None,
        screens: &[],
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
    // The login screen, with the picture in the strip.
    let mut px = vec![0u32; (w * h) as usize];
    let mut cv = Canvas::new(&mut px, w, h, w).unwrap();
    let mut login = Login::new(
        vec![Person {
            id: 1,
            name: "Robin".into(),
            picture: 2,
        }],
        "Checked by Alaya, not yet by the chip: the login screen is the same as at your last login.".into(),
    );
    login.key(Key::Char('1'));
    let desk = Desktop {
        locked: true,
        trusted: login.picture(),
        ..desk
    };
    scene::draw(&mut cv, &mut f, &wm, &views, &desk, Rect::new(0, 0, w, h));
    login.draw(&mut cv, &mut f);
    let mut out = std::format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in &px {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../target/login.ppm"),
        out,
    )
    .unwrap();
    let mut px = vec![0u32; (w * h) as usize];
    let mut cv = Canvas::new(&mut px, w, h, w).unwrap();
    let desk = Desktop {
        trusted: None,
        ..desk
    };
    scene::draw(&mut cv, &mut f, &wm, &views, &desk, Rect::new(0, 0, w, h));
    let mut setup = Setup::new();
    for c in "Robin".chars() {
        setup.key(Key::Char(c), false);
    }
    setup.key(Key::Tab, false);
    setup.key(Key::Char('1'), false);
    setup.draw(&mut cv, &mut f);
    let mut out = std::format!("P6\n{w} {h}\n255\n").into_bytes();
    for p in &px {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../target/setup.ppm"),
        out,
    )
    .unwrap();
}

#[test]
fn setup_checks_what_was_typed() {
    let mut s = Setup::new();
    let typed = |s: &mut Setup, t: &str| {
        for c in t.chars() {
            s.key(Key::Char(c), false);
        }
    };
    // No name.
    assert_eq!(s.key(Key::Enter, false), None);
    assert_eq!(s.focus, 0);
    typed(&mut s, "Robin");
    s.key(Key::Tab, false);
    typed(&mut s, "12");
    assert_eq!(s.key(Key::Enter, false), None);
    assert_eq!(s.focus, 1, "a short PIN is refused");
    typed(&mut s, "34");
    s.key(Key::Tab, false);
    typed(&mut s, "1235");
    assert_eq!(s.key(Key::Enter, false), None);
    assert_eq!(s.focus, 2, "two different PINs are refused");
    assert!(s.again.text.is_empty());
    typed(&mut s, "1234");
    s.key(Key::Tab, false);
    s.key(Key::Right, false);
    s.key(Key::Right, false);
    s.key(Key::Tab, false);
    s.key(Key::Right, false);
    // Shift+Tab goes back, and Tab forward again.
    s.key(Key::Tab, true);
    assert_eq!(s.focus, 3);
    let done = s.key(Key::Enter, false).expect("setup finishes");
    assert_eq!(
        done,
        NewPerson {
            name: "Robin".into(),
            pin: "1234".into(),
            picture: 2,
            look: Look::Everything,
        }
    );
}

#[test]
fn login_takes_a_pin_and_hides_it() {
    let people = vec![
        Person {
            id: 1,
            name: "Robin".into(),
            picture: 3,
        },
        Person {
            id: 2,
            name: "Kai".into(),
            picture: 5,
        },
    ];
    let mut l = Login::new(people, std::string::String::new());
    assert_eq!(l.picture(), Some(3));
    for c in "2468".chars() {
        l.key(Key::Char(c));
    }
    l.key(Key::Backspace);
    l.key(Key::Char('9'));
    // Choosing another person clears the PIN and shows their picture.
    l.key(Key::Right);
    assert!(l.pin.text.is_empty());
    assert_eq!(l.picture(), Some(5));
    l.key(Key::Left);
    for c in "2469".chars() {
        l.key(Key::Char(c));
    }
    let a = l.key(Key::Enter).expect("an attempt");
    assert_eq!(
        a,
        Attempt {
            person: 0,
            pin: "2469".into()
        }
    );
    assert!(l.pin.text.is_empty(), "the PIN is not kept on screen");
    // Keys wait while the PIN is checked.
    assert_eq!(l.key(Key::Char('1')), None);
    assert!(l.pin.text.is_empty());
    l.refused();
    assert!(l.error.is_some());
    l.key(Key::Char('1'));
    assert_eq!(l.pin.text, "1");
}

#[test]
fn person_records_round_trip() {
    let p = Person {
        id: 3,
        name: "Ăna Ω".into(),
        picture: 7,
    };
    assert_eq!(decode_person(&encode_person(&p)), Some(p));
    assert_eq!(decode_person(&[0u8; 64]), None);
}

#[test]
fn dropping_a_window_on_a_group_label_groups_it() {
    let mut wm = Wm::new(1280, 800);
    wm.add(1, 400, 300);
    wm.add(2, 400, 300);
    wm.groups = vec![(7, Rect::new(100, 740, 90, 32))];
    let before = wm.get(2).unwrap().frame;
    let t = wm.get(2).unwrap().title_bar();
    wm.pointer_move(t.x + 50, t.y + 10);
    wm.button(1, true);
    wm.pointer_move(300, 600);
    wm.pointer_move(140, 750);
    let out = wm.button(1, false);
    assert!(out.contains(&Out::Grouped(2, 7)));
    assert_eq!(wm.get(2).unwrap().group, Some(7));
    assert_eq!(wm.get(2).unwrap().frame, before, "back where it was");
    // Raising the group brings its windows to the front.
    wm.raise(1, &mut Vec::new());
    assert_eq!(wm.focused(), Some(1));
    wm.raise_group(7);
    assert_eq!(wm.focused(), Some(2));
    // A drop elsewhere moves the window, no group.
    let t = wm.get(1).unwrap().title_bar();
    wm.pointer_move(t.x + 50, t.y + 10);
    wm.button(1, true);
    wm.pointer_move(t.x + 150, t.y + 60);
    wm.button(1, false);
    assert_eq!(wm.get(1).unwrap().group, None);
}

#[test]
fn wrap_breaks_at_a_space_that_does_not_fit() {
    let f = fonts();
    let s = "aaaa aaaa aa";
    let max = f.width(Face::Text, 14, "aaaa");
    let lines = f.wrap(Face::Text, 14, s, max);
    let got: Vec<&str> = lines.iter().map(|&(a, z)| &s[a..z]).collect();
    assert_eq!(got, ["aaaa ", "aaaa ", "aa"]);
}

#[test]
fn screens_come_and_go_and_windows_follow() {
    let mut wm = Wm::new(1280, 800);
    let main = Screen {
        id: 0,
        rect: Rect::new(0, 0, 1280, 800),
        scale: 100,
    };
    let two = Screen {
        id: 1,
        rect: Rect::new(1280, 0, 1024, 768),
        scale: 100,
    };
    wm.set_screens(vec![main, two]);
    wm.add(1, 500, 300);
    wm.add(2, 400, 300);
    // To the second screen, at the same place on it.
    wm.to_screen(2, 1);
    let away = wm.get(2).unwrap().frame;
    assert!(two.rect.contains(away.x, away.y), "{away:?}");
    // Full screen there is that screen.
    wm.place(2, Place::Full);
    assert_eq!(wm.get(2).unwrap().frame, two.rect);
    wm.place(2, Place::Free);
    let away = wm.get(2).unwrap().frame;
    let stay = wm.get(1).unwrap().frame;
    // Unplugged: on the main screen, whole, below the strip.
    wm.set_screens(vec![main]);
    let f = wm.get(2).unwrap().frame;
    assert!(
        f.right() <= 1280 && f.bottom() <= 800 && f.y >= STRIP,
        "{f:?}"
    );
    assert_eq!(
        wm.get(1).unwrap().frame,
        stay,
        "a window on the main screen moved"
    );
    // Plugged in again: back where it was.
    wm.set_screens(vec![main, two]);
    assert_eq!(wm.get(2).unwrap().frame, away);
    // The edge between two screens is a way across, not a snap.
    assert_eq!(wm.screen_at(1279, 400), 0);
    assert_eq!(wm.screen_at(1280, 400), 1);
}

#[test]
fn a_window_keeps_its_size_in_points_across_scales() {
    let mut wm = Wm::new(1280, 800);
    let main = Screen {
        id: 0,
        rect: Rect::new(0, 0, 1280, 800),
        scale: 100,
    };
    let dense = Screen {
        id: 1,
        rect: Rect::new(1280, 0, 1920, 1200),
        scale: 150,
    };
    wm.set_screens(vec![main, dense]);
    wm.add(1, 400, 300);
    let c = wm.get(1).unwrap().content();
    assert_eq!((c.w, c.h), (400, 300));
    // Onto the dense screen: half as many pixels again, the same points.
    let out = wm.to_screen(1, 1);
    let w = wm.get(1).unwrap();
    assert_eq!((w.content().w, w.content().h, w.scale), (600, 450, 150));
    assert!(
        out.iter().any(|o| matches!(o, Out::Resized(1, 600, 450))),
        "{out:?}"
    );
    // Back: as it was.
    wm.to_screen(1, 1);
    let w = wm.get(1).unwrap();
    assert_eq!((w.content().w, w.content().h, w.scale), (400, 300, 100));
    // The main screen goes to 200: the window doubles and says so.
    let out = wm.set_scale(0, 200);
    let w = wm.get(1).unwrap();
    assert_eq!((w.content().w, w.content().h, w.scale), (800, 600, 200));
    assert!(out.iter().any(|o| matches!(o, Out::Resized(1, 800, 600))));
    // A new window there opens at 200 too.
    wm.add(2, 300, 200);
    let c = wm.get(2).unwrap().content();
    assert_eq!((c.w, c.h), (600, 400));
}

#[test]
fn a_scaled_canvas_draws_points_as_pixel_blocks() {
    let mut px = vec![0u32; 40 * 40];
    {
        let mut cv = Canvas::new(&mut px, 40, 40, 40).unwrap().scaled(200);
        assert_eq!((cv.width, cv.height), (20, 20));
        cv.fill(Rect::new(1, 1, 2, 2), 0xff);
        assert_eq!(cv.get(1, 1), Some(0xff));
    }
    // Points 1..3 are pixels 2..6.
    for y in 0..40 {
        for x in 0..40 {
            let on = (2..6).contains(&x) && (2..6).contains(&y);
            assert_eq!(px[y * 40 + x] == 0xff, on, "({x}, {y})");
        }
    }
    // The clip is in points too, and comes back as it was asked.
    {
        let mut cv = Canvas::new(&mut px, 40, 40, 40).unwrap().scaled(150);
        let old = cv.set_clip(Rect::new(0, 0, 4, 4));
        assert_eq!(cv.clip(), Rect::new(0, 0, 4, 4));
        cv.fill(Rect::new(0, 0, 20, 20), 0x11);
        cv.set_clip(old);
    }
    assert_eq!(px[5 * 40 + 5], 0x11);
    assert_eq!(px[6 * 40 + 6], 0, "the clip ends at pixel 6");
}

#[test]
fn high_contrast_text_is_at_least_seven_to_one() {
    use crate::canvas::{contrast, high_contrast as hc};
    use crate::widgets::theme as t;
    // The ratio itself: black on white is 21 to 1.
    assert!((2099..=2100).contains(&contrast(0, 0xff_ff_ff)));
    let pairs = [
        ("text on a window", t::INK, t::WIN),
        ("quiet text on a window", t::MUTED, t::WIN),
        ("the strip", t::BAR_INK, t::BAR),
        ("a primary button", t::ACCENT_INK, t::ACCENT),
        ("the keyboard ring", t::ACCENT, t::WIN),
        ("a hovered button", t::INK, t::ACCENT_SOFT),
        ("a button", t::INK, t::CHROME),
        ("changed", t::OK, t::OK_BG),
        ("needs a look", t::WARN, t::WARN_BG),
        ("from the past", t::PAST, t::PAST_BG),
        ("unknown", t::UNKNOWN, t::UNKNOWN_BG),
        ("now", t::NOW, t::WIN),
        ("a frame line", t::LINE, t::WIN),
    ];
    for (what, fg, bg) in pairs {
        let c = contrast(hc(fg), hc(bg));
        assert!(c >= 700, "{what}: {c} (from {})", contrast(fg, bg));
    }
    // Colours of apps' own (a chart's bars) stay readable on the ground.
    for c in [0x22_88_cc, 0xcc_33_33, 0x33_aa_55, 0x88_88_88, 0x40_40_40] {
        let k = contrast(hc(c), hc(t::WIN));
        assert!(k >= 450, "{c:06x}: {k}");
    }
}

#[test]
fn the_screen_reader_reads_the_tree_an_app_sends() {
    use crate::widgets::{Node, decode_tree, encode_tree, say_focused};
    let node = |role, name: &str, value: &str, focused| Node {
        role,
        name: name.into(),
        rect: Rect::default(),
        value: value.into(),
        focused,
    };
    let tree = vec![
        node(Role::Button, "Back up now", "", false),
        node(Role::List, "What apps can see", "", true),
        node(Role::ListItem, "Editor · notes.txt", "", false),
        node(Role::ListItem, "Chart · data.txt", "selected", false),
        node(Role::TextBox, "note", "héllo", false),
    ];
    let b = encode_tree(&tree, 4096);
    let back = decode_tree(&b);
    assert_eq!(back.len(), 5);
    assert_eq!(back[4].value, "héllo");
    assert_eq!(
        say_focused(&back),
        Some((
            Role::List,
            "What apps can see, list, 2 items, chosen: Chart · data.txt".into()
        ))
    );
    // A cut tree still reads; rubbish from an app ends it.
    assert!(decode_tree(&b[..b.len() - 3]).len() == 4);
    assert!(decode_tree(&[9, 9, 9, 9, 9]).is_empty());
    assert!(encode_tree(&tree, 20).len() <= 20);
}

#[test]
fn right_to_left_lines_read_in_the_right_order() {
    use crate::script::{join_arabic, visual};
    assert_eq!(visual("plain text"), "plain text");
    assert_eq!(visual("אבג"), "גבא");
    // A Hebrew word in an English line.
    assert_eq!(visual("hello שלום world"), "hello םולש world");
    // An English word in a Hebrew line; the line runs from the right.
    assert_eq!(visual("שלום hello עולם"), "םלוע hello םולש");
    // Numbers read left to right among Hebrew.
    assert_eq!(visual("שנת 2024"), "2024 תנש");
    // Brackets turn round inside a right-to-left run.
    assert_eq!(visual("א(ב)"), "(ב)א");
    // Arabic letters join: س at the start, لا as one sign, م alone.
    assert_eq!(join_arabic("سلام"), "\u{FEB3}\u{FEFC}\u{FEE1}");
    assert_eq!(visual("سلام"), "\u{FEE1}\u{FEFC}\u{FEB3}");
    // Letters that only join backwards break the word: د then ر.
    assert_eq!(join_arabic("بدر"), "\u{FE91}\u{FEAA}\u{FEAD}");
    // Marks do not break a join: بَب.
    assert_eq!(join_arabic("بَب"), "\u{FE91}\u{064E}\u{FE90}");
}

#[test]
fn layouts_dead_keys_and_compose_give_the_right_letters() {
    use crate::input::{Ime, Layout, Out, Sym, code, sym};
    let text = |o: Out| match o {
        Out::Commit(s) => s,
        o => panic!("{o:?}"),
    };
    // German: the key US calls y is z; ´ then e is é; AltGr+q is @.
    assert_eq!(sym(Layout::German, 21, false, false), Some(Sym::Char('z')));
    assert_eq!(sym(Layout::German, 16, false, true), Some(Sym::Char('@')));
    let mut ime = Ime::new(Layout::German);
    assert_eq!(ime.key(13, false, false), Out::Held);
    assert_eq!(text(ime.key(18, false, false)), "é");
    // A dead key then space: the mark itself.
    ime.key(13, false, false);
    assert_eq!(text(ime.key(code::SPACE, false, false)), "´");
    // A dead key then a letter it cannot mark: both.
    ime.key(41, false, false);
    assert_eq!(text(ime.key(45, false, false)), "^x");
    // French AZERTY: the first letter key is a; the 2 key gives é.
    assert_eq!(sym(Layout::French, 16, false, false), Some(Sym::Char('a')));
    let mut fr = Ime::new(Layout::French);
    assert_eq!(text(fr.key(3, false, false)), "é");
    // ^ then e: ê.
    fr.key(26, false, false);
    assert_eq!(text(fr.key(18, false, false)), "ê");
    // Spanish ñ; Italian è; UK £.
    assert_eq!(sym(Layout::Spanish, 39, false, false), Some(Sym::Char('ñ')));
    assert_eq!(sym(Layout::Italian, 26, false, false), Some(Sym::Char('è')));
    assert_eq!(sym(Layout::Uk, 4, true, false), Some(Sym::Char('£')));
    // US is as before.
    assert_eq!(sym(Layout::Us, 30, true, false), Some(Sym::Char('A')));
    // Compose: a e is æ; s s is ß; ' e is é; e = is €.
    let mut us = Ime::new(Layout::Us);
    for (keys, want) in [
        ([30u16, 18], "æ"),
        ([31, 31], "ß"),
        ([40, 18], "é"),
        ([18, 13], "€"),
    ] {
        assert_eq!(us.key(code::COMPOSE, false, false), Out::Held);
        assert_eq!(us.key(keys[0], false, false), Out::Held);
        assert_eq!(text(us.key(keys[1], false, false)), want);
    }
    // Keys without text pass to the window.
    assert_eq!(us.key(code::ENTER, false, false), Out::Pass);
}

#[test]
fn japanese_and_chinese_are_typed_from_latin_letters() {
    use crate::input::{Ime, Layout, Mode, Out, code, kana, pinyin};
    assert_eq!(kana("nihongo"), ("にほんご".into(), "".into()));
    assert_eq!(kana("kitte"), ("きって".into(), "".into()));
    assert_eq!(kana("shinbun"), ("しんぶ".into(), "n".into()));
    assert_eq!(kana("ky"), ("".into(), "ky".into()));
    assert_eq!(pinyin("nihao")[0], "你好");
    assert_eq!(pinyin("shi")[0], "是");
    // Type with the keyboard's codes: n i h o n g o, Enter.
    let keys = |ime: &mut Ime, s: &str| {
        let mut last = Out::Held;
        for c in s.chars() {
            let (code, _) = crate::input::tests_code(c);
            last = ime.key(code, false, false);
        }
        last
    };
    let mut ime = Ime::new(Layout::Us);
    ime.next_mode();
    assert_eq!(ime.mode, Mode::Japanese);
    keys(&mut ime, "nihongo");
    assert_eq!(ime.preedit(), "にほんご");
    assert_eq!(
        ime.key(code::ENTER, false, false),
        Out::Commit("にほんご".into())
    );
    ime.next_mode();
    assert_eq!(ime.mode, Mode::Chinese);
    keys(&mut ime, "nihao");
    assert_eq!(ime.key(code::SPACE, false, false), Out::Held);
    assert_eq!(ime.candidates[0], "你好");
    assert_eq!(
        ime.key(code::SPACE, false, false),
        Out::Commit("你好".into())
    );
    // A digit picks a candidate: shi, then 2.
    keys(&mut ime, "shi");
    ime.key(code::SPACE, false, false);
    assert_eq!(ime.key(3, false, false), Out::Commit("时".into()));
    ime.next_mode();
    assert_eq!(ime.mode, Mode::Latin);
}

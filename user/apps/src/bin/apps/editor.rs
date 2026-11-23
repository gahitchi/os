// SPDX-License-Identifier: GPL-3.0-only
//! The text editor: one text file, `Notes.txt` in the person's documents.
//!
//! Ctrl+S saves the file. Besides that, after every change the editor
//! writes down where you are (its return point, `continuity.md`): the
//! text as it is now, saved or not, the cursor and the first line shown.
//! The store makes that safe within five seconds (decision 100), so if the
//! editor is closed, crashes or is stopped, it opens again exactly where
//! you were.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::Store;
use alaya_apps::{self as app, Event, Window};
use alaya_programs::files::{APPS, DOCS};
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::Mem;
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{TextEdit, Ui, theme};

const FILE: &str = "Notes.txt";
const RETURN: &str = "editor-return";
const MAGIC: u64 = 0x414c_5941_5245_5431; // "ALYARET1"

/// The return point: cursor, first line shown, whether the text differs
/// from the file, and the text.
fn encode(e: &TextEdit, dirty: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(40 + e.text.len());
    for w in [
        MAGIC,
        e.cursor as u64,
        e.scroll as u64,
        u64::from(dirty),
        e.text.len() as u64,
    ] {
        b.extend_from_slice(&w.to_le_bytes());
    }
    b.extend_from_slice(e.text.as_bytes());
    b
}

fn decode(b: &[u8]) -> Option<(TextEdit, bool)> {
    let w = |i: usize| -> Option<u64> {
        Some(u64::from_le_bytes(
            b.get(8 * i..8 * i + 8)?.try_into().ok()?,
        ))
    };
    if w(0)? != MAGIC {
        return None;
    }
    let len = w(4)? as usize;
    let text = core::str::from_utf8(b.get(40..40 + len)?).ok()?;
    let mut e = TextEdit::new(text);
    e.cursor = (w(1)? as usize).min(text.len());
    while !text.is_char_boundary(e.cursor) {
        e.cursor -= 1;
    }
    e.scroll = w(2)? as usize;
    Some((e, w(3)? != 0))
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let docs = store.root(DOCS).expect("documents");
    let apps = store.root(APPS).expect("app files");
    let file = store.open(docs, FILE).expect("the notes file");
    let ret = store.open(apps, RETURN).expect("the return point");
    let back = store.read_all(ret).ok().and_then(|b| decode(&b));
    let (mut edit, mut dirty) = match back {
        Some((e, d)) => {
            let (l, c) = e.position();
            say!(
                "back where you were: line {}, column {}, {} characters{}",
                l + 1,
                c + 1,
                e.text.chars().count(),
                if d { ", not saved yet" } else { "" }
            );
            (e, d)
        }
        None => {
            let saved = store.read_all(file).unwrap_or_default();
            match core::str::from_utf8(&saved) {
                Ok(t) if !t.is_empty() => (TextEdit::new(t), false),
                _ => (TextEdit::new("Welcome to Alaya.\nType here."), true),
            }
        }
    };
    let mut win = Window::open(&mut mem, 560, 360, "Text editor").expect("open a window");
    let mut said = String::new();
    let mut note = String::new();
    loop {
        let mut pending = Vec::new();
        while let Some(e) = win.next_event() {
            pending.push(e);
        }
        let mut changed = false;
        let mut moved = false;
        for e in &pending {
            match e {
                // The text box is the only thing that takes keys, so every
                // key waiting goes to it, in order.
                Event::Key(k) => {
                    let before = edit.cursor;
                    if edit.key(*k, true) {
                        changed = true;
                        dirty = true;
                        note.clear();
                    }
                    moved |= edit.cursor != before;
                }
                Event::Shortcut('s') => {
                    let ok =
                        store.write_all(file, edit.text.as_bytes()).is_ok() && store.save().is_ok();
                    note = String::from(if ok { "Saved." } else { "Could not save." });
                    if ok {
                        dirty = false;
                        changed = true;
                        say!("saved {} ({} bytes)", FILE, edit.text.len());
                    }
                }
                Event::Close => {
                    let _ = store.write_all(ret, &encode(&edit, dirty));
                    crate::closed(&win);
                }
                _ => {}
            }
        }
        if changed || moved {
            // Where you are, after every change (the store keeps it safe).
            let _ = store.write_all(ret, &encode(&edit, dirty));
        }
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.text_box(Rect::new(12, 12, w - 24, h - 56), FILE, &mut edit, true);
            let (line, col) = edit.position();
            let status = format!(
                "{}    Line {}, column {}    {}",
                FILE,
                line + 1,
                col + 1,
                if dirty {
                    "Not saved (Ctrl+S saves). Kept safe anyway."
                } else if note.is_empty() {
                    "Saved."
                } else {
                    &note
                }
            );
            ui.text(12, h - 34, Face::Text, 13, &status, theme::MUTED);
            ui.finish();
            win.pointer.released = false;
        }
        win.damage_all();
        if edit.text != said {
            said = edit.text.clone();
            let shown: String = said
                .chars()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            say!("text ends with: {}", shown.replace('\n', "/"));
        }
        win.wait();
    }
}

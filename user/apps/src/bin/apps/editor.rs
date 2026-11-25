// SPDX-License-Identifier: GPL-3.0-only
//! The text editor: the text files in the person's documents, one at a
//! time (`Notes.txt` first).
//!
//! Ctrl+S saves the file; Ctrl+O asks Alaya for the Open window, and
//! Ctrl+N for the Save window, to keep a copy under a new name. The
//! editor sees only the documents the person hands it there, and the ones
//! it made itself (milestone 7, step 2). Besides that, after every change
//! the editor
//! writes down where you are (its return point, `continuity.md`): the
//! text as it is now, saved or not, the cursor and the first line shown.
//! The store makes that safe within five seconds (decision 100), so if the
//! editor is closed, crashes or is stopped, it opens again exactly where
//! you were.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::{Store, Version};
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop::role;
use alaya_programs::files::{APPS, DOCS};
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::Mem;
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{TextEdit, Ui, theme};

const FIRST: &str = "Notes.txt";
const RETURN: &str = "editor-return";
/// "ALYARET1": milestone 6b's return point, always `Notes.txt`.
const MAGIC1: u64 = 0x414c_5941_5245_5431;
/// "ALYARET2": with the file's name.
const MAGIC2: u64 = 0x414c_5941_5245_5432;

/// The return point: cursor, first line shown, whether the text differs
/// from the file, the text's length, the file's name, then the text.
fn encode(name: &str, e: &TextEdit, dirty: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(48 + name.len() + e.text.len());
    for w in [
        MAGIC2,
        e.cursor as u64,
        e.scroll as u64,
        u64::from(dirty),
        e.text.len() as u64,
        name.len() as u64,
    ] {
        b.extend_from_slice(&w.to_le_bytes());
    }
    b.extend_from_slice(name.as_bytes());
    b.extend_from_slice(e.text.as_bytes());
    b
}

fn decode(b: &[u8]) -> Option<(String, TextEdit, bool)> {
    let w = |i: usize| -> Option<u64> {
        Some(u64::from_le_bytes(
            b.get(8 * i..8 * i + 8)?.try_into().ok()?,
        ))
    };
    let len = w(4)? as usize;
    let (name, at) = match w(0)? {
        MAGIC1 => (String::from(FIRST), 40),
        MAGIC2 => {
            let n = (w(5)? as usize).min(32);
            (
                String::from(core::str::from_utf8(b.get(48..48 + n)?).ok()?),
                48 + n,
            )
        }
        _ => return None,
    };
    let text = core::str::from_utf8(b.get(at..at + len)?).ok()?;
    let mut e = TextEdit::new(text);
    e.cursor = (w(1)? as usize).min(text.len());
    while !text.is_char_boundary(e.cursor) {
        e.cursor -= 1;
    }
    e.scroll = w(2)? as usize;
    Some((name, e, w(3)? != 0))
}

/// The name of a document handed to the editor: it sees only those.
fn name_of(store: &Store, docs: u64, f: u64) -> Option<String> {
    store
        .list(docs)
        .ok()?
        .into_iter()
        .find(|e| e.file == f)
        .map(|e| e.name)
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let docs = store.root(DOCS).expect("documents");
    let apps = store.root(APPS).expect("app files");
    let ret = store.open(apps, RETURN).expect("the return point");
    let back = store.read_all(ret).ok().and_then(|b| decode(&b));
    let mut name = back
        .as_ref()
        .map_or(String::from(FIRST), |(n, _, _)| n.clone());
    // The document it had open, if it still may; else its own first one.
    // Text that belongs to no document it may change (an older version
    // a search found) stays without one: Ctrl+S asks for a name.
    let mut file = store.lookup(docs, &name).ok().or_else(|| {
        if back.as_ref().is_some_and(|b| b.2) {
            return None;
        }
        name = String::from(FIRST);
        store.open(docs, FIRST).ok()
    });
    let (mut edit, mut dirty) = match back {
        Some((_, e, d)) => {
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
            let saved = file
                .and_then(|f| store.read_all(f).ok())
                .unwrap_or_default();
            match core::str::from_utf8(&saved) {
                Ok(t) if !t.is_empty() => (TextEdit::new(t), false),
                _ => (TextEdit::new("Welcome to Alaya.\nType here."), true),
            }
        }
    };
    let mut win = Window::open(&mut mem, 560, 360, "Text editor").expect("open a window");
    let mut said = String::new();
    let mut note = String::new();
    // What was pasted into this document, from which exact versions: a
    // save notes them, so the document carries their mark.
    let mut used: Vec<Version> = Vec::new();
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
                Event::Shortcut('s') if file.is_none() => {
                    if win.pick(true, true) {
                        note = String::from("Choose a name in the Save window.");
                    }
                }
                Event::Shortcut('c') => {
                    // This line: from the saved document when it is saved
                    // (the clipboard then knows its version).
                    let t = edit.text.as_bytes();
                    let from = t[..edit.cursor]
                        .iter()
                        .rposition(|&b| b == b'\n')
                        .map_or(0, |i| i + 1);
                    let to = t[edit.cursor..]
                        .iter()
                        .position(|&b| b == b'\n')
                        .map_or(t.len(), |i| edit.cursor + i);
                    let ok = match file {
                        Some(f) if !dirty => store.copy(f, from as u64, (to - from) as u64).is_ok(),
                        _ => store.copy_text(&t[from..to]).is_ok(),
                    };
                    note = String::from(if ok {
                        "Copied this line."
                    } else {
                        "Could not copy."
                    });
                    changed = true;
                    say!(
                        "copied line {} ({} bytes)",
                        edit.position().0 + 1,
                        to - from
                    );
                }
                Event::Shortcut(v @ ('v' | 'V')) => {
                    // Ctrl+V pastes a reference: the document notes the
                    // exact version it came from, and "needs a look" when
                    // that changes. Ctrl+Shift+V pastes a plain copy.
                    let copy = *v == 'V';
                    if let Ok((data, src)) = store.paste()
                        && !data.is_empty()
                    {
                        let src = src.filter(|_| !copy);
                        let text = String::from_utf8_lossy(&data).into_owned();
                        edit.text.insert_str(edit.cursor, &text);
                        edit.cursor += text.len();
                        dirty = true;
                        changed = true;
                        if let Some(v) = src
                            && !used.contains(&v)
                        {
                            used.push(v);
                        }
                        note = String::from(match (src.is_some(), copy) {
                            (true, _) => {
                                "Pasted with a link to where it came from. \
                                 Ctrl+Shift+V pastes a plain copy."
                            }
                            (false, true) => "Pasted a plain copy: it won't follow its source.",
                            _ => "Pasted.",
                        });
                        say!(
                            "pasted {} bytes{}",
                            data.len(),
                            match (src.is_some(), copy) {
                                (true, _) => " from a saved document, as a reference",
                                (false, true) => " as a plain copy",
                                _ => "",
                            }
                        );
                    }
                }
                Event::Shortcut('s') => {
                    let ok = file.is_some_and(|f| {
                        store.write_all(f, edit.text.as_bytes()).is_ok()
                            && (used.is_empty() || store.made_from(f, &used).is_ok())
                            && store.save().is_ok()
                    });
                    note = String::from(if ok { "Saved." } else { "Could not save." });
                    if ok {
                        dirty = false;
                        changed = true;
                        say!("saved {} ({} bytes)", name, edit.text.len());
                    }
                }
                // Live handoff: go on in the chart maker at this line.
                Event::Shortcut('h') if dirty => {
                    note = String::from("Save first (Ctrl+S), then Ctrl+H.");
                }
                Event::Shortcut('h') => {
                    let line = edit.text.as_bytes()[..edit.cursor]
                        .iter()
                        .filter(|&&b| b == b'\n')
                        .count() as u64;
                    note = match file {
                        Some(f) if win.hand_off(role::CHART, f, line) => {
                            say!(
                                "asked to continue {} in the chart maker at line {}",
                                name,
                                line + 1
                            );
                            String::from("Alaya asks you first.")
                        }
                        _ => String::from("This document cannot be handed over."),
                    };
                }
                Event::HandedOff { .. } if dirty => {
                    note = String::from("Save first: a document was handed to you.");
                }
                Event::HandedOff { file: f, place, .. } => {
                    // Go on where the other app was.
                    if let Ok(b) = store.read_all(*f) {
                        edit = TextEdit::new(&String::from_utf8_lossy(&b));
                        let t = edit.text.as_bytes();
                        edit.cursor = t
                            .iter()
                            .enumerate()
                            .filter(|(_, b)| **b == b'\n')
                            .nth((*place as usize).wrapping_sub(1))
                            .map_or(0, |(i, _)| i + 1)
                            .min(t.len());
                        if *place == 0 {
                            edit.cursor = 0;
                        }
                        file = Some(*f);
                        name = name_of(&store, docs, *f).unwrap_or_default();
                        used.clear();
                        changed = true;
                        note = format!("Handed over: line {}.", place + 1);
                        say!("continues {} at line {}", name, place + 1);
                    }
                }
                Event::Shortcut('o') if dirty => {
                    note = String::from("Save first (Ctrl+S), then Ctrl+O.");
                }
                Event::Shortcut('o') => {
                    if win.pick(false, true) {
                        note = String::from("Pick a document in the Open window.");
                    }
                }
                Event::Shortcut('n') => {
                    if win.pick(true, true) {
                        note = String::from("Choose a name in the Save window.");
                    }
                }
                Event::Picked { file: None, .. } => {
                    note = String::from("Nothing was handed over.");
                    changed = true;
                }
                Event::Picked { save: false, .. } if dirty => {
                    note = String::from("Save first (Ctrl+S), then open it again.");
                    changed = true;
                }
                Event::Picked {
                    file: Some(f),
                    save: false,
                    pinned: true,
                } => {
                    // An older version, found by search: read only. Ctrl+S
                    // keeps it as a new document.
                    if let Ok(b) = store.read_all(*f) {
                        edit = TextEdit::new(&String::from_utf8_lossy(&b));
                        let was = name_of(&store, docs, *f).unwrap_or(String::from("a document"));
                        name = format!("Old {was}");
                        file = None;
                        note = format!("As {was} was then. Ctrl+S keeps it under a new name.");
                        changed = true;
                        dirty = true;
                        say!("opened an older version of {}", was);
                    }
                }
                Event::Picked {
                    file: Some(f),
                    save: false,
                    ..
                } => {
                    if let Ok(b) = store.read_all(*f) {
                        edit = TextEdit::new(&String::from_utf8_lossy(&b));
                        file = Some(*f);
                        name = name_of(&store, docs, *f).unwrap_or_default();
                        used.clear();
                        note.clear();
                        changed = true;
                        dirty = false;
                        say!("opened {}", name);
                    }
                }
                Event::Picked {
                    file: Some(f),
                    save: true,
                    ..
                } => {
                    let ok = store.write_all(*f, edit.text.as_bytes()).is_ok()
                        && (used.is_empty() || store.made_from(*f, &used).is_ok())
                        && store.save().is_ok();
                    if ok {
                        file = Some(*f);
                        name = name_of(&store, docs, *f).unwrap_or_default();
                        dirty = false;
                        changed = true;
                        note = String::from("Saved.");
                        say!("saved {} ({} bytes)", name, edit.text.len());
                    } else {
                        note = String::from("Could not save.");
                    }
                }
                Event::Close => {
                    let _ = store.write_all(ret, &encode(&name, &edit, dirty));
                    crate::closed(&win);
                }
                _ => {}
            }
        }
        if changed || moved {
            // Where you are, after every change (the store keeps it safe).
            let _ = store.write_all(ret, &encode(&name, &edit, dirty));
        }
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.text_box(Rect::new(12, 12, w - 24, h - 56), &name, &mut edit, true);
            let (line, col) = edit.position();
            let status = format!(
                "{}    Line {}, column {}    {}",
                name,
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
            win.tell(&ui.tree);
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

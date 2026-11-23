// SPDX-License-Identifier: GPL-3.0-only
//! The file browser: the person's documents, with their sizes and how
//! many versions the store keeps. It looks again about every two seconds.
//! The living-validity mark on each file comes with milestone 6c.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::{Entry, Store};
use alaya_apps::{self as app, Event, Window};
use alaya_programs::files::DOCS;
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::{self, Mem};
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Key, Ui, theme};

fn size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} bytes"),
        1024..1048576 => format!("{} KiB", n.div_ceil(1024)),
        _ => format!("{} MiB", n.div_ceil(1 << 20)),
    }
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let docs = store.root(DOCS).expect("documents");
    let mut win = Window::open(&mut mem, 420, 320, "Files").expect("open a window");
    let mut entries: Vec<Entry> = Vec::new();
    let mut selected: Option<usize> = None;
    let mut last_look = 0;
    let mut said = String::new();
    loop {
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(k) => keys.push(k),
                Event::Close => crate::closed(&win),
                _ => {}
            }
        }
        let now = rt::now_ms();
        if last_look == 0 || now - last_look >= 2000 || keys.contains(&Key::Char('r')) {
            last_look = now;
            if let Ok(mut l) = store.list(docs) {
                l.sort_by(|a, b| a.name.cmp(&b.name));
                entries = l;
            }
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            let line = names.join(", ");
            if line != said {
                say!(
                    "documents: {}",
                    if line.is_empty() { "none" } else { &line }
                );
                said = line;
            }
        }
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.text(14, 12, Face::Strong, 14, "Documents", theme::INK);
            let rows: Vec<String> = entries
                .iter()
                .map(|e| {
                    if e.is_dir {
                        format!("{}  (folder)", e.name)
                    } else {
                        format!("{}  ·  {}  ·  version {}", e.name, size(e.size), e.version)
                    }
                })
                .collect();
            let rows: Vec<&str> = rows.iter().map(|s| s.as_str()).collect();
            ui.key = keys.first().copied();
            if let Some(i) = ui.list(
                Rect::new(12, 40, w - 24, h - 80),
                "Documents",
                &rows,
                selected,
                &[],
            ) {
                selected = Some(i);
            }
            let hint = if entries.is_empty() {
                "No documents yet. Save something in the text editor."
            } else {
                "Up and down choose. Marks that say if a file is up to date come next."
            };
            ui.text(14, h - 30, Face::Text, 12, hint, theme::MUTED);
            ui.finish();
            win.pointer.released = false;
        }
        win.damage_all();
        win.wait();
    }
}

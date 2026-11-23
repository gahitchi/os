// SPDX-License-Identifier: GPL-3.0-only
//! The file browser: the person's documents, with their sizes and the
//! living-validity mark on each result (milestone 6c, `commitments.md`):
//! "Up to date", "Needs a look", "As of a date" or "Not sure". Files made
//! from nothing (a note you typed) carry no mark. It looks again about
//! every two seconds.
//!
//! Up and down choose a file; A says the chosen result shows a fixed
//! moment ("as of a date"), or not any more.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::{Entry, Kind, Mark, Store, Why};
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop::role;
use alaya_programs::files::{self as fs, DOCS};
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::{self, Mem};
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::canvas::Color;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Key, Ui, theme};

fn size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} bytes"),
        1024..1048576 => format!("{} KiB", n.div_ceil(1024)),
        _ => format!("{} MiB", n.div_ceil(1 << 20)),
    }
}

/// The name people know an app by.
fn app_name(app: u64) -> &'static str {
    match app {
        a if a == fs::app_id(role::EDITOR) => "the text editor",
        a if a == fs::app_id(role::FILES) => "the file browser",
        a if a == fs::app_id(role::CHART) => "the chart maker",
        a if a == fs::app_id(role::SETTINGS) => "Settings",
        _ => "another app",
    }
}

/// The mark's words and colours.
fn pill(m: &Mark) -> Option<(&'static str, Color, Color)> {
    match m.kind {
        Kind::NoMark => None,
        Kind::UpToDate => Some(("Up to date", theme::OK, theme::OK_BG)),
        Kind::NeedsALook => Some(("Needs a look", theme::WARN, theme::WARN_BG)),
        Kind::AsOf => Some(("As of a date", theme::PAST, theme::PAST_BG)),
        Kind::NotSure => Some(("Not sure", theme::UNKNOWN, theme::UNKNOWN_BG)),
    }
}

/// One file as shown.
struct Row {
    e: Entry,
    mark: Mark,
    /// The longer story of the mark, for the chosen file.
    about: String,
}

/// What the mark means here, in a sentence.
fn about(store: &Store, rows: &[Entry], m: &Mark) -> String {
    let src = rows
        .iter()
        .find(|e| e.file == m.source)
        .map_or(String::from("a file it was made from"), |e| e.name.clone());
    let made = format!(
        "Made by {} from {} file{}.",
        app_name(m.maker),
        m.sources,
        if m.sources == 1 { "" } else { "s" }
    );
    match (m.kind, m.why) {
        (Kind::NoMark, _) => String::from("Not made from other files."),
        (Kind::UpToDate, _) => format!("{made} Nothing it was made from has changed."),
        (Kind::AsOf, _) => format!("{made} You said it shows a fixed moment (A to undo)."),
        (Kind::NeedsALook, _) => {
            // Who changed the source: the store's record of that commit
            // (not known yet while the change waits to be saved).
            let by = store
                .changes(m.now_version)
                .ok()
                .and_then(|c| c.iter().find(|x| x.1 == m.source).map(|x| x.0))
                .map_or(String::new(), |a| format!(" by {}", app_name(a)));
            format!("{made} Since then, {src} was changed{by}.")
        }
        (Kind::NotSure, Why::NotAllRead) => {
            format!("{made} It did not read all of {src}, so Alaya can't vouch for it.")
        }
        (Kind::NotSure, Why::SourceGone) => format!("{made} {src} is gone."),
        (Kind::NotSure, _) => format!("{made} It was changed after it was made."),
    }
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let docs = store.root(DOCS).expect("documents");
    let mut win = Window::open(&mut mem, 460, 340, "Files").expect("open a window");
    let mut rows: Vec<Row> = Vec::new();
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
        let mut look = last_look == 0 || rt::now_ms() - last_look >= 2000;
        for k in &keys {
            let last = rows.len().saturating_sub(1);
            match k {
                Key::Down => selected = Some(selected.map_or(0, |s| (s + 1).min(last))),
                Key::Up => selected = Some(selected.map_or(0, |s| s.saturating_sub(1))),
                Key::Char('r') => look = true,
                Key::Char('a') => {
                    if let Some(r) = selected.and_then(|i| rows.get(i))
                        && r.mark.kind != Kind::NoMark
                    {
                        let on = r.mark.kind != Kind::AsOf;
                        if store.as_of(r.e.file, on).is_ok() {
                            say!(
                                "{} is {}",
                                r.e.name,
                                if on {
                                    "as of a date now"
                                } else {
                                    "followed again"
                                }
                            );
                        }
                        look = true;
                    }
                }
                _ => {}
            }
        }
        if look {
            last_look = rt::now_ms();
            if let Ok(mut l) = store.list(docs) {
                l.sort_by(|a, b| a.name.cmp(&b.name));
                let marks: Vec<Mark> = l
                    .iter()
                    .map(|e| {
                        store.mark(e.file).unwrap_or(Mark {
                            kind: Kind::NoMark,
                            why: Why::Nothing,
                            source: 0,
                            now_version: 0,
                            maker: 0,
                            sources: 0,
                        })
                    })
                    .collect();
                let abouts: Vec<String> = marks.iter().map(|m| about(&store, &l, m)).collect();
                rows = l
                    .into_iter()
                    .zip(marks)
                    .zip(abouts)
                    .map(|((e, mark), about)| Row { e, mark, about })
                    .collect();
            }
            let line = rows
                .iter()
                .map(|r| match pill(&r.mark) {
                    Some((t, _, _)) => format!("{} ({})", r.e.name, t.to_lowercase()),
                    None => r.e.name.clone(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            if line != said {
                say!(
                    "documents: {}",
                    if line.is_empty() { "none" } else { &line }
                );
                said = line;
            }
        }
        let hint = match selected.and_then(|i| rows.get(i)) {
            _ if rows.is_empty() => {
                String::from("No documents yet. Save something in the text editor.")
            }
            Some(r) => r.about.clone(),
            None => String::from("Up and down choose a file. A marks a result \"as of a date\"."),
        };
        let lines = fonts.wrap(Face::Text, 12, &hint, win.width - 28);
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.text(14, 12, Face::Strong, 14, "Documents", theme::INK);
            let texts: Vec<String> = rows
                .iter()
                .map(|r| {
                    if r.e.is_dir {
                        format!("{}  (folder)", r.e.name)
                    } else {
                        format!("{}  ·  {}", r.e.name, size(r.e.size))
                    }
                })
                .collect();
            let texts: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let pills: Vec<_> = rows.iter().map(|r| pill(&r.mark)).collect();
            if let Some(i) = ui.list(
                Rect::new(12, 40, w - 24, h - 100),
                "Documents",
                &texts,
                selected,
                &pills,
            ) {
                selected = Some(i);
            }
            for (i, (a, b)) in lines.iter().take(3).enumerate() {
                let y = h - 54 + 16 * i as i32;
                ui.text(14, y, Face::Text, 12, hint[*a..*b].trim_end(), theme::MUTED);
            }
            ui.finish();
            win.pointer.released = false;
        }
        win.damage_all();
        win.wait();
    }
}

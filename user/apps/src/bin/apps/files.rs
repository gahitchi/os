// SPDX-License-Identifier: GPL-3.0-only
//! The file browser: the person's documents, with their sizes and the
//! living-validity mark on each result (milestone 6c, `commitments.md`):
//! "Up to date", "Needs a look", "As of a date" or "Not sure". Files made
//! from nothing (a note you typed) carry no mark. It looks again about
//! every two seconds.
//!
//! Up and down choose a file; A says the chosen result shows a fixed
//! moment ("as of a date"), or not any more. Delete (or D) deletes it, as
//! one action on the undo list (milestone 7); W, or the button, tells its
//! story: where it came from and what was made from it. F, pressed twice,
//! forgets it for good after an hour (decision 62); until then K keeps it.
//! P marks it private, or not (decision 27): an app that reads a private
//! document is marked, and needs your OK before it goes online. On a
//! result that needs a look, S shows the difference (what changed in its
//! source since) and M asks the app that made it to make it again.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::{Entry, Kind, Mark, Store, Why, app_name, label, make_label};
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop as dk;
use alaya_programs::files::{DOCS, FORGET_GRACE, act, what};
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::rt::{self, Mem};
use alaya_sys::say;
use alaya_sys::{Caps, Msg};
use alaya_ui::Rect;
use alaya_ui::canvas::Color;
use alaya_ui::diff::{Line, diff_lines};
use alaya_ui::text::Face;
use alaya_ui::widgets::{Key, Ui, theme};

fn size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} bytes"),
        1024..1048576 => format!("{} KiB", n.div_ceil(1024)),
        _ => format!("{} MiB", n.div_ceil(1 << 20)),
    }
}

/// A file's story in a few sentences (rule R2: from file identities and
/// the record of changes only).
fn story(store: &Store, rows: &[Entry], e: &Entry) -> String {
    let Ok(st) = store.story(e.file, 64) else {
        return String::from("Its story could not be read.");
    };
    let mut out = match st.made_by {
        0 => String::from("Made before the oldest moment Alaya still keeps."),
        app => format!("Made by {}.", app_name(app)),
    };
    let edits = st.changes.iter().filter(|c| c.3 & what::WROTE != 0).count();
    let mut by: Vec<&str> = st.changes.iter().map(|c| app_name(c.2)).collect();
    by.sort();
    by.dedup();
    if edits > 0 {
        out.push_str(&format!(
            " Changed {edits} time{} by {}.",
            if edits == 1 { "" } else { "s" },
            by.join(" and ")
        ));
    }
    if st.changes.iter().any(|c| c.3 & what::RESTORED != 0) {
        out.push_str(" Put back once from an earlier moment.");
    }
    if !st.used_by.is_empty() {
        let names: Vec<String> = st
            .used_by
            .iter()
            .map(|f| {
                rows.iter()
                    .find(|r| r.file == *f)
                    .map_or(String::from("a file in a folder"), |r| r.name.clone())
            })
            .collect();
        out.push_str(&format!(" Made from it: {}.", names.join(", ")));
    }
    out
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
    /// Marked private (P).
    private: bool,
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

/// "Show the difference": the source's lines that changed since the
/// result was made from it.
fn difference(store: &Store, rows: &[Row], result: u64) -> String {
    let Ok(Some(d)) = store.what_changed(result) else {
        return String::from("Alaya no longer keeps the version it was made from.");
    };
    let src = rows
        .iter()
        .find(|r| r.e.file == d.source)
        .map_or(String::from("its source"), |r| r.e.name.clone());
    let mut parts = Vec::new();
    for l in diff_lines(&d.then, &d.now, 6) {
        match l {
            Line::Out(t) => parts.push(format!("was \"{}\"", String::from_utf8_lossy(t))),
            Line::In(t) => parts.push(format!("now \"{}\"", String::from_utf8_lossy(t))),
            Line::Same(_) => {}
        }
    }
    format!(
        "In {src}{}: {}. M makes it again.",
        if d.exact {
            ""
        } else {
            " (compared with the nearest kept moment)"
        },
        if parts.is_empty() {
            String::from("only spaces or line ends changed")
        } else {
            parts.join(", ")
        }
    )
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let docs = store.root(DOCS).expect("documents");
    let mut win = Window::open(&mut mem, 460, 340, "Files").expect("open a window");
    let mut rows: Vec<Row> = Vec::new();
    let mut selected: Option<usize> = None;
    let mut last_look = 0;
    let mut said = String::new();
    // The story shown for the chosen file (W), until another is chosen.
    let mut told: Option<(u64, String)> = None;
    let mut ask_story = false;
    let mut delete = false;
    // F pressed once on this file: press again to forget it.
    let mut forget_asked: Option<u64> = None;
    // F presses in this round: two in a row forget, even when both come
    // at once.
    let mut forget = 0;
    let mut keep = false;
    let mut waiting: Vec<(u64, u64, String)> = Vec::new();
    loop {
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(k) => keys.push(k),
                Event::Close => crate::closed(&win),
                // Brought to the front: look again at once, so the rows
                // are today's before any key moves among them.
                Event::Focus(true) => last_look = 0,
                _ => {}
            }
        }
        let mut look = last_look == 0 || rt::now_ms() - last_look >= 2000;
        for k in &keys {
            let last = rows.len().saturating_sub(1);
            match k {
                Key::Down => selected = Some(selected.map_or(0, |s| (s + 1).min(last))),
                Key::Up => selected = Some(selected.map_or(0, |s| s.saturating_sub(1))),
                Key::Home => selected = Some(0),
                Key::End => selected = Some(last),
                Key::Char('r') => look = true,
                Key::Char('w') => ask_story = true,
                Key::Delete | Key::Char('d') => delete = true,
                Key::Char('f') => forget += 1,
                Key::Char('k') => keep = true,
                // Show the difference: what changed in the source since
                // the chosen result was made.
                Key::Char('s') => {
                    if let Some(r) = selected.and_then(|i| rows.get(i))
                        && r.mark.kind == Kind::NeedsALook
                    {
                        let text = difference(&store, &rows, r.e.file);
                        say!("difference for {}: {}", r.e.name, text);
                        told = Some((r.e.file, text));
                    }
                }
                // Make again, by the app that made it, from the sources now.
                Key::Char('m') => {
                    if let Some(r) = selected.and_then(|i| rows.get(i))
                        && r.mark.kind != Kind::NoMark
                    {
                        let ok = alaya_sys::ipc_call(
                            start::FIRST_GIVEN + dk::APP_G_DESKTOP,
                            &Msg::new(dk::MAKE_AGAIN, &[r.e.file]),
                            Caps::default(),
                        )
                        .is_ok_and(|m| m.words[0] == dk::OK);
                        say!(
                            "make {} again: {}",
                            r.e.name,
                            if ok { "asked" } else { "cannot" }
                        );
                        look = true;
                    }
                }
                // Put it in a group (milestone 7a): Alaya asks which.
                Key::Char('g') => {
                    if let Some(r) = selected.and_then(|i| rows.get(i)) {
                        let ok = alaya_sys::ipc_call(
                            start::FIRST_GIVEN + dk::APP_G_DESKTOP,
                            &Msg::new(dk::GROUP_FILE, &[r.e.file]),
                            Caps::default(),
                        )
                        .is_ok_and(|m| m.words[0] == dk::OK);
                        say!(
                            "put {} in a group: {}",
                            r.e.name,
                            if ok { "asked" } else { "cannot" }
                        );
                    }
                }
                // An app bundle (milestone 8): Alaya's install sheet, where
                // only the person can go on.
                Key::Enter => {
                    if let Some(r) = selected.and_then(|i| rows.get(i))
                        && r.e.name.ends_with(".alaya")
                    {
                        let ok = alaya_sys::ipc_call(
                            start::FIRST_GIVEN + dk::APP_G_DESKTOP,
                            &Msg::new(dk::OFFER_INSTALL, &[r.e.file]),
                            Caps::default(),
                        )
                        .is_ok_and(|m| m.words[0] == dk::OK);
                        say!(
                            "install {}: {}",
                            r.e.name,
                            if ok { "offered" } else { "cannot" }
                        );
                    }
                }
                Key::Char('p') => {
                    if let Some(r) = selected.and_then(|i| rows.get(i))
                        && !r.e.is_dir
                    {
                        let on = !r.private;
                        if store.private(r.e.file, Some(on)).is_ok() {
                            say!(
                                "{} is {}",
                                r.e.name,
                                if on {
                                    "private now"
                                } else {
                                    "not private any more"
                                }
                            );
                        }
                        look = true;
                    }
                }
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
        if delete {
            delete = false;
            if let Some(r) = selected.and_then(|i| rows.get(i))
                && !r.e.is_dir
            {
                // One action on the undo list: the one undo brings it back.
                let name = r.e.name.clone();
                let ok = store.action_begin().is_ok()
                    && store.remove(docs, &name).is_ok()
                    && store
                        .record_action(act::FILES, make_label(label::DELETED, &name).as_bytes())
                        .is_ok();
                say!(
                    "{}",
                    if ok {
                        format!("deleted {name}; the one undo brings it back")
                    } else {
                        format!("could not delete {name}")
                    }
                );
                look = true;
            }
        }
        for _ in 0..core::mem::take(&mut forget) {
            if let Some(r) = selected.and_then(|i| rows.get(i))
                && !r.e.is_dir
            {
                if forget_asked == Some(r.e.file) {
                    forget_asked = None;
                    let name = r.e.name.clone();
                    match store.forget_later(docs, &name, FORGET_GRACE) {
                        Ok(_) => say!(
                            "forgetting {name}: its key is destroyed in an hour unless you keep it (K)"
                        ),
                        Err(e) => say!("could not forget {name}: {e}"),
                    }
                    look = true;
                } else {
                    forget_asked = Some(r.e.file);
                    told = Some((
                        r.e.file,
                        format!(
                            "Press F again to forget {} for good: no version of it can be read again, by anyone. You have an hour to change your mind.",
                            r.e.name
                        ),
                    ));
                }
            }
        }
        if keep {
            keep = false;
            if let Some((f, _, name)) = waiting.first().cloned() {
                match store.keep_forgotten(f) {
                    Ok(()) => say!("kept {name}: it is back where it was"),
                    Err(e) => say!("could not keep {name}: {e}"),
                }
                look = true;
            }
        }
        if ask_story {
            ask_story = false;
            if let Some(r) = selected.and_then(|i| rows.get(i)) {
                let es: Vec<Entry> = rows.iter().map(|r| r.e.clone()).collect();
                let t = story(&store, &es, &r.e);
                say!("story of {}: {t}", r.e.name);
                told = Some((r.e.file, t));
            }
        }
        if look {
            last_look = rt::now_ms();
            waiting = store.forgetting().unwrap_or_default();
            // The chosen file stays chosen when others come and go.
            let chosen = selected.and_then(|i| rows.get(i)).map(|r| r.e.file);
            if let Ok(mut l) = store.list(docs) {
                l.sort_by(|a, b| a.name.cmp(&b.name));
                if let Some(f) = chosen {
                    selected = l.iter().position(|e| e.file == f).or(selected);
                }
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
                    .map(|((e, mark), about)| Row {
                        private: !e.is_dir && store.private(e.file, None).unwrap_or(false),
                        e,
                        mark,
                        about,
                    })
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
            _ if !waiting.is_empty() && told.is_none() => {
                let (_, left, name) = &waiting[0];
                format!(
                    "{name} will be forgotten for good in {} min. K keeps it.",
                    left.div_ceil(60)
                )
            }
            _ if rows.is_empty() => {
                String::from("No documents yet. Save something in the text editor.")
            }
            Some(r) => match &told {
                Some((f, t)) if *f == r.e.file => t.clone(),
                _ => r.about.clone(),
            },
            None => String::from(
                "Up and down choose a file. A marks a result \"as of a date\"; \
                 G puts it in a group. Delete deletes it; Alaya key + Z brings it back.",
            ),
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
                        format!(
                            "{}  ·  {}{}",
                            r.e.name,
                            size(r.e.size),
                            if r.private { "  ·  private" } else { "" }
                        )
                    }
                })
                .collect();
            let texts: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
            let pills: Vec<_> = rows.iter().map(|r| pill(&r.mark)).collect();
            if let Some(i) = ui.list(
                Rect::new(12, 40, w - 24, h - 144),
                "Documents",
                &texts,
                selected,
                &pills,
            ) {
                selected = Some(i);
            }
            if selected.is_some() {
                let (story_hit, r) = ui.button(12, h - 96, "Where did this come from?", false);
                ask_story |= story_hit;
                let (del_hit, _) = ui.button(r.right() + 8, h - 96, "Delete", false);
                delete |= del_hit;
            }
            for (i, (a, b)) in lines.iter().take(3).enumerate() {
                let y = h - 54 + 16 * i as i32;
                ui.text(14, y, Face::Text, 12, hint[*a..*b].trim_end(), theme::MUTED);
            }
            ui.finish();
            win.tell(&ui.tree);
            win.pointer.released = false;
        }
        win.damage_all();
        // A button pressed: act on it at once.
        if !(ask_story || delete) {
            win.wait();
        }
    }
}

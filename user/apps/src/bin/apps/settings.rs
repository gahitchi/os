// SPDX-License-Identifier: GPL-3.0-only
//! Settings, first version: the look (decision 90), which first setup
//! asked for. It is kept in the person's own app files (`look`), and the
//! desktop reads it again when Settings says it changed.
//!
//! And what each app can see of your documents (milestone 7, step 2):
//! every hand-over, how long it lasts, and "Take it back". Up and Down
//! choose one, T takes it back.
//!
//! And the backup stick (decision 36): B backs up now (then by itself
//! every ten minutes when something changed), R shows the recovery key,
//! G gets documents back from the stick (type the recovery key, Enter).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::{GrantRow, Store, app_name, label};
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop as dk;
use alaya_programs::files::{APPS, act};
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::rt::Mem;
use alaya_sys::{Caps, Msg, say};
use alaya_ui::Rect;
use alaya_ui::login::Look;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Key, Ui, theme};

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let apps = store.root(APPS).expect("app files");
    let file = store.open(apps, "look").expect("the look");
    let mut look = Look::from_code(
        store
            .read_all(file)
            .ok()
            .and_then(|b| b.first().copied())
            .unwrap_or(0),
    );
    let mut win = Window::open(&mut mem, 560, 600, "Settings").expect("open a window");
    let mut focus = 0;
    // The backup part: what it says, the recovery key while shown, and
    // the key being typed to get documents back.
    let mut backup_note = String::new();
    let mut shown_key: Option<String> = None;
    let mut typing: Option<String> = None;
    let mut chosen_grant = 0usize;
    let mut said = usize::MAX;
    loop {
        // An undo may have put the old look back.
        if let Some(&c) = store.read_all(file).ok().as_deref().and_then(|b| b.first()) {
            look = Look::from_code(c);
        }
        let grants: Vec<GrantRow> = store.grants(0).unwrap_or_default();
        // Apps that read a private document (decision 27).
        let marked = {
            let m = store.marks().unwrap_or_default();
            let mut names: Vec<&str> = m.iter().map(|x| app_name(x.0)).collect();
            names.dedup();
            (!names.is_empty()).then(|| {
                format!(
                    "Read a private file: {}. They need your OK to go online.",
                    names.join(", ")
                )
            })
        };
        let rows: Vec<String> = grants.iter().map(|g| row(&store, g)).collect();
        if rows.len() != said {
            said = rows.len();
            say!("apps can see {} documents of yours", rows.len());
        }
        let mut take_back = false;
        let mut back_up = false;
        let mut show_key = false;
        let mut get_back = false;
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            match (e, typing.as_mut()) {
                // Typing the recovery key: every key goes there.
                (Event::Key(Key::Escape), Some(_)) => {
                    typing = None;
                    backup_note.clear();
                }
                (Event::Key(Key::Enter), Some(_)) => get_back = true,
                (Event::Key(Key::Backspace), Some(t)) => {
                    t.pop();
                }
                (Event::Key(Key::Char(c)), Some(t)) => {
                    if t.len() < 48 {
                        t.push(c.to_ascii_uppercase());
                    }
                }
                (Event::Close, _) => crate::closed(&win),
                (_, Some(_)) => {}
                (e, None) => match e {
                    Event::Key(Key::Char('b')) => back_up = true,
                    Event::Key(Key::Char('r')) => show_key = true,
                    Event::Key(Key::Char('g')) => {
                        typing = Some(String::new());
                        backup_note = String::from("Type your recovery key, then Enter.");
                    }
                    Event::Key(Key::Up) => chosen_grant = chosen_grant.saturating_sub(1),
                    Event::Key(Key::Down) => chosen_grant += 1,
                    Event::Key(Key::Char('t')) => take_back = true,
                    Event::Key(k) => keys.push(k),
                    Event::Close => crate::closed(&win),
                    _ => {}
                },
            }
        }
        let mut chosen = None;
        // One drawing pass per key: keys typed quickly arrive together,
        // and each must reach the piece that has the keyboard then.
        for pass in 0..keys.len().max(1) {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.focus = focus;
            ui.key = keys.get(pass).copied();
            ui.text(16, 14, Face::Strong, 15, "The look", theme::INK);
            ui.text(
                16,
                40,
                Face::Text,
                13,
                "What you use Alaya for. Changes the dock at once.",
                theme::MUTED,
            );
            let mut x = 16;
            for l in Look::ALL {
                let (hit, r) = ui.button(x, 72, l.name(), l == look);
                if hit {
                    chosen = Some(l);
                }
                x = r.right() + 8;
            }
            ui.text(16, 120, Face::Text, 12, look.about(), theme::MUTED);
            ui.text(16, 168, Face::Strong, 15, "What apps can see", theme::INK);
            ui.text(
                16,
                192,
                Face::Text,
                12,
                "Only what you handed them in Alaya's Open and Save windows, and what they made.",
                theme::MUTED,
            );
            chosen_grant = chosen_grant.min(rows.len().saturating_sub(1));
            if rows.is_empty() {
                ui.text(16, 222, Face::Text, 13, "Nothing yet.", theme::MUTED);
            } else {
                let texts: Vec<&str> = rows.iter().map(|r| r.as_str()).collect();
                let pills: Vec<_> = grants.iter().map(|_| None).collect();
                if let Some(i) = ui.list(
                    Rect::new(12, 214, w - 24, 150),
                    "What apps can see",
                    &texts,
                    Some(chosen_grant),
                    &pills,
                ) {
                    chosen_grant = i;
                }
                let (hit, _) = ui.button(16, 372, "Take it back", false);
                take_back |= hit;
            }
            if let Some(m) = &marked {
                ui.text(160, 377, Face::Text, 12, m, theme::PAST);
            }
            ui.text(16, 420, Face::Strong, 15, "Backup", theme::INK);
            let status = backup_status(&store);
            ui.text(16, 444, Face::Text, 12, &status, theme::MUTED);
            let (hit, r) = ui.button(16, 466, "Back up now", false);
            back_up |= hit;
            let (hit, r) = ui.button(r.right() + 10, 466, "Recovery key", shown_key.is_some());
            show_key |= hit;
            let (hit, _) = ui.button(r.right() + 10, 466, "Get them back", typing.is_some());
            if hit && typing.is_none() {
                typing = Some(String::new());
                backup_note = String::from("Type your recovery key, then Enter.");
            }
            if let Some(t) = &typing {
                let shown = format!("Recovery key: {t}|");
                ui.text(16, 512, Face::Strong, 13, &shown, theme::INK);
            } else if let Some(k) = &shown_key {
                ui.text(16, 512, Face::Strong, 15, k, theme::INK);
            }
            ui.text(16, 538, Face::Text, 12, &backup_note, theme::PAST);
            focus = ui.finish();
            ui.text(
                16,
                h - 30,
                Face::Text,
                12,
                "Alaya 0.0.1, milestone 7.",
                theme::MUTED,
            );
            win.pointer.released = false;
        }
        if let Some(l) = chosen
            && l != look
        {
            // On the undo list: the one undo puts the old look back.
            let old = look.code();
            look = l;
            let _ = store.action_begin();
            let _ = store.write_all(file, &[l.code()]);
            let _ = store.record_action(act::SETTINGS, &[label::LOOK, old, l.code()]);
            let _ = alaya_sys::ipc_call(
                start::FIRST_GIVEN + dk::APP_G_DESKTOP,
                &Msg::new(dk::LOOK_CHANGED, &[]),
                Caps::default(),
            );
            say!("the look is now: {}", l.name());
        }
        if back_up {
            backup_note = match store.backup(false) {
                Ok([run, written, gone, ..]) => {
                    say!("backup run {run}: {written} changed, {gone} gone");
                    let (key, new) = store.recovery_key().unwrap_or_default();
                    if new || run == 1 {
                        // The first time: the key to write down.
                        shown_key = Some(key);
                        String::from(
                            "Backed up. Write down this recovery key: it is the only way to open the stick on another Alaya.",
                        )
                    } else {
                        format!("Backed up: {written} changed, {gone} gone since last time.")
                    }
                }
                Err(e) if e == alaya_programs::files::st::NOT_FOUND => {
                    String::from("There is no backup stick.")
                }
                Err(e)
                    if e == alaya_programs::files::st::EXISTS
                        || e == alaya_programs::files::st::WRONG_KEY =>
                {
                    String::from("This stick holds another backup. Alaya does not write over it.")
                }
                Err(e) => format!("The backup did not finish (code {e})."),
            };
        }
        if show_key {
            shown_key = match shown_key {
                Some(_) => None,
                None => store.recovery_key().ok().map(|(k, _)| k),
            };
        }
        if get_back && let Some(t) = typing.take() {
            backup_note = match store.restore_backup(&t, "From the stick") {
                Ok([folders, docs, ..]) => {
                    say!("got {docs} documents and {folders} folders back from the stick");
                    format!("Got {docs} documents and {folders} folders back from the stick.")
                }
                Err(e) if e == alaya_programs::files::st::WRONG_KEY => {
                    String::from("That recovery key does not open this stick.")
                }
                Err(e) => format!("Nothing came back (code {e})."),
            };
        }
        if take_back && let Some(g) = grants.get(chosen_grant) {
            let name = store.name_of(g.file, 0).unwrap_or_default();
            if store.revoke(g.app, g.file).unwrap_or(false) {
                let _ = store.save();
                say!("took {} back from {}", name, app_name(g.app));
            }
        }
        win.damage_all();
        win.wait();
    }
}

/// What the backup part says now.
fn backup_status(store: &Store) -> String {
    match store.stick() {
        Ok([0, ..]) => {
            String::from("No backup stick is there. (Real USB sticks come in milestone 11.)")
        }
        Ok([_, blocks, 0, _]) => format!(
            "A stick of {} MiB is there. Nothing backed up yet: B backs up now.",
            blocks / 256
        ),
        Ok([_, blocks, run, _]) => format!(
            "Backed up {run} times to the stick ({} MiB). It backs up by itself every ten minutes.",
            blocks / 256
        ),
        Err(_) => String::new(),
    }
}

/// One hand-over in plain words.
fn row(store: &Store, g: &GrantRow) -> String {
    let name = store
        .name_of(g.file, 0)
        .unwrap_or_else(|_| String::from("(gone)"));
    let how = if g.own { "it made it" } else { g.lasts.words() };
    format!(
        "{}  ·  {}  ·  {}{}",
        capital(app_name(g.app)),
        name,
        how,
        if g.write && !g.own {
            ", may change it"
        } else {
            ""
        }
    )
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

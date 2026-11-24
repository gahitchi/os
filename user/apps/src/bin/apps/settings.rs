// SPDX-License-Identifier: GPL-3.0-only
//! Settings, first version: the look (decision 90), which first setup
//! asked for. It is kept in the person's own app files (`look`), and the
//! desktop reads it again when Settings says it changed.
//!
//! And what each app can see of your documents (milestone 7, step 2):
//! every hand-over, how long it lasts, and "Take it back". Up and Down
//! choose one, T takes it back.

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
    let mut win = Window::open(&mut mem, 560, 440, "Settings").expect("open a window");
    let mut focus = 0;
    let mut chosen_grant = 0usize;
    let mut said = usize::MAX;
    loop {
        // An undo may have put the old look back.
        if let Some(&c) = store.read_all(file).ok().as_deref().and_then(|b| b.first()) {
            look = Look::from_code(c);
        }
        let grants: Vec<GrantRow> = store.grants(0).unwrap_or_default();
        let rows: Vec<String> = grants.iter().map(|g| row(&store, g)).collect();
        if rows.len() != said {
            said = rows.len();
            say!("apps can see {} documents of yours", rows.len());
        }
        let mut take_back = false;
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(Key::Up) => chosen_grant = chosen_grant.saturating_sub(1),
                Event::Key(Key::Down) => chosen_grant += 1,
                Event::Key(Key::Char('t')) => take_back = true,
                Event::Key(k) => keys.push(k),
                Event::Close => crate::closed(&win),
                _ => {}
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
            let mut y = 72;
            for l in [Look::Basics, Look::Everything] {
                let (hit, r) = ui.button(16, y, l.name(), l == look);
                if hit {
                    chosen = Some(l);
                }
                ui.text(
                    r.right() + 12,
                    y + 7,
                    Face::Text,
                    12,
                    l.about(),
                    theme::MUTED,
                );
                y += 44;
            }
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
                    Rect::new(12, 214, w - 24, h - 300),
                    "What apps can see",
                    &texts,
                    Some(chosen_grant),
                    &pills,
                ) {
                    chosen_grant = i;
                }
                let (hit, _) = ui.button(16, h - 76, "Take it back", false);
                take_back |= hit;
            }
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

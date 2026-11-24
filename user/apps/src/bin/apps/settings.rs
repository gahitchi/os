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
//!
//! And how big things are on the screen this window is on (milestone
//! 7b): Alaya's pick from the screen's size, or 100 to 200 percent. S
//! steps through them.

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
use alaya_ui::input::Layout;
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
    let mut win = Window::open(&mut mem, 640, 460, "Settings").expect("open a window");
    let mut focus = 0;
    // The backup part: what it says, the recovery key while shown, and
    // the key being typed to get documents back.
    let mut backup_note = String::new();
    let mut shown_key: Option<String> = None;
    let mut typing: Option<String> = None;
    let mut chosen_grant = 0usize;
    let mut said = usize::MAX;
    let mut part = Part::Look;
    let mut seeing;
    loop {
        let mut again = false;
        // Alaya key + C or Enter may have changed it meanwhile.
        seeing = win.seeing();
        let mut new_part = None;
        let mut new_seeing = None;
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
        let mut next_scale = 0usize;
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
                    // Letter keys work from any part, and show theirs.
                    Event::Key(Key::Char('b')) => {
                        back_up = true;
                        new_part = Some(Part::Backup);
                    }
                    Event::Key(Key::Char('r')) => {
                        show_key = true;
                        new_part = Some(Part::Backup);
                    }
                    Event::Key(Key::Char('g')) => {
                        typing = Some(String::new());
                        backup_note = String::from("Type your recovery key, then Enter.");
                        new_part = Some(Part::Backup);
                    }
                    Event::Key(Key::Up) => {
                        chosen_grant = chosen_grant.saturating_sub(1);
                        new_part = Some(Part::Apps);
                    }
                    Event::Key(Key::Down) => {
                        chosen_grant += 1;
                        new_part = Some(Part::Apps);
                    }
                    Event::Key(Key::Char('t')) => {
                        take_back = true;
                        new_part = Some(Part::Apps);
                    }
                    Event::Key(Key::Char('s')) => {
                        next_scale += 1;
                        new_part = Some(Part::Screen);
                    }
                    Event::Key(k) => keys.push(k),
                    Event::Close => crate::closed(&win),
                    _ => {}
                },
            }
        }
        let (scale_now, scale_chosen) = win.screen_scale();
        let (layout, _) = win.typing(u64::MAX);
        let mut new_layout = None;
        let mut chosen = None;
        let mut chosen_scale = None;
        // One drawing pass per key: keys typed quickly arrive together,
        // and each must reach the piece that has the keyboard then.
        for pass in 0..keys.len().max(1) {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            cv.fill(Rect::new(0, 0, SIDE, h), theme::CHROME);
            cv.fill(Rect::new(SIDE, 0, 1, h), theme::LINE);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.focus = focus;
            ui.key = keys.get(pass).copied();
            let x0 = SIDE + 16;
            let right = w - 16;
            // The part shown first, so Tab reaches its pieces first; the
            // list of parts after it.
            match part {
                Part::Look => {
                    ui.text(x0, 14, Face::Strong, 15, "The look", theme::INK);
                    ui.text(
                        x0,
                        40,
                        Face::Text,
                        13,
                        "What you use Alaya for. Changes the dock at once.",
                        theme::MUTED,
                    );
                    let (mut x, mut y) = (x0, 68);
                    for l in Look::ALL {
                        let bw = ui.fonts.width(Face::Strong, 14, l.name()) + 24;
                        if x + bw > right {
                            (x, y) = (x0, y + 38);
                        }
                        let (hit, r) = ui.button(x, y, l.name(), l == look);
                        if hit {
                            chosen = Some(l);
                        }
                        x = r.right() + 8;
                    }
                    wrapped(
                        &mut ui,
                        x0,
                        y + 50,
                        right - x0,
                        12,
                        look.about(),
                        theme::MUTED,
                    );
                }
                Part::Seeing => {
                    ui.text(x0, 14, Face::Strong, 15, "Seeing", theme::INK);
                    let contrast = seeing & dk::seeing::CONTRAST != 0;
                    let reader = seeing & dk::seeing::READER != 0;
                    let (hit, _) = ui.button(x0, 44, &on_off("High contrast", contrast), contrast);
                    if hit {
                        new_seeing = Some(seeing ^ dk::seeing::CONTRAST);
                    }
                    wrapped(
                        &mut ui,
                        x0,
                        82,
                        right - x0,
                        12,
                        "Black ground, white text, yellow for what has the keyboard. Only colours change, never what anything does. Alaya key + C, anywhere.",
                        theme::MUTED,
                    );
                    let (hit, _) = ui.button(x0, 140, &on_off("Screen reader", reader), reader);
                    if hit {
                        new_seeing = Some(seeing ^ dk::seeing::READER);
                    }
                    wrapped(
                        &mut ui,
                        x0,
                        178,
                        right - x0,
                        12,
                        "Says what has the keyboard, and how many of your files the app in front can see. Until Alaya has sound, it shows its words in a bar at the top. Alaya key + Enter, anywhere; Alaya key + Tab walks the strip, the dock and the group labels.",
                        theme::MUTED,
                    );
                }
                Part::Typing => {
                    ui.text(x0, 14, Face::Strong, 15, "Typing", theme::INK);
                    ui.text(x0, 40, Face::Text, 13, "Your keyboard:", theme::MUTED);
                    let (mut x, mut y) = (x0, 62);
                    for l in Layout::ALL {
                        let bw = ui.fonts.width(Face::Strong, 14, l.name()) + 24;
                        if x + bw > right {
                            (x, y) = (x0, y + 38);
                        }
                        let (hit, r) = ui.button(x, y, l.name(), u64::from(l.code()) == layout);
                        if hit {
                            new_layout = Some(u64::from(l.code()));
                        }
                        x = r.right() + 6;
                    }
                    wrapped(
                        &mut ui,
                        x0,
                        y + 46,
                        right - x0,
                        12,
                        "Alaya key + Space switches between Latin letters, Japanese (type romaji, get kana) and Chinese (type pinyin, Space shows the characters, a number picks one). Accents: the dead keys of your keyboard, or Compose then two keys (a e: æ). Alaya key + . shows emoji, Alaya key + K a keyboard on the screen.",
                        theme::MUTED,
                    );
                    ui.text(
                        x0,
                        y + 150,
                        Face::Text,
                        16,
                        "Alaya: עברית · العربية · 日本語 · 中文 · 😀",
                        theme::INK,
                    );
                }
                Part::Screen => {
                    ui.text(x0, 14, Face::Strong, 15, "Size on this screen", theme::INK);
                    let about = if scale_chosen == 0 {
                        format!("Now {scale_now}%, Alaya's pick from the screen's size.")
                    } else {
                        format!("Now {scale_now}%.")
                    };
                    ui.text(x0, 40, Face::Text, 13, &about, theme::MUTED);
                    let (mut x, mut y) = (x0, 68);
                    for &p in &dk::SCALES {
                        let name = if p == 0 {
                            String::from("Alaya picks")
                        } else {
                            format!("{p}%")
                        };
                        let bw = ui.fonts.width(Face::Strong, 14, &name) + 24;
                        if x + bw > right {
                            (x, y) = (x0, y + 38);
                        }
                        let (hit, r) = ui.button(x, y, &name, p == scale_chosen);
                        if hit {
                            chosen_scale = Some(p);
                        }
                        x = r.right() + 6;
                    }
                    wrapped(
                        &mut ui,
                        x0,
                        y + 50,
                        right - x0,
                        12,
                        "For the screen this window is on; each screen keeps its own. Windows keep their size when they move between screens. S steps through.",
                        theme::MUTED,
                    );
                }
                Part::Apps => {
                    ui.text(x0, 14, Face::Strong, 15, "What apps can see", theme::INK);
                    wrapped(
                        &mut ui,
                        x0,
                        38,
                        right - x0,
                        12,
                        "Only what you handed them in Alaya's Open and Save windows, and what they made.",
                        theme::MUTED,
                    );
                    chosen_grant = chosen_grant.min(rows.len().saturating_sub(1));
                    if rows.is_empty() {
                        ui.text(x0, 80, Face::Text, 13, "Nothing yet.", theme::MUTED);
                    } else {
                        let texts: Vec<&str> = rows.iter().map(|r| r.as_str()).collect();
                        let pills: Vec<_> = grants.iter().map(|_| None).collect();
                        if let Some(i) = ui.list(
                            Rect::new(x0 - 4, 76, right - x0 + 4, 270),
                            "What apps can see",
                            &texts,
                            Some(chosen_grant),
                            &pills,
                        ) {
                            chosen_grant = i;
                        }
                        let (hit, _) = ui.button(x0, 356, "Take it back", false);
                        take_back |= hit;
                    }
                    if let Some(m) = &marked {
                        wrapped(&mut ui, x0, 396, right - x0, 12, m, theme::PAST);
                    }
                }
                Part::Backup => {
                    ui.text(x0, 14, Face::Strong, 15, "Backup", theme::INK);
                    let status = backup_status(&store);
                    wrapped(&mut ui, x0, 38, right - x0, 12, &status, theme::MUTED);
                    let (hit, r) = ui.button(x0, 80, "Back up now", false);
                    back_up |= hit;
                    let (hit, r) =
                        ui.button(r.right() + 10, 80, "Recovery key", shown_key.is_some());
                    show_key |= hit;
                    let (hit, _) = ui.button(r.right() + 10, 80, "Get them back", typing.is_some());
                    if hit && typing.is_none() {
                        typing = Some(String::new());
                        backup_note = String::from("Type your recovery key, then Enter.");
                    }
                    if let Some(t) = &typing {
                        let shown = format!("Recovery key: {t}|");
                        ui.text(x0, 126, Face::Strong, 13, &shown, theme::INK);
                    } else if let Some(k) = &shown_key {
                        ui.text(x0, 126, Face::Strong, 15, k, theme::INK);
                    }
                    wrapped(&mut ui, x0, 154, right - x0, 12, &backup_note, theme::PAST);
                }
            }
            for (i, p) in Part::ALL.iter().enumerate() {
                let (hit, _) = ui.button(12, 14 + i as i32 * 40, p.name(), *p == part);
                if hit {
                    new_part = Some(*p);
                }
            }
            ui.text(14, h - 26, Face::Text, 11, "Alaya 0.0.1, 7b", theme::MUTED);
            focus = ui.finish();
            win.tell(&ui.tree);
            win.pointer.released = false;
        }
        if let Some(p) = new_part.take()
            && p != part
        {
            part = p;
            // The keyboard starts on the new part's first piece.
            focus = 0;
            say!("Settings shows {}", p.name());
            again = true;
        }
        if let Some(l) = new_layout
            && l != layout
        {
            win.typing(l);
            again = true;
        }
        if let Some(b) = new_seeing
            && win.set_seeing(b)
        {
            // Drawn again with it at once.
            again = true;
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
        // Presses that arrive together each count.
        if next_scale > 0 {
            let i = dk::SCALES
                .iter()
                .position(|&p| p == scale_chosen)
                .unwrap_or(0);
            chosen_scale = Some(dk::SCALES[(i + next_scale) % dk::SCALES.len()]);
        }
        if let Some(p) = chosen_scale
            && p != scale_chosen
            && win.set_screen_scale(p)
        {
            let what = if p == 0 {
                String::from("Alaya's pick")
            } else {
                format!("{p}%")
            };
            say!("things on this screen are now drawn at {what}");
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
        if !again {
            win.wait();
        }
    }
}

/// The width of the list of parts, at the left.
const SIDE: i32 = 150;

/// Settings' parts, one shown at a time.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Look,
    Seeing,
    Typing,
    Screen,
    Apps,
    Backup,
}

impl Part {
    const ALL: [Part; 6] = [
        Part::Look,
        Part::Seeing,
        Part::Typing,
        Part::Screen,
        Part::Apps,
        Part::Backup,
    ];

    fn name(self) -> &'static str {
        match self {
            Part::Look => "The look",
            Part::Seeing => "Seeing",
            Part::Typing => "Typing",
            Part::Screen => "Screen size",
            Part::Apps => "What apps see",
            Part::Backup => "Backup",
        }
    }
}

fn on_off(what: &str, on: bool) -> String {
    format!("{what}: {}", if on { "on" } else { "off" })
}

/// Text wrapped to `width`, from (`x`, `y`) down.
fn wrapped(ui: &mut Ui, x: i32, y: i32, width: i32, px: u32, s: &str, c: alaya_ui::canvas::Color) {
    let (_, lh) = ui.fonts.metrics(Face::Text, px);
    for (i, &(a, z)) in ui.fonts.wrap(Face::Text, px, s, width).iter().enumerate() {
        ui.text(x, y + i as i32 * lh, Face::Text, px, &s[a..z], c);
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

// SPDX-License-Identifier: GPL-3.0-only
//! "What happened?" and the timeline (milestone 7, decisions 33, 58, 108
//! to 110). Opened from the strip's "What happened?" button.
//!
//! What happened? turns the store's record of changes into plain
//! sentences, newest first ("The text editor changed notes.txt"), with an
//! undo on each line. Up and down choose a line; U or Enter undoes it (the
//! desktop does the undo, and asks first if something else changed the
//! same thing since). Undoing an undo is a redo.
//!
//! The timeline: left and right move back and forward through the moments
//! history keeps; the list shows the documents as they were then (nothing
//! changes until "Bring this back"). B brings the chosen file back; N names
//! this version (named versions are never thinned); Home and End go to the
//! oldest and the newest. Tab switches between the two (1 and 2 choose).
//!
//! Only this person's own record is shown: the store answers for the
//! person whose badge asks, and nobody else (decision 68).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::{Action, Entry, Happening, Store, app_name, describe};
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop as dk;
use alaya_programs::files::{self as fs, DOCS, act, what};
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::rt::{self, Mem};
use alaya_sys::{Caps, Msg, say};
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Key, TextEdit, Ui, theme};

/// One line of "What happened?".
struct Line {
    text: String,
    when: String,
    /// What undoing it means: an action, or what an app changed in
    /// these commits (newest first).
    action: Option<Action>,
    changes: Vec<(u64, u64)>,
}

fn clock(t: u64) -> String {
    format!("{}:{:02}", t / 3600, t / 60 % 60)
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The verb for what a change did.
fn verb(w: u64) -> &'static str {
    if w & what::FORGOTTEN != 0 {
        "forgot"
    } else if w & what::RESTORED != 0 {
        "put back"
    } else if w & what::KEPT != 0 {
        "kept"
    } else if w & what::DELETED != 0 {
        "deleted"
    } else if w & what::CREATED != 0 {
        "made"
    } else if w & what::RENAMED != 0 {
        "renamed"
    } else if w & what::WROTE != 0 {
        "changed"
    } else {
        "noted where it came from:"
    }
}

/// A file's name now, or when it was last there.
fn name(store: &Store, f: u64, cn: u64, moments: &[(u64, u64)]) -> String {
    if let Ok(n) = store.name_of(f, 0) {
        return n;
    }
    if let Ok(n) = store.name_of(f, cn) {
        return n;
    }
    // Deleted in that commit: its name at the moment before.
    if let Some(&(prev, _)) = moments.iter().rev().find(|m| m.0 < cn)
        && let Ok(n) = store.name_of(f, prev)
    {
        return n;
    }
    String::from("a file")
}

fn names(list: &[String]) -> String {
    match list.len() {
        0 => String::from("nothing"),
        1 => list[0].clone(),
        2 => format!("{} and {}", list[0], list[1]),
        n => format!("{}, {} and {} more", list[0], list[1], n - 2),
    }
}

/// What one app changed in one commit: commit, app, time, (file, what).
type Item = (u64, u64, u64, Vec<(u64, u64)>);

/// Turn the record into lines, newest first.
fn lines(store: &Store) -> Vec<Line> {
    let rows: Vec<Happening> = store.happenings(0, 40).unwrap_or_default();
    let actions: Vec<Action> = store.actions(0, 64).unwrap_or_default();
    let moments = store.history().unwrap_or_default();
    // (commit, app) → what changed in it.
    let mut items: Vec<Item> = Vec::new();
    // Documents only: apps' own files (a return point, the look) are
    // theirs, and the look's undo shows as the action that changed it.
    for h in rows.iter().filter(|h| h.file & 3 == DOCS) {
        match items.iter_mut().find(|i| i.0 == h.cn && i.1 == h.app) {
            Some(i) => i.3.push((h.file, h.what)),
            None => items.push((h.cn, h.app, h.time, alloc::vec![(h.file, h.what)])),
        }
    }
    // Actions with no file changes (a window, the look) are lines too.
    for a in &actions {
        if !items.iter().any(|i| i.0 == a.cn && i.1 == a.app) {
            // Its commit may hold no moment of the documents: the time of
            // the moment just before it.
            let t = moments
                .iter()
                .rev()
                .find(|m| m.0 <= a.cn)
                .map_or(0, |m| m.1);
            items.push((a.cn, a.app, t, Vec::new()));
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut out: Vec<Line> = Vec::new();
    for (cn, app, time, files) in items {
        let action = actions
            .iter()
            .filter(|a| a.cn == cn && a.app == app)
            .max_by_key(|a| a.no)
            .copied();
        let when = clock(time);
        if let Some(a) = action {
            let d = describe(&a.label);
            let text = match a.kind {
                act::UNDO if d == "a change" => String::from("You undid a change"),
                act::UNDO => format!("You undid: {d}"),
                _ if a.app == fs::DESKTOP_APP => format!("You chose to {d}"),
                _ => format!("You chose to {d} (in {})", app_name(a.app)),
            };
            out.push(Line {
                text,
                when,
                action: Some(a),
                changes: alloc::vec![(cn, app)],
            });
            continue;
        }
        let ns: Vec<String> = files
            .iter()
            .map(|(f, _)| name(store, *f, cn, &moments))
            .collect();
        let v = files.first().map_or("changed", |(_, w)| verb(*w));
        // One line a commit: each can be undone by itself. (Lines that
        // stand for several saves need an undo of all of them at once,
        // checked against the newest; that comes with grouping by time.)
        out.push(Line {
            text: format!("{} {v} {}", capital(app_name(app)), names(&ns)),
            when,
            action: None,
            changes: alloc::vec![(cn, app)],
        });
    }
    out
}

/// Ask the desktop to undo (it asks the person first if needed): done,
/// and whether the desktop is asking now.
fn undo(by: u64, n: u64, app: u64) -> (bool, bool) {
    alaya_sys::ipc_call(
        start::FIRST_GIVEN + dk::APP_G_DESKTOP,
        &Msg::new(dk::UNDO, &[by, n, app]),
        Caps::default(),
    )
    .map_or((false, false), |m| (m.words[0] == dk::OK, m.words[1] != 0))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Happened,
    Timeline,
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let docs = store.root(DOCS).expect("documents");
    let mut win = Window::open(&mut mem, 640, 460, "What happened?").expect("open a window");
    let mut view = View::Happened;
    let mut list: Vec<Line> = Vec::new();
    let mut chosen = 0usize;
    let mut moments: Vec<(u64, u64)> = Vec::new();
    let mut named: Vec<(u64, u64, String)> = Vec::new();
    // The moment the timeline shows (an index in `moments`).
    let mut at = usize::MAX;
    let mut then: Vec<Entry> = Vec::new();
    let mut now_files: Vec<Entry> = Vec::new();
    let mut file = 0usize;
    let mut naming: Option<TextEdit> = None;
    let mut said = String::new();
    let mut note = String::new();
    let mut last_look = 0;
    let mut shown_at = u64::MAX;
    // Drawn again only when something changed: an idle window costs
    // nothing, also while another person's history is being made.
    let mut dirty = true;
    loop {
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            dirty = true;
            match e {
                Event::Key(k) => keys.push(k),
                Event::Close => crate::closed(&win),
                _ => {}
            }
        }
        let mut look = last_look == 0 || rt::now_ms() - last_look >= 2000;
        let mut press = None;
        for k in keys {
            if let Some(edit) = &mut naming {
                match k {
                    Key::Escape => naming = None,
                    Key::Enter => press = Some("Name it"),
                    k => {
                        edit.key(k, false);
                    }
                }
                continue;
            }
            match (view, k) {
                (_, Key::Tab) => {
                    view = match view {
                        View::Happened => View::Timeline,
                        View::Timeline => View::Happened,
                    };
                    look = true;
                }
                (_, Key::Char('1')) => view = View::Happened,
                (_, Key::Char('2')) => view = View::Timeline,
                (View::Happened, Key::Up) => chosen = chosen.saturating_sub(1),
                (View::Happened, Key::Down) => {
                    chosen = (chosen + 1).min(list.len().saturating_sub(1))
                }
                (View::Happened, Key::Enter | Key::Char('u')) => press = Some("Undo this"),
                (View::Happened, Key::Char('a')) => press = Some("Run it again"),
                (View::Timeline, Key::Left) => at = at.saturating_sub(1),
                (View::Timeline, Key::Home) => at = 0,
                (View::Timeline, Key::End) => at = moments.len().saturating_sub(1),
                (View::Timeline, Key::Right) => at = (at + 1).min(moments.len().saturating_sub(1)),
                (View::Timeline, Key::Up) => file = file.saturating_sub(1),
                // Kept within the list when it is shown: keys typed quickly
                // can come before the moment's list is read.
                (View::Timeline, Key::Down) => file = file.saturating_add(1),
                (View::Timeline, Key::Char('b')) => press = Some("Bring this back"),
                // At once: the next keys are the name.
                (View::Timeline, Key::Char('n')) => naming = Some(TextEdit::new("")),
                (_, Key::Char('r')) => look = true,
                _ => {}
            }
        }
        // Draw, then act on a button pressed (by key or mouse).
        if look {
            last_look = rt::now_ms();
            let before = (
                moments.last().copied(),
                named.len(),
                list.len(),
                list.first().map(|l| l.text.clone()),
                now_files.clone(),
            );
            list = lines(&store);
            chosen = chosen.min(list.len().saturating_sub(1));
            let follow = at == usize::MAX || at + 1 >= moments.len();
            moments = store.history().unwrap_or_default();
            named = store.named().unwrap_or_default();
            if follow || at >= moments.len() {
                at = moments.len().saturating_sub(1);
            }
            now_files = store.list(docs).unwrap_or_default();
            let after = (
                moments.last().copied(),
                named.len(),
                list.len(),
                list.first().map(|l| l.text.clone()),
                now_files.clone(),
            );
            if before != after {
                shown_at = u64::MAX;
                dirty = true;
            }
            let first = list
                .first()
                .map_or(String::from("nothing yet"), |l| l.text.clone());
            if first != said {
                say!("what happened: {first}");
                said = first;
            }
        }
        let cn = moments.get(at).map_or(0, |m| m.0);
        if view == View::Timeline && cn != 0 && cn != shown_at {
            let t0 = rt::now_us();
            then = store.past_list(cn, docs).unwrap_or_default();
            then.sort_by(|a, b| a.name.cmp(&b.name));
            say!(
                "timeline: moment {} of {} shows {} documents (opened in {} us)",
                at + 1,
                moments.len(),
                then.len(),
                rt::now_us() - t0
            );
            shown_at = cn;
            dirty = true;
        }
        if !then.is_empty() {
            file = file.min(then.len() - 1);
        }
        if dirty || press.is_some() {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            let (a, ra) = ui.button(14, 10, "What happened?", view == View::Happened);
            let (b, _) = ui.button(ra.right() + 8, 10, "Timeline", view == View::Timeline);
            if a && view != View::Happened {
                view = View::Happened;
            }
            if b && view != View::Timeline {
                view = View::Timeline;
            }
            match view {
                View::Happened => {
                    let texts: Vec<String> = list
                        .iter()
                        .map(|l| format!("{}  ·  {}", l.when, l.text))
                        .collect();
                    let texts: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    let pills: Vec<_> = list
                        .iter()
                        .map(|l| {
                            l.action
                                .filter(|a| a.undone)
                                .map(|_| ("Undone", theme::PAST, theme::PAST_BG))
                        })
                        .collect();
                    if list.is_empty() {
                        ui.text(
                            16,
                            56,
                            Face::Text,
                            14,
                            "Nothing has happened yet.",
                            theme::MUTED,
                        );
                    } else if let Some(i) = ui.list(
                        Rect::new(12, 52, w - 24, h - 120),
                        "What happened",
                        &texts,
                        Some(chosen),
                        &pills,
                    ) {
                        chosen = i;
                    }
                    let can = list
                        .get(chosen)
                        .is_some_and(|l| l.action.is_none_or(|a| !a.undone));
                    if can {
                        let (hit, _) = ui.button(12, h - 58, "Undo this", true);
                        if hit {
                            press = Some("Undo this");
                        }
                    }
                    // An app's own change: open it again on what it read.
                    if list
                        .get(chosen)
                        .is_some_and(|l| l.changes.first().is_some_and(|c| c.1 >= fs::app_id(1)))
                    {
                        let (hit, _) = ui.button(140, h - 58, "Run it again", true);
                        if hit {
                            press = Some("Run it again");
                        }
                    }
                }
                View::Timeline => {
                    // The slider: one tick per moment kept, named ones
                    // taller; the chosen one marked.
                    let bar = Rect::new(20, 64, w - 40, 6);
                    ui.cv.round_rect(bar, 3, theme::LINE);
                    let n = moments.len().max(1) as i32;
                    for (i, m) in moments.iter().enumerate() {
                        let x = bar.x + (bar.w - 1) * i as i32 / (n - 1).max(1);
                        let tall = named.iter().any(|v| v.0 == m.0);
                        let c = if tall { theme::PAST } else { theme::MUTED };
                        ui.cv.fill(
                            Rect::new(
                                x,
                                bar.y - if tall { 8 } else { 3 },
                                1,
                                if tall { 22 } else { 12 },
                            ),
                            c,
                        );
                    }
                    if at < moments.len() {
                        let x = bar.x + (bar.w - 1) * at as i32 / (n - 1).max(1);
                        ui.cv
                            .round_rect(Rect::new(x - 7, bar.y - 5, 14, 16), 7, theme::ACCENT);
                    }
                    let p = win.pointer;
                    if p.released && bar.grow(10).contains(p.x, p.y) && !moments.is_empty() {
                        let i = ((p.x - bar.x).max(0) * (n - 1) + bar.w / 2) / bar.w.max(1);
                        at = (i as usize).min(moments.len() - 1);
                    }
                    let here = moments.get(at).copied().unwrap_or((0, 0));
                    let name_now = named
                        .iter()
                        .find(|v| v.0 == here.0)
                        .map_or(String::new(), |v| format!(", named \"{}\"", v.2));
                    let latest = at + 1 == moments.len();
                    ui.text(
                        20,
                        86,
                        Face::Strong,
                        13,
                        &format!(
                            "{} {} of {} kept, at {}{}",
                            if latest { "Now: moment" } else { "Moment" },
                            at + 1,
                            moments.len(),
                            clock(here.1),
                            name_now
                        ),
                        theme::INK,
                    );
                    let texts: Vec<String> = then
                        .iter()
                        .map(|e| {
                            if e.is_dir {
                                format!("{}  (folder)", e.name)
                            } else {
                                format!("{}  ·  {} bytes", e.name, e.size)
                            }
                        })
                        .collect();
                    let texts: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    let pills: Vec<_> = then
                        .iter()
                        .map(|e| match now_files.iter().find(|x| x.file == e.file) {
                            None => Some(("Not here now", theme::WARN, theme::WARN_BG)),
                            Some(x) if x.size != e.size || x.name != e.name => {
                                Some(("Changed since", theme::PAST, theme::PAST_BG))
                            }
                            _ => None,
                        })
                        .collect();
                    if let Some(i) = ui.list(
                        Rect::new(12, 112, w - 24, h - 180),
                        "Documents then",
                        &texts,
                        Some(file),
                        &pills,
                    ) {
                        file = i;
                    }
                    let y = h - 58;
                    if let Some(edit) = &mut naming {
                        ui.text_box(Rect::new(12, y, 240, 30), "Version name", edit, false);
                        let (hit, _) = ui.button(262, y, "Name it", true);
                        if hit {
                            press = Some("Name it");
                        }
                    } else {
                        let (b1, r1) = ui.button(12, y, "Bring this back", !latest);
                        let (b2, _) = ui.button(r1.right() + 8, y, "Name this version", false);
                        if b1 {
                            press = Some("Bring this back");
                        }
                        if b2 {
                            press = Some("Name this version");
                        }
                    }
                }
            }
            if !note.is_empty() {
                ui.text(14, h - 22, Face::Text, 12, &note, theme::MUTED);
            }
            ui.finish();
            win.pointer.released = false;
        }
        match press {
            Some("Run it again") => {
                if let Some(&(cn, app)) = list.get(chosen).and_then(|l| l.changes.first()) {
                    let r = alaya_sys::ipc_call(
                        start::FIRST_GIVEN + dk::APP_G_DESKTOP,
                        &Msg::new(dk::RUN_AGAIN, &[app, cn]),
                        Caps::default(),
                    )
                    .map_or([u64::MAX; 6], |m| m.words);
                    note = if r[0] == dk::OK {
                        format!(
                            "{} opens again on {} file(s) as it read them{}.",
                            capital(app_name(app)),
                            r[1],
                            if r[2] == r[1] {
                                ""
                            } else {
                                " (some from the nearest kept moment)"
                            }
                        )
                    } else {
                        String::from("That app cannot run again.")
                    };
                    say!("run it again ({}): {}", app_name(app), note);
                }
            }
            Some("Undo this") => {
                if let Some(l) = list.get(chosen) {
                    let ok = match l.action {
                        Some(a) if !a.undone => undo(dk::undo_by::ACTION, a.no, 0).0,
                        Some(_) => false,
                        None => {
                            // Newest first; stop where the desktop asks.
                            let mut ok = true;
                            for &(cn, app) in &l.changes {
                                let (done, asking) = undo(dk::undo_by::CHANGE, cn, app);
                                ok &= done;
                                if !done || asking {
                                    break;
                                }
                            }
                            ok
                        }
                    };
                    note = String::from(if ok {
                        "Asked the desktop to undo it (see the strip)."
                    } else {
                        "That could not be undone."
                    });
                    say!(
                        "undo this line ({}): {}",
                        l.text,
                        if ok { "asked" } else { "refused" }
                    );
                    look = true;
                }
            }
            Some("Bring this back") => {
                if let Some(e) = then.get(file)
                    && let Some(&(cn, _)) = moments.get(at)
                {
                    note = match store.restore(cn, &[e.file]) {
                        Ok(alaya_apps::store::Undone::Done { .. }) => {
                            format!("{} is back as it was then.", e.name)
                        }
                        Ok(alaya_apps::store::Undone::Conflicts(_)) => format!(
                            "{} cannot go back: its folder is gone or its name is taken.",
                            e.name
                        ),
                        Err(_) => String::from("That could not be brought back."),
                    };
                    say!("timeline: {note}");
                    look = true;
                }
            }
            Some("Name this version") => naming = Some(TextEdit::new("")),
            Some("Name it") => {
                if let (Some(edit), Some(&(cn, _))) = (&naming, moments.get(at)) {
                    let n = edit.text.trim();
                    if !n.is_empty() {
                        note = match store.name_version(cn, n) {
                            Ok(()) => format!("This version is named \"{n}\" and will be kept."),
                            Err(_) => String::from("That name could not be given."),
                        };
                        say!("timeline: {note}");
                    }
                }
                naming = None;
                look = true;
            }
            _ => {}
        }
        if dirty || press.is_some() {
            win.damage_all();
        }
        dirty = press.is_some();
        if press.is_none() && !look {
            win.wait();
        }
    }
}

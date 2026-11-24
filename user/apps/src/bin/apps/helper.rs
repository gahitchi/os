// SPDX-License-Identifier: GPL-3.0-only
//! The tidy-up helper (milestone 7, step 3): a small program you don't
//! have to trust, because it works only through a job with rules. It asks
//! Alaya for a job; the person picks the one folder it may change and
//! sees the rules; it then tidies that folder on a side copy (neat
//! names, no spaces at line ends, a contents list) and says it is done.
//! Alaya runs the checks itself, and the person keeps the work or not.
//!
//! The same program, started as the hostile test helper, tries every way
//! around the rules it can think of (files outside the folder, the files
//! that must stay unchanged, private files, moving things out, keeping
//! its own work, a second job), then damages the folder so the checks
//! fail. Each try is counted: Alaya must refuse all of them.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::Store;
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop::job_event;
use alaya_programs::files::DOCS;
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::Mem;
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Key, Ui, theme};

/// What the helper makes in the folder.
const CONTENTS: &str = "contents.txt";
const CHECKLIST: &str = "checklist.txt";

/// A neat name: lower case, dashes for spaces.
fn neat(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c == ' ' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .collect()
}

/// Text without spaces at the ends of its lines.
fn trimmed(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    for (i, line) in text.split(|&b| b == b'\n').enumerate() {
        if i > 0 {
            out.push(b'\n');
        }
        out.extend_from_slice(line.trim_ascii_end());
    }
    out
}

/// Tidy the folder (the job's side copy). Returns what it did, in words.
fn tidy(store: &Store, folder: u64) -> String {
    let Ok(files) = store.list(folder) else {
        return String::from("could not look in the folder");
    };
    let (mut renamed, mut trimmed_n, mut refused) = (0, 0, 0);
    let mut names = Vec::new();
    for e in files.iter().filter(|e| !e.is_dir) {
        let mut name = e.name.clone();
        let want = neat(&name);
        if want != name && name != CHECKLIST && store.lookup(folder, &want).is_err() {
            match store.rename(folder, &name, folder, &want) {
                Ok(()) => {
                    renamed += 1;
                    name = want;
                }
                Err(_) => refused += 1,
            }
        }
        if name.ends_with(".txt") && name != CHECKLIST && name != CONTENTS {
            let before = store.read_all(e.file).unwrap_or_default();
            let after = trimmed(&before);
            if after != before {
                match store.write_all(e.file, &after) {
                    Ok(()) => trimmed_n += 1,
                    Err(_) => refused += 1,
                }
            }
        }
        if name != CONTENTS {
            names.push(name);
        }
    }
    names.sort();
    let mut list = String::from("What is in this folder (made by the tidy-up helper):\n");
    for n in &names {
        list.push_str(n);
        list.push('\n');
    }
    let made = store
        .open(folder, CONTENTS)
        .and_then(|f| store.write_all(f, list.as_bytes()))
        .is_ok();
    format!(
        "renamed {renamed}, trimmed {trimmed_n}, {} {CONTENTS}, {refused} refused",
        if made { "made" } else { "could not make" }
    )
}

/// The hostile test helper's tries. Returns (tries, got through).
fn attack(store: &Store, folder: u64) -> (u32, u32) {
    let mut tries = 0;
    let mut through = 0;
    let mut t = |what: &str, ok: bool| {
        tries += 1;
        if ok {
            through += 1;
            say!("GOT THROUGH: {what}");
        }
    };
    let docs = store.root(DOCS).unwrap_or(0);
    // Everything of the person's outside the folder: list it, guess it.
    t("listed the documents' top folder", store.list(docs).is_ok());
    t(
        "found summary.txt by name",
        store.lookup(docs, "summary.txt").is_ok(),
    );
    let inside: Vec<u64> = store
        .list(folder)
        .unwrap_or_default()
        .iter()
        .map(|e| e.file)
        .collect();
    let mut seen_outside = 0;
    for obj in 1..400u64 {
        let f = obj << 2;
        if f == folder || inside.contains(&f) {
            continue;
        }
        if store.stat(f).is_ok() {
            seen_outside += 1;
        }
    }
    t("saw files by guessing their numbers", seen_outside > 0);
    t(
        "made a file in the top folder",
        store.open(docs, "planted.txt").is_ok(),
    );
    // The folder itself: away, or out of it.
    for name in ["Trip notes", "trip-notes", "Notes.txt"] {
        t(
            "removed a folder or file by name",
            store.remove(docs, name).is_ok(),
        );
    }
    if let Some(e) = store
        .list(folder)
        .unwrap_or_default()
        .into_iter()
        .find(|e| !e.is_dir && e.name != CHECKLIST)
    {
        t(
            "moved a file out of the folder",
            store.rename(folder, &e.name, docs, "escaped.txt").is_ok(),
        );
    }
    // What must stay unchanged.
    if let Ok(c) = store.lookup(folder, CHECKLIST) {
        t(
            "changed the checklist",
            store.write_all(c, b"has nothing\n").is_ok(),
        );
        t(
            "removed the checklist",
            store.remove(folder, CHECKLIST).is_ok(),
        );
    }
    // Private files are not there for it at all.
    t(
        "found the private passport.txt",
        store.lookup(folder, "passport.txt").is_ok(),
    );
    // Keeping its own work, or starting another job.
    t(
        "kept its own work",
        store.job_keep(store_app(), true, &[]).is_ok(),
    );
    t(
        "started a job for itself",
        store
            .job_start(store_app(), docs, 0, 0, &[], b"mine")
            .is_ok(),
    );
    t(
        "threw the person's work away",
        store.job_discard(store_app()).is_ok(),
    );
    // Then the damage it may do, inside the folder: the checks catch it.
    let files = store.list(folder).unwrap_or_default();
    let mut emptied = false;
    let mut removed = false;
    for e in files.iter().filter(|e| !e.is_dir && e.name != CHECKLIST) {
        if !emptied && e.size > 0 {
            emptied = store.write_all(e.file, b"").is_ok();
        } else if !removed {
            removed = store.remove(folder, &e.name).is_ok();
        }
    }
    say!("inside the folder it emptied a file ({emptied}) and removed one ({removed})");
    (tries, through)
}

/// The app number Alaya knows the hostile helper by.
fn store_app() -> u64 {
    alaya_programs::files::app_id(alaya_programs::desktop::role::HOSTILE)
}

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store, hostile: bool) -> ! {
    let mut fonts = app::fonts(info);
    let title = if hostile {
        "Hostile test helper"
    } else {
        "Tidy-up helper"
    };
    let mut win = Window::open(&mut mem, 460, 260, title).expect("open a window");
    let mut note = String::from("I only work through a job with rules. Press Enter to ask.");
    let mut asked = false;
    // A job that survived a restart: done already, or still to do.
    let mut work = None;
    if let Ok(Some((folder, done, _))) = store.job() {
        if done {
            note = String::from("My job is done; Alaya has the result.");
        } else {
            note = String::from("Picking up my job where it was.");
            work = Some(folder);
        }
    }
    loop {
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(k) => keys.push(k),
                Event::Close => crate::closed(&win),
                Event::Job { what, arg } => match what {
                    job_event::STARTED => work = Some(arg),
                    job_event::REFUSED => {
                        asked = false;
                        say!("no job this time");
                        note = String::from("No job this time.");
                    }
                    job_event::KEPT => {
                        asked = false;
                        say!("the person kept my work ({arg} files)");
                        note = format!("You kept my work ({arg} files).");
                    }
                    job_event::THROWN => {
                        asked = false;
                        say!("my work was thrown away");
                        note = String::from("My work was thrown away.");
                    }
                    _ => {}
                },
                _ => {}
            }
        }
        let mut ask = false;
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.text(14, 14, Face::Strong, 14, title, theme::INK);
            ui.text(
                14,
                42,
                Face::Text,
                12,
                if hostile {
                    "For testing: it tries to get around every rule."
                } else {
                    "Neat names, no spaces at line ends, a contents list."
                },
                theme::MUTED,
            );
            ui.key = keys.first().copied();
            let (pressed, _) = ui.button(14, 80, "Ask for a job", !asked && work.is_none());
            ask |= pressed;
            ui.text(14, h - 40, Face::Text, 12, &note, theme::MUTED);
            ui.finish();
            win.pointer.released = false;
        }
        ask |= keys.contains(&Key::Enter);
        if ask && !asked && work.is_none() {
            asked = win.ask_job(false);
            say!("asked for a job ({})", if asked { "shown" } else { "busy" });
            note = String::from("Asked. Alaya shows you the rules.");
        }
        if let Some(folder) = work.take() {
            say!("the job started; working on the side copy");
            let did = if hostile {
                let (tries, through) = attack(&store, folder);
                say!("{tries} tries around the rules, {through} got through");
                format!("{tries} tries around the rules, {through} got through")
            } else {
                let did = tidy(&store, folder);
                say!("{did}");
                did
            };
            let _ = store.save();
            let done = store.job_done().is_ok();
            if hostile {
                // After done: nothing more may change.
                let late = store
                    .list(folder)
                    .ok()
                    .and_then(|l| l.into_iter().find(|e| !e.is_dir))
                    .is_some_and(|e| store.write_all(e.file, b"late").is_ok());
                if late {
                    say!("GOT THROUGH: changed a file after it said done");
                }
            }
            let shown = done && win.ask_job(true);
            say!("done ({}), Alaya shows the result ({shown})", done);
            note = format!("Done: {did}. Alaya checks it now.");
            asked = true;
        }
        win.damage_all();
        win.wait();
    }
}

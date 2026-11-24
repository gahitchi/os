// SPDX-License-Identifier: GPL-3.0-only
//! Postcards (milestone 8): the first app that is not built into Alaya.
//! It comes as a signed bundle, is installed and approved by the person,
//! and gets only what they left ticked of its promise.
//!
//! It adds a line to a postcard (`postcard.txt`) when it may use files, and
//! says plainly when it may not: with "Open and save the files you hand
//! it" unticked, it is given no way to reach the store at all. When it
//! starts it says hello with a notification, if it may.
//!
//! Its version is the bundle's (Alaya passes it). For the Experience
//! Guardian's check (milestone 8, step 2), versions 4 and 5 are slow to
//! start on purpose: they wait a few seconds before opening their window.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use alaya_apps::store::Store;
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop as dk;
use alaya_programs::files::DOCS;
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::rt::{self, Mem};
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Ui, theme};

alaya_sys::heap!();
alaya_sys::entry!(main);

/// Versions from this one on are slow to start, on purpose.
const SLOW_FROM: u64 = 4;
/// How long they wait (ms).
const SLOW_MS: u64 = 8000;
const CARD: &str = "postcard.txt";
const CLOCK_BIT: u64 = 2;

fn main(info: &'static StartInfo) -> ! {
    let mut mem = Mem::new(info);
    alaya_sys::heap::init(&mut mem, 4).expect("heap");
    let ntfn = start::FIRST_GIVEN + dk::APP_G_NTFN;
    rt::start_clock(&mut mem, ntfn, CLOCK_BIT, 100, info.priority).expect("clock");
    let version = info.args[dk::ARG_VERSION];
    if version >= SLOW_FROM {
        // A deliberately slow version, for the checks.
        // (The clock signals once a second.)
        let t0 = rt::now_ms();
        while rt::now_ms() - t0 < SLOW_MS {
            let _ = alaya_sys::wait(ntfn);
        }
    }
    let mut fonts = app::fonts(info);
    let store = Store::new(start::FIRST_GIVEN + dk::APP_G_STORE);
    // Without the files line, the store handle is simply not there.
    let docs = store.root(DOCS).ok();
    say!(
        "postcards {version}: files {}",
        if docs.is_some() {
            "allowed"
        } else {
            "not allowed"
        }
    );
    let Some(mut win) = Window::open(&mut mem, 420, 220, "Postcards") else {
        // Taken away while starting (undone, or the Experience Guardian
        // went back to the version before): nothing more to do.
        say!("postcards {version}: no window; stopping");
        loop {
            let _ = alaya_sys::wait(ntfn);
        }
    };
    // Hello, if it may: an unticked line is simply refused.
    let hello = win.notify(dk::inbox::WAIT, "Postcards is ready");
    say!(
        "postcards: hello {}",
        if hello { "sent" } else { "refused" }
    );
    let mut note = String::new();
    let mut write = false;
    loop {
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(k) => keys.push(k),
                Event::Shortcut('s') => write = true,
                Event::Close => {
                    win.close();
                    loop {
                        let _ = alaya_sys::wait(ntfn);
                    }
                }
                _ => {}
            }
        }
        if core::mem::take(&mut write) {
            note = match docs {
                Some(d) => {
                    // One more line on the postcard (it reads what is there).
                    let line = format!("Greetings from Postcards {version}, installed on Alaya.\n");
                    let ok = store
                        .open(d, CARD)
                        .and_then(|f| {
                            let mut text = store.read_all(f)?;
                            text.extend_from_slice(line.as_bytes());
                            store.write_all(f, &text)
                        })
                        .and_then(|_| store.save())
                        .is_ok();
                    if ok {
                        say!("postcards: wrote {CARD}");
                        format!("Wrote {CARD}.")
                    } else {
                        String::from("Could not write the postcard.")
                    }
                }
                None => String::from("You did not allow files, so nothing was written."),
            };
        }
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.text(
                16,
                14,
                Face::Strong,
                16,
                "Greetings from Alaya!",
                theme::INK,
            );
            let files = if docs.is_some() {
                "Files: you allowed them."
            } else {
                "Files: not allowed. Postcards cannot see or save anything."
            };
            ui.text(16, 48, Face::Text, 13, files, theme::MUTED);
            ui.text(
                16,
                72,
                Face::Text,
                13,
                &format!("Version {version}, installed from a signed bundle."),
                theme::MUTED,
            );
            ui.key = keys.first().copied();
            let (pressed, _) = ui.button(16, 104, "Write a postcard", true);
            write = pressed;
            ui.text(16, h - 30, Face::Text, 12, &note, theme::MUTED);
            ui.finish();
            win.tell(&ui.tree);
            win.pointer.released = false;
        }
        win.damage_all();
        if write {
            // The button was pressed: write on the next turn.
            continue;
        }
        win.wait();
    }
}

// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7b checks, step two: everything by keyboard, the screen
//! reader, and high contrast. The reader is checked by what it says (it
//! shows its words until Alaya has sound): the window in front, what its
//! app can see, and the piece that has the keyboard.

use alaya_programs::desktop::{self as dk, reader_role as rr, role, seeing};

use crate::m3::sleep_until;
use crate::m6b::{Session, ask};

const SLOW: u64 = 30_000;

/// What the reader said last: how many times it spoke, the role, the
/// files the app in front can see, and the first 16 bytes.
fn heard(ep: u64) -> (u64, u64, u64, [u8; 16]) {
    let r = ask(ep, dk::READER, &[]);
    let mut b = [0u8; 16];
    b[..8].copy_from_slice(&r[4].to_le_bytes());
    b[8..].copy_from_slice(&r[5].to_le_bytes());
    (r[1], r[2], r[3], b)
}

fn starts(b: &[u8; 16], s: &str) -> bool {
    let n = s.len().min(16);
    b[..n] == s.as_bytes()[..n]
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let bits = || ask(ep, dk::SEEING, &[0, u64::MAX])[1];
    let settings = x.window_of(role::SETTINGS).expect("Settings' window");
    let editor = x.window_of(role::EDITOR).expect("the editor's window");

    // ---- 105. The screen reader: on and off with Alaya key + Enter; it
    // says the window in front and what its app can see, then what has
    // the keyboard; high contrast with Alaya key + C.
    ask(ep, dk::RAISE, &[settings]);
    assert!(bits() & seeing::READER == 0, "the reader is on already");
    assert!(
        x.keys("{super+enter}", || bits() & seeing::READER != 0, SLOW),
        "Alaya key + Enter did not turn the reader on"
    );
    // Settings is part of Alaya: it sees all the person's files.
    assert!(
        sleep_until(SLOW, || {
            let h = heard(ep);
            // (The piece with the keyboard may be said right after.)
            h.2 == u64::MAX && h.0 >= 2
        }),
        "the reader did not say Settings: {:?}",
        heard(ep)
    );
    // Tab in Settings: the reader says the button that has it.
    let before = heard(ep).0;
    assert!(
        x.keys(
            "{tab}",
            || {
                let h = heard(ep);
                h.0 > before && h.1 == rr::BUTTON
            },
            SLOW
        ),
        "the reader did not follow Tab: {:?}",
        heard(ep)
    );
    let h = heard(ep);
    crate::say!(
        "the reader says: {}",
        core::str::from_utf8(&h.3).unwrap_or("?")
    );
    // The editor sees only what it was handed: a number, not "all".
    // (Counted before the raise: the reader may speak while it is done.)
    let before = heard(ep).0;
    ask(ep, dk::RAISE, &[editor]);
    assert!(
        sleep_until(SLOW, || {
            let h = heard(ep);
            // (The window is said first; the piece with the keyboard
            // may follow at once.)
            h.0 > before && h.2 != u64::MAX
        }),
        "the reader did not say the editor: {:?}",
        heard(ep)
    );
    crate::say!("the reader says the editor can see {} files", heard(ep).2);
    // High contrast: on, a screenshot, off.
    assert!(
        x.keys("{super+c}", || bits() & seeing::CONTRAST != 0, SLOW),
        "Alaya key + C did not turn high contrast on"
    );
    ask(ep, dk::RAISE, &[settings]);
    sleep_until(1500, || false);
    crate::say!("screenshot contrast");
    sleep_until(500, || false);
    assert!(x.keys("{super+c}", || bits() & seeing::CONTRAST == 0, SLOW));
    pass(
        "the screen reader says the window in front, what its app can see, and what has the keyboard; high contrast turns on and off",
    );

    // ---- 106. Alaya's own parts by keyboard: Alaya key + Tab walks the
    // strip, the dock and the group labels, Enter presses, Escape goes
    // back to the window.
    let said = |s: &'static str| {
        move || {
            let h = heard(ep);
            h.1 == rr::SHELL && starts(&h.3, s)
        }
    };
    assert!(
        x.keys("{super+tab}", said("Undo, button on"), SLOW),
        "Alaya key + Tab did not start at Undo: {:?}",
        heard(ep)
    );
    assert!(
        x.keys("{tab}", said("What happened?, "), SLOW),
        "Tab did not go on: {:?}",
        heard(ep)
    );
    assert!(x.keys("{tab}", said("Search, button o"), SLOW));
    // On into the dock: the first app there.
    assert!(
        x.keys(
            "{tab}",
            || {
                let h = heard(ep);
                h.1 == rr::SHELL && !starts(&h.3, "Search")
            },
            SLOW
        ),
        "the walk did not reach the dock: {:?}",
        heard(ep)
    );
    // Back, and Enter on What happened?: its window comes to the front.
    assert!(x.keys("{shift+tab}", said("Search, button o"), SLOW));
    assert!(x.keys("{shift+tab}", said("What happened?, "), SLOW));
    let back = || {
        x.window_of(role::BACK)
            .is_some_and(|w| ask(ep, dk::STATE, &[])[2] == w)
    };
    assert!(
        x.keys("{enter}", back, SLOW),
        "Enter on What happened? did not bring it to the front"
    );
    // Escape from a fresh walk goes back to the window in front.
    assert!(x.keys("{super+tab}", said("Undo, button on"), SLOW));
    assert!(
        x.keys("{escape}", || heard(ep).1 == rr::WINDOW, SLOW),
        "Escape did not go back to the window: {:?}",
        heard(ep)
    );
    assert!(
        x.keys("{super+enter}", || bits() & seeing::READER == 0, SLOW),
        "Alaya key + Enter did not turn the reader off"
    );
    pass(
        "Alaya's own parts by keyboard: Alaya key + Tab walks the strip and the dock, Enter presses",
    );
}

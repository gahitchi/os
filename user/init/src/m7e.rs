// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7a checks, step three: the decision inbox, Finish, copy or
//! reference, and the looks.

use alaya_programs::desktop::{self as dk, inbox, place, role};
use alaya_sys::rights;

use crate::m6b::{Session, ask};

const SLOW: u64 = 30_000;

/// Text in four message words, with its length.
fn packed(text: &str) -> (u64, [u64; 4]) {
    let mut b = [0u8; 32];
    let n = text.len().min(32);
    b[..n].copy_from_slice(&text.as_bytes()[..n]);
    let mut w = [0u64; 4];
    for (k, x) in w.iter_mut().enumerate() {
        *x = u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
    }
    (n as u64, w)
}

fn tell(ep: u64, label: u64, kind: u64, text: &str) -> u64 {
    let (n, w) = packed(text);
    ask(ep, label, &[kind | n << 8, w[0], w[1], w[2], w[3]])[0]
}

/// The inbox: (now shown at once, now, about, can wait, just so you know).
fn counts(ep: u64) -> [u64; 5] {
    let r = ask(ep, dk::INBOX, &[]);
    [r[1], r[2], r[3], r[4], r[5]]
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;

    // ---- 101. A busy day: nothing urgent is held back, and no app makes
    // itself urgent.
    let app = x.s.k.slot();
    alaya_sys::mint(x.s.k.at(app), x.s.k.at(ep), rights::ALL, role::CHART).unwrap();
    let before = counts(ep);
    for _ in 0..40 {
        assert!(
            tell(app, dk::NOTIFY, inbox::NOW, "URGENT!!! buy now") == dk::OK,
            "an app could not tell the person something"
        );
    }
    let c = counts(ep);
    assert!(c[1] == before[1], "an app's own \"now\" was trusted: {c:?}");
    assert!(
        c[2] + c[3] == before[2] + before[3] + 40,
        "an app's things were lost: {c:?}"
    );
    // An app cannot use Alaya's own channel.
    assert!(
        tell(app, dk::NOW, inbox::NOW, "pretend system") == dk::BAD_REQUEST,
        "an app used the channel only Alaya may use"
    );
    // A full-screen game in front: Alaya's "now" still shows at once.
    let editor = x.window_of(role::EDITOR).expect("the editor's window");
    ask(ep, dk::RAISE, &[editor]);
    let full = || ask(ep, dk::WINDOW, &[editor])[5] == place::FULL;
    if !x.keys("{super+up}", full, SLOW) {
        let st = ask(ep, dk::STATE, &[]);
        let wi = ask(ep, dk::WINDOW, &[editor]);
        panic!("the editor did not go full screen: state {st:x?}, window {wi:?}, editor {editor}");
    }
    for t in [
        "Battery almost empty",
        "The disk is failing",
        "Login changed",
    ] {
        assert!(tell(ep, dk::NOW, inbox::NOW, t) == dk::OK);
    }
    let c = counts(ep);
    assert!(
        c[0] == before[0] + 3 && c[1] == before[1] + 3,
        "a \"now\" was held back on a busy day: {c:?}"
    );
    // Let the screen catch up before the picture.
    crate::m3::sleep_until(1500, || false);
    crate::say!("screenshot now over full screen");
    // The person's own rule lets an app through. The inbox: now first,
    // then the chart maker's things.
    assert!(
        x.keys(
            "{super+i}",
            || ask(ep, dk::STATE, &[])[5] >> 44 & 1 == 1,
            SLOW
        ),
        "Alaya key + I did not open the inbox"
    );
    // Let the screen catch up before the picture.
    crate::m3::sleep_until(1500, || false);
    crate::say!("screenshot inbox");
    let ups = c[1] as usize;
    let mut keys = [0u8; 128];
    let mut n = 0;
    for _ in 0..ups.min(20) {
        keys[n..n + 6].copy_from_slice(b"{down}");
        n += 6;
    }
    keys[n] = b'a';
    let spec = core::str::from_utf8(&keys[..n + 1]).unwrap();
    let rules = || ask(ep, dk::STATE, &[])[5] >> 45 & 7;
    let had = rules();
    assert!(
        x.keys(spec, || rules() == had + 1, SLOW),
        "A did not make your rule"
    );
    assert!(x.keys(
        "{escape}",
        || ask(ep, dk::STATE, &[])[5] >> 44 & 1 == 0,
        SLOW
    ));
    let c0 = counts(ep);
    assert!(tell(app, dk::NOTIFY, inbox::NOW, "a call from a friend") == dk::OK);
    assert!(
        counts(ep)[1] == c0[1] + 1,
        "your rule did not let the app through"
    );
    assert!(x.keys("{super+down}", || !full(), SLOW));
    pass(
        "a busy day: every \"now\" shows at once, also over full screen; an app's own label is never trusted; your rule lets it through",
    );
}

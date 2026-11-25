// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step three: answers instead of data (R8 to R10).
//! Postcards (installed in place 0 by the checks before) asks for the
//! total of a column of a document the person picks, and gets one number,
//! never the document. Then game mode: a full-screen window in front
//! freezes apps running with no window, and install offers wait. Then
//! notifications that belong to a task come forward with it.

use alaya_programs::desktop::{self as dk, inbox, role};
use alaya_programs::files::{self as fs, DOCS, question, st};
use alaya_sys::rights;

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask, key_list};
use crate::m8a::{example, place, put};
use crate::m8b::{close_it, ended, version};

const SLOW: u64 = 30_000;
const COSTS: &str = "postcard-costs.txt";
const TEXT: &[u8] = b"what cost\nstamp 1.20\ncard 2.50\nstamp 1.20\n";
/// What the total of column 2 must be, in hundredths.
const TOTAL: u64 = 490;

/// The answers the ledger holds about `file` for `app`: (how many, the
/// last answer and question). The store commits first, so the newest
/// are in it.
fn answers(me: &Files, app: u64, file: u64) -> (usize, u64, u64) {
    me.call(fs::SAVE, &[]);
    let r = me.call(fs::ANSWERS, &[0, 64]);
    if r[0] != st::OK {
        return (0, 0, 0);
    }
    let b = me.buf();
    let mut n = 0;
    let mut last = (0, 0);
    for i in (0..r[1] as usize).rev() {
        let o = fs::ANSWER_BYTES * i;
        let w = |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
        if w(2) == app && w(3) == file {
            n += 1;
            last = (w(5), w(4));
        }
    }
    (n, last.0, last.1)
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let file = put(&me, docs, COSTS, TEXT, None);
    let app = fs::app_id(role::INSTALLED);
    let picking = || ask(ep, dk::STATE, &[])[5] >> 62 & 1 != 0;

    // ---- 121. An answer instead of the document.
    let w = x.wait_window(role::INSTALLED);
    ask(ep, dk::RAISE, &[w]);
    assert!(
        x.keys("{ctrl+t}", picking, SLOW),
        "Postcards' answer window did not open"
    );
    let at = me.place_in_docs(docs, COSTS.as_bytes());
    let mut keys = [0u8; 256];
    let pick = key_list(&mut keys, at.min(60));
    assert!(
        x.keys(pick, || answers(&me, app, file).0 == 1, SLOW),
        "no answer in the ledger after picking"
    );
    let (_, value, q) = answers(&me, app, file);
    assert!(
        value == TOTAL && q == question::TOTAL | 2 << 8,
        "the wrong answer: {value} to question {q:#x}"
    );
    // It was never handed the document itself.
    let r = me.call(fs::GRANTS, &[app]);
    let b = me.buf();
    let handed = (0..r[1] as usize).any(|i| {
        let o = fs::GRANT_BYTES * i;
        u64::from_le_bytes(b[o + 8..o + 16].try_into().unwrap()) == file
    });
    assert!(r[0] == st::OK && !handed, "Postcards holds the document");
    // Asking again gives the same number, until the day's questions are
    // used up; then nothing more.
    for n in 2..=fs::ANSWERS_PER_DAY as usize {
        assert!(
            x.keys("{ctrl+t}", || answers(&me, app, file).0 == n, SLOW),
            "answer {n} did not come"
        );
    }
    for _ in 0..3 {
        x.keys("{ctrl+t}", || false, 300);
    }
    sleep_until(1000, || false);
    let (n, value, _) = answers(&me, app, file);
    assert!(
        n == fs::ANSWERS_PER_DAY as usize && value == TOTAL,
        "the question limit did not hold: {n} answers"
    );
    pass(
        "an app asks for the total of a column and gets one number, never the document; every answer is in the ledger, and the day's question limit holds",
    );

    // ---- 122. Game mode.
    // Version 3 wants to keep running with no window: taken, and allowed
    // when it first asks.
    close_it(x);
    assert!(ended(x), "Postcards 1 kept running with no window");
    let v3 = put(&me, docs, "Postcards-3.alaya", example(x, 2), None);
    let v4 = put(&me, docs, "Postcards-4.alaya", example(x, 3), None);
    assert!(ask(ep, dk::OFFER_INSTALL, &[v3])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || version(ep) == 3 && place(ep).2 == 4),
        "version 3 was not taken"
    );
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    x.start_app(role::INSTALLED);
    x.wait_window(role::INSTALLED);
    if sleep_until(SLOW, || place(ep).2 == 5) {
        assert!(x.keys("{enter}", || place(ep).2 == 0, SLOW));
    }
    close_it(x);
    assert!(
        !ended(x),
        "it was ended although it may run in the background"
    );
    let game = || ask(ep, dk::GAME, &[]);
    assert!(game()[1] == 0, "game mode is on with nothing full screen");
    // A full-screen window in front: game mode.
    let editor = match x.window_of(role::EDITOR) {
        Some(w) => w,
        None => {
            x.start_app(role::EDITOR);
            x.wait_window(role::EDITOR)
        }
    };
    ask(ep, dk::RAISE, &[editor]);
    assert!(
        x.keys("{super+up}", || game()[1] == 1, SLOW),
        "no game mode with a full-screen window in front"
    );
    assert!(
        game()[2] & 1 << role::INSTALLED != 0,
        "the app with no window was not frozen: {:x?}",
        game()
    );
    x.pause_now();
    assert!(x.frozen(role::INSTALLED), "its thread still runs");
    // An update offered now waits for the game to end.
    assert!(ask(ep, dk::OFFER_INSTALL, &[v4])[0] == dk::OK);
    assert!(game()[3] == 1 && version(ep) == 3 && place(ep).2 == 0);
    // Out of full screen: everything goes on, and the offer comes.
    assert!(
        x.keys("{super+down}", || game()[1] == 0, SLOW),
        "game mode did not end"
    );
    x.pause_now();
    assert!(
        !x.frozen(role::INSTALLED),
        "it did not go on after the game"
    );
    assert!(
        sleep_until(SLOW, || version(ep) == 4 && game()[3] == 0),
        "the update offered during the game did not come after it"
    );
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    pass(
        "game mode: with a full-screen window in front, an app running with no window is frozen and an update waits; both go on when the game ends",
    );

    // ---- 123. Notifications belong to tasks: what the editor said while
    // the person was elsewhere waits, and comes forward with its group.
    let screen = || (ask(ep, dk::STATE, &[])[5] >> 40) & 0xf;
    let editor_ep = x.s.k.slot();
    alaya_sys::mint(x.s.k.at(editor_ep), x.s.k.at(ep), rights::ALL, role::EDITOR).unwrap();
    let text = b"your draft is checked";
    let mut words = [0u64; 4];
    for (k, c) in text.chunks(8).enumerate() {
        let mut b = [0u8; 8];
        b[..c.len()].copy_from_slice(c);
        words[k] = u64::from_le_bytes(b);
    }
    let held = || {
        let c = ask(ep, dk::INBOX, &[]);
        c[4]
    };
    let before = held();
    assert!(
        ask(
            editor_ep,
            dk::NOTIFY,
            &[
                inbox::WAIT | (text.len() as u64) << 8,
                words[0],
                words[1],
                words[2],
                words[3]
            ]
        )[0] == dk::OK
    );
    assert!(held() == before + 1, "the editor's note was not held");
    ask(ep, dk::RAISE, &[editor]);
    let r = ask(ep, dk::GROUP_GO, &[0]);
    assert!(
        r[0] == dk::OK && r[1] == 1,
        "the editor's note did not come forward with its group: {r:?}"
    );
    if screen() != 0 {
        x.keys("{escape}", || screen() == 0, SLOW);
    }
    // Once forward, it does not come again.
    assert!(ask(ep, dk::GROUP_GO, &[0])[1] == 0);
    if screen() != 0 {
        x.keys("{escape}", || screen() == 0, SLOW);
    }
    pass(
        "a note from an app in a task waits while the person is elsewhere, and comes forward when they go into that task",
    );
}

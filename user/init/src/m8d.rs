// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step four: the old world. C programs on Alaya's
//! own C library, run by the terminal, which sees only what it is handed.

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, APPS, DOCS};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask, key_list};

const SLOW: u64 = 30_000;

/// The end of the terminal's own record of what it ran.
fn log_tail(t: &Files, out: &mut [u8; 2048]) -> usize {
    let Some(f) = t.lookup(t.root(APPS), "terminal.log") else {
        return 0;
    };
    let size = t.size(f).unwrap_or(0);
    let from = size.saturating_sub(out.len() as u64);
    t.read_at(f, from, out).unwrap_or(0)
}

fn log_ends(t: &Files, with: &str) -> bool {
    let mut b = [0u8; 2048];
    let n = log_tail(t, &mut b);
    b[..n].ends_with(with.as_bytes())
}

fn log_has(t: &Files, what: &str) -> bool {
    let mut b = [0u8; 2048];
    let n = log_tail(t, &mut b);
    b[..n].windows(what.len()).any(|w| w == what.as_bytes())
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let person = x.session()[2];
    let term = x.files_as(person, fs::app_id(role::TERMINAL));

    // ---- 125. C programs in the terminal.
    x.start_app(role::TERMINAL);
    let w = x.wait_window(role::TERMINAL);
    ask(ep, dk::RAISE, &[w]);
    assert!(
        x.keys("hello{enter}", || log_ends(&term, "[exit 0]\n"), SLOW),
        "the terminal did not run hello"
    );
    assert!(
        log_has(&term, "Hello from C, on Alaya.\n"),
        "hello printed something else"
    );
    // The C library checks itself.
    assert!(
        x.keys("selftest{enter}", || log_has(&term, "selftest: "), SLOW),
        "the self test did not run"
    );
    let mut b = [0u8; 2048];
    let n = log_tail(&term, &mut b);
    let tail = core::str::from_utf8(&b[..n]).unwrap_or("");
    assert!(
        tail.ends_with("[exit 0]\n") && !tail.contains("FAILED"),
        "the C library failed its self test:\n{tail}"
    );
    // Not handed over: not there.
    assert!(
        x.keys(
            "wc numbers.txt{enter}",
            || log_ends(&term, "[exit 1]\n"),
            SLOW
        ),
        "wc read a document the terminal was not handed"
    );
    // Handed over in Alaya's Open window, then counted.
    let picking = || ask(ep, dk::STATE, &[])[5] >> 62 & 1 != 0;
    assert!(x.keys("pick{enter}", picking, SLOW), "no Open window");
    let at = me.place_in_docs(docs, b"numbers.txt");
    let mut keys = [0u8; 256];
    let spec = key_list(&mut keys, at.min(60));
    assert!(x.keys(spec, || !picking(), SLOW));
    assert!(
        x.keys(
            "wc numbers.txt{enter}",
            || log_ends(&term, " numbers.txt\n[exit 0]\n"),
            SLOW
        ),
        "wc did not count the handed document"
    );
    assert!(
        sleep_until(SLOW, || log_has(&term, "$ wc numbers.txt\n")),
        "no record"
    );
    pass(
        "C programs on Alaya's own C library run in the terminal: its self test passes, and wc counts a document only once it is handed over",
    );
}

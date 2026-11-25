// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step three: answers instead of data (R8 to R10).
//! Postcards (installed in place 0 by the checks before) asks for the
//! total of a column of a document the person picks, and gets one number,
//! never the document.

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, DOCS, question, st};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask, key_list};
use crate::m8a::put;

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
}

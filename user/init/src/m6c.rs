// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 6 checks, part three: living validity, first show
//! (`commitments.md`). The chart maker tells the store what its picture
//! and summary were made from; the numbers change in the text editor; the
//! file browser marks both "Needs a look"; the chart maker saves again and
//! they are up to date. Then, through the real store server: a result
//! made from part of a file is never "up to date", one app cannot speak
//! for another's result, and "as of a date" holds.

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, DOCS, st};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask, return_point_of};
use crate::say;

/// The kinds of mark (`alaya_store::made::Kind`) and one reason.
const UP_TO_DATE: u64 = 1;
const NEEDS_A_LOOK: u64 = 2;
const AS_OF: u64 = 3;
const NOT_SURE: u64 = 4;
const NOT_ALL_READ: u64 = 2;

impl Files {
    /// The mark of `f`: kind, why, the source it is about.
    fn mark(&self, f: u64) -> (u64, u64, u64) {
        let r = self.call(fs::MARK, &[f]);
        if r[0] != st::OK {
            return (u64::MAX, 0, 0);
        }
        (r[1] & 0xff, r[1] >> 8, r[2])
    }

    /// Exactly which version of `f` this is: (file, version, edits).
    fn version(&self, f: u64) -> [u64; 3] {
        let r = self.call(fs::STAT, &[f]);
        [f, r[2], r[4]]
    }

    fn made_from(&self, result: u64, src: [u64; 3]) -> u64 {
        for (i, w) in src.iter().enumerate() {
            self.buf()[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
        }
        self.call(fs::MADE_FROM, &[result, 1])[0]
    }

    fn create(&self, dir: u64, name: &str) -> Option<u64> {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
        let r = self.call(fs::CREATE, &[dir, 0, name.len() as u64]);
        (r[0] == st::OK).then_some(r[1])
    }

    fn write(&self, f: u64, data: &[u8]) -> bool {
        self.buf()[..data.len()].copy_from_slice(data);
        self.call(fs::WRITE, &[f, 0, data.len() as u64])[0] == st::OK
    }

    /// Read `n` bytes from the start of `f` (and nothing else).
    fn read_part(&self, f: u64, n: u64) -> bool {
        self.call(fs::READ, &[f, 0, n])[0] == st::OK
    }
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let numbers = me.lookup(docs, "numbers.txt").expect("numbers.txt");
    let chart = me.lookup(docs, "chart.ppm").expect("chart.ppm");
    let mut summary = 0;
    assert!(
        sleep_until(10_000, || me.lookup(docs, "summary.txt").is_some()),
        "the chart maker saved no summary"
    );
    if let Some(f) = me.lookup(docs, "summary.txt") {
        summary = f;
    }

    // ---- The results the chart maker saved (check 67) are up to date;
    // the numbers themselves are a source, with no mark.
    assert!(
        sleep_until(10_000, || me.mark(chart).0 == UP_TO_DATE
            && me.mark(summary).0 == UP_TO_DATE),
        "the chart is not up to date: {:?}",
        me.mark(chart)
    );
    assert!(me.mark(numbers).0 == 0, "numbers.txt has a mark");
    pass("results made from other files are marked up to date; sources have no mark");

    // ---- The numbers change in the text editor.
    let editor = x.window_of(role::EDITOR).expect("the editor's window");
    ask(ep, dk::RAISE, &[editor]);
    let saved = || return_point_of(&me, &mut [0u8; 512]).is_some_and(|r| r.2);
    assert!(
        x.keys("{ctrl+s}", saved, 8000),
        "the editor did not save its file"
    );
    let open = || {
        let mut b = [0u8; 512];
        return_point_of(&me, &mut b)
            .is_some_and(|(_, _, _, n)| &b[48..48 + n.min(64)] == b"numbers.txt")
    };
    assert!(
        x.keys("{ctrl+o}", open, 30_000),
        "Ctrl+O did not open numbers.txt"
    );
    // "Mon 3" becomes "Mon 9", saved.
    let stale = || me.mark(chart) == (NEEDS_A_LOOK, 1, numbers);
    assert!(
        x.keys(
            "{right}{right}{right}{right}{right}{backspace}9{ctrl+s}",
            stale,
            30_000
        ),
        "the chart is not marked after its numbers changed: {:?}",
        me.mark(chart)
    );
    assert!(
        me.mark(summary).0 == NEEDS_A_LOOK,
        "the summary is not marked"
    );
    // The file browser in front, to see the marks.
    let files = x.window_of(role::FILES).expect("the file browser's window");
    ask(ep, dk::RAISE, &[files]);
    sleep_until(2500, || false);
    // Choose the last file (the summary) to see its story; more presses
    // than files, so a repeat lands in the same place.
    x.keys("{down}{down}{down}{down}", || false, 1000);
    say!("screenshot marks");
    pass("the numbers change: the chart and its summary are marked \"Needs a look\"");

    // ---- The chart maker saves again from the new numbers.
    let chart_win = x.window_of(role::CHART).expect("the chart maker's window");
    ask(ep, dk::RAISE, &[chart_win]);
    // It reads changed numbers within two seconds.
    sleep_until(2500, || false);
    let fresh = || me.mark(chart).0 == UP_TO_DATE && me.mark(summary).0 == UP_TO_DATE;
    assert!(
        x.keys("{enter}", fresh, 30_000),
        "the chart is not up to date after saving again: {:?}",
        me.mark(chart)
    );
    pass("made again from the new numbers, the chart is up to date again");

    // ---- Through the store server, as the first program (an app of its
    // own): part of a file is never enough.
    let half = me.create(docs, "half.txt").expect("half.txt");
    assert!(me.write(half, b"made from the start of the numbers\n"));
    let v = me.version(numbers);
    assert!(me.read_part(numbers, 6));
    assert!(me.made_from(half, v) == st::OK, "MADE_FROM was refused");
    assert!(
        me.mark(half) == (NOT_SURE, NOT_ALL_READ, numbers),
        "a result made from part of a file is {:?}",
        me.mark(half)
    );
    let mut all = [0u8; 256];
    assert!(me.read(numbers, &mut all).is_some());
    assert!(me.made_from(half, v) == st::OK);
    assert!(
        me.mark(half).0 == UP_TO_DATE,
        "read in full, still not sure"
    );
    // Only the app that made a result may say what it was made from.
    assert!(
        me.made_from(chart, v) == st::BAD_REQUEST,
        "another app spoke for the chart maker's picture"
    );
    // "As of a date": changes no longer touch it.
    assert!(me.call(fs::AS_OF, &[half, 1])[0] == st::OK);
    assert!(me.write(numbers, b"Mon 1"));
    assert!(me.mark(half).0 == AS_OF, "as of a date did not hold");
    assert!(me.mark(chart).0 == NEEDS_A_LOOK);
    pass("part of a file is never up to date; one app cannot speak for another's result");
}

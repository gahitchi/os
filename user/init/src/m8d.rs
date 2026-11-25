// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step four: the old world. C programs on Alaya's
//! own C library, run by the terminal, which sees the person's documents,
//! and inside a protected task only the task's (decisions 40 and 71).

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, APPS, DOCS, st};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask};

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
    // Outside any task it sees the person's documents, like a terminal
    // anywhere (decision 40).
    assert!(
        x.keys(
            "wc numbers.txt{enter}",
            || log_ends(&term, " numbers.txt\n[exit 0]\n"),
            SLOW
        ),
        "wc did not count a document outside tasks"
    );
    assert!(
        sleep_until(SLOW, || log_has(&term, "$ wc numbers.txt\n")),
        "no record"
    );
    // Inside a protected task: only the task's files (decision 71).
    let notes = me.new_in(docs, "task-notes.txt", false);
    let g = me.new_group("Terminal work");
    assert!(me.call(fs::GROUP_ADD, &[g, notes])[0] == st::OK);
    me.name_in("Terminal work");
    assert!(me.call(fs::GROUP_SET, &[g, fs::GROUP_TASK, 0x5b_4b_8a, 0, 13])[0] == st::OK);
    let app = fs::app_id(role::TERMINAL);
    assert!(me.call(fs::OPEN_IN_TASK, &[app, g])[0] == st::OK);
    assert!(
        x.keys(
            "wc numbers.txt{enter}",
            || log_ends(&term, "[exit 1]\n"),
            SLOW
        ),
        "the terminal reached a file outside its task"
    );
    assert!(
        x.keys(
            "wc task-notes.txt{enter}",
            || log_ends(&term, " task-notes.txt\n[exit 0]\n"),
            SLOW
        ),
        "the terminal could not reach its task's file"
    );
    // "all": Alaya asks; "Only the task" keeps the wall.
    let asking = || ask(ep, dk::STATE, &[])[5] & 1 != 0;
    assert!(x.keys("all{enter}", asking, SLOW), "Alaya did not ask");
    assert!(x.keys("{escape}", || !asking(), SLOW));
    assert!(
        term.lookup(docs, "numbers.txt").is_none(),
        "the wall fell without a yes"
    );
    // "Open all": everything, until it closes; one undo closes it again.
    assert!(
        x.keys("all{enter}", asking, SLOW),
        "Alaya did not ask again"
    );
    assert!(
        x.keys(
            "{enter}",
            || term.lookup(docs, "numbers.txt").is_some(),
            SLOW
        ),
        "Open all did not open the person's files"
    );
    assert!(
        x.keys(
            "wc numbers.txt{enter}",
            || log_ends(&term, " numbers.txt\n[exit 0]\n"),
            SLOW
        ),
        "wc did not count once all files were open"
    );
    assert!(
        x.keys(
            "{super+z}",
            || term.lookup(docs, "numbers.txt").is_none(),
            SLOW
        ),
        "undo did not close the files again"
    );
    // Out of the task again.
    assert!(me.call(fs::OPEN_IN_TASK, &[app, 0])[0] == st::OK);
    assert!(me.call(fs::GROUP_REMOVE, &[g])[0] == st::OK);
    assert!(term.lookup(docs, "numbers.txt").is_some());
    pass(
        "C programs on Alaya's own C library run in the terminal: its self test passes; it sees your documents, inside a task only the task's until you open all, and one undo closes them again",
    );
}

// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7a checks, step one: groups and protected tasks. A group is
//! only a label: no app sees anything more for it. A protected task is a
//! wall the person confirmed after seeing exactly what apps in it will
//! see: an app in it reaches those files and nothing else, even files it
//! was handed before. A paused task's apps are frozen, not closed.

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, APPS, DOCS, st};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask};

/// The stranger app of milestone 7's checks.
pub(crate) const STRANGER: u64 = fs::app_id(77);
const SLOW: u64 = 30_000;

impl Files {
    pub(crate) fn name_in(&self, name: &str) {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
    }

    pub(crate) fn new_in(&self, dir: u64, name: &str, folder: bool) -> u64 {
        self.name_in(name);
        let r = self.call(fs::CREATE, &[dir, folder as u64, name.len() as u64]);
        assert!(r[0] == st::OK, "could not make {name}");
        r[1]
    }

    pub(crate) fn new_group(&self, name: &str) -> u64 {
        self.name_in(name);
        let r = self.call(fs::GROUP_NEW, &[0x5b_4b_8a, name.len() as u64]);
        assert!(r[0] == st::OK, "could not make the group {name}");
        r[1]
    }

    /// A group's flags (1 task, 2 paused), if it is there.
    pub(crate) fn group_flags(&self, id: u64) -> Option<u64> {
        let r = self.call(fs::GROUPS, &[]);
        if r[0] != st::OK {
            return None;
        }
        let b = self.buf();
        (0..r[1] as usize).find_map(|i| {
            let o = fs::GROUP_BYTES * i;
            let w = |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
            (w(0) == id).then(|| w(1))
        })
    }

    pub(crate) fn in_group(&self, id: u64, f: u64) -> bool {
        let r = self.call(fs::GROUP_FILES, &[id]);
        let b = self.buf();
        r[0] == st::OK
            && (0..r[1] as usize)
                .any(|i| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap()) == f)
    }

    pub(crate) fn reads(&self, f: u64) -> bool {
        self.call(fs::READ, &[f, 0, 8])[0] == st::OK
    }
}

/// Act on the desktop's pause signals as they come, until the editor is
/// frozen (or going again): the desktop signals just after the store
/// says the task is paused.
fn frozen_soon(x: &mut Session<'_>, want: bool) -> bool {
    for _ in 0..300 {
        x.pause_now();
        if x.frozen(role::EDITOR) == want {
            return true;
        }
        sleep_until(100, || false);
    }
    false
}

/// Which group screen is up (STATE): 0 none, 1 a group, 2 "make it a
/// task?", 3 naming, 4 choosing a group for a file.
fn screen(ep: u64) -> u64 {
    (ask(ep, dk::STATE, &[])[5] >> 40) & 0xf
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let p = x.person;
    let them = x.files_as(p, STRANGER);

    let dir = me.new_in(docs, "Thesis", true);
    let results = me.new_in(dir, "results.txt", false);
    let refs = me.new_in(docs, "refs.txt", false);
    let diary = me.new_in(docs, "diary.txt", false);
    for f in [results, refs, diary] {
        me.buf()[..6].copy_from_slice(b"words\n");
        assert!(me.call(fs::WRITE, &[f, 0, 6])[0] == st::OK);
    }
    me.call(fs::SAVE, &[]);

    // ---- 97. A loose group is only a label.
    let g = me.new_group("Thesis");
    assert!(me.call(fs::GROUP_ADD, &[g, dir])[0] == st::OK);
    // The person puts refs.txt in from the file browser's G: Alaya asks
    // which group (Work, Home, Thesis).
    assert!(
        ask(ep, dk::GROUP_FILE, &[refs])[0] == dk::OK && screen(ep) == 4,
        "Alaya did not ask which group"
    );
    crate::say!("screenshot choose group");
    assert!(
        x.keys("{down}{down}{enter}", || me.in_group(g, refs), SLOW),
        "refs.txt did not go into the group"
    );
    assert!(screen(ep) == 0, "the group chooser stayed up");
    // Nothing is handed to anyone for it.
    for f in [dir, results, refs] {
        assert!(
            !them.reads(f),
            "an app read a file because it is in a loose group"
        );
    }
    them.name_in("refs.txt");
    assert!(
        them.call(fs::LOOKUP, &[docs, 8])[0] == st::NOT_FOUND,
        "an app found a file of a loose group"
    );
    assert!(
        me.call(fs::OPEN_IN_TASK, &[STRANGER, g])[0] != st::OK,
        "an app was put inside a loose group"
    );
    pass("a loose group is only a label: no app sees anything more for it");

    // ---- 98. A task is a wall the person confirmed.
    assert!(me.call(fs::GRANT, &[STRANGER, diary, 1, 0, 0])[0] == st::OK);
    assert!(them.reads(diary), "the hand-over did not work");
    assert!(
        ask(ep, dk::GROUP_SCREEN, &[g])[0] == dk::OK && screen(ep) == 1,
        "the group screen did not open"
    );
    assert!(
        x.keys("t", || screen(ep) == 2, SLOW),
        "T did not show what apps in the task would see"
    );
    assert!(
        me.group_flags(g) == Some(0),
        "a task before the person confirmed"
    );
    crate::say!("screenshot task confirm");
    assert!(
        x.keys("{enter}", || me.group_flags(g) == Some(1), SLOW),
        "Enter did not make it a task"
    );
    assert!(me.call(fs::OPEN_IN_TASK, &[STRANGER, g])[0] == st::OK);
    assert!(
        them.reads(results) && them.reads(refs) && them.call(fs::STAT, &[dir])[0] == st::OK,
        "an app in the task could not reach the task's files"
    );
    assert!(
        !them.reads(diary),
        "an app in the task reached a file outside it, handed over before"
    );
    let r = them.call(fs::LIST, &[docs]);
    assert!(
        r[0] == st::OK && r[1] == 2,
        "an app in the task listed {} documents, not the task's 2",
        r[1]
    );
    // What it makes joins the task.
    let draft = them.new_in(docs, "draft.txt", false);
    assert!(
        them.reads(draft) && me.in_group(g, draft),
        "what an app made in the task is not in it"
    );
    // Taking a file out of the task takes it out of reach; undo puts it
    // back. The rows: Thesis, draft.txt, refs.txt.
    assert!(ask(ep, dk::GROUP_SCREEN, &[g])[0] == dk::OK);
    assert!(
        x.keys("{down}{down}{delete}", || !me.in_group(g, refs), SLOW),
        "Delete did not take refs.txt out of the task"
    );
    assert!(
        !them.reads(refs),
        "a file taken out of the task stayed in reach"
    );
    assert!(x.keys("{escape}", || screen(ep) == 0, SLOW));
    assert!(
        ask(ep, dk::UNDO, &[dk::undo_by::LAST, 0, 0])[0] == dk::OK
            && sleep_until(SLOW, || me.in_group(g, refs)),
        "undo did not put refs.txt back in the task"
    );
    assert!(them.reads(refs), "refs.txt came back but not in reach");
    pass(
        "a task is a wall you confirm: its files and nothing else, even what was handed over before",
    );

    // ---- 99. A window dropped on a task's label goes inside the wall; a
    // paused task's apps are frozen, not closed.
    let apps = me.root(APPS);
    let lf = me.lookup(apps, "look").expect("the look file");
    me.buf()[0] = 1;
    me.call(fs::WRITE, &[lf, 0, 1]);
    ask(ep, dk::LOOK_CHANGED, &[]);
    assert!(
        sleep_until(SLOW, || ask(ep, dk::PLACES, &[dk::places::CHIP + 2])[0]
            == dk::OK),
        "the task's label is not there"
    );
    let editor = match x.window_of(role::EDITOR) {
        Some(w) => w,
        None => {
            x.start_app(role::EDITOR);
            x.wait_window(role::EDITOR)
        }
    };
    ask(ep, dk::RAISE, &[editor]);
    let chip = ask(ep, dk::PLACES, &[dk::places::CHIP + 2]);
    let e = ask(ep, dk::WINDOW, &[editor]);
    let editor_app = fs::app_id(role::EDITOR);
    assert!(
        x.drag(
            (e[1] + e[3] / 2 - 40, e[2] + 14),
            (chip[1] + chip[3] / 2, chip[2] + chip[4] / 2),
            || me.call(fs::TASK_OF, &[editor_app])[1] == g,
        ),
        "the editor did not go inside the task"
    );
    let ed = x.files_as(p, editor_app);
    assert!(
        ed.reads(results) && !ed.reads(diary),
        "the editor in the task reached the wrong files"
    );
    // Pause: P on the task's screen.
    assert!(ask(ep, dk::GROUP_SCREEN, &[g])[0] == dk::OK);
    assert!(
        x.keys("p", || me.group_flags(g) == Some(3), SLOW),
        "P did not pause the task"
    );
    assert!(
        frozen_soon(x, true),
        "the paused task's editor is still running"
    );
    assert!(
        x.window_of(role::EDITOR) == Some(editor),
        "pausing closed the editor"
    );
    crate::say!("screenshot paused task");
    assert!(ask(ep, dk::GROUP_SCREEN, &[g])[0] == dk::OK);
    assert!(
        x.keys("p", || me.group_flags(g) == Some(1), SLOW),
        "P did not let the task go on"
    );
    assert!(frozen_soon(x, false), "the editor stayed frozen");
    // A loose group again: every app is let go.
    assert!(ask(ep, dk::GROUP_SCREEN, &[g])[0] == dk::OK);
    assert!(
        x.keys("t", || me.group_flags(g) == Some(0), SLOW),
        "T did not make it a loose group again"
    );
    assert!(
        me.call(fs::TASK_OF, &[editor_app])[1] == 0 && me.call(fs::TASK_OF, &[STRANGER])[1] == 0,
        "an app stayed inside a task that is gone"
    );
    assert!(
        them.reads(diary) && !them.reads(results),
        "the hand-overs did not come back"
    );
    me.buf()[0] = 0;
    me.call(fs::WRITE, &[lf, 0, 1]);
    ask(ep, dk::LOOK_CHANGED, &[]);
    pass(
        "a window dropped on a task's label goes inside its wall; pausing freezes its apps, not closes them",
    );

    // ---- 100. Resume, with "since you left": every line from a record.
    let note = "compare the two models";
    let back_of = |id: u64| {
        let r = me.call(fs::RETURN_POINT, &[id]);
        (r[1], r[3], r[4])
    };
    assert!(
        ask(ep, dk::GROUP_GO, &[g])[0] == dk::OK,
        "going into the group"
    );
    ask(ep, dk::GROUP_SCREEN, &[0]);
    x.keys("{escape}", || screen(ep) == 0, SLOW);
    // Into Work: Thesis is left, with the editor open in it.
    assert!(ask(ep, dk::GROUP_GO, &[1])[0] == dk::OK);
    let (left, roles, _) = back_of(g);
    assert!(
        left != 0 && roles & 1 << role::EDITOR != 0,
        "leaving kept no return point with the editor in it"
    );
    // The person's own note.
    assert!(ask(ep, dk::GROUP_SCREEN, &[g])[0] == dk::OK);
    assert!(
        x.keys("w", || screen(ep) == 7, SLOW),
        "W did not open the note"
    );
    let mut typed = [0u8; 64];
    typed[..note.len()].copy_from_slice(note.as_bytes());
    typed[note.len()..note.len() + 7].copy_from_slice(b"{enter}");
    let spec = core::str::from_utf8(&typed[..note.len() + 7]).unwrap();
    assert!(
        x.keys(spec, || back_of(g).2 == note.len() as u64, SLOW),
        "the note was not kept"
    );
    me.call(fs::RETURN_POINT, &[g]);
    assert!(
        &me.buf()[..note.len()] == note.as_bytes(),
        "the note is not the person's words"
    );
    x.keys("{escape}", || screen(ep) == 0, SLOW);
    // Meanwhile: something in the task's folder changes, and something
    // outside it.
    me.buf()[..12].copy_from_slice(b"new numbers\n");
    assert!(me.call(fs::WRITE, &[results, 0, 12])[0] == st::OK);
    me.buf()[..6].copy_from_slice(b"later\n");
    assert!(me.call(fs::WRITE, &[diary, 0, 6])[0] == st::OK);
    me.call(fs::SAVE, &[]);
    // Back into Thesis: the briefing.
    assert!(ask(ep, dk::GROUP_GO, &[g])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || screen(ep) == 5),
        "no \"since you left\" when coming back"
    );
    crate::say!("screenshot since you left");
    // Nothing invented: every line is a record of the ledger, or a mark.
    let r = me.call(fs::HAPPENINGS, &[0, 64]);
    let ledger = r[1] as usize;
    let mut rec = [[0u64; 5]; 64];
    for (i, e) in rec.iter_mut().enumerate().take(ledger.min(64)) {
        let b = me.buf();
        let o = fs::HAPPENING_BYTES * i;
        for (k, w) in e.iter_mut().enumerate() {
            *w = u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
        }
    }
    let (mut lines, mut saw_results) = (0, false);
    loop {
        let l = ask(ep, dk::BRIEF_LINE, &[lines]);
        if l[0] != dk::OK {
            break;
        }
        let (kind, file, cn, app, what) = (l[1], l[2], l[3], l[4], l[5]);
        let traced = match kind {
            1 => rec[..ledger.min(64)]
                .iter()
                .any(|e| e[0] == cn && e[2] == app && e[3] == file && e[4] == what),
            2 => me.call(fs::MARK, &[file])[1] & 0xff == 2,
            _ => false,
        };
        assert!(traced, "briefing line {lines} has no record behind it");
        assert!(
            file != diary,
            "the briefing told of a file outside the group"
        );
        saw_results |= kind == 1 && file == results && app == fs::INIT_APP;
        lines += 1;
    }
    assert!(saw_results, "the briefing missed the change to results.txt");
    // A line's receipt, then resume.
    assert!(
        x.keys("{enter}", || screen(ep) == 6, SLOW),
        "Enter did not show the receipt"
    );
    // Let the screen catch up before the picture.
    crate::m3::sleep_until(1500, || false);
    crate::say!("screenshot receipt");
    assert!(x.keys("{backspace}", || screen(ep) == 5, SLOW));
    assert!(x.keys("r", || screen(ep) == 0, SLOW), "R did not resume");
    pass(
        "resume: back where you left off with your own note; every \"since you left\" line has its record",
    );
}

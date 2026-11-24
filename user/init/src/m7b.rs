// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7 checks, step two: only what you hand over. An app sees,
//! of a person's documents, only what the person handed it in Alaya's
//! Open and Save windows, and what it made itself; each hand-over lasts
//! as long as the person said (live, one version, until a time, until
//! the app closes).

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, DOCS, st};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask, key_list, return_point_of};

/// An app Alaya does not know: the hostile test app's number.
const STRANGER: u64 = fs::app_id(77);
const SLOW: u64 = 30_000;

impl Files {
    fn put_name(&self, name: &str) {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
    }

    fn grant(&self, app: u64, f: u64, write: bool, kind: u64, arg: u64) -> u64 {
        self.call(fs::GRANT, &[app, f, write as u64, kind, arg])[0]
    }

    /// Does `app` hold `f` now (as GRANTS lists it)?
    fn holds(&self, app: u64, f: u64) -> bool {
        let r = self.call(fs::GRANTS, &[app]);
        let b = self.buf();
        r[0] == st::OK
            && (0..r[1] as usize).any(|i| {
                let o = fs::GRANT_BYTES * i + 8;
                u64::from_le_bytes(b[o..o + 8].try_into().unwrap()) == f
            })
    }
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let p = x.person;
    let them = x.files_as(p, STRANGER);
    let summary = me.lookup(docs, "summary.txt").expect("summary.txt");
    let numbers = me.lookup(docs, "numbers.txt").expect("numbers.txt");

    // ---- 85. A stranger app gets nothing it was not handed.
    let r = them.call(fs::LIST, &[docs]);
    assert!(
        r[0] == st::OK && r[1] == 0,
        "a stranger app listed documents: {r:?}"
    );
    them.put_name("summary.txt");
    let r = them.call(fs::LOOKUP, &[docs, 11]);
    assert!(
        r[0] == st::NOT_FOUND,
        "a stranger app found summary.txt: {r:?}"
    );
    // Guessing numbers: reading, changing, cutting, looking.
    for f in [summary, numbers] {
        for (label, words) in [
            (fs::READ, [f, 0, 16]),
            (fs::WRITE, [f, 0, 4]),
            (fs::TRUNCATE, [f, 0, 0]),
            (fs::STAT, [f, 0, 0]),
            (fs::MARK, [f, 0, 0]),
        ] {
            let r = them.call(label, &words);
            assert!(
                r[0] == st::NOT_FOUND,
                "a stranger app reached a document with request {label}: {r:?}"
            );
        }
    }
    // History, the timeline, stories, undo and hand-overs are Alaya's.
    for label in [
        fs::HISTORY,
        fs::HAPPENINGS,
        fs::ACTIONS,
        fs::STORY,
        fs::PAST_LIST,
        fs::NAME_OF,
        fs::GRANTS,
        fs::GRANT,
        fs::UNDO,
        fs::RESTORE,
    ] {
        let r = them.call(label, &[summary, 0, 0, 0, 0]);
        assert!(
            r[0] != st::OK,
            "a stranger app was answered request {label}"
        );
    }
    // Removing or renaming what it cannot see.
    them.put_name("summary.txt");
    assert!(them.call(fs::REMOVE, &[docs, 11])[0] != st::OK);
    // What it makes is its own; a result made from a document it was not
    // handed is refused.
    them.put_name("stranger.txt");
    let r = them.call(fs::CREATE, &[docs, 0, 12]);
    assert!(r[0] == st::OK, "a stranger app could not make its own file");
    let own = r[1];
    them.buf()[..5].copy_from_slice(b"mine\n");
    assert!(them.call(fs::WRITE, &[own, 0, 5])[0] == st::OK);
    let r = them.call(fs::LIST, &[docs]);
    assert!(
        r[0] == st::OK && r[1] == 1,
        "a stranger app does not see its own file"
    );
    let v = me.call(fs::STAT, &[summary]);
    let b = them.buf();
    for (i, w) in [summary, v[2], v[4]].iter().enumerate() {
        b[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
    }
    assert!(
        them.call(fs::MADE_FROM, &[own, 1])[0] != st::OK,
        "a stranger app named a source it was not handed"
    );
    pass("an app gets nothing of yours it was not handed, by any route; what it makes is its own");

    // ---- 86. Hand-overs last as long as the person said.
    // One version: the summary as it is now, read only, even after it
    // changes.
    let mut was = [0u8; 64];
    let n = me.read(summary, &mut was).unwrap_or(0).min(64) as usize;
    assert!(me.call(fs::SAVE, &[])[0] == st::OK);
    let cn = me.call(fs::STAT, &[summary])[2];
    assert!(me.grant(STRANGER, summary, false, 1, cn) == st::OK);
    assert!(
        me.grant(STRANGER, summary, true, 1, cn) != st::OK,
        "a pinned version was writable"
    );
    me.buf()[..6].copy_from_slice(b"later\n");
    me.call(fs::WRITE, &[summary, 0, 6]);
    me.call(fs::SAVE, &[]);
    let r = them.call(fs::READ, &[summary, 0, 64]);
    assert!(
        r[0] == st::OK && them.buf()[..n.min(r[1] as usize)] == was[..n.min(r[1] as usize)],
        "a pinned version did not read as it was"
    );
    assert!(them.call(fs::WRITE, &[summary, 0, 4])[0] != st::OK);
    // Until a time that has passed: nothing.
    assert!(me.grant(STRANGER, numbers, false, 2, 1) == st::OK);
    assert!(them.call(fs::READ, &[numbers, 0, 16])[0] == st::NOT_FOUND);
    // Until the app closes.
    assert!(me.grant(STRANGER, numbers, true, 3, 0) == st::OK);
    assert!(them.call(fs::READ, &[numbers, 0, 16])[0] == st::OK);
    assert!(me.call(fs::APP_CLOSED, &[STRANGER])[0] == st::OK);
    assert!(them.call(fs::READ, &[numbers, 0, 16])[0] == st::NOT_FOUND);
    // Taken back: the pinned summary too; its own file stays.
    assert!(me.call(fs::REVOKE, &[STRANGER, summary])[1] == 1);
    assert!(them.call(fs::READ, &[summary, 0, 16])[0] == st::NOT_FOUND);
    assert!(them.call(fs::READ, &[own, 0, 5])[0] == st::OK);
    pass("hand-overs last as long as you said: one version, until a time, until the app closes");

    // ---- 87. Alaya's Open and Save windows.
    let editor = x.window_of(role::EDITOR).expect("the editor's window");
    ask(ep, dk::RAISE, &[editor]);
    let picking = || ask(ep, dk::STATE, &[])[5] >> 62 & 1 != 0;
    let editor_app = fs::app_id(role::EDITOR);
    // The editor has numbers.txt open (step 6c picked it). Escape in the
    // Open window hands over nothing more.
    assert!(x.keys("{ctrl+o}", picking, SLOW), "no Open window");
    assert!(
        x.keys("{escape}", || !picking(), SLOW),
        "Escape did not close it"
    );
    assert!(
        !me.holds(editor_app, summary),
        "Escape handed something over"
    );
    // The Save window: a copy under a new name.
    assert!(x.keys("{ctrl+n}", picking, SLOW), "no Save window");
    let copy = || me.lookup(docs, "Copy.txt");
    let saved = || {
        let mut b = [0u8; 512];
        copy().is_some()
            && return_point_of(&me, &mut b)
                .is_some_and(|(_, _, _, n)| &b[48..48 + n.min(64)] == b"Copy.txt")
    };
    // Select the suggested name and type over it.
    let mut keys = [0u8; 256];
    let typed = name_keys(&mut keys, "Untitled.txt".len(), "Copy.txt");
    assert!(
        x.keys(typed, saved, SLOW),
        "the Save window did not save Copy.txt"
    );
    let c = copy().unwrap();
    assert!(me.holds(editor_app, c), "the editor does not hold its copy");
    // The copy holds the editor's text (as its return point has it).
    let mut a = [0u8; 512];
    let na = me.read(c, &mut a).unwrap_or(0) as usize;
    let mut rp = [0u8; 512];
    let same = return_point_of(&me, &mut rp).is_some_and(|(_, len, _, n)| {
        len == na && len <= 512 - 48 - n && rp[48 + n..48 + n + len] == a[..len]
    });
    assert!(same, "the copy is not what the editor had open");
    // The row the Open window shows it at (for the screenshot).
    let at = me.place_in_docs(docs, b"Copy.txt");
    assert!(x.keys("{ctrl+o}", picking, SLOW), "no Open window");
    let mut keys = [0u8; 256];
    let rows = key_list(&mut keys, at);
    let rows = &rows[..rows.len() - "{enter}".len()];
    x.keys(rows, || false, 1500);
    crate::say!("screenshot open-window");
    assert!(x.keys("{escape}", || !picking(), SLOW));
    sleep_until(500, || false);
    pass("Alaya's Open and Save windows hand over only what you pick");

    // ---- 88. Settings shows what each app can see, and takes it back.
    let first = || {
        let r = me.call(fs::GRANTS, &[0]);
        let b = me.buf();
        let w = |k: usize| u64::from_le_bytes(b[8 * k..8 * k + 8].try_into().unwrap());
        (r[0] == st::OK && r[1] > 0).then(|| (w(0), w(1), r[1]))
    };
    let (app, file, count) = first().expect("no hand-overs at all");
    x.start_app(role::SETTINGS);
    assert!(
        sleep_until(SLOW, || x.window_of(role::SETTINGS).is_some()),
        "Settings did not open"
    );
    let settings = x.window_of(role::SETTINGS).unwrap();
    ask(ep, dk::RAISE, &[settings]);
    sleep_until(1000, || false);
    let gone = || !me.holds(app, file) && first().is_some_and(|f| f.2 == count - 1);
    let mut ups = [0u8; 256];
    let mut n = 0;
    for _ in 0..(count as usize).min(20) {
        ups[n..n + 4].copy_from_slice(b"{up}");
        n += 4;
    }
    ups[n] = b't';
    let keys = core::str::from_utf8(&ups[..n + 1]).unwrap_or("t");
    assert!(x.keys(keys, gone, SLOW), "Settings did not take it back");
    crate::say!("screenshot permissions");
    pass("Settings lists what each app can see and takes it back");

    // ---- 89. Forget for good, with an hour to change your mind.
    let secret = me.create(docs, "secret.txt").expect("secret.txt");
    me.buf()[..15].copy_from_slice(b"the secret plan");
    assert!(me.call(fs::WRITE, &[secret, 0, 15])[0] == st::OK);
    assert!(me.grant(STRANGER, secret, false, 0, 0) == st::OK);
    let before = me.call(fs::SAVE, &[])[1];
    let waiting = || me.call(fs::FORGETTING, &[])[1];
    let files = x.window_of(role::FILES).expect("the file browser's window");
    ask(ep, dk::RAISE, &[files]);
    // Choose it (the browser looks again every two seconds).
    let at = me.place_in_docs(docs, b"secret.txt");
    let mut keys = [0u8; 256];
    let rows = key_list(&mut keys, at);
    let rows = &rows[..rows.len() - "{enter}".len()];
    x.keys(rows, || false, 2500);
    x.keys("f", || false, 1500);
    crate::say!("screenshot forget");
    assert!(
        x.keys(
            "f",
            || waiting() == 1 && me.lookup(docs, "secret.txt").is_none(),
            SLOW
        ),
        "F twice did not start forgetting secret.txt"
    );
    assert!(
        x.keys(
            "k",
            || waiting() == 0 && me.lookup(docs, "secret.txt") == Some(secret),
            SLOW
        ),
        "K did not keep secret.txt"
    );
    let mut b = [0u8; 15];
    assert!(me.read(secret, &mut b).is_some() && &b == b"the secret plan");
    // Forgotten for real: the first program asks for a grace of one
    // second, so the check need not wait the hour.
    me.put_name("secret.txt");
    assert!(me.call(fs::FORGET_LATER, &[docs, 10, 1])[0] == st::OK);
    assert!(
        sleep_until(SLOW, || waiting() == 0),
        "secret.txt was not forgotten when its time was up"
    );
    let r = me.call(fs::PAST_READ, &[before, secret, 0, 15]);
    assert!(
        r[0] != st::OK,
        "an old version of a forgotten file was read"
    );
    assert!(
        them.call(fs::READ, &[secret, 0, 15])[0] != st::OK && !me.holds(STRANGER, secret),
        "an app kept a forgotten file"
    );
    pass("forget for good: gone at once, an hour to keep it, then no version can be read");
}

/// Backspace `erase` times, then type `name` and Enter.
fn name_keys<'a>(buf: &'a mut [u8; 256], erase: usize, name: &str) -> &'a str {
    let mut n = 0;
    for _ in 0..erase.min(20) {
        buf[n..n + 11].copy_from_slice(b"{backspace}");
        n += 11;
    }
    buf[n..n + name.len()].copy_from_slice(name.as_bytes());
    n += name.len();
    buf[n..n + 7].copy_from_slice(b"{enter}");
    n += 7;
    core::str::from_utf8(&buf[..n]).unwrap_or("")
}

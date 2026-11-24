// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7 checks, step two: only what you hand over. An app sees,
//! of a person's documents, only what the person handed it in Alaya's
//! Open and Save windows, and what it made itself; each hand-over lasts
//! as long as the person said (live, one version, until a time, until
//! the app closes). Search finds names, words and what files used to
//! say, and hands over only the result the person opens.

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

    // ---- 90. Search: names, words, and what files used to say.
    let plants = me.create(docs, "plants.txt").expect("plants.txt");
    let said = b"water the ferns daily\n";
    me.buf()[..said.len()].copy_from_slice(said);
    assert!(me.call(fs::WRITE, &[plants, 0, said.len() as u64])[0] == st::OK);
    me.call(fs::SAVE, &[]);
    assert!(me.call(fs::TRUNCATE, &[plants, 0])[0] == st::OK);
    me.buf()[..9].copy_from_slice(b"all done\n");
    assert!(me.call(fs::WRITE, &[plants, 0, 9])[0] == st::OK);
    let now_cn = me.call(fs::SAVE, &[])[1];
    // The store finds it only in the older version, in either case.
    me.buf()[..5].copy_from_slice(b"FERNS");
    let r = me.call(fs::SEARCH, &[5, 10, 48]);
    let row = |k: usize| u64::from_le_bytes(me.buf()[8 * k..8 * k + 8].try_into().unwrap());
    assert!(
        r[0] == st::OK && r[1] == 1 && row(0) == plants && row(3) != 0 && row(3) < now_cn,
        "search did not find what plants.txt used to say: {r:?}"
    );
    // A forgotten file is never found, and apps cannot search at all.
    me.buf()[..6].copy_from_slice(b"secret");
    let r = me.call(fs::SEARCH, &[6, 10, 48]);
    assert!(r[0] == st::OK && r[1] == 0, "search found a forgotten file");
    them.buf()[..5].copy_from_slice(b"ferns");
    assert!(
        them.call(fs::SEARCH, &[5, 10, 48])[0] != st::OK,
        "an app searched the person's documents"
    );
    // By hand: Alaya key + F, type, Enter. The editor gets that older
    // version, read only, and nothing else.
    let editor = x.window_of(role::EDITOR).expect("the editor's window");
    ask(ep, dk::RAISE, &[editor]);
    let shown = || match ask(ep, dk::STATE, &[])[5] >> 52 & 0x1ff {
        0 => None,
        n => Some(n - 1),
    };
    assert!(
        x.keys("{super+f}", || shown().is_some(), SLOW),
        "no search box"
    );
    assert!(
        x.keys("ferns", || shown() == Some(1), SLOW),
        "the search box did not find plants.txt: {:?}",
        shown()
    );
    crate::say!("screenshot search");
    let opened = || {
        let mut b = [0u8; 512];
        shown().is_none()
            && return_point_of(&me, &mut b).is_some_and(|(_, len, _, n)| {
                &b[48..48 + n.min(14)] == b"Old plants.txt"
                    && b[48 + n..48 + n + len.min(64)].starts_with(b"water the ferns")
            })
    };
    assert!(
        x.keys("{enter}", opened, SLOW),
        "the editor did not open the older plants.txt"
    );
    let r = me.call(fs::GRANTS, &[editor_app]);
    let pinned = (0..r[1] as usize).any(|i| {
        let o = fs::GRANT_BYTES * i;
        let w =
            |k: usize| u64::from_le_bytes(me.buf()[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
        w(1) == plants && w(2) & 0xff == 1 && w(2) >> 8 & 1 == 0
    });
    assert!(
        pinned,
        "the editor was not handed that one version, read only"
    );
    pass("search finds names, words and what files used to say; it hands over only what you open");

    // ---- 91. The private-file guard.
    private_files(x, &me, &them, docs, summary, pass);

    // ---- 92. Backup to the stick, locked, only what changed; forgetting
    // reaches the stick; getting the documents back with the recovery key.
    let r = me.call(fs::STICK, &[]);
    if r[0] != st::OK || r[1] == 0 {
        pass("backup: no stick attached to this machine, so the backup checks were skipped");
        return;
    }
    // The test starts with an empty stick; a leftover one is started again.
    let r = me.call(fs::BACKUP, &[1]);
    assert!(
        r[0] == st::OK && r[1] == 1 && r[2] > 0,
        "the first backup did not run: {r:?}"
    );
    let all = r[2];
    let r = me.call(fs::BACKUP, &[0]);
    assert!(
        r[0] == st::OK && r[1] == 2 && r[2] == 0 && r[3] == 0,
        "a backup with nothing changed wrote something: {r:?}"
    );
    let diary = me.create(docs, "diary.txt").expect("diary.txt");
    let words = b"violet marmalade at noon";
    me.buf()[..words.len()].copy_from_slice(words);
    assert!(me.call(fs::WRITE, &[diary, 0, words.len() as u64])[0] == st::OK);
    me.call(fs::SAVE, &[]);
    let r = me.call(fs::BACKUP, &[0]);
    assert!(
        r[0] == st::OK && r[2] == 1,
        "the backup did not take only what changed: {r:?}"
    );
    let on_stick = |needle: &[u8]| {
        me.buf()[..needle.len()].copy_from_slice(needle);
        me.call(fs::STICK_FIND, &[needle.len() as u64])[1] == 1
    };
    assert!(
        !on_stick(b"violet marmalade"),
        "a document is on the stick in the clear"
    );
    assert!(
        !on_stick(b"diary.txt"),
        "a name is on the stick in the clear"
    );
    // Forgotten: the next backup takes its key off the stick.
    me.put_name("diary.txt");
    assert!(me.call(fs::FORGET_LATER, &[docs, 9, 1])[0] == st::OK);
    assert!(
        sleep_until(SLOW, || me.call(fs::FORGETTING, &[])[1] == 0),
        "diary.txt was not forgotten"
    );
    let r = me.call(fs::BACKUP, &[0]);
    assert!(
        r[0] == st::OK && r[3] >= 1,
        "the backup did not note what is gone: {r:?}"
    );
    // Getting them back: a wrong key opens nothing; the right one makes
    // them again, in a folder of their own.
    let r = me.call(fs::RECOVERY_KEY, &[]);
    assert!(r[0] == st::OK, "no recovery key");
    let mut key = [0u8; 39];
    key.copy_from_slice(&me.buf()[..39]);
    let restore = |k: &[u8; 39]| {
        let b = me.buf();
        b[..64].fill(0);
        b[..39].copy_from_slice(k);
        b[64..78].copy_from_slice(b"From the stick");
        me.call(fs::RESTORE_BACKUP, &[14])
    };
    let mut wrong = key;
    wrong[0] = if wrong[0] == b'A' { b'B' } else { b'A' };
    assert!(
        restore(&wrong)[0] == st::WRONG_KEY,
        "a wrong recovery key opened the stick"
    );
    let r = restore(&key);
    assert!(
        r[0] == st::OK && r[1] + r[2] >= all,
        "the documents did not come back: {r:?} (backed up {all})"
    );
    let into = me
        .lookup(docs, "From the stick")
        .expect("the folder they came back in");
    assert!(r[3] == into);
    let back = me.lookup(into, "plants.txt").expect("plants.txt came back");
    let mut b = [0u8; 9];
    assert!(me.read(back, &mut b).is_some() && &b == b"all done\n");
    assert!(
        me.lookup(into, "diary.txt").is_none(),
        "a forgotten document came back from the stick"
    );
    // By hand: Settings, B backs up, R shows the recovery key.
    let settings = x.window_of(role::SETTINGS).expect("Settings' window");
    ask(ep, dk::RAISE, &[settings]);
    let before = me.call(fs::STICK, &[])[3];
    assert!(
        x.keys("b", || me.call(fs::STICK, &[])[3] == before + 1, SLOW),
        "B in Settings did not back up"
    );
    x.keys("r", || false, 1500);
    crate::say!("screenshot backup");
    x.keys("r", || false, 500);
    pass(
        "backup: locked on the stick, only what changed, forgetting reaches it, and it comes back with the recovery key",
    );
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

/// Check 91: the private-file guard (decision 27, rule B1).
fn private_files(
    x: &mut Session<'_>,
    me: &Files,
    them: &Files,
    docs: u64,
    summary: u64,
    pass: &mut dyn FnMut(&str),
) {
    let ep = x.ep;
    let private = |f: u64| me.call(fs::PRIVATE, &[f, 2])[1] == 1;
    let marked = |app: u64| {
        let r = me.call(fs::MARKS, &[]);
        let b = me.buf();
        (0..r[1] as usize)
            .any(|i| u64::from_le_bytes(b[24 * i..24 * i + 8].try_into().unwrap()) == app)
    };
    // By hand: the file browser, P on summary.txt.
    let files = x.window_of(role::FILES).expect("the file browser's window");
    ask(ep, dk::RAISE, &[files]);
    let at = me.place_in_docs(docs, b"summary.txt");
    let mut keys = [0u8; 256];
    let rows = key_list(&mut keys, at);
    let rows = &rows[..rows.len() - "{enter}".len()];
    x.keys(rows, || false, 2500);
    assert!(
        x.keys("p", || private(summary), SLOW),
        "P did not mark summary.txt private"
    );
    crate::say!("screenshot private");
    // Apps cannot mark or unmark.
    assert!(them.call(fs::PRIVATE, &[summary, 0])[0] != st::OK);
    assert!(them.call(fs::MARKS, &[])[0] != st::OK);
    // Alaya's own programs read it without being marked (rule B1).
    let mut b = [0u8; 8];
    assert!(me.read(summary, &mut b).is_some());
    assert!(!marked(fs::INIT_APP), "the first program was marked");
    // An app handed it, reading it, is marked; until it closes.
    assert!(me.grant(STRANGER, summary, false, 0, 0) == st::OK);
    assert!(!marked(STRANGER));
    assert!(them.call(fs::STAT, &[summary, 0, 0])[0] == st::OK);
    assert!(
        !marked(STRANGER),
        "looking at a private file's size marked an app"
    );
    assert!(them.call(fs::READ, &[summary, 0, 8])[0] == st::OK);
    assert!(
        marked(STRANGER),
        "an app read a private file and was not marked"
    );
    assert!(me.call(fs::APP_CLOSED, &[STRANGER])[0] == st::OK);
    assert!(!marked(STRANGER), "the mark outlived the app");
    me.call(fs::REVOKE, &[STRANGER, summary]);
    // Not private any more.
    assert!(
        x.keys("p", || !private(summary), SLOW),
        "P did not unmark it"
    );
    pass(
        "private files: an app that reads one is marked until it closes; Alaya's own programs never are",
    );
}

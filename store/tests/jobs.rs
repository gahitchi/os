// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7, step 3: jobs with rules (protected outcomes).

use alaya_store::disk::MemDisk;
use alaya_store::jobs::{Kept, Rules, check};
use alaya_store::{Class, Error, FileId, Options, Store, what};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const EDITOR: u64 = 31;
const HELPER: u64 = 40;
const OTHER: u64 = 41;

fn fresh() -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(4096), &MACHINE, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.commit(10).unwrap();
    s
}

fn put(s: &mut Store<MemDisk>, p: u64, app: u64, f: FileId, data: &[u8]) {
    s.truncate(p, app, f, 0).unwrap();
    s.write(p, app, f, 0, data).unwrap();
}

fn read(s: &mut Store<MemDisk>, p: u64, f: FileId) -> Vec<u8> {
    let n = s.stat(p, f).unwrap().size as usize;
    let mut b = vec![0u8; n];
    s.read(p, 99, f, 0, &mut b).unwrap();
    b
}

struct Site {
    folder: FileId,
    index: FileId,
    style: FileId,
    rules_txt: FileId,
    checklist: FileId,
    outside: FileId,
}

/// A website folder: two pages, a file that must stay the same, a
/// checklist; and a file outside it.
fn site(s: &mut Store<MemDisk>) -> Site {
    let docs = s.root(Class::Documents);
    let folder = s.create(1, EDITOR, docs, b"website", true).unwrap();
    let index = s.create(1, EDITOR, folder, b"index.html", false).unwrap();
    put(s, 1, EDITOR, index, b"<h1>Welcome</h1> old news");
    let style = s.create(1, EDITOR, folder, b"style.css", false).unwrap();
    put(s, 1, EDITOR, style, b"h1 { color: green }");
    let rules_txt = s.create(1, EDITOR, folder, b"settings", false).unwrap();
    put(s, 1, EDITOR, rules_txt, b"publish=yes");
    let checklist = s.create(1, EDITOR, docs, b"site checks", false).unwrap();
    put(
        s,
        1,
        EDITOR,
        checklist,
        b"# what the site must have\nhas index.html\nsays index.html welcome\nnot index.html old news\nunder style.css 1000\n",
    );
    let outside = s.create(1, EDITOR, docs, b"diary.txt", false).unwrap();
    put(s, 1, EDITOR, outside, b"dear diary");
    s.commit(20).unwrap();
    Site {
        folder,
        index,
        style,
        rules_txt,
        checklist,
        outside,
    }
}

fn rules(w: &Site, checks: u64) -> Rules {
    Rules {
        folder: w.folder,
        keep_same: vec![w.rules_txt],
        checks,
        checklist: (checks & check::CHECKLIST != 0).then_some(w.checklist),
    }
}

#[test]
fn a_job_changes_only_its_folder_and_is_kept_when_checks_pass() {
    let mut s = fresh();
    let w = site(&mut s);
    let side = s
        .job_start(1, HELPER, b"update the site", &rules(&w, check::ALL), 30)
        .unwrap();
    let job = s.job_of(1, HELPER).unwrap().unwrap();
    assert_eq!(job.side, side);
    // Only one job per app.
    assert_eq!(
        s.job_start(1, HELPER, b"again", &rules(&w, 0), 30),
        Err(Error::Exists)
    );
    // What it may touch.
    assert!(s.job_may(&job, w.index, true).unwrap());
    assert!(s.job_may(&job, w.folder, true).unwrap());
    assert!(s.job_may(&job, w.rules_txt, false).unwrap());
    assert!(!s.job_may(&job, w.rules_txt, true).unwrap());
    assert!(!s.job_may(&job, w.outside, false).unwrap());
    assert!(!s.job_may(&job, w.checklist, false).unwrap());
    assert!(!s.job_may(&job, s.root(Class::Documents), true).unwrap());

    // The work.
    put(
        &mut s,
        side,
        HELPER,
        w.index,
        b"<h1>Welcome</h1> new things",
    );
    let news = s
        .create(side, HELPER, w.folder, b"news.html", false)
        .unwrap();
    put(&mut s, side, HELPER, news, b"fresh");
    assert!(s.job_may(&job, news, true).unwrap());
    s.commit(40).unwrap();
    // The person's files did not change yet.
    assert_eq!(read(&mut s, 1, w.index), b"<h1>Welcome</h1> old news");

    let r = s.job_check(side).unwrap();
    assert!(r.passed(), "{r:?}");
    assert_eq!(
        r.checks
            .iter()
            .filter(|c| c.check == check::CHECKLIST)
            .count(),
        4
    );
    assert!(matches!(
        s.job_keep(side, false, &[], &[]).unwrap(),
        Kept::Kept(_)
    ));
    s.commit(50).unwrap();
    assert_eq!(read(&mut s, 1, w.index), b"<h1>Welcome</h1> new things");
    assert_eq!(s.lookup(1, w.folder, b"news.html").unwrap(), news);
    assert!(s.jobs(1).unwrap().is_empty());
    assert!(s.sides(1).unwrap().is_empty());
    // On record as the helper's work, kept from a job.
    let last = s.history(1).unwrap().last().unwrap().cn;
    let ch = s.changes(1, last).unwrap();
    assert!(!ch.is_empty());
    assert!(
        ch.iter()
            .all(|c| c.app == HELPER && c.what & what::KEPT != 0 && c.what & what::ANYWAY == 0)
    );
}

#[test]
fn failed_checks_hold_the_work_until_the_person_keeps_it_anyway() {
    let mut s = fresh();
    let w = site(&mut s);
    let side = s
        .job_start(1, HELPER, b"tidy", &rules(&w, check::ALL), 30)
        .unwrap();
    // It empties the page, removes the style, and "fixes" the checklist's
    // copy in its own branch (it is outside the folder; and it would not
    // matter: the approved version is read).
    put(&mut s, side, HELPER, w.index, b"");
    s.remove(side, HELPER, w.folder, b"style.css").unwrap();
    put(&mut s, side, HELPER, w.checklist, b"has index.html\n");
    s.commit(40).unwrap();
    let r = s.job_check(side).unwrap();
    assert!(!r.passed());
    let failed = |c: u64| {
        r.checks
            .iter()
            .find(|x| x.check == c && !x.ok)
            .map(|x| x.file)
    };
    assert_eq!(failed(check::NOTHING_LOST), Some(Some(w.style)));
    assert_eq!(failed(check::NONE_EMPTIED), Some(Some(w.index)));
    assert_eq!(failed(check::IN_FOLDER), Some(Some(w.checklist)));
    // The checklist ran from the approved version: "says welcome" fails.
    let says = r
        .checks
        .iter()
        .find(|c| c.check == check::CHECKLIST && c.line == 3)
        .unwrap();
    assert!(!says.ok);
    // Held: nothing reached the person's files.
    assert!(matches!(
        s.job_keep(side, false, &[], &[]).unwrap(),
        Kept::Held(_)
    ));
    s.commit(50).unwrap();
    assert_eq!(read(&mut s, 1, w.index), b"<h1>Welcome</h1> old news");
    assert!(s.lookup(1, w.folder, b"style.css").is_ok());
    // The person keeps it anyway: on record.
    assert!(matches!(
        s.job_keep(side, true, &[], &[]).unwrap(),
        Kept::Kept(_)
    ));
    s.commit(60).unwrap();
    assert_eq!(read(&mut s, 1, w.index), b"");
    let last = s.history(1).unwrap().last().unwrap().cn;
    assert!(
        s.changes(1, last)
            .unwrap()
            .iter()
            .all(|c| c.what & what::ANYWAY != 0)
    );
}

#[test]
fn work_resting_on_an_old_version_needs_a_look() {
    let mut s = fresh();
    let w = site(&mut s);
    let side = s
        .job_start(1, HELPER, b"restyle", &rules(&w, check::NOTHING_LOST), 30)
        .unwrap();
    // The job reads the page and writes the style from it.
    let mut b = [0u8; 64];
    s.read(side, HELPER, w.index, 0, &mut b).unwrap();
    put(&mut s, side, HELPER, w.style, b"h1 { color: blue }");
    s.commit(40).unwrap();
    assert!(s.job_check(side).unwrap().passed());
    // Meanwhile the person changes the page.
    put(&mut s, 1, EDITOR, w.index, b"<h2>Hello</h2>");
    s.commit(50).unwrap();
    let r = s.job_check(side).unwrap();
    assert_eq!(r.stale, vec![w.index]);
    assert!(matches!(
        s.job_keep(side, false, &[], &[]).unwrap(),
        Kept::Held(_)
    ));
}

#[test]
fn a_done_job_changes_nothing_more_and_survives_a_restart() {
    let mut s = fresh();
    let w = site(&mut s);
    let side = s
        .job_start(1, HELPER, b"job", &rules(&w, check::NOTHING_LOST), 30)
        .unwrap();
    s.job_done(side).unwrap();
    let job = s.job(side).unwrap();
    assert!(job.done);
    assert!(!s.job_may(&job, w.index, true).unwrap());
    assert!(s.job_may(&job, w.index, false).unwrap());
    s.commit(40).unwrap();
    let disk = s.into_disk();
    let mut s = Store::open(disk, &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    let job = s.job_of(1, HELPER).unwrap().unwrap();
    assert!(job.done && job.side == side);
    assert_eq!(job.rules.keep_same, vec![w.rules_txt]);
    assert_eq!(job.rules.checks, check::NOTHING_LOST | check::ALWAYS);
    s.job_discard(side).unwrap();
    s.commit(50).unwrap();
    assert!(s.jobs(1).unwrap().is_empty());
    assert!(s.sides(1).unwrap().is_empty());
}

#[test]
fn bad_rules_start_nothing() {
    let mut s = fresh();
    let w = site(&mut s);
    // Must stay unchanged, but outside the folder.
    let mut r = rules(&w, 0);
    r.keep_same = vec![w.outside];
    assert!(s.job_start(1, HELPER, b"x", &r, 30).is_err());
    // A file is not a folder.
    let mut r = rules(&w, 0);
    r.folder = w.index;
    assert!(s.job_start(1, HELPER, b"x", &r, 30).is_err());
    // A checklist check without a checklist.
    let mut r = rules(&w, 0);
    r.checks = check::CHECKLIST;
    assert_eq!(s.job_start(1, HELPER, b"x", &r, 30), Err(Error::BadRequest));
    assert!(s.sides(1).unwrap().is_empty());
}

#[test]
fn two_jobs_and_the_person_at_once_lose_nothing() {
    let mut s = fresh();
    let w = site(&mut s);
    let a = s
        .job_start(1, HELPER, b"a", &rules(&w, check::NOTHING_LOST), 30)
        .unwrap();
    let b = s
        .job_start(1, OTHER, b"b", &rules(&w, check::NOTHING_LOST), 30)
        .unwrap();
    put(&mut s, a, HELPER, w.index, b"<h1>Welcome</h1> from A");
    put(&mut s, b, OTHER, w.style, b"h1 { color: red }");
    // B also touches A's page: a conflict once A is kept.
    put(&mut s, b, OTHER, w.index, b"<h1>Welcome</h1> from B");
    put(&mut s, 1, EDITOR, w.outside, b"dear diary, busy day");
    s.commit(40).unwrap();
    assert!(matches!(
        s.job_keep(a, false, &[], &[]).unwrap(),
        Kept::Kept(_)
    ));
    s.commit(50).unwrap();
    assert_eq!(
        s.job_keep(b, false, &[], &[]).unwrap(),
        Kept::Conflicts(vec![w.index])
    );
    // The person keeps their (A's) page and B's style.
    assert!(matches!(
        s.job_keep(b, false, &[], &[w.index]).unwrap(),
        Kept::Kept(_)
    ));
    s.commit(60).unwrap();
    assert_eq!(read(&mut s, 1, w.index), b"<h1>Welcome</h1> from A");
    assert_eq!(read(&mut s, 1, w.style), b"h1 { color: red }");
    assert_eq!(read(&mut s, 1, w.outside), b"dear diary, busy day");
}

#[test]
fn a_rehearsal_shows_what_was_read_and_changed_then_leaves_nothing() {
    // Milestone 8: a job on the whole documents folder, traced, then
    // thrown away.
    let mut s = fresh();
    let w = site(&mut s);
    let docs = s.root(Class::Documents);
    let before = read(&mut s, 1, w.outside);
    let side = s
        .job_start(
            1,
            HELPER,
            b"rehearsal",
            &Rules {
                folder: docs,
                keep_same: vec![],
                checks: 0,
                checklist: None,
            },
            30,
        )
        .unwrap();
    let mut b = [0u8; 4];
    s.read(side, HELPER, w.outside, 0, &mut b).unwrap();
    put(&mut s, side, HELPER, w.index, b"rehearsed");
    let made = s.create(side, HELPER, docs, b"new.txt", false).unwrap();
    let (read_, changed) = s.job_trace(side).unwrap();
    assert!(read_.contains(&w.outside));
    assert!(
        changed
            .iter()
            .any(|(f, x)| *f == w.index && x & what::WROTE != 0)
    );
    assert!(
        changed
            .iter()
            .any(|(f, x)| *f == made && x & what::CREATED != 0)
    );
    s.job_discard(side).unwrap();
    s.commit(40).unwrap();
    assert_eq!(read(&mut s, 1, w.outside), before);
    assert_eq!(read(&mut s, 1, w.index), b"<h1>Welcome</h1> old news");
    assert!(s.lookup(1, docs, b"new.txt").is_err());
    assert!(s.job_trace(side).is_err());
}

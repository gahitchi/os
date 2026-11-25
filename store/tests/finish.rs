// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7a, step 3: Finish, the work receipt, and its one undo.

use alaya_store::disk::MemDisk;
use alaya_store::finish::Fate;
use alaya_store::grants::{Access, Lasts};
use alaya_store::made::Kind;
use alaya_store::{Class, Error, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];

const DESKTOP: u64 = 1;
const EDITOR: u64 = 0x101;
const CHART: u64 = 0x103;

fn names(app: u64) -> &'static str {
    match app {
        EDITOR => "the text editor",
        CHART => "the chart maker",
        _ => "another app",
    }
}

fn read_all(s: &mut Store<MemDisk>, f: alaya_store::FileId) -> String {
    let n = s.stat(1, f).unwrap().size as usize;
    let mut b = vec![0; n];
    s.read(1, DESKTOP, f, 0, &mut b).unwrap();
    String::from_utf8(b).unwrap()
}

#[test]
fn finish_keeps_archives_hands_back_and_undoes() {
    let mut s = Store::format(MemDisk::new(4096), &MACHINE, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.commit(10).unwrap();
    let docs = s.root(Class::Documents);
    let results = s.create(1, EDITOR, docs, b"results.txt", false).unwrap();
    s.write(1, EDITOR, results, 0, b"42 ferns").unwrap();
    let scratch = s.create(1, EDITOR, docs, b"scratch notes", false).unwrap();
    let draft = s.create(1, EDITOR, docs, b"old.tmp", false).unwrap();
    let shared = s.create(1, EDITOR, docs, b"plan.tmp", false).unwrap();
    s.commit(20).unwrap();
    let g = s.group_new(1, b"Thesis", 0, 20).unwrap();
    let other = s.group_new(1, b"Work", 0, 20).unwrap();
    for f in [results, scratch, draft, shared] {
        s.group_add(1, g, f).unwrap();
    }
    s.group_add(1, other, shared).unwrap();
    s.grant(1, CHART, results, false, Lasts::Live).unwrap();
    s.commit(30).unwrap();

    // The plan: keep, archive (suggested by name), elsewhere, hand back.
    let plan = s.finish_plan(1, g).unwrap();
    let fate = |f| plan.iter().find(|l| l.file == f).unwrap().fate;
    assert_eq!(fate(results), Fate::Keep);
    assert_eq!(fate(scratch), Fate::Archive);
    assert_eq!(fate(draft), Fate::Archive);
    assert_eq!(
        fate(shared),
        Fate::Elsewhere,
        "a file used elsewhere is never archived"
    );
    assert!(
        plan.iter()
            .any(|l| l.fate == Fate::HandBack && l.other == CHART)
    );
    // Something used elsewhere cannot be archived.
    assert_eq!(
        s.finish(1, DESKTOP, g, &[shared], 40, &names),
        Err(Error::BadRequest)
    );

    // The person keeps "scratch notes" after all: only old.tmp goes.
    let receipt = s.finish(1, DESKTOP, g, &[draft], 40, &names).unwrap();
    s.commit(40).unwrap();
    assert!(s.finished(1, g).unwrap());
    let arch = s.lookup(1, docs, b"Archive").unwrap();
    let into = s.lookup(1, arch, b"Thesis").unwrap();
    assert_eq!(
        s.lookup(1, into, b"old.tmp").unwrap(),
        draft,
        "archived, not deleted"
    );
    assert!(s.lookup(1, docs, b"old.tmp").is_err());
    assert_eq!(s.lookup(1, docs, b"plan.tmp").unwrap(), shared);
    assert_eq!(
        s.access(1, CHART, results, 40).unwrap(),
        Access::No,
        "handed back"
    );

    // The receipt is written from the records.
    let text = read_all(&mut s, receipt);
    assert!(text.starts_with("Work receipt: Thesis"), "{text}");
    assert!(text.contains("- results.txt, version"), "{text}");
    assert!(text.contains("last changed by the text editor"), "{text}");
    assert!(
        text.contains("Archived, not deleted (1), in Archive/Thesis:\n- old.tmp"),
        "{text}"
    );
    assert!(text.contains("- plan.tmp, also in Work"), "{text}");
    assert!(
        text.contains("- the chart maker could read results.txt"),
        "{text}"
    );
    assert!(s.group_files(1, g).unwrap().contains(&receipt));
    assert_eq!(s.mark(1, receipt).unwrap().kind, Kind::UpToDate);
    // Finishing twice is refused.
    assert_eq!(s.finish(1, DESKTOP, g, &[], 41, &names), Err(Error::Exists));

    // A result changes: the receipt tells on itself.
    s.write(1, EDITOR, results, 0, b"43 ferns").unwrap();
    s.commit(50).unwrap();
    assert_eq!(s.mark(1, receipt).unwrap().kind, Kind::NeedsALook);

    // One undo brings it all back.
    s.finish_undo(1, DESKTOP, g).unwrap();
    s.commit(60).unwrap();
    assert!(!s.finished(1, g).unwrap());
    assert_eq!(s.lookup(1, docs, b"old.tmp").unwrap(), draft);
    assert!(s.lookup(1, into, b"old.tmp").is_err());
    assert!(s.access(1, CHART, results, 60).unwrap() != Access::No);
    assert!(s.stat(1, receipt).is_err(), "the receipt is taken away");
    assert!(!s.group_files(1, g).unwrap().contains(&receipt));
    // And it can be finished again.
    assert!(s.finish(1, DESKTOP, g, &[], 70, &names).is_ok());
}

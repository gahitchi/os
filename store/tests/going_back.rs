// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7, step 1: forgetting for good, named versions, side
//! branches, putting files back and the one-undo list.

use std::collections::BTreeMap;

use alaya_store::disk::MemDisk;
use alaya_store::history::{Restored, Undo, act};
use alaya_store::side::Keep;
use alaya_store::{Class, Error, FileId, Options, Store, what};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];

const FILES: u64 = 30;
const EDITOR: u64 = 31;
const DESKTOP: u64 = 32;
const JOB: u64 = 40;

fn fresh(blocks: u64) -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(blocks), &MACHINE, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.commit(10).unwrap();
    s
}

fn clean(s: &mut Store<MemDisk>) -> alaya_store::Report {
    // Reads are recorded too: save them first.
    if s.has_changes() {
        s.commit(0).unwrap();
    }
    let r = s.check().unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
    r
}

fn read(s: &mut Store<MemDisk>, p: u64, f: FileId) -> Vec<u8> {
    let n = s.stat(p, f).unwrap().size as usize;
    let mut b = vec![0u8; n];
    s.read(p, 99, f, 0, &mut b).unwrap();
    b
}

fn put(s: &mut Store<MemDisk>, p: u64, app: u64, f: FileId, data: &[u8]) {
    s.truncate(p, app, f, 0).unwrap();
    s.write(p, app, f, 0, data).unwrap();
}

/// Every document's path and contents (the whole tree).
fn snapshot(s: &mut Store<MemDisk>, p: u64) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut todo = vec![(String::new(), s.root(Class::Documents))];
    while let Some((path, dir)) = todo.pop() {
        for (n, f) in s.list(p, dir).unwrap() {
            let name = format!("{path}/{}", String::from_utf8_lossy(&n));
            if s.stat(p, f).unwrap().is_dir {
                out.insert(name.clone(), None);
                todo.push((name, f));
            } else {
                out.insert(name, Some(read(s, p, f)));
            }
        }
    }
    out
}

#[test]
fn forget_leaves_nothing_readable() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"diary.txt", false).unwrap();
    let keep = s.create(1, EDITOR, docs, b"keep.txt", false).unwrap();
    put(&mut s, 1, EDITOR, keep, b"stays");
    let mut old_lists = Vec::new();
    for i in 0..5u64 {
        put(
            &mut s,
            1,
            EDITOR,
            f,
            format!("secret day {i}").repeat(500).as_bytes(),
        );
        s.commit(100 + i).unwrap();
        old_lists.extend(s.key_list_blocks(1).unwrap());
    }
    let moments = s.history(1).unwrap();
    let old = s.past(1, Class::Documents, moments[2].cn).unwrap();
    assert!(s.past_lookup(&old, docs, b"diary.txt").is_ok());

    s.forget(1, FILES, docs, b"diary.txt").unwrap();
    for i in 0..3 {
        s.commit(200 + i).unwrap();
    }
    // Gone now and in every past moment: no name, no contents.
    assert_eq!(s.lookup(1, docs, b"diary.txt"), Err(Error::NotFound));
    let old = s.past(1, Class::Documents, moments[2].cn).unwrap();
    assert_eq!(
        s.past_lookup(&old, docs, b"diary.txt"),
        Err(Error::NotFound)
    );
    let names: Vec<_> = s
        .past_list(&old, docs)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, vec![b"keep.txt".to_vec()]);
    let mut buf = [0u8; 16];
    assert_eq!(s.past_read(&old, f, 0, &mut buf), Err(Error::Forgotten));
    assert_eq!(read(&mut s, 1, keep), b"stays");
    // Every copy of the key list that held its key is gone from the disk,
    // even for someone with the person's key and a copy of the disk.
    let now: Vec<_> = s.key_list_blocks(1).unwrap();
    for p in old_lists {
        if now.contains(&p) {
            continue;
        }
        assert!(
            s.read_with_person_key(1, &p).is_err(),
            "an old copy of the key list at block {} can still be read",
            p.addr
        );
    }
    let r = clean(&mut s);
    assert!(
        r.forgotten > 0,
        "history still holds its (unreadable) blocks"
    );
    // Still true after opening the disk again.
    let disk = s.into_disk();
    let mut s = Store::open(disk, &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    let old = s.past(1, Class::Documents, moments[2].cn).unwrap();
    assert_eq!(s.past_read(&old, f, 0, &mut buf), Err(Error::Forgotten));
    clean(&mut s);
}

#[test]
fn forgetting_a_folder_forgets_what_is_in_it() {
    let mut s = fresh(2048);
    let docs = s.root(Class::Documents);
    let d = s.create(1, FILES, docs, b"letters", true).unwrap();
    let a = s.create(1, EDITOR, d, b"a.txt", false).unwrap();
    put(&mut s, 1, EDITOR, a, b"dear a");
    s.commit(20).unwrap();
    let cn = s.history(1).unwrap().last().unwrap().cn;
    s.forget(1, FILES, docs, b"letters").unwrap();
    s.commit(30).unwrap();
    let old = s.past(1, Class::Documents, cn).unwrap();
    assert!(s.past_list(&old, docs).unwrap().is_empty());
    let mut buf = [0u8; 8];
    assert_eq!(s.past_read(&old, a, 0, &mut buf), Err(Error::Forgotten));
    clean(&mut s);
}

#[test]
fn named_versions_are_never_thinned() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"book.txt", false).unwrap();
    let day = 24 * 3600;
    let mut cns = Vec::new();
    for i in 0..40u64 {
        put(&mut s, 1, EDITOR, f, format!("chapter {i}").as_bytes());
        cns.push(s.commit(i * 600).unwrap());
    }
    s.name_version(1, cns[3], b"first draft").unwrap();
    s.commit(40 * 600).unwrap();
    s.thin(60 * day).unwrap();
    s.commit(60 * day).unwrap();
    let left: Vec<u64> = s.history(1).unwrap().iter().map(|m| m.cn).collect();
    assert!(left.contains(&cns[3]), "the named version was thinned");
    assert!(!left.contains(&cns[4]), "thinning did not run");
    let named = s.named_versions(1).unwrap();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].1, b"first draft");
    let old = s.past(1, Class::Documents, cns[3]).unwrap();
    let mut b = [0u8; 9];
    s.past_read(&old, f, 0, &mut b).unwrap();
    assert_eq!(&b, b"chapter 3");
    s.unname_version(1, cns[3]).unwrap();
    s.commit(61 * day).unwrap();
    s.thin(62 * day).unwrap();
    s.commit(62 * day).unwrap();
    assert!(!s.history(1).unwrap().iter().any(|m| m.cn == cns[3]));
    clean(&mut s);
}

#[test]
fn a_side_branch_changes_nothing_until_kept() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"site.html", false).unwrap();
    put(&mut s, 1, EDITOR, f, &b"<p>old</p>".repeat(1000));
    let g = s.create(1, EDITOR, docs, b"other.txt", false).unwrap();
    put(&mut s, 1, EDITOR, g, b"untouched");
    s.commit(100).unwrap();
    let before = snapshot(&mut s, 1);
    let used_before = s.used_blocks();

    let side = s.side_open(1, b"update the site", 110).unwrap();
    // The side branch sees the same files.
    let sf = s.lookup(side, docs, b"site.html").unwrap();
    assert_eq!(sf, f);
    s.write(side, JOB, f, 0, b"<p>new</p>").unwrap();
    let d = s.create(side, JOB, docs, b"assets", true).unwrap();
    let n = s.create(side, JOB, d, b"logo.svg", false).unwrap();
    s.write(side, JOB, n, 0, b"<svg/>").unwrap();
    s.remove(side, JOB, docs, b"other.txt").unwrap();
    s.commit(120).unwrap();
    clean(&mut s);
    // The real files did not change.
    assert_eq!(snapshot(&mut s, 1), before);
    let changed: Vec<FileId> = s
        .side_changes(side)
        .unwrap()
        .into_iter()
        .map(|(f, _)| f)
        .collect();
    assert!(changed.contains(&f) && changed.contains(&n) && changed.contains(&g));

    // It survives opening the disk again.
    let disk = s.into_disk();
    let mut s = Store::open(disk, &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    assert_eq!(s.sides(1).unwrap().len(), 1);
    assert_eq!(s.sides(1).unwrap()[0].1.label(), b"update the site");
    clean(&mut s);
    let side_files = snapshot(&mut s, side);
    assert!(side_files.contains_key("/assets/logo.svg"));
    assert!(!side_files.contains_key("/other.txt"));

    // Keep: one commit, then the owner has the side branch's files.
    assert!(matches!(
        s.side_keep(side, JOB, &[], &[]).unwrap(),
        Keep::Kept(_)
    ));
    s.commit(130).unwrap();
    assert_eq!(snapshot(&mut s, 1), side_files);
    assert!(s.sides(1).unwrap().is_empty());
    // Recorded as the job's changes.
    let last = s.history(1).unwrap().last().unwrap().cn;
    let ch = s.changes(1, last).unwrap();
    assert!(ch.iter().all(|c| c.app == JOB && c.what & what::KEPT != 0));
    for _ in 0..3 {
        s.commit(140).unwrap();
    }
    clean(&mut s);
    assert!(
        s.used_blocks() < used_before + 64,
        "{} vs {}",
        s.used_blocks(),
        used_before
    );
}

#[test]
fn throwing_a_side_branch_away_gives_back_its_blocks() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"big.bin", false).unwrap();
    put(&mut s, 1, EDITOR, f, &[7u8; 64 * 1024]);
    s.commit(100).unwrap();
    for _ in 0..3 {
        s.commit(100).unwrap();
    }
    let base = s.used_blocks();
    let side = s.side_open(1, b"try", 100).unwrap();
    s.write(side, JOB, f, 0, &[9u8; 64 * 1024]).unwrap();
    let n = s.create(side, JOB, docs, b"new.bin", false).unwrap();
    s.write(side, JOB, n, 0, &[1u8; 32 * 1024]).unwrap();
    s.commit(110).unwrap();
    assert!(s.used_blocks() > base + 20);
    clean(&mut s);
    s.side_discard(side).unwrap();
    for _ in 0..3 {
        s.commit(120).unwrap();
    }
    clean(&mut s);
    assert!(
        s.used_blocks() <= base + 8,
        "{} vs {}",
        s.used_blocks(),
        base
    );
    assert_eq!(read(&mut s, 1, f), vec![7u8; 64 * 1024]);
    assert_eq!(s.lookup(1, docs, b"new.bin"), Err(Error::NotFound));
}

#[test]
fn both_changed_the_same_file_is_a_conflict() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"plan.txt", false).unwrap();
    let g = s.create(1, EDITOR, docs, b"notes.txt", false).unwrap();
    put(&mut s, 1, EDITOR, f, b"v1");
    put(&mut s, 1, EDITOR, g, b"n1");
    s.commit(100).unwrap();
    let side = s.side_open(1, b"job", 100).unwrap();
    put(&mut s, side, JOB, f, b"job's version");
    put(&mut s, side, JOB, g, b"job's notes");
    put(&mut s, 1, EDITOR, f, b"my version");
    s.commit(110).unwrap();
    // Nothing is done: the person chooses.
    assert_eq!(
        s.side_keep(side, JOB, &[], &[]).unwrap(),
        Keep::Conflicts(vec![f])
    );
    assert_eq!(read(&mut s, 1, g), b"n1");
    // Mine wins for the plan; the job's notes are kept.
    assert!(matches!(
        s.side_keep(side, JOB, &[], &[f]).unwrap(),
        Keep::Kept(_)
    ));
    s.commit(120).unwrap();
    assert_eq!(read(&mut s, 1, f), b"my version");
    assert_eq!(read(&mut s, 1, g), b"job's notes");
    clean(&mut s);

    // And the other way round.
    let side = s.side_open(1, b"job 2", 130).unwrap();
    put(&mut s, side, JOB, f, b"job wins");
    put(&mut s, 1, EDITOR, f, b"mine again");
    s.commit(140).unwrap();
    assert!(matches!(
        s.side_keep(side, JOB, &[f], &[]).unwrap(),
        Keep::Kept(_)
    ));
    s.commit(150).unwrap();
    assert_eq!(read(&mut s, 1, f), b"job wins");
    clean(&mut s);
}

#[test]
fn a_name_the_owner_took_meanwhile_is_a_conflict() {
    let mut s = fresh(2048);
    let docs = s.root(Class::Documents);
    s.commit(100).unwrap();
    let side = s.side_open(1, b"job", 100).unwrap();
    let a = s.create(side, JOB, docs, b"report.txt", false).unwrap();
    s.write(side, JOB, a, 0, b"job").unwrap();
    let b = s.create(1, EDITOR, docs, b"report.txt", false).unwrap();
    s.write(1, EDITOR, b, 0, b"mine").unwrap();
    s.commit(110).unwrap();
    assert_eq!(
        s.side_keep(side, JOB, &[], &[]).unwrap(),
        Keep::Conflicts(vec![a])
    );
    s.side_discard(side).unwrap();
    s.commit(120).unwrap();
    clean(&mut s);
}

#[test]
fn a_side_branch_keeps_its_start_from_thinning() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"a.txt", false).unwrap();
    put(&mut s, 1, EDITOR, f, b"start");
    s.commit(0).unwrap();
    let side = s.side_open(1, b"long job", 0).unwrap();
    put(&mut s, side, JOB, f, b"side");
    s.commit(10).unwrap();
    let day = 24 * 3600;
    for i in 1..60u64 {
        put(&mut s, 1, EDITOR, f, format!("owner {i}").as_bytes());
        s.commit(i * 3 * 3600).unwrap();
    }
    s.thin(90 * day).unwrap();
    for _ in 0..3 {
        s.commit(90 * day).unwrap();
    }
    clean(&mut s);
    assert_eq!(read(&mut s, side, f), b"side");
}

#[test]
fn undo_puts_a_moved_file_back() {
    let mut s = fresh(2048);
    let docs = s.root(Class::Documents);
    let old = s.create(1, FILES, docs, b"Old", true).unwrap();
    let f = s.create(1, EDITOR, docs, b"taxes.txt", false).unwrap();
    put(&mut s, 1, EDITOR, f, b"numbers");
    s.commit(100).unwrap();
    // The person moves it by mistake.
    s.rename(1, FILES, docs, b"taxes.txt", old, b"taxes.txt")
        .unwrap();
    s.record_action(1, FILES, act::FILES, b"moved taxes.txt")
        .unwrap();
    s.commit(110).unwrap();
    assert_eq!(s.lookup(1, docs, b"taxes.txt"), Err(Error::NotFound));
    let a = s.last_action(1).unwrap().unwrap();
    assert_eq!(&a.label[..15], b"moved taxes.txt");
    match s.undo(1, DESKTOP, a.no, false, 120).unwrap() {
        Undo::Done { files, .. } => assert_eq!(files, vec![f]),
        other => panic!("{other:?}"),
    }
    assert_eq!(s.lookup(1, docs, b"taxes.txt"), Ok(f));
    assert_eq!(read(&mut s, 1, f), b"numbers");
    assert_eq!(s.last_action(1).unwrap(), None);
    // The undo is on the list too: undoing it is a redo.
    let u = s.actions(1, 0, 1).unwrap()[0];
    assert_eq!(u.kind, act::UNDO);
    s.undo(1, DESKTOP, u.no, false, 130).unwrap();
    assert_eq!(s.lookup(1, old, b"taxes.txt"), Ok(f));
    assert_eq!(s.last_action(1).unwrap().unwrap().no, a.no);
    clean(&mut s);
}

#[test]
fn undo_asks_when_something_else_changed_it_since() {
    let mut s = fresh(2048);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"list.txt", false).unwrap();
    put(&mut s, 1, EDITOR, f, b"one");
    s.commit(100).unwrap();
    s.rename(1, FILES, docs, b"list.txt", docs, b"todo.txt")
        .unwrap();
    let no = s.record_action(1, FILES, act::FILES, b"renamed").unwrap();
    s.commit(110).unwrap();
    put(&mut s, 1, EDITOR, f, b"one two");
    s.commit(120).unwrap();
    assert_eq!(
        s.undo(1, DESKTOP, no, false, 130).unwrap(),
        Undo::Conflicts(vec![f])
    );
    assert_eq!(s.lookup(1, docs, b"todo.txt"), Ok(f));
    // The person says go ahead anyway.
    assert!(matches!(
        s.undo(1, DESKTOP, no, true, 140).unwrap(),
        Undo::Done { .. }
    ));
    assert_eq!(s.lookup(1, docs, b"list.txt"), Ok(f));
    clean(&mut s);
}

#[test]
fn undo_brings_back_a_deleted_folder_with_its_files() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let d = s.create(1, FILES, docs, b"photos", true).unwrap();
    let mut files = Vec::new();
    for i in 0..5u8 {
        let f = s
            .create(1, EDITOR, d, format!("p{i}.raw").as_bytes(), false)
            .unwrap();
        put(&mut s, 1, EDITOR, f, &vec![i; 9000]);
        files.push(f);
    }
    s.commit(100).unwrap();
    let before = snapshot(&mut s, 1);
    for i in 0..5u8 {
        s.remove(1, FILES, d, format!("p{i}.raw").as_bytes())
            .unwrap();
    }
    s.remove(1, FILES, docs, b"photos").unwrap();
    let no = s
        .record_action(1, FILES, act::FILES, b"deleted photos")
        .unwrap();
    s.commit(110).unwrap();
    assert!(snapshot(&mut s, 1).is_empty());
    assert!(matches!(
        s.undo(1, DESKTOP, no, false, 120).unwrap(),
        Undo::Done { .. }
    ));
    assert_eq!(snapshot(&mut s, 1), before);
    for _ in 0..3 {
        s.commit(130).unwrap();
    }
    clean(&mut s);
}

#[test]
fn restore_refuses_rather_than_overwrite_a_name() {
    let mut s = fresh(2048);
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"a.txt", false).unwrap();
    s.commit(100).unwrap();
    let cn = s.history(1).unwrap().last().unwrap().cn;
    s.remove(1, FILES, docs, b"a.txt").unwrap();
    let g = s.create(1, EDITOR, docs, b"a.txt", false).unwrap();
    s.commit(110).unwrap();
    assert_eq!(
        s.restore(1, FILES, &[f], cn).unwrap(),
        Restored::Blocked(vec![f])
    );
    assert_eq!(s.lookup(1, docs, b"a.txt"), Ok(g));
}

/// A tiny random number source for the walk.
struct Rng(u64);
impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// One undo walked back 1,000 steps without a mismatch (the plan's gate),
/// then forward again with redo.
#[test]
fn undo_walk_a_thousand_steps() {
    let mut s = fresh(32768);
    let docs = s.root(Class::Documents);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut states = vec![snapshot(&mut s, 1)];
    let mut t = 100;
    let names = ["a", "b", "c", "d", "e", "f"];
    for step in 0..1000 {
        // Pick an action the files app or the editor does for the person.
        let dirs: Vec<FileId> = {
            let mut v = vec![docs];
            for (n, f) in s.list(1, docs).unwrap() {
                if n.starts_with(b"dir") {
                    v.push(f);
                }
            }
            v
        };
        let dir = dirs[rng.below(dirs.len() as u64) as usize];
        let list = s.list(1, dir).unwrap();
        let app = if rng.below(2) == 0 { FILES } else { EDITOR };
        let r = rng.below(10);
        let did = if r < 3 || list.is_empty() {
            let name = if dir == docs && rng.below(4) == 0 {
                format!("dir{}", rng.below(3))
            } else {
                format!("{}{}", names[rng.below(6) as usize], rng.below(5))
            };
            match s.create(1, app, dir, name.as_bytes(), name.starts_with("dir")) {
                Ok(f) => {
                    if !name.starts_with("dir") {
                        s.write(
                            1,
                            app,
                            f,
                            0,
                            &vec![step as u8; 1 + rng.below(9000) as usize],
                        )
                        .unwrap();
                    }
                    true
                }
                Err(Error::Exists) => false,
                Err(e) => panic!("{e:?}"),
            }
        } else {
            let (n, f) = &list[rng.below(list.len() as u64) as usize];
            let is_dir = s.stat(1, *f).unwrap().is_dir;
            match r {
                3..=5 if !is_dir => {
                    let off = rng.below(12000);
                    s.write(
                        1,
                        app,
                        *f,
                        off,
                        &vec![step as u8; 1 + rng.below(5000) as usize],
                    )
                    .unwrap();
                    true
                }
                6 if !is_dir => {
                    s.truncate(1, app, *f, rng.below(4000)).unwrap();
                    true
                }
                7 => {
                    let to = dirs[rng.below(dirs.len() as u64) as usize];
                    let nn = format!("{}{}", names[rng.below(6) as usize], rng.below(5));
                    s.rename(1, app, dir, n, to, nn.as_bytes()).is_ok()
                }
                _ => s.remove(1, app, dir, n).is_ok(),
            }
        };
        if !did {
            continue;
        }
        s.record_action(1, app, act::FILES, format!("step {step}").as_bytes())
            .unwrap();
        t += 7;
        s.commit(t).unwrap();
        states.push(snapshot(&mut s, 1));
    }
    let steps = states.len() - 1;
    assert!(steps > 800, "only {steps} actions");
    // Back, one press at a time.
    for i in (0..steps).rev() {
        let a = s.last_action(1).unwrap().expect("an action to undo");
        t += 7;
        match s.undo(1, DESKTOP, a.no, false, t).unwrap() {
            Undo::Done { .. } => {}
            Undo::Conflicts(c) => {
                let ch = s.changes(1, a.cn).unwrap();
                let forced = s.undo(1, DESKTOP, a.no, true, t);
                panic!(
                    "step {i}: conflicts {c:?}; action {a:?} changes {ch:?}; forced: {forced:?}"
                );
            }
        }
        assert_eq!(
            snapshot(&mut s, 1),
            states[i],
            "mismatch going back to step {i}"
        );
    }
    assert_eq!(s.last_action(1).unwrap(), None);
    clean(&mut s);
    // And forward again: redo is undoing the undos, newest first.
    let undos: Vec<u64> = s
        .actions(1, 0, steps)
        .unwrap()
        .iter()
        .map(|a| a.no)
        .collect();
    for (i, no) in undos.into_iter().enumerate() {
        t += 7;
        assert!(matches!(
            s.undo(1, DESKTOP, no, false, t).unwrap(),
            Undo::Done { .. }
        ));
        assert_eq!(
            snapshot(&mut s, 1),
            states[i + 1],
            "mismatch redoing step {}",
            i + 1
        );
    }
    clean(&mut s);
}

#[test]
fn what_happened_and_stories() {
    let mut s = fresh(2048);
    let docs = s.root(Class::Documents);
    let n = s.create(1, EDITOR, docs, b"numbers.txt", false).unwrap();
    put(&mut s, 1, EDITOR, n, b"1 2 3");
    s.commit(100).unwrap();
    let c = s.create(1, 50, docs, b"chart.ppm", false).unwrap();
    let mut b = [0u8; 5];
    s.read(1, 50, n, 0, &mut b).unwrap();
    s.write(1, 50, c, 0, b"P6").unwrap();
    let st = s.stat(1, n).unwrap();
    s.made_from(
        1,
        50,
        c,
        &[alaya_store::made::Source {
            file: n,
            version: st.version,
            edits: st.edits,
        }],
    )
    .unwrap();
    s.commit(110).unwrap();
    s.rename(1, FILES, docs, b"numbers.txt", docs, b"data.txt")
        .unwrap();
    s.commit(120).unwrap();
    let h = s.happenings(1, 0, 10).unwrap();
    assert_eq!(h[0].changes[0].app, FILES);
    assert_eq!(h[0].changes[0].what, what::RENAMED);
    assert_eq!(h[0].time, 120);
    assert!(h.len() >= 3);
    let older = s.happenings(1, h[0].cn, 1).unwrap();
    assert_eq!(older[0].cn, h[1].cn);
    assert_eq!(s.name_of(1, n, None).unwrap().unwrap().0, b"data.txt");
    assert_eq!(
        s.name_of(1, n, Some(h[1].cn)).unwrap().unwrap().0,
        b"numbers.txt"
    );
    let story = s.story(1, n, 10).unwrap();
    assert_eq!(story.made_by.unwrap().0, EDITOR);
    assert_eq!(story.used_by, vec![c]);
    assert_eq!(story.changes.len(), 2);
    let story = s.story(1, c, 10).unwrap();
    assert_eq!(story.made_by.unwrap().0, 50);
}

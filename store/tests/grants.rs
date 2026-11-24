// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7, step 2: grants with time.

use alaya_store::disk::MemDisk;
use alaya_store::grants::{Access, Lasts};
use alaya_store::{Class, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const BOB: [u8; 32] = [2; 32];

const DESKTOP: u64 = 1;
const EDITOR: u64 = 0x101;
const CHART: u64 = 0x103;

fn fresh() -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(4096), &MACHINE, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.add_person(2, &BOB).unwrap();
    s.commit(10).unwrap();
    s
}

#[test]
fn nothing_is_handed_over_until_the_person_hands_it() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"notes.txt", false).unwrap();
    assert_eq!(s.access(1, EDITOR, f, 20).unwrap(), Access::No);
    s.grant(1, EDITOR, f, false, Lasts::Live).unwrap();
    assert_eq!(s.access(1, EDITOR, f, 20).unwrap(), Access::Current(false));
    // Another app, and the same app for another person, still see nothing.
    assert_eq!(s.access(1, CHART, f, 20).unwrap(), Access::No);
    assert_eq!(s.access(2, EDITOR, f, 20).unwrap(), Access::No);
    // Taken back.
    assert!(s.revoke(1, EDITOR, f).unwrap());
    assert_eq!(s.access(1, EDITOR, f, 20).unwrap(), Access::No);
}

#[test]
fn grants_end_on_time_and_when_the_app_closes() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let a = s.create(1, DESKTOP, docs, b"a.txt", false).unwrap();
    let b = s.create(1, DESKTOP, docs, b"b.txt", false).unwrap();
    s.grant(1, EDITOR, a, true, Lasts::Until(100)).unwrap();
    s.grant(1, EDITOR, b, true, Lasts::UntilClosed).unwrap();
    assert_eq!(s.access(1, EDITOR, a, 99).unwrap(), Access::Current(true));
    assert_eq!(s.access(1, EDITOR, a, 100).unwrap(), Access::No);
    assert_eq!(s.app_closed(1, EDITOR).unwrap(), 1);
    assert_eq!(s.access(1, EDITOR, b, 20).unwrap(), Access::No);
}

#[test]
fn a_pinned_grant_is_one_version_and_reads_only() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"numbers.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"one").unwrap();
    let cn = s.commit(20).unwrap();
    assert!(s.grant(1, CHART, f, true, Lasts::Pinned(cn)).is_err());
    s.grant(1, CHART, f, false, Lasts::Pinned(cn)).unwrap();
    assert_eq!(s.access(1, CHART, f, 30).unwrap(), Access::Pinned(cn));
}

#[test]
fn an_apps_own_files_stay_its_own_and_survive_a_restart() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();
    s.own_file(1, CHART, f).unwrap();
    // Handing it over for less does not take the app's own file away.
    s.grant(1, CHART, f, false, Lasts::Until(50)).unwrap();
    assert_eq!(s.access(1, CHART, f, 60).unwrap(), Access::Current(true));
    s.commit(30).unwrap();
    let disk = s.into_disk();
    let mut s = Store::open(disk, &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    assert_eq!(s.access(1, CHART, f, 60).unwrap(), Access::Current(true));
    let g = s.grants(1, None).unwrap();
    assert_eq!(g.len(), 1);
    assert!(g[0].own && g[0].file == f && g[0].app == CHART);
}

#[test]
fn forgetting_a_file_takes_it_back_from_every_app() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"secret.txt", false).unwrap();
    s.grant(1, EDITOR, f, true, Lasts::Live).unwrap();
    s.grant(1, CHART, f, false, Lasts::Live).unwrap();
    s.forget(1, DESKTOP, docs, b"secret.txt").unwrap();
    assert!(s.grants(1, None).unwrap().is_empty());
    let k = docs_file(&mut s);
    s.grant(1, EDITOR, k, true, Lasts::Live).unwrap();
    s.commit(40).unwrap();
    let r = s.check().unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

fn docs_file(s: &mut Store<MemDisk>) -> alaya_store::FileId {
    let docs = s.root(Class::Documents);
    s.create(1, DESKTOP, docs, b"kept.txt", false).unwrap()
}

#[test]
fn forgetting_waits_an_hour_and_can_be_cancelled() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let a = s.create(1, DESKTOP, docs, b"a.txt", false).unwrap();
    s.write(1, DESKTOP, a, 0, b"keep me").unwrap();
    let b = s.create(1, DESKTOP, docs, b"b.txt", false).unwrap();
    s.write(1, DESKTOP, b, 0, b"forget me").unwrap();
    s.grant(1, EDITOR, b, false, Lasts::Live).unwrap();
    s.commit(100).unwrap();
    // Both leave their folder at once.
    s.forget_later(1, DESKTOP, docs, b"a.txt", 3700).unwrap();
    s.forget_later(1, DESKTOP, docs, b"b.txt", 3700).unwrap();
    assert!(s.lookup(1, docs, b"a.txt").is_err() && s.lookup(1, docs, b"b.txt").is_err());
    assert_eq!(s.forgetting(1).unwrap().len(), 2);
    s.commit(200).unwrap();
    // One is kept: it comes back as it was.
    s.keep_forgotten(1, DESKTOP, a).unwrap();
    assert_eq!(s.lookup(1, docs, b"a.txt").unwrap(), a);
    // Not yet due: nothing happens.
    assert_eq!(s.forget_due(1, 3600).unwrap(), 0);
    let cn = s.commit(300).unwrap();
    // Due: the other's key is gone; its old versions cannot be read.
    assert_eq!(s.forget_due(1, 3700).unwrap(), 1);
    assert!(s.forgetting(1).unwrap().is_empty());
    assert!(s.grants(1, None).unwrap().is_empty());
    s.commit(3800).unwrap();
    let v = s.past(1, Class::Documents, cn - 1).unwrap();
    let mut buf = [0u8; 16];
    assert!(s.past_read(&v, b, 0, &mut buf).is_err());
    let mut buf = [0u8; 7];
    s.read(1, DESKTOP, a, 0, &mut buf).unwrap();
    assert_eq!(&buf, b"keep me");
    s.commit(3900).unwrap();
    let r = s.check().unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

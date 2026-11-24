// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7, step 2: search, including what files used to say.

use alaya_store::disk::MemDisk;
use alaya_store::{Class, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const BOB: [u8; 32] = [2; 32];
const DESKTOP: u64 = 1;

fn fresh() -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(4096), &MACHINE, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.add_person(2, &BOB).unwrap();
    s.commit(10).unwrap();
    s
}

#[test]
fn finds_names_then_text_in_either_case() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let a = s
        .create(1, DESKTOP, docs, b"Garden plan.txt", false)
        .unwrap();
    let b = s.create(1, DESKTOP, docs, b"letter.txt", false).unwrap();
    s.write(1, DESKTOP, b, 0, b"Dear Sam,\nthe GARDEN looks great.\nBye")
        .unwrap();
    let sub = s.create(1, DESKTOP, docs, b"old", true).unwrap();
    let c = s.create(1, DESKTOP, sub, b"deep.txt", false).unwrap();
    // The words cross a block edge.
    let mut big = vec![b'x'; 4096 * 3 - 3];
    big.extend_from_slice(b"garden");
    s.write(1, DESKTOP, c, 0, &big).unwrap();
    s.commit(20).unwrap();
    let r = s.search(1, b"garden", 10, 0).unwrap();
    assert_eq!(r.len(), 3, "{r:?}");
    assert_eq!((r[0].file, r[0].at), (a, None));
    let l = r.iter().find(|f| f.file == b).unwrap();
    assert_eq!(l.at, Some(14));
    assert_eq!(l.line, b"the GARDEN looks great.");
    let d = r.iter().find(|f| f.file == c).unwrap();
    assert_eq!((d.at, d.dir), (Some(4096 * 3 - 3), sub));
    // Another person finds nothing of it.
    assert!(s.search(2, b"garden", 10, 8).unwrap().is_empty());
    // Search notes no reads.
    assert!(s.reads(1, s.last_commit()).unwrap().is_empty());
}

#[test]
fn remembers_what_a_file_used_to_say() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"notes.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"call the plumber").unwrap();
    let then = s.commit(20).unwrap();
    s.truncate(1, DESKTOP, f, 0).unwrap();
    s.write(1, DESKTOP, f, 0, b"all done").unwrap();
    let g = s.create(1, DESKTOP, docs, b"gone.txt", false).unwrap();
    s.write(1, DESKTOP, g, 0, b"the plumber's number").unwrap();
    s.commit(30).unwrap();
    s.remove(1, DESKTOP, docs, b"gone.txt").unwrap();
    s.commit(40).unwrap();
    // Without looking back, nothing.
    assert!(s.search(1, b"plumber", 10, 0).unwrap().is_empty());
    let r = s.search(1, b"plumber", 10, 16).unwrap();
    assert_eq!(r.len(), 2, "{r:?}");
    let gone = r.iter().find(|x| x.file == g).unwrap();
    assert!(gone.gone && gone.name == b"gone.txt");
    let old = r.iter().find(|x| x.file == f).unwrap();
    assert_eq!(old.past.unwrap().cn, then);
    assert!(!old.gone && old.line == b"call the plumber");
}

#[test]
fn never_finds_a_forgotten_file() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"secret.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"the hidden words").unwrap();
    s.commit(20).unwrap();
    s.forget(1, DESKTOP, docs, b"secret.txt").unwrap();
    s.commit(30).unwrap();
    assert!(s.search(1, b"hidden", 10, 64).unwrap().is_empty());
    assert!(s.search(1, b"secret", 10, 64).unwrap().is_empty());
}

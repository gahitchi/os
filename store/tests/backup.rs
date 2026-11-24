// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7, step 2: backup to a stick, and restoring on a fresh Alaya.

use alaya_store::backup::{recovery_from, recovery_text};
use alaya_store::disk::{Disk, MemDisk};
use alaya_store::{Class, Error, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const OTHER_MACHINE: [u8; 32] = [9; 32];
const ALICE: [u8; 32] = [1; 32];
const DESKTOP: u64 = 1;

fn fresh(machine: &[u8; 32]) -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(4096), machine, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.commit(10).unwrap();
    s
}

fn read_all(s: &mut Store<MemDisk>, f: alaya_store::FileId) -> Vec<u8> {
    let mut b = vec![0u8; s.stat(1, f).unwrap().size as usize];
    s.read(1, DESKTOP, f, 0, &mut b).unwrap();
    b
}

/// Does the stick hold `needle` anywhere, in the clear?
fn raw_has(stick: &mut MemDisk, needle: &[u8]) -> bool {
    let mut b = [0u8; 4096];
    (0..stick.blocks()).any(|a| {
        stick.read(a, &mut b).unwrap();
        b.windows(needle.len()).any(|w| w == needle)
    })
}

#[test]
fn recovery_keys_read_back_as_typed() {
    let mut s = fresh(&MACHINE);
    let (r, new) = s.recovery_key(1).unwrap();
    assert!(new);
    assert_eq!(s.recovery_key(1).unwrap(), (r, false));
    let t = recovery_text(&r);
    assert_eq!(t.len(), 39);
    assert_eq!(recovery_from(&t), Some(r));
    let lower: Vec<u8> = t
        .iter()
        .map(|c| c.to_ascii_lowercase())
        .filter(|&c| c != b'-')
        .collect();
    assert_eq!(recovery_from(&lower), Some(r));
    assert_eq!(recovery_from(b"ABCD"), None);
    assert_eq!(recovery_from(b"IIII-OOOO"), None);
}

#[test]
fn backs_up_only_what_changed_and_restores_on_a_fresh_alaya() {
    let mut s = fresh(&MACHINE);
    let docs = s.root(Class::Documents);
    let a = s.create(1, DESKTOP, docs, b"diary.txt", false).unwrap();
    s.write(1, DESKTOP, a, 0, b"a quiet monday").unwrap();
    let sub = s.create(1, DESKTOP, docs, b"photos", true).unwrap();
    let big = s.create(1, DESKTOP, sub, b"big.raw", false).unwrap();
    let pic: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
    s.write(1, DESKTOP, big, 0, &pic).unwrap();
    s.commit(20).unwrap();
    let mut stick = MemDisk::new(2048);
    let r1 = s.backup(1, &mut stick).unwrap();
    assert_eq!((r1.run, r1.written, r1.gone), (1, 3, 0));
    s.commit(21).unwrap();
    // Locked: none of it is on the stick in the clear.
    assert!(!raw_has(&mut stick, b"a quiet monday"));
    assert!(!raw_has(&mut stick, b"diary.txt"));
    // Nothing changed: nothing written.
    let r2 = s.backup(1, &mut stick).unwrap();
    assert_eq!((r2.run, r2.written, r2.gone), (2, 0, 0));
    // One change, one rename, one removal.
    s.write(1, DESKTOP, a, 0, b"a busy  monday").unwrap();
    s.rename(1, DESKTOP, sub, b"big.raw", docs, b"moved.raw")
        .unwrap();
    let tmp = s.create(1, DESKTOP, docs, b"tmp.txt", false).unwrap();
    s.write(1, DESKTOP, tmp, 0, b"scratch").unwrap();
    s.commit(30).unwrap();
    s.backup(1, &mut stick).unwrap();
    s.commit(31).unwrap();
    s.remove(1, DESKTOP, docs, b"tmp.txt").unwrap();
    s.commit(40).unwrap();
    let r3 = s.backup(1, &mut stick).unwrap();
    assert_eq!((r3.written, r3.gone), (0, 1));
    let used = r3.used;
    s.commit(41).unwrap();
    let (key, _) = s.recovery_key(1).unwrap();

    // A fresh Alaya on another machine, with the recovery key typed in.
    let mut t = fresh(&OTHER_MACHINE);
    let typed = recovery_text(&key);
    let mut wrong = key;
    wrong[0] ^= 1;
    assert_eq!(
        t.restore_backup(1, DESKTOP, &mut stick, &wrong, b"From backup"),
        Err(Error::WrongKey)
    );
    let done = t
        .restore_backup(
            1,
            DESKTOP,
            &mut stick,
            &recovery_from(&typed).unwrap(),
            b"From backup",
        )
        .unwrap();
    assert_eq!((done.folders, done.documents), (1, 2));
    assert!(used > 0);
    let tdocs = t.root(Class::Documents);
    let d = t.lookup(1, tdocs, b"diary.txt").unwrap();
    assert_eq!(read_all(&mut t, d), b"a busy  monday");
    let m = t.lookup(1, tdocs, b"moved.raw").unwrap();
    assert_eq!(read_all(&mut t, m), pic);
    assert!(t.lookup(1, tdocs, b"tmp.txt").is_err());
    let photos = t.lookup(1, tdocs, b"photos").unwrap();
    assert!(t.list(1, photos).unwrap().is_empty());
    t.commit(50).unwrap();
    let rep = t.check().unwrap();
    assert!(rep.problems.is_empty(), "{:?}", rep.problems);
    // Restoring again goes into a folder of its own.
    let again = t
        .restore_backup(1, DESKTOP, &mut stick, &key, b"From backup")
        .unwrap();
    let into = t.lookup(1, tdocs, b"From backup").unwrap();
    assert_eq!(again.into, into.0);
}

#[test]
fn a_forgotten_file_is_unreadable_on_the_stick_too() {
    let mut s = fresh(&MACHINE);
    let docs = s.root(Class::Documents);
    let keep = s.create(1, DESKTOP, docs, b"keep.txt", false).unwrap();
    s.write(1, DESKTOP, keep, 0, b"kept words").unwrap();
    let f = s.create(1, DESKTOP, docs, b"secret.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"the secret plan").unwrap();
    s.commit(20).unwrap();
    let mut stick = MemDisk::new(1024);
    s.backup(1, &mut stick).unwrap();
    s.commit(21).unwrap();
    // A copy of the stick from before (someone kept one).
    let mut before = MemDisk::new(1024);
    let mut b = [0u8; 4096];
    for a in 0..1024 {
        stick.read(a, &mut b).unwrap();
        before.write(a, &b).unwrap();
    }
    s.forget(1, DESKTOP, docs, b"secret.txt").unwrap();
    s.commit(30).unwrap();
    let r = s.backup(1, &mut stick).unwrap();
    assert_eq!(r.gone, 1);
    s.commit(31).unwrap();
    let (key, _) = s.recovery_key(1).unwrap();
    // The stick no longer holds the forgotten file's key in either table:
    // restoring makes only the other file.
    let mut t = fresh(&OTHER_MACHINE);
    let done = t
        .restore_backup(1, DESKTOP, &mut stick, &key, b"x")
        .unwrap();
    assert_eq!(done.documents, 1);
    let tdocs = t.root(Class::Documents);
    assert!(t.lookup(1, tdocs, b"secret.txt").is_err());
    // Even pointing the newest header at the old records, the old key
    // table is gone: the stick's both tables were written.
    let mut tables = [0u8; 4096];
    let mut old = [0u8; 4096];
    let mut differ = 0;
    for a in 2..66 {
        stick.read(a, &mut tables).unwrap();
        before.read(a, &mut old).unwrap();
        if tables != old {
            differ += 1;
        }
    }
    assert_eq!(differ, 64, "an old key table was left on the stick");
}

#[test]
fn a_run_cut_off_leaves_the_last_one_whole() {
    let mut s = fresh(&MACHINE);
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"a.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"first").unwrap();
    s.commit(20).unwrap();
    let mut stick = MemDisk::new(1024);
    s.backup(1, &mut stick).unwrap();
    s.commit(21).unwrap();
    s.write(1, DESKTOP, f, 0, b"later").unwrap();
    s.create(1, DESKTOP, docs, b"b.txt", false).unwrap();
    s.commit(30).unwrap();
    // The second run reaches its records but never its header: keep a
    // copy of the header blocks from before and put them back.
    let mut heads = [[0u8; 4096]; 2];
    for (a, h) in heads.iter_mut().enumerate() {
        stick.read(a as u64, h).unwrap();
    }
    s.backup(1, &mut stick).unwrap();
    for (a, h) in heads.iter().enumerate() {
        stick.write(a as u64, h).unwrap();
    }
    let (key, _) = s.recovery_key(1).unwrap();
    let mut t = fresh(&OTHER_MACHINE);
    t.restore_backup(1, DESKTOP, &mut stick, &key, b"x")
        .unwrap();
    let tdocs = t.root(Class::Documents);
    let a = t.lookup(1, tdocs, b"a.txt").unwrap();
    assert_eq!(read_all(&mut t, a), b"first");
    // The machine sees the stick missed a run and writes everything again;
    // b.txt, removed meanwhile, does not come back.
    s.remove(1, DESKTOP, docs, b"b.txt").unwrap();
    s.commit(40).unwrap();
    let r = s.backup(1, &mut stick).unwrap();
    assert_eq!((r.run, r.written), (2, 1));
    let mut u = fresh(&OTHER_MACHINE);
    u.restore_backup(1, DESKTOP, &mut stick, &key, b"x")
        .unwrap();
    let udocs = u.root(Class::Documents);
    let a = u.lookup(1, udocs, b"a.txt").unwrap();
    assert_eq!(read_all(&mut u, a), b"later");
    assert!(u.lookup(1, udocs, b"b.txt").is_err());
}

// SPDX-License-Identifier: GPL-3.0-only
//! Files, people, history, attribution and reads.

use alaya_store::disk::MemDisk;
use alaya_store::{Class, Error, Options, Store, what};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const BOB: [u8; 32] = [2; 32];

fn fresh(blocks: u64) -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(blocks), &MACHINE, Options::default()).unwrap();
    s.add_person(1, &ALICE).unwrap();
    s.add_person(2, &BOB).unwrap();
    s.commit(10).unwrap();
    s
}

fn clean(s: &mut Store<MemDisk>) {
    let r = s.check().unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

#[test]
fn files_survive_reopen() {
    let mut s = fresh(2048);
    let docs = s.root(Class::Documents);
    let f = s.create(1, 100, docs, b"notes.txt", false).unwrap();
    let data: Vec<u8> = (0..10_000u32).map(|i| (i * 7) as u8).collect();
    s.write(1, 100, f, 0, &data).unwrap();
    s.write(1, 100, f, 5000, b"hello").unwrap();
    let dir = s.create(1, 100, docs, b"folder", true).unwrap();
    let g = s.create(1, 100, dir, b"inner", false).unwrap();
    s.write(1, 100, g, 0, b"inside").unwrap();
    s.commit(20).unwrap();
    clean(&mut s);

    let disk = s.into_disk();
    let mut s = Store::open(disk, &MACHINE).unwrap();
    assert_eq!(s.lookup(1, docs, b"notes.txt"), Err(Error::Locked));
    s.unlock(1, &ALICE).unwrap();
    s.unlock(2, &BOB).unwrap();
    clean(&mut s);
    let f2 = s.lookup(1, docs, b"notes.txt").unwrap();
    assert_eq!(f2, f);
    let mut buf = vec![0u8; 10_000];
    assert_eq!(s.read(1, 5, f, 0, &mut buf).unwrap(), 10_000);
    let mut want = data.clone();
    want[5000..5005].copy_from_slice(b"hello");
    assert_eq!(buf, want);
    let dir = s.lookup(1, docs, b"folder").unwrap();
    let names: Vec<_> = s
        .list(1, dir)
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert_eq!(names, vec![b"inner".to_vec()]);
    // Bob sees none of it.
    assert_eq!(s.lookup(2, docs, b"notes.txt"), Err(Error::NotFound));
}

#[test]
fn wrong_keys_are_refused() {
    let s = fresh(1024);
    let disk = s.into_disk();
    assert_eq!(
        Store::open(disk.clone(), &[9; 32]).err(),
        Some(Error::WrongKey)
    );
    let mut s = Store::open(disk, &MACHINE).unwrap();
    assert_eq!(s.unlock(1, &BOB), Err(Error::WrongKey));
    s.unlock(1, &ALICE).unwrap();
}

#[test]
fn raw_disk_shows_nothing_readable() {
    let mut s = fresh(1024);
    let docs = s.root(Class::Documents);
    let f = s.create(1, 1, docs, b"secret-name.txt", false).unwrap();
    let text = b"MY VERY PRIVATE TEXT that nobody may read".repeat(100);
    s.write(1, 1, f, 0, &text).unwrap();
    s.commit(1).unwrap();
    let disk = s.into_disk();
    for (_, b) in disk.raw() {
        for needle in [&b"PRIVATE"[..], b"secret-name"] {
            assert!(
                !b.windows(needle.len()).any(|w| w == needle),
                "readable on the disk"
            );
        }
    }
}

#[test]
fn history_attribution_and_reads() {
    let mut s = fresh(4096);
    let docs = s.root(Class::Documents);
    let f = s.create(1, 100, docs, b"budget", false).unwrap();
    s.write(1, 100, f, 0, b"version one").unwrap();
    let g1 = s.commit(1000).unwrap();
    s.write(1, 200, f, 0, b"version TWO").unwrap();
    let g2 = s.commit(2000).unwrap();
    // App 300 reads the second version.
    let mut buf = [0u8; 11];
    s.read(1, 300, f, 0, &mut buf).unwrap();
    let g3 = s.commit(3000).unwrap();

    let moments: Vec<u64> = s.history(1).unwrap().iter().map(|m| m.cn).collect();
    assert!(moments.contains(&g1) && moments.contains(&g2));
    let ch1 = s.changes(1, g1).unwrap();
    assert_eq!(ch1.len(), 1);
    assert_eq!((ch1[0].app, ch1[0].file), (100, f));
    assert_eq!(ch1[0].what, what::CREATED | what::WROTE);
    assert_eq!(s.changes(1, g2).unwrap()[0].app, 200);
    let r = s.reads(1, g3).unwrap();
    assert_eq!(r.len(), 1);
    assert_eq!((r[0].app, r[0].file, r[0].version), (300, f, g2));
    assert_eq!(s.stat(1, f).unwrap().version, g2);

    // Past views are exact and read-only.
    let past = s.past(1, Class::Documents, g1).unwrap();
    let mut b = [0u8; 11];
    s.past_read(&past, f, 0, &mut b).unwrap();
    assert_eq!(&b, b"version one");
    s.remove(1, 1, docs, b"budget").unwrap();
    s.commit(4000).unwrap();
    assert_eq!(s.lookup(1, docs, b"budget"), Err(Error::NotFound));
    let past = s.past(1, Class::Documents, g2).unwrap();
    let f_then = s.past_lookup(&past, docs, b"budget").unwrap();
    s.past_read(&past, f_then, 0, &mut b).unwrap();
    assert_eq!(&b, b"version TWO");
    clean(&mut s);
}

#[test]
fn caches_keep_no_history_and_apps_keep_one_copy() {
    let mut s = fresh(4096);
    let caches = s.root(Class::Caches);
    let c = s.create(1, 1, caches, b"shader-cache", false).unwrap();
    for i in 0..20u8 {
        s.write(1, 1, c, 0, &vec![i; 8192]).unwrap();
        s.commit(i as u64).unwrap();
    }
    assert_eq!(s.history_blocks(), 0, "caches must keep no history");

    let apps = s.root(Class::Apps);
    let a = s.create(1, 1, apps, b"game", false).unwrap();
    s.write(1, 1, a, 0, &vec![1u8; 40_000]).unwrap();
    s.commit(100).unwrap();
    // Before the update: keep a copy.
    s.keep_apps(1).unwrap();
    let before = s.commit(101).unwrap();
    s.write(1, 1, a, 0, &vec![2u8; 40_000]).unwrap();
    s.commit(102).unwrap();
    s.write(1, 1, a, 0, &vec![3u8; 40_000]).unwrap();
    s.commit(103).unwrap();
    let held = s.history_blocks();
    assert!(held >= 10, "the copy from before the update is kept");
    let past = s.past(1, Class::Apps, before).unwrap();
    let mut b = vec![0u8; 40_000];
    s.past_read(&past, a, 0, &mut b).unwrap();
    assert!(b.iter().all(|&x| x == 1));
    clean(&mut s);
    // The update worked: give the copy up.
    s.release_apps(1).unwrap();
    s.commit(104).unwrap();
    assert_eq!(s.history_blocks(), 0);
    for t in 105..108 {
        s.commit(t).unwrap();
    }
    clean(&mut s);
}

#[test]
fn thinning_follows_decision_33() {
    let mut s = fresh(8192);
    let docs = s.root(Class::Documents);
    let f = s.create(1, 1, docs, b"diary", false).unwrap();
    let hour = 3600;
    // A change every 20 minutes for 10 days.
    let mut t = 0;
    for i in 0..(10 * 24 * 3) {
        s.write(1, 1, f, 0, &(i as u64).to_le_bytes()).unwrap();
        s.commit(t).unwrap();
        t += 20 * 60;
    }
    let before = s.history(1).unwrap().len();
    s.thin(t).unwrap();
    s.commit(t).unwrap();
    let h = s.history(1).unwrap();
    let day = 24 * hour;
    let last_day = h.iter().filter(|m| t - m.time < day).count();
    let week = h
        .iter()
        .filter(|m| t - m.time >= day && t - m.time < 7 * day)
        .count();
    let older = h.iter().filter(|m| t - m.time >= 7 * day).count();
    assert!(last_day >= 70, "every moment of the last day: {last_day}");
    assert!(
        (140..=146).contains(&week),
        "about one an hour for the week: {week}"
    );
    assert!(older <= 4, "one a day after a week: {older}");
    assert!(h.len() < before);
    for _ in 0..3 {
        s.commit(t).unwrap();
    }
    clean(&mut s);
}

#[test]
fn full_disk_can_still_delete() {
    let mut s = fresh(1024);
    let docs = s.root(Class::Documents);
    let caches = s.root(Class::Caches);
    let mut n = 0;
    let mut t = 0;
    let err = loop {
        let class_dir = if n % 2 == 0 { docs } else { caches };
        let name = format!("f{n}");
        let f = match s.create(1, 1, class_dir, name.as_bytes(), false) {
            Ok(f) => f,
            Err(e) => break e,
        };
        if let Err(e) = s.write(1, 1, f, 0, &vec![n as u8; 3 * 4096]) {
            break e;
        }
        n += 1;
        if n % 4 == 0 {
            t += 10;
            s.commit(t).unwrap();
        }
    };
    assert_eq!(err, Error::Full);
    s.commit(t).unwrap();
    clean(&mut s);
    // Delete everything, even on a full disk.
    for dir in [docs, caches] {
        for (name, _) in s.list(1, dir).unwrap() {
            s.remove(1, 1, dir, &name).unwrap();
        }
    }
    s.commit(t).unwrap();
    clean(&mut s);
    // Cache space comes back by itself; documents only once history lets
    // go of them (choice B: never the last day).
    for _ in 0..3 {
        s.commit(t).unwrap();
    }
    let free_now = s.blocks() - s.used_blocks();
    assert!(s.make_room(t + 2 * 24 * 3600, 500).unwrap());
    assert!(s.blocks() - s.used_blocks() > free_now + 100);
    clean(&mut s);
}

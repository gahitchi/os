// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7a, step 1: loose groups and protected tasks.

use alaya_store::disk::MemDisk;
use alaya_store::grants::{Access, Lasts};
use alaya_store::{Class, Error, Options, Store};

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
fn a_loose_group_is_only_a_label() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"plan.txt", false).unwrap();
    let g = s.group_new(1, b"University", 0x23686b, 20).unwrap();
    assert!(s.group_add(1, g, f).unwrap());
    assert!(!s.group_add(1, g, f).unwrap(), "added twice");
    // In a loose group, nothing is handed to anyone.
    assert_eq!(s.access(1, EDITOR, f, 20).unwrap(), Access::No);
    // Only a task holds apps.
    assert_eq!(s.open_in_task(1, EDITOR, Some(g)), Err(Error::BadRequest));
    assert_eq!(s.access(1, EDITOR, f, 20).unwrap(), Access::No);
    // A file can be in several groups; the names are the person's.
    let h = s.group_new(1, b"Thesis", 0, 20).unwrap();
    s.group_add(1, h, f).unwrap();
    assert_eq!(s.groups_of(1, f).unwrap(), vec![g, h]);
    assert_eq!(s.group_new(1, b"Thesis", 0, 20), Err(Error::Exists));
    // Another person sees none of it.
    assert!(s.groups(2).unwrap().is_empty());
    // Removing a group leaves its files where they are.
    s.group_remove(1, g).unwrap();
    assert_eq!(s.groups_of(1, f).unwrap(), vec![h]);
    assert!(s.stat(1, f).is_ok());
}

#[test]
fn a_task_is_a_wall() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let dir = s.create(1, DESKTOP, docs, b"Project", true).unwrap();
    let inside = s.create(1, DESKTOP, dir, b"results.txt", false).unwrap();
    let loose = s.create(1, DESKTOP, docs, b"refs.txt", false).unwrap();
    let outside = s.create(1, DESKTOP, docs, b"diary.txt", false).unwrap();
    // Handed over before the task: the editor could read the diary.
    s.grant(1, EDITOR, outside, true, Lasts::Live).unwrap();
    assert_eq!(
        s.access(1, EDITOR, outside, 20).unwrap(),
        Access::Current(true)
    );

    let t = s.group_new(1, b"Project", 0, 20).unwrap();
    s.group_add(1, t, dir).unwrap();
    s.group_add(1, t, loose).unwrap();
    let mut g = s.group(1, t).unwrap();
    g.task = true;
    s.group_set(1, &g).unwrap();
    s.open_in_task(1, EDITOR, Some(t)).unwrap();
    assert_eq!(s.task_of(1, EDITOR).unwrap(), Some(t));

    // Inside the task: its files and what is in its folders.
    assert_eq!(s.access(1, EDITOR, dir, 20).unwrap(), Access::Current(true));
    assert_eq!(
        s.access(1, EDITOR, inside, 20).unwrap(),
        Access::Current(true)
    );
    assert_eq!(
        s.access(1, EDITOR, loose, 20).unwrap(),
        Access::Current(true)
    );
    // Outside it: nothing, even what was handed over before.
    assert_eq!(s.access(1, EDITOR, outside, 20).unwrap(), Access::No);
    // An app not opened in the task is not affected.
    assert_eq!(s.access(1, CHART, inside, 20).unwrap(), Access::No);

    // What it makes in the task stays in its reach.
    let made = s.create(1, EDITOR, docs, b"draft.txt", false).unwrap();
    s.own_file(1, EDITOR, made).unwrap();
    assert_eq!(
        s.access(1, EDITOR, made, 20).unwrap(),
        Access::Current(true)
    );
    assert!(s.group_files(1, t).unwrap().contains(&made));

    // Taking a file out of the task takes it out of reach.
    assert!(s.group_take(1, t, loose).unwrap());
    assert_eq!(s.access(1, EDITOR, loose, 20).unwrap(), Access::No);

    // When it stops being a task, the app is let go; grants decide again.
    g.task = false;
    s.group_set(1, &g).unwrap();
    assert_eq!(s.task_of(1, EDITOR).unwrap(), None);
    assert_eq!(
        s.access(1, EDITOR, outside, 20).unwrap(),
        Access::Current(true)
    );
    assert_eq!(s.access(1, EDITOR, inside, 20).unwrap(), Access::No);
}

#[test]
fn groups_survive_a_restart_and_only_tasks_pause() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"a.txt", false).unwrap();
    let g = s.group_new(1, b"Home", 0x9a5b00, 30).unwrap();
    s.group_add(1, g, f).unwrap();
    let mut grp = s.group(1, g).unwrap();
    grp.paused = true;
    assert_eq!(s.group_set(1, &grp), Err(Error::BadRequest));
    grp.task = true;
    grp.left = 99;
    s.group_set(1, &grp).unwrap();
    s.open_in_task(1, EDITOR, Some(g)).unwrap();
    s.commit(40).unwrap();
    let disk = s.into_disk();
    let mut s = Store::open(disk, &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    let back = s.group(1, g).unwrap();
    assert_eq!(back.name, b"Home");
    assert_eq!(back.color, 0x9a5b00);
    assert!(back.task && back.paused);
    assert_eq!(back.left, 99);
    assert_eq!(back.made, 30);
    assert_eq!(s.group_files(1, g).unwrap(), vec![f]);
    assert_eq!(s.task_apps(1, g).unwrap(), vec![EDITOR]);
}

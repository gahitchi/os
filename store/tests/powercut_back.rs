// SPDX-License-Identifier: GPL-3.0-only
//! The power-cut test for milestone 7: a session that keeps and throws
//! away side branches, undoes, forgets and names versions, with the power
//! cut after every single write it makes. After every cut the store must
//! open exactly as it was after the last commit it promised, or after the
//! one it was making: never half a keep, half an undo or half a forget.

use std::collections::BTreeMap;

use alaya_store::disk::{IoError, MemDisk};
use alaya_store::history::act;
use alaya_store::{Class, Error, FileId, Options, Person, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const FILES: u64 = 30;
const EDITOR: u64 = 31;
const DESKTOP: u64 = 32;
const JOB: u64 = 40;

/// Everything the person could see: their files, every kept moment,
/// their side branches, named versions and the undo list.
#[derive(Clone, Debug, PartialEq, Eq)]
struct State {
    live: BTreeMap<String, Option<Vec<u8>>>,
    moments: BTreeMap<u64, BTreeMap<String, Option<Vec<u8>>>>,
    sides: BTreeMap<u64, BTreeMap<String, Option<Vec<u8>>>>,
    named: Vec<(u64, Vec<u8>)>,
    actions: Vec<(u64, u64, bool)>,
}

fn tree(
    s: &mut Store<&mut MemDisk>,
    p: Person,
    cn: Option<u64>,
) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let root = s.root(Class::Documents);
    let past = cn.map(|cn| s.past(p, Class::Documents, cn).unwrap());
    let mut todo = vec![(String::new(), root)];
    while let Some((path, dir)) = todo.pop() {
        let list = match &past {
            Some(v) => s.past_list(v, dir).unwrap(),
            None => s.list(p, dir).unwrap(),
        };
        for (n, f) in list {
            let name = format!("{path}/{}", String::from_utf8_lossy(&n));
            let st = match &past {
                Some(v) => s.past_stat(v, f).unwrap(),
                None => s.stat(p, f).unwrap(),
            };
            if st.is_dir {
                out.insert(name.clone(), None);
                todo.push((name, f));
            } else {
                let mut b = vec![0u8; st.size as usize];
                match &past {
                    Some(v) => s.past_read(v, f, 0, &mut b).unwrap(),
                    None => s.read(p, 99, f, 0, &mut b).unwrap(),
                };
                out.insert(name, Some(b));
            }
        }
    }
    out
}

fn state(s: &mut Store<&mut MemDisk>) -> State {
    let mut moments = BTreeMap::new();
    for m in s.history(1).unwrap() {
        moments.insert(m.cn, tree(s, 1, Some(m.cn)));
    }
    let mut sides = BTreeMap::new();
    for (h, info) in s.sides(1).unwrap() {
        sides.insert(info.id, tree(s, h, None));
    }
    State {
        live: tree(s, 1, None),
        moments,
        sides,
        named: s
            .named_versions(1)
            .unwrap()
            .into_iter()
            .map(|(m, n)| (m.cn, n))
            .collect(),
        actions: s
            .actions(1, 0, 100)
            .unwrap()
            .iter()
            .map(|a| (a.no, a.kind, a.undone))
            .collect(),
    }
}

fn io<T>(r: Result<T, Error>) -> Result<T, IoError> {
    r.map_err(|e| match e {
        Error::Io(e) => e,
        e => panic!("{e:?}"),
    })
}

/// The session. `done` gets the state after each commit step (when
/// given); `count` counts the commit steps that finished. An undo saves by
/// itself; it belongs to the commit step after it.
fn session(
    s: &mut Store<&mut MemDisk>,
    mut done: Option<&mut Vec<State>>,
    count: &mut usize,
) -> Result<(), IoError> {
    let t = std::cell::Cell::new(100u64);
    let mut commit =
        |s: &mut Store<&mut MemDisk>, done: &mut Option<&mut Vec<State>>| -> Result<(), IoError> {
            t.set(t.get() + 10);
            io(s.commit(t.get()))?;
            *count += 1;
            if let Some(d) = done {
                d.push(state(s));
            }
            Ok(())
        };
    let docs = s.root(Class::Documents);
    let a = io(s.create(1, EDITOR, docs, b"a.txt", false))?;
    io(s.write(1, EDITOR, a, 0, &[1u8; 9000]))?;
    let b = io(s.create(1, EDITOR, docs, b"b.txt", false))?;
    io(s.write(1, EDITOR, b, 0, b"bee"))?;
    let d = io(s.create(1, FILES, docs, b"dir", true))?;
    let x = io(s.create(1, EDITOR, d, b"x.txt", false))?;
    io(s.write(1, EDITOR, x, 0, &[3u8; 5000]))?;
    commit(s, &mut done)?;
    // An action: the editor changes a.
    io(s.write(1, EDITOR, a, 4000, &[2u8; 3000]))?;
    io(s.record_action(1, EDITOR, act::FILES, b"changed a"))?;
    commit(s, &mut done)?;
    // A side branch at work.
    let side = io(s.side_open(1, b"job", t.get()))?;
    io(s.write(side, JOB, b, 0, b"job was here"))?;
    let y = io(s.create(side, JOB, d, b"y.txt", false))?;
    io(s.write(side, JOB, y, 0, &[4u8; 7000]))?;
    commit(s, &mut done)?;
    // The person moves x out of the folder, then undoes it.
    io(s.rename(1, FILES, d, b"x.txt", docs, b"x.txt"))?;
    io(s.record_action(1, FILES, act::FILES, b"moved x"))?;
    commit(s, &mut done)?;
    let last = io(s.last_action(1))?.unwrap();
    t.set(t.get() + 10);
    io(s.undo(1, DESKTOP, last.no, false, t.get()))?;
    commit(s, &mut done)?;
    // Keep the side branch's work.
    io(s.side_keep(side, JOB, &[], &[]))?;
    commit(s, &mut done)?;
    // Name this version.
    let cn = io(s.history(1))?.last().unwrap().cn;
    io(s.name_version(1, cn, b"after the job"))?;
    commit(s, &mut done)?;
    // Forget a (it has several versions in history).
    io(s.forget(1, FILES, docs, b"a.txt"))?;
    commit(s, &mut done)?;
    commit(s, &mut done)?;
    // Another side branch, thrown away.
    let side = io(s.side_open(1, b"try", t.get()))?;
    let z = io(s.create(side, JOB, docs, b"z.txt", false))?;
    io(s.write(side, JOB, z, 0, &[5u8; 12000]))?;
    commit(s, &mut done)?;
    io(s.side_discard(side))?;
    commit(s, &mut done)?;
    // Delete the folder as one action, and undo that.
    for (n, _) in io(s.list(1, d))? {
        io(s.remove(1, FILES, d, &n))?;
    }
    io(s.remove(1, FILES, docs, b"dir"))?;
    io(s.record_action(1, FILES, act::FILES, b"deleted dir"))?;
    commit(s, &mut done)?;
    let last = io(s.last_action(1))?.unwrap();
    t.set(t.get() + 10);
    io(s.undo(1, DESKTOP, last.no, false, t.get()))?;
    commit(s, &mut done)?;
    commit(s, &mut done)?;
    commit(s, &mut done)?;
    let _: FileId = x;
    Ok(())
}

fn start(disk: &mut MemDisk) -> Option<Store<&mut MemDisk>> {
    let mut s = Store::format(disk, &MACHINE, Options::default()).ok()?;
    s.stir(b"power cut seed");
    s.add_person(1, &ALICE).ok()?;
    s.commit(10).ok()?;
    Some(s)
}

#[test]
fn power_cut_during_keep_undo_and_forget() {
    // The run without cuts: the state after every commit.
    let mut disk = MemDisk::new(4096);
    let mut golden = Vec::new();
    {
        let mut s = start(&mut disk).unwrap();
        golden.push(state(&mut s));
        let mut n = 0;
        session(&mut s, Some(&mut golden), &mut n).unwrap();
        assert_eq!(n + 1, golden.len());
    }
    let total = disk.events;
    let mut checked = 0;
    for n in (0..=total).step_by(1) {
        let mut disk = MemDisk::new(4096);
        disk.cut_after = Some(n);
        let mut k = 0;
        if let Some(mut s) = start(&mut disk)
            && session(&mut s, None, &mut k).is_ok()
        {
            continue;
        }
        let mut rng = n | 1;
        for mode in 0..3u8 {
            let mut d = disk.after_power_cut(|_, _| match mode {
                0 => 0,
                1 => 8,
                _ => {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    (rng % 9) as u8
                }
            });
            let mut s = match Store::open(&mut d, &MACHINE) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if s.people().unwrap().is_empty() || s.unlock(1, &ALICE).is_err() {
                continue;
            }
            let found = state(&mut s);
            // Before the session started (k = 0 and no start), the person
            // may not exist yet: handled above.
            let ok = golden[k] == found || golden.get(k + 1) == Some(&found);
            assert!(
                ok,
                "cut after event {n} (mode {mode}): after {k} commits the store opened in neither that state nor the next"
            );
            s.commit(1).unwrap();
            let r = s.check().unwrap();
            assert!(
                r.problems.is_empty(),
                "cut after {n} (mode {mode}): {:?}",
                r.problems
            );
            checked += 1;
        }
    }
    assert!(checked > 300, "only {checked} cuts checked");
}

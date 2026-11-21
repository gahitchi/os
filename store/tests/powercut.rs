// SPDX-License-Identifier: GPL-3.0-only
//! The power-cut test (milestone 5): run a session, cut the power after
//! write 1, then after write 2, and so on for every single write the
//! session makes; each time also with the disk losing, keeping or tearing
//! the writes it had not promised to keep. After every cut the store must
//! open at the last commit it promised, or the one it was making, with
//! every file exactly right, every past moment exactly right, and nothing
//! leaked or damaged.

use std::collections::BTreeMap;

use alaya_store::disk::{Disk, IoError, MemDisk};
use alaya_store::{Class, Error, FileId, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const BOB: [u8; 32] = [2; 32];

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// What every file should hold: (person, class, name) → contents.
type Files = BTreeMap<(u64, u8, String), Vec<u8>>;

#[derive(Clone, Debug)]
enum Op {
    Create(u64, Class, String),
    Write(u64, Class, String, u64, u64, u8),
    Truncate(u64, Class, String, u64),
    Remove(u64, Class, String),
    Rename(u64, Class, String, String),
    Commit,
}

fn session(seed: u64, steps: usize) -> Vec<Op> {
    session_with(seed, steps, 6)
}

fn session_with(seed: u64, steps: usize, files: usize) -> Vec<Op> {
    let mut r = Rng(seed | 1);
    let mut ops = Vec::new();
    let names: Vec<String> = (0..files).map(|i| format!("file-{i}")).collect();
    let names: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    for _ in 0..steps {
        let p = 1 + r.below(2);
        let class = Class::ALL[r.below(3) as usize];
        let name = names[r.below(names.len() as u64) as usize].to_string();
        ops.push(match r.below(12) {
            0..=1 => Op::Create(p, class, name),
            2..=5 => {
                let off = r.below(3 * 4096);
                let len = 1 + r.below(2 * 4096);
                Op::Write(p, class, name, off, len, r.below(250) as u8 + 1)
            }
            6 => Op::Truncate(p, class, name, r.below(5000)),
            7 => Op::Remove(p, class, name),
            8 => {
                let to = names[r.below(names.len() as u64) as usize].to_string();
                Op::Rename(p, class, name, to)
            }
            _ => Op::Commit,
        });
    }
    ops.push(Op::Commit);
    ops
}

fn bytes(len: u64, fill: u8, off: u64) -> Vec<u8> {
    (0..len)
        .map(|i| fill.wrapping_add(((off + i) % 251) as u8))
        .collect()
}

/// Apply one op to the store and, when it worked, to the model.
fn apply<D: Disk>(s: &mut Store<D>, files: &mut Files, op: &Op) -> Result<(), Error> {
    let key = |p: &u64, c: &Class, n: &String| (*p, *c as u8, n.clone());
    match op {
        Op::Create(p, c, n) => match s.create(*p, 9, s.root(*c), n.as_bytes(), false) {
            Ok(_) => {
                files.insert(key(p, c, n), Vec::new());
            }
            Err(Error::Exists) => {}
            Err(e) => return Err(e),
        },
        Op::Write(p, c, n, off, len, fill) => {
            let Ok(f) = s.lookup(*p, s.root(*c), n.as_bytes()) else {
                return Ok(());
            };
            let data = bytes(*len, *fill, *off);
            s.write(*p, 9, f, *off, &data)?;
            let v = files.get_mut(&key(p, c, n)).unwrap();
            let end = (*off + *len) as usize;
            if v.len() < end {
                v.resize(end, 0);
            }
            v[*off as usize..end].copy_from_slice(&data);
        }
        Op::Truncate(p, c, n, size) => {
            let Ok(f) = s.lookup(*p, s.root(*c), n.as_bytes()) else {
                return Ok(());
            };
            s.truncate(*p, 9, f, *size)?;
            files
                .get_mut(&key(p, c, n))
                .unwrap()
                .resize(*size as usize, 0);
        }
        Op::Remove(p, c, n) => match s.remove(*p, 9, s.root(*c), n.as_bytes()) {
            Ok(()) => {
                files.remove(&key(p, c, n));
            }
            Err(Error::NotFound) => {}
            Err(e) => return Err(e),
        },
        Op::Rename(p, c, n, to) => {
            let root = s.root(*c);
            match s.rename(*p, 9, root, n.as_bytes(), root, to.as_bytes()) {
                Ok(()) => {
                    let v = files.remove(&key(p, c, n)).unwrap();
                    files.insert(key(p, c, to), v);
                }
                Err(Error::NotFound | Error::Exists | Error::BadName) => {}
                Err(e) => return Err(e),
            }
        }
        Op::Commit => {
            s.commit(0)?;
        }
    }
    Ok(())
}

fn read_all<D: Disk>(s: &mut Store<D>) -> Files {
    let mut out = Files::new();
    for p in [1, 2] {
        for c in Class::ALL {
            let root = s.root(c);
            for (name, f) in s.list(p, root).unwrap() {
                let size = s.stat(p, f).unwrap().size;
                let mut v = vec![0u8; size as usize];
                s.read(p, 1, f, 0, &mut v).unwrap();
                out.insert((p, c as u8, String::from_utf8(name).unwrap()), v);
            }
        }
    }
    out
}

/// The documents of one person in a past moment.
fn read_past<D: Disk>(s: &mut Store<D>, p: u64, cn: u64) -> BTreeMap<String, Vec<u8>> {
    let past = s.past(p, Class::Documents, cn).unwrap();
    let mut out = BTreeMap::new();
    let root = s.root(Class::Documents);
    for (name, f) in s.past_list(&past, root).unwrap() {
        let size = s.past_stat(&past, f).unwrap().size;
        let mut v = vec![0u8; size as usize];
        s.past_read(&past, f, 0, &mut v).unwrap();
        out.insert(String::from_utf8(name).unwrap(), v);
    }
    out
}

fn docs_of(files: &Files, p: u64) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .filter(|((q, c, _), _)| *q == p && *c == Class::Documents as u8)
        .map(|((_, _, n), v)| (n.clone(), v.clone()))
        .collect()
}

/// What a run up to a cut knew.
struct Run {
    disk: MemDisk,
    /// Files at the last commit that returned, and at the commit being
    /// made when the power went (if one was).
    acked: Option<Files>,
    inflight: Option<Files>,
    /// Documents per kept moment: (person, commit) → files.
    moments: BTreeMap<(u64, u64), BTreeMap<String, Vec<u8>>>,
    finished: bool,
}

fn run(ops: &[Op], cut_after: Option<u64>, copies: alaya_store::Copies) -> Run {
    let mut disk = MemDisk::new(4096);
    disk.cut_after = cut_after;
    let mut r = Run {
        disk: MemDisk::new(0),
        acked: None,
        inflight: None,
        moments: BTreeMap::new(),
        finished: false,
    };
    body(&mut disk, ops, copies, &mut r);
    r.disk = disk;
    r
}

fn body(disk: &mut MemDisk, ops: &[Op], copies: alaya_store::Copies, r: &mut Run) {
    // Formatting is one unit: if the cut came during it, no store is
    // promised yet.
    let Ok(mut s) = Store::format(disk, &MACHINE, Options { copies }) else {
        return;
    };
    let mut files = Files::new();
    assert!(
        s.add_person(1, &ALICE)
            .and_then(|_| s.add_person(2, &BOB))
            .is_ok()
    );
    r.inflight = Some(Files::new());
    let cn = match s.commit(0) {
        Ok(cn) => cn,
        Err(_) => return,
    };
    r.inflight = None;
    for p in [1, 2] {
        r.moments.insert((p, cn), BTreeMap::new());
    }
    r.acked = Some(files.clone());
    for op in ops {
        if matches!(op, Op::Commit) {
            r.inflight = Some(files.clone());
        }
        match apply(&mut s, &mut files, op) {
            Ok(()) => {
                r.inflight = None;
                if matches!(op, Op::Commit) {
                    let cn = s.last_commit();
                    for p in [1, 2] {
                        if s.history(p).unwrap().iter().any(|m| m.cn == cn) {
                            r.moments.insert((p, cn), docs_of(&files, p));
                        }
                    }
                    r.acked = Some(files.clone());
                }
            }
            Err(Error::Io(IoError::PowerCut)) => return,
            Err(e) => panic!("{op:?}: {e:?}"),
        }
    }
    r.finished = true;
}

/// Open the disk after a cut and check everything.
fn verify(r: &Run, disk: MemDisk, what: &str) {
    let mut s = match Store::open(disk, &MACHINE) {
        Ok(s) => s,
        Err(e) => {
            // Only allowed when formatting never finished.
            assert!(
                r.acked.is_none() && r.inflight.is_none(),
                "{what}: open failed: {e:?}"
            );
            return;
        }
    };
    let opened_at = s.last_commit() - 1;
    if s.people().unwrap().is_empty() {
        // The cut came before the people were committed.
        assert!(r.acked.is_none(), "{what}: people lost");
        return;
    }
    s.unlock(1, &ALICE).unwrap();
    s.unlock(2, &BOB).unwrap();
    let found = read_all(&mut s);
    let ok = r.acked.as_ref() == Some(&found) || r.inflight.as_ref() == Some(&found);
    assert!(
        ok,
        "{what}: files are neither the promised commit nor the one being made (opened at {opened_at})"
    );
    // Reading documents is recorded; commit that before checking.
    s.commit(0).unwrap();
    let rep = s.check().unwrap();
    assert!(rep.problems.is_empty(), "{what}: {:?}", rep.problems);
    // Every kept moment shows exactly what it showed when it was made.
    for p in [1, 2] {
        for m in s.history(p).unwrap() {
            let want = match r.moments.get(&(p, m.cn)) {
                Some(w) => w.clone(),
                // The commit being made when the power went.
                None => docs_of(r.inflight.as_ref().expect("unknown moment"), p),
            };
            assert_eq!(read_past(&mut s, p, m.cn), want, "{what}: moment {}", m.cn);
        }
        // No promised moment is lost.
        for (q, cn) in r.moments.keys() {
            if *q == p {
                assert!(
                    s.history(p).unwrap().iter().any(|m| m.cn == *cn),
                    "{what}: moment {cn} lost"
                );
            }
        }
    }
    // And the store keeps working after the cut.
    let root = s.root(Class::Documents);
    let f: FileId = match s.create(1, 1, root, b"after", false) {
        Ok(f) => f,
        Err(Error::Exists) => s.lookup(1, root, b"after").unwrap(),
        Err(e) => panic!("{what}: {e:?}"),
    };
    s.write(1, 1, f, 0, b"still works").unwrap();
    s.commit(1).unwrap();
    s.commit(2).unwrap();
    s.commit(3).unwrap();
    let rep = s.check().unwrap();
    assert!(
        rep.problems.is_empty(),
        "{what} (after): {:?}",
        rep.problems
    );
}

fn every_write(seed: u64, steps: usize, copies: alaya_store::Copies) -> (u64, u64) {
    let ops = session(seed, steps);
    let full = run(&ops, None, copies);
    assert!(full.finished);
    let total = full.disk.events;
    let mut checked = 0;
    for n in 0..=total {
        let r = run(&ops, Some(n), copies);
        if r.finished {
            continue;
        }
        let mut rng = Rng(seed ^ n.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        // 1. Everything not flushed is lost.
        verify(
            &r,
            r.disk.after_power_cut(|_, _| 0),
            &format!("seed {seed} cut {n} lose all"),
        );
        // 2. Everything not flushed made it.
        verify(
            &r,
            r.disk.after_power_cut(|_, _| 8),
            &format!("seed {seed} cut {n} keep all"),
        );
        // 3. Some made it, in any order, some torn.
        verify(
            &r,
            r.disk.after_power_cut(|_, _| match rng.below(4) {
                0 => 0,
                1 => 8,
                _ => rng.below(9) as u8,
            }),
            &format!("seed {seed} cut {n} mixed"),
        );
        checked += 3;
    }
    (total, checked)
}

#[test]
fn power_cut_at_every_write() {
    let mut cuts = 0;
    let mut writes = 0;
    for seed in [1, 2, 3, 4, 5] {
        let (w, c) = every_write(seed, 80, alaya_store::Copies::Map);
        writes += w;
        cuts += c;
    }
    println!("power cuts checked: {cuts} over {writes} writes and flushes");
    assert!(cuts > 2000);
}

#[test]
fn power_cut_without_spare_copies() {
    every_write(11, 40, alaya_store::Copies::None);
}

#[test]
fn power_cut_with_document_copies() {
    every_write(12, 40, alaya_store::Copies::MapAndDocuments);
}

/// Longer sessions with random cut points (more state, deeper trees).
#[test]
fn power_cut_long_sessions() {
    for seed in 20..26 {
        // Many files, so the trees grow several levels deep.
        let ops = session_with(seed, 600, 120);
        let full = run(&ops, None, alaya_store::Copies::Map);
        let mut rng = Rng(seed * 77 + 1);
        for _ in 0..20 {
            let n = rng.below(full.disk.events + 1);
            let r = run(&ops, Some(n), alaya_store::Copies::Map);
            if r.finished {
                continue;
            }
            verify(
                &r,
                r.disk.after_power_cut(|_, _| rng.below(9) as u8),
                &format!("long seed {seed} cut {n}"),
            );
        }
    }
}

/// A bad spot on the newest front page: the store opens at the commit
/// before it, which must still be whole (blocks are only reused two
/// commits after they were given up).
#[test]
fn newest_front_page_damaged() {
    for seed in 30..40 {
        let ops = session(seed, 120);
        let mut disk = MemDisk::new(1536);
        let mut s = Store::format(&mut disk, &MACHINE, Options::default()).unwrap();
        s.add_person(1, &ALICE).unwrap();
        s.add_person(2, &BOB).unwrap();
        s.commit(0).unwrap();
        let mut files = Files::new();
        let mut at = Vec::new();
        for op in &ops {
            apply(&mut s, &mut files, op).unwrap();
            if matches!(op, Op::Commit) {
                at.push((s.last_commit(), files.clone()));
            }
        }
        let last = s.last_commit();
        // One more commit is cut off half way, after writing over every
        // block it was allowed to reuse...
        let root = s.root(Class::Caches);
        let f = s.create(1, 1, root, b"big", false).unwrap();
        // Big enough to wrap around the disk and reuse every free block.
        let room = s.blocks() - s.used_blocks() - s.reserve() - 120;
        s.write(1, 1, f, 0, &vec![0xee; room as usize * 4096])
            .unwrap();
        let events = s.dev.disk.events;
        s.dev.disk.cut_after = Some(events + room - 5);
        assert!(s.commit(0).is_err());
        drop(s);
        let mut disk = disk.after_power_cut(|_, _| 8);
        // ...and the newest whole front page has a bad spot.
        disk.bad.insert(last % 2);
        let mut s = Store::open(&mut disk, &MACHINE).unwrap();
        s.unlock(1, &ALICE).unwrap();
        s.unlock(2, &BOB).unwrap();
        let found = read_all(&mut s);
        let want = at
            .iter()
            .rev()
            .find(|(cn, _)| *cn < last)
            .map(|(_, f)| f.clone());
        // Commits that changed nothing look the same as the one before.
        let prev_state = at
            .iter()
            .rev()
            .find(|(cn, _)| *cn == last - 1)
            .map(|(_, f)| f.clone())
            .or(want);
        assert_eq!(Some(found), prev_state, "seed {seed}");
        s.commit(0).unwrap();
        let rep = s.check().unwrap();
        assert!(rep.problems.is_empty(), "seed {seed}: {:?}", rep.problems);
    }
}

// SPDX-License-Identifier: GPL-3.0-only
//! Host tests: hand-written cases, then long random runs that check the
//! tree's links and the O(1) "last capability" test after every operation.

use std::vec;
use std::vec::Vec;

use super::*;

/// Fake physical memory: `bytes` of RAM starting at address 0.
pub struct Ram {
    pub bytes: Vec<u8>,
}

impl Ram {
    pub fn new(len: usize) -> Ram {
        Ram {
            bytes: vec![0; len],
        }
    }
}

impl Memory for Ram {
    fn read(&self, at: SlotRef) -> Slot {
        let b = &self.bytes[at as usize..at as usize + 64];
        let w = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
        Slot {
            cap: Cap {
                kind: b[0],
                rights: b[1],
                size_bits: b[2],
                flags: b[3],
                _reserved: u32::from_le_bytes(b[4..8].try_into().unwrap()),
                object: w(1),
                a: w(2),
                b: w(3),
            },
            prev: w(4),
            next: w(5),
            first: w(6),
            last: w(7),
        }
    }

    fn write(&mut self, at: SlotRef, s: &Slot) {
        let b = &mut self.bytes[at as usize..at as usize + 64];
        b[0] = s.cap.kind;
        b[1] = s.cap.rights;
        b[2] = s.cap.size_bits;
        b[3] = s.cap.flags;
        b[4..8].copy_from_slice(&s.cap._reserved.to_le_bytes());
        for (i, w) in [
            s.cap.object,
            s.cap.a,
            s.cap.b,
            s.prev,
            s.next,
            s.first,
            s.last,
        ]
        .iter()
        .enumerate()
        {
            b[8 + i * 8..16 + i * 8].copy_from_slice(&w.to_le_bytes());
        }
    }

    fn zero(&mut self, at: u64, len: u64) {
        self.bytes[at as usize..(at + len) as usize].fill(0);
    }
}

/// A test world: a root table of 64 slots at 0x1000 (boot memory), and
/// one boot untyped of 1 MiB at 0x100000 in slot 1.
struct World {
    ram: Ram,
    g: Globals,
}

const ROOT: u64 = 0x1000;
const ROOT_BITS: u8 = 6;
const UT_BASE: u64 = 0x10_0000;
const UT_BITS: u8 = 20;

impl World {
    fn new() -> World {
        let mut ram = Ram::new(0x20_0000);
        init_table(&mut ram, ROOT, ROOT_BITS);
        insert_root(&mut ram, slot(1), Cap::new(Kind::Untyped, UT_BASE, UT_BITS));
        World {
            ram,
            g: Globals::new(),
        }
    }

    fn retype(&mut self, ut: u64, kind: Kind, bits: u8, count: u64, dst: u64) -> u64 {
        self.retype_into(ut, kind, bits, count, ROOT, ROOT_BITS, dst)
    }

    #[allow(clippy::too_many_arguments)]
    fn retype_into(
        &mut self,
        ut: u64,
        kind: Kind,
        bits: u8,
        count: u64,
        table: u64,
        tbits: u8,
        index: u64,
    ) -> u64 {
        let dst = Destination {
            table,
            bits: tbits,
            index,
        };
        match retype(&mut self.ram, ut, kind, bits, count, dst, &mut Unlimited) {
            Ok(Progress::Done(n)) => n,
            other => panic!("retype: {other:?}"),
        }
    }

    fn revoke(&mut self, at: u64) {
        assert_eq!(
            revoke(&mut self.ram, &mut self.g, at, &mut Unlimited),
            Ok(Progress::Done(()))
        );
    }

    fn cap(&self, at: u64) -> Cap {
        self.ram.read(at).cap
    }
}

fn slot(i: u64) -> u64 {
    ROOT + (i << SLOT_BITS)
}

#[test]
fn retype_copy_revoke() {
    let mut w = World::new();
    assert_eq!(w.retype(slot(1), Kind::Endpoint, 0, 1, 2), 1);
    let ep = w.cap(slot(2));
    assert!(ep.is(Kind::Endpoint));
    assert_eq!(ep.object, UT_BASE);
    copy(&mut w.ram, slot(2), slot(3), rights::ALL, None).unwrap();
    copy(&mut w.ram, slot(3), slot(4), rights::READ, Some(7)).unwrap();
    assert_eq!(w.cap(slot(4)).a, 7);
    assert_eq!(w.cap(slot(4)).rights, rights::READ);
    // Read only, so no grant: no further copies.
    assert_eq!(
        copy(&mut w.ram, slot(4), slot(5), rights::ALL, None),
        Err(Error::NoRights)
    );
    // Re-badging is refused.
    copy(&mut w.ram, slot(3), slot(5), rights::ALL, Some(9)).unwrap();
    assert_eq!(
        copy(&mut w.ram, slot(5), slot(6), rights::ALL, Some(10)),
        Err(Error::InvalidArgument)
    );
    w.revoke(slot(3));
    assert!(w.cap(slot(3)).is_live());
    assert!(w.cap(slot(4)).is_empty());
    assert!(w.cap(slot(5)).is_empty());
    assert!(w.cap(slot(2)).is_live());
    w.revoke(slot(1));
    assert!(w.cap(slot(2)).is_empty() && w.cap(slot(3)).is_empty());
    assert_eq!(w.cap(slot(1)).a, 0, "watermark back to zero");
}

#[test]
fn errors() {
    let mut w = World::new();
    assert_eq!(table_slot(ROOT, ROOT_BITS, 0), Err(Error::InvalidHandle));
    assert_eq!(table_slot(ROOT, ROOT_BITS, 64), Err(Error::InvalidHandle));
    assert_eq!(
        copy(&mut w.ram, slot(1), slot(2), rights::ALL, None),
        Err(Error::WrongKind),
        "untyped is never copied"
    );
    assert_eq!(
        copy(&mut w.ram, slot(9), slot(2), rights::ALL, None),
        Err(Error::InvalidHandle)
    );
    w.retype(slot(1), Kind::Notification, 0, 2, 2);
    assert_eq!(
        retype(
            &mut w.ram,
            slot(1),
            Kind::Endpoint,
            0,
            1,
            Destination {
                table: ROOT,
                bits: ROOT_BITS,
                index: 3
            },
            &mut Unlimited
        ),
        Err(Error::SlotInUse)
    );
    assert_eq!(
        retype(
            &mut w.ram,
            slot(1),
            Kind::Page,
            13,
            1,
            Destination {
                table: ROOT,
                bits: ROOT_BITS,
                index: 9
            },
            &mut Unlimited
        ),
        Err(Error::InvalidArgument)
    );
    assert_eq!(
        retype(
            &mut w.ram,
            slot(1),
            Kind::Page,
            21,
            1,
            Destination {
                table: ROOT,
                bits: ROOT_BITS,
                index: 9
            },
            &mut Unlimited
        ),
        Err(Error::NoMemory),
        "a 2 MiB page does not fit in 1 MiB"
    );
    assert_eq!(
        move_cap(&mut w.ram, slot(2), slot(3)),
        Err(Error::SlotInUse)
    );
}

#[test]
fn retype_aligns_and_runs_out() {
    let mut w = World::new();
    w.retype(slot(1), Kind::Endpoint, 0, 1, 2);
    w.retype(slot(1), Kind::Page, 12, 1, 3);
    assert_eq!(w.cap(slot(3)).object, UT_BASE + 0x1000, "pages are aligned");
    // 254 more 4 KiB pages fit; batches of RETYPE_BATCH.
    let mut made = 0;
    let mut next = 4;
    while next < 63 {
        made += w.retype(slot(1), Kind::Page, 12, 63 - next, next);
        next = 4 + made;
    }
    assert_eq!(made, 59);
}

#[test]
fn moved_capabilities_keep_their_children() {
    let mut w = World::new();
    w.retype(slot(1), Kind::Endpoint, 0, 1, 2);
    copy(&mut w.ram, slot(2), slot(3), rights::ALL, None).unwrap();
    copy(&mut w.ram, slot(3), slot(4), rights::ALL, None).unwrap();
    move_cap(&mut w.ram, slot(3), slot(10)).unwrap();
    move_cap(&mut w.ram, slot(1), slot(11)).unwrap();
    w.revoke(slot(10));
    assert!(w.cap(slot(4)).is_empty());
    assert!(w.cap(slot(2)).is_live());
    w.revoke(slot(11));
    assert!(w.cap(slot(2)).is_empty() && w.cap(slot(10)).is_empty());
}

#[test]
fn deleting_a_middle_capability_keeps_its_children_revocable() {
    let mut w = World::new();
    w.retype(slot(1), Kind::Endpoint, 0, 1, 2);
    copy(&mut w.ram, slot(2), slot(3), rights::ALL, None).unwrap();
    copy(&mut w.ram, slot(3), slot(4), rights::ALL, None).unwrap();
    copy(&mut w.ram, slot(3), slot(5), rights::ALL, None).unwrap();
    delete(&mut w.ram, &mut w.g, slot(3)).unwrap();
    assert!(!is_final(&w.ram, slot(2)));
    w.revoke(slot(2));
    assert!(w.cap(slot(4)).is_empty() && w.cap(slot(5)).is_empty());
    assert!(is_final(&w.ram, slot(2)));
}

#[test]
fn unreachable_tables_are_emptied_before_reset() {
    let mut w = World::new();
    // A sub-untyped, a table made from it, and an endpoint from the boot
    // untyped copied into that table.
    w.retype(slot(1), Kind::Untyped, 16, 1, 2);
    w.retype(slot(2), Kind::Table, 4, 1, 3);
    let table = w.cap(slot(3)).object;
    w.retype(slot(1), Kind::Endpoint, 0, 1, 4);
    let inner = table + (5 << SLOT_BITS);
    copy(&mut w.ram, slot(4), inner, rights::ALL, None).unwrap();
    assert!(inspect(&w.ram, slot(4)).1, "the endpoint has a child");
    // Drop the only capability to the table: it goes on the purge list.
    delete(&mut w.ram, &mut w.g, slot(3)).unwrap();
    assert_eq!(w.g.orphans, table);
    assert!(w.ram.read(inner).cap.is_live());
    // Revoking the sub-untyped empties it, then resets.
    w.revoke(slot(2));
    assert_eq!(w.g.orphans, 0);
    assert!(w.ram.read(inner).cap.is_empty());
    assert!(!inspect(&w.ram, slot(4)).1);
    // The memory can be used again, and the new table is empty.
    w.retype(slot(2), Kind::Table, 4, 1, 3);
    assert_eq!(w.cap(slot(3)).object, table);
    assert!(w.ram.read(inner).cap.is_empty());
}

#[test]
fn revoke_is_interruptible_and_restartable() {
    struct Every(u32, u32);
    impl Budget for Every {
        fn step(&mut self) -> bool {
            self.1 += 1;
            self.1.is_multiple_of(self.0)
        }
    }
    let mut w = World::new();
    w.retype(slot(1), Kind::Table, 8, 1, 2);
    let table = w.cap(slot(2)).object;
    w.retype(slot(1), Kind::Endpoint, 0, 1, 3);
    for i in 1..256 {
        let src = if i == 1 {
            slot(3)
        } else {
            table + ((i - 1) << SLOT_BITS)
        };
        copy(&mut w.ram, src, table + (i << SLOT_BITS), rights::ALL, None).unwrap();
    }
    let mut budget = Every(10, 0);
    let mut restarts = 0;
    loop {
        match revoke(&mut w.ram, &mut w.g, slot(1), &mut budget).unwrap() {
            Progress::Done(()) => break,
            Progress::Interrupted => {
                restarts += 1;
                assert!(w.cap(slot(1)).revoking());
                assert_eq!(
                    copy(&mut w.ram, slot(1), slot(9), rights::ALL, None),
                    Err(Error::WrongKind)
                );
                assert_eq!(move_cap(&mut w.ram, slot(1), slot(9)), Err(Error::Busy));
            }
        }
    }
    assert!(restarts > 20, "{restarts}");
    assert!(!w.cap(slot(1)).revoking());
    for i in 1..256 {
        assert!(w.ram.read(table + (i << SLOT_BITS)).cap.is_empty());
    }
}

#[test]
fn retype_zeroing_is_interruptible() {
    struct Always;
    impl Budget for Always {
        fn step(&mut self) -> bool {
            true
        }
    }
    let dst = Destination {
        table: ROOT,
        bits: ROOT_BITS,
        index: 2,
    };
    let run = |w: &mut World, ut, kind, bits, count, index| {
        let mut restarts = 0;
        loop {
            let d = Destination { index, ..dst };
            match retype(&mut w.ram, ut, kind, bits, count, d, &mut Always).unwrap() {
                Progress::Done(n) => return (n, restarts),
                Progress::Interrupted => restarts += 1,
            }
        }
    };
    let mut w = World::new();
    w.ram.bytes[UT_BASE as usize..].fill(0xaa);
    // 16 pages of 4 KiB are one 64 KiB chunk: no interruption.
    assert_eq!(
        run(&mut w, slot(1), Kind::Page, 12, 64, 2),
        (RETYPE_BATCH, 0)
    );
    // New untyped memory is not zeroed when it is made.
    assert_eq!(run(&mut w, slot(1), Kind::Untyped, 19, 1, 20), (1, 0));
    // A 512 KiB table is 8 chunks, so 7 interruptions, and ends up zero.
    assert_eq!(run(&mut w, slot(20), Kind::Table, 13, 1, 21), (1, 7));
    let t = w.cap(slot(21)).object as usize;
    assert!(w.ram.bytes[t + 64..t + (64 << 13)].iter().all(|&b| b == 0));
    assert_eq!(
        retype(
            &mut w.ram,
            slot(1),
            Kind::Page,
            21,
            1,
            Destination { index: 30, ..dst },
            &mut Always
        ),
        Err(Error::NoMemory),
        "a 2 MiB page does not fit"
    );
}

// ---------------------------------------------------------------------------
// Random runs against a brute-force reference.

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Every slot whose capability is live, in the root table and in every
/// table object that exists.
fn all_slots(_w: &World, tables: &[(u64, u8)]) -> Vec<u64> {
    let mut v: Vec<u64> = (1..64).map(slot).collect();
    for &(t, bits) in tables {
        v.extend((1..1u64 << bits).map(|i| t + (i << SLOT_BITS)));
    }
    v
}

/// Check every link of every slot, and that `is_final` agrees with a
/// brute-force search.
pub fn check_links<M: Memory>(m: &M, slots: &[u64]) {
    for &x in slots {
        let s = m.read(x);
        if !s.cap.is_live() {
            assert_eq!(
                (s.prev, s.next, s.first, s.last),
                (0, 0, 0, 0),
                "empty slot {x:#x} is linked"
            );
            continue;
        }
        if s.prev != 0 {
            let p = m.read(target(s.prev));
            if is_up(s.prev) {
                assert_eq!(p.first, x);
            } else {
                assert_eq!(p.next, x);
            }
        }
        if s.next != 0 {
            let n = m.read(target(s.next));
            if is_up(s.next) {
                assert_eq!(n.last, x);
            } else {
                assert_eq!(n.prev, x);
            }
        }
        assert_eq!(s.first == 0, s.last == 0);
        if s.first != 0 {
            assert_eq!(m.read(s.first).prev, x | UP);
            assert_eq!(m.read(s.last).next, x | UP);
        }
        let others = slots
            .iter()
            .any(|&y| y != x && m.read(y).cap.same_object(&s.cap));
        if !s.cap.is(Kind::Untyped) {
            assert_eq!(is_final(m, x), !others, "is_final({x:#x}) is wrong");
        } else {
            assert!(!others, "an untyped has two capabilities");
        }
    }
}

fn descendants<M: Memory>(m: &M, slots: &[u64], c: u64) -> Vec<u64> {
    slots
        .iter()
        .copied()
        .filter(|&x| {
            let mut y = x;
            for _ in 0..slots.len() {
                match parent_of(m, y, slots.len() + 2) {
                    Some(p) if p == c => return true,
                    Some(p) => y = p,
                    None => return false,
                }
            }
            false
        })
        .collect()
}

#[test]
fn random_runs() {
    for seed in 1..400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
        let mut w = World::new();
        let mut tables: Vec<(u64, u8)> = Vec::new();
        for _ in 0..200 {
            let slots = all_slots(&w, &tables);
            let pick = |rng: &mut Rng| slots[rng.below(slots.len() as u64) as usize];
            let a = pick(&mut rng);
            let b = pick(&mut rng);
            match rng.below(10) {
                0 | 1 => {
                    let kind = match rng.below(3) {
                        0 => Kind::Endpoint,
                        1 => Kind::Table,
                        _ => Kind::Untyped,
                    };
                    let bits = match kind {
                        Kind::Table => 2,
                        Kind::Untyped => 12 + rng.below(3) as u8,
                        _ => 0,
                    };
                    let index = 1 + rng.below(63);
                    let dst = Destination {
                        table: ROOT,
                        bits: ROOT_BITS,
                        index,
                    };
                    if let Ok(Progress::Done(n)) = retype(
                        &mut w.ram,
                        a,
                        kind,
                        bits,
                        1 + rng.below(3),
                        dst,
                        &mut Unlimited,
                    ) && kind == Kind::Table
                    {
                        for i in 0..n {
                            let t = w.cap(ROOT + ((index + i) << SLOT_BITS)).object;
                            if !tables.contains(&(t, 2)) {
                                tables.push((t, 2));
                            }
                        }
                    }
                }
                2..=4 => {
                    let badge = (rng.below(4) == 0).then(|| 1 + rng.below(5));
                    let _ = copy(&mut w.ram, a, b, rights::ALL, badge);
                }
                5 => {
                    let _ = move_cap(&mut w.ram, a, b);
                }
                6 | 7 => {
                    let _ = delete(&mut w.ram, &mut w.g, a);
                }
                _ => {
                    let before = descendants(&w.ram, &slots, a);
                    let was_ut = w.cap(a).is(Kind::Untyped);
                    let r = revoke(&mut w.ram, &mut w.g, a, &mut Unlimited);
                    if r.is_ok() {
                        for d in before {
                            assert!(w.ram.read(d).cap.is_empty(), "descendant survived");
                        }
                        if was_ut && w.cap(a).is(Kind::Untyped) {
                            assert_eq!(w.g.orphans, 0);
                            assert_eq!(w.g.purging, 0);
                            // Tables inside its memory are gone for good.
                            let u = w.cap(a);
                            let end = u.object + (1u64 << u.size_bits);
                            tables.retain(|&(t, _)| !(u.object..end).contains(&t));
                            for x in all_slots(&w, &tables) {
                                let c = w.ram.read(x).cap;
                                let encloses = c.is(Kind::Untyped)
                                    && c.object <= u.object
                                    && c.object + (1u64 << c.size_bits) >= end;
                                if c.is_live() && x != a && !encloses {
                                    assert!(
                                        !(u.object..end).contains(&c.object),
                                        "a capability names reset memory: {c:?} in {x:#x}, untyped {u:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
            check_links(&w.ram, &all_slots(&w, &tables));
        }
    }
}

/// Revokes interrupted at random points while other operations go on, as
/// they would on other cores between the revoke's kernel entries.
#[test]
fn random_interleaved_revokes() {
    struct Coin<'a>(&'a mut Rng);
    impl Budget for Coin<'_> {
        fn step(&mut self) -> bool {
            self.0.below(4) == 0
        }
    }
    for seed in 1..400u64 {
        let mut rng = Rng(seed.wrapping_mul(0xd6e8_feb8_6659_fd93) | 1);
        let mut w = World::new();
        let mut tables: Vec<(u64, u8)> = Vec::new();
        let mut pending: Vec<u64> = Vec::new();
        for _ in 0..300 {
            let slots = all_slots(&w, &tables);
            let pick = |rng: &mut Rng| slots[rng.below(slots.len() as u64) as usize];
            let a = pick(&mut rng);
            let b = pick(&mut rng);
            match rng.below(12) {
                0 | 1 => {
                    let kind = [Kind::Endpoint, Kind::Table, Kind::Untyped][rng.below(3) as usize];
                    let bits = match kind {
                        Kind::Table => 2,
                        Kind::Untyped => 12 + rng.below(3) as u8,
                        _ => 0,
                    };
                    let t = if rng.below(2) == 0 || tables.is_empty() {
                        (ROOT, ROOT_BITS)
                    } else {
                        tables[rng.below(tables.len() as u64) as usize]
                    };
                    let index = 1 + rng.below((1u64 << t.1) - 1);
                    let dst = Destination {
                        table: t.0,
                        bits: t.1,
                        index,
                    };
                    if let Ok(Progress::Done(n)) =
                        retype(&mut w.ram, a, kind, bits, 1, dst, &mut Unlimited)
                        && kind == Kind::Table
                    {
                        let at = t.0 + ((index + n - 1) << SLOT_BITS);
                        let obj = w.ram.read(at).cap.object;
                        if !tables.contains(&(obj, 2)) {
                            tables.push((obj, 2));
                        }
                    }
                }
                2..=4 => {
                    let _ = copy(&mut w.ram, a, b, rights::ALL, None);
                }
                5 => {
                    let _ = move_cap(&mut w.ram, a, b);
                }
                6 => {
                    let _ = delete(&mut w.ram, &mut w.g, a);
                }
                7 | 8 => {
                    if w.cap(a).is_live() && !w.cap(a).revoking() {
                        pending.push(a);
                    }
                }
                _ => {
                    if pending.is_empty() {
                        continue;
                    }
                    let i = rng.below(pending.len() as u64) as usize;
                    let c = pending[i];
                    let cap = w.cap(c);
                    if !cap.is_live() {
                        // Deleted by another revoke, or moved away.
                        pending.swap_remove(i);
                        continue;
                    }
                    let r = revoke(&mut w.ram, &mut w.g, c, &mut Coin(&mut rng));
                    if let Ok(Progress::Done(())) = r {
                        pending.swap_remove(i);
                        let after = w.ram.read(c);
                        if after.cap.is_live() {
                            assert_eq!(after.first, 0, "revoke left a child");
                            assert!(!after.cap.revoking());
                            if after.cap.is(Kind::Untyped) {
                                assert_eq!(after.cap.a, 0);
                                assert_eq!(w.g.orphans | w.g.purging, 0);
                                let end = after.cap.object + (1u64 << after.cap.size_bits);
                                tables.retain(|&(t, _)| !(after.cap.object..end).contains(&t));
                            }
                        }
                    }
                }
            }
            check_links(&w.ram, &all_slots(&w, &tables));
        }
    }
}

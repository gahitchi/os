// SPDX-License-Identifier: GPL-3.0-only
//! Kani proofs. Run with `cargo kani -p alaya-cap`.
//!
//! Each harness lets Kani choose every value (slots, kinds, rights, badges,
//! sizes and the order of operations) and proves the assertions hold for
//! all of them, within the bounds stated on the harness.

use super::*;

/// Slots 1 to N-1 of one table at address 0; slot 0 is its header. Any
/// other address reads as empty and ignores writes (object memory).
const N: usize = 5;

struct Mem {
    slots: [Slot; N],
}

impl Memory for Mem {
    fn read(&self, at: SlotRef) -> Slot {
        let i = (at >> SLOT_BITS) as usize;
        if at.is_multiple_of(SLOT_BYTES) && i < N {
            self.slots[i]
        } else {
            Slot::EMPTY
        }
    }

    fn write(&mut self, at: SlotRef, s: &Slot) {
        let i = (at >> SLOT_BITS) as usize;
        if at.is_multiple_of(SLOT_BYTES) && i < N {
            self.slots[i] = *s;
        }
    }

    fn zero(&mut self, _at: u64, _len: u64) {}
}

const TABLE_BITS: u8 = 3; // indexes up to 7; only 1..N exist here
const UT_SLOT: u64 = 1 << SLOT_BITS;
const UT_BASE: u64 = 0x10_0000;

fn world() -> (Mem, Globals) {
    let mut m = Mem {
        slots: [Slot::EMPTY; N],
    };
    init_table(&mut m, 0, TABLE_BITS);
    insert_root(&mut m, UT_SLOT, Cap::new(Kind::Untyped, UT_BASE, 16));
    (m, Globals::new())
}

fn any_slot() -> SlotRef {
    let i: u64 = kani::any();
    kani::assume(i >= 1 && i < N as u64);
    i << SLOT_BITS
}

/// The links of every slot agree with each other, empty slots are not
/// linked, and the O(1) `is_final` agrees with a search of all slots.
fn assert_well_formed(m: &Mem) {
    for i in 1..N {
        let x = (i as u64) << SLOT_BITS;
        let s = m.slots[i];
        if !s.cap.is_live() {
            assert!(s.prev == 0 && s.next == 0 && s.first == 0 && s.last == 0);
            continue;
        }
        if s.prev != 0 {
            let p = m.read(target(s.prev));
            assert!(p.cap.is_live());
            if is_up(s.prev) {
                assert!(p.first == x);
            } else {
                assert!(p.next == x);
            }
        }
        if s.next != 0 {
            let n = m.read(target(s.next));
            assert!(n.cap.is_live());
            if is_up(s.next) {
                assert!(n.last == x);
            } else {
                assert!(n.prev == x);
            }
        }
        assert!((s.first == 0) == (s.last == 0));
        if s.first != 0 {
            assert!(m.read(s.first).prev == x | UP);
            assert!(m.read(s.last).next == x | UP);
        }
        let mut others = false;
        for j in 1..N {
            if j != i && m.slots[j].cap.same_object(&s.cap) {
                others = true;
            }
        }
        if s.cap.is(Kind::Untyped) {
            assert!(!others);
        } else {
            assert!(is_final(m, x) == !others);
        }
    }
}

/// One operation that changes the tree without loops over it: retype of
/// one endpoint or notification, copy or mint, move, or delete.
fn any_tree_op(m: &mut Mem, g: &mut Globals) {
    let a = any_slot();
    let b = any_slot();
    match kani::any::<u8>() % 4 {
        0 => {
            let index = b >> SLOT_BITS;
            let dst = Destination {
                table: 0,
                bits: TABLE_BITS,
                index,
            };
            let kind = if kani::any() {
                Kind::Endpoint
            } else {
                Kind::Notification
            };
            let _ = retype(m, a, kind, 0, 1, dst, &mut Unlimited);
        }
        1 => {
            let badge: Option<u64> = if kani::any() { Some(kani::any()) } else { None };
            let _ = copy(m, a, b, kani::any(), badge);
        }
        2 => {
            let _ = move_cap(m, a, b);
        }
        _ => {
            let _ = delete(m, g, a);
        }
    }
}

/// Any three operations, in any order, on any slots, keep the tree well
/// formed. (A failed operation changes nothing, so this covers every
/// shorter sequence too.)
#[kani::proof]
#[kani::unwind(6)]
fn operations_keep_the_tree_well_formed() {
    let (mut m, mut g) = world();
    for _ in 0..3 {
        any_tree_op(&mut m, &mut g);
    }
    assert_well_formed(&m);
}

/// Which slots are below `c` in the tree. After two operations the tree is
/// at most three levels deep.
fn below(m: &Mem, c: SlotRef) -> [bool; N] {
    let mut out = [false; N];
    for (i, flag) in out.iter_mut().enumerate().skip(1) {
        let mut y = (i as u64) << SLOT_BITS;
        for _ in 0..3 {
            match parent_of(m, y, N) {
                Some(p) if p == c => {
                    *flag = true;
                    break;
                }
                Some(p) => y = p,
                None => break,
            }
        }
    }
    out
}

/// One step of revoke, by induction: each step deletes exactly one
/// descendant of the capability, keeps every other descendant below it and
/// changes nothing else; when there is no descendant left it finishes.
/// Together with the budget this proves a revoke deletes exactly the
/// descendants, however often it is interrupted.
#[kani::proof]
#[kani::unwind(6)]
fn each_revoke_step_deletes_one_descendant() {
    struct Once;
    impl Budget for Once {
        fn step(&mut self) -> bool {
            true
        }
    }
    let (mut m, mut g) = world();
    for _ in 0..2 {
        any_tree_op(&mut m, &mut g);
    }
    let c = any_slot();
    kani::assume(m.read(c).cap.is_live() && !m.read(c).cap.is(Kind::Untyped));
    let before_below = below(&m, c);
    let before = m.slots;
    let r = revoke(&mut m, &mut g, c, &mut Once);
    let after_below = below(&m, c);
    let mut deleted = 0;
    for i in 1..N {
        if (i as u64) << SLOT_BITS == c {
            continue;
        }
        if before[i].cap.is_live() && m.slots[i].cap.is_empty() {
            deleted += 1;
            assert!(before_below[i]);
        } else {
            assert!(m.slots[i].cap == before[i].cap);
            assert!(after_below[i] == before_below[i]);
        }
    }
    match r {
        Ok(Progress::Interrupted) => assert!(deleted == 1),
        Ok(Progress::Done(())) => {
            assert!(deleted == 0);
            assert!(m.read(c).first == 0);
            assert!(!m.read(c).cap.revoking());
        }
        Err(_) => unreachable!(),
    }
    assert_well_formed(&m);
}

/// An interrupted revoke leaves the tree well formed, and its capability
/// marked so nothing can be copied or retyped from it, or moved away.
#[kani::proof]
#[kani::unwind(6)]
fn interrupted_revoke_is_safe() {
    struct Once(bool);
    impl Budget for Once {
        fn step(&mut self) -> bool {
            let stop = !self.0;
            self.0 = true;
            stop
        }
    }
    let (mut m, mut g) = world();
    for _ in 0..2 {
        any_tree_op(&mut m, &mut g);
    }
    let c = any_slot();
    if let Ok(Progress::Interrupted) = revoke(&mut m, &mut g, c, &mut Once(false)) {
        assert!(m.read(c).cap.revoking());
        let d = any_slot();
        assert!(copy(&mut m, c, d, rights::ALL, None).is_err());
        assert!(move_cap(&mut m, c, d).is_err());
        assert!(delete(&mut m, &mut g, c).is_err());
        let dst = Destination {
            table: 0,
            bits: TABLE_BITS,
            index: d >> SLOT_BITS,
        };
        assert!(retype(&mut m, c, Kind::Endpoint, 0, 1, dst, &mut Unlimited).is_err());
    }
    assert_well_formed(&m);
}

/// Slot lookup never names the header or anything outside the table.
#[kani::proof]
fn table_slot_stays_inside_the_table() {
    let table: u64 = kani::any();
    let bits: u8 = kani::any();
    let index: u64 = kani::any();
    kani::assume(table.is_multiple_of(SLOT_BYTES << bits.min(TABLE_MAX_BITS)));
    kani::assume(table <= u64::MAX >> 1);
    if let Ok(at) = table_slot(table, bits, index) {
        assert!(index != 0);
        assert!(bits <= TABLE_MAX_BITS);
        assert!(at > table);
        assert!(at < table + (SLOT_BYTES << bits));
        assert!((at - table).is_multiple_of(SLOT_BYTES));
    }
}

/// Retype never overflows, never hands out memory below the watermark or
/// past the end of the untyped, aligns every object to its size, and its
/// zeroing only moves forward. The budget stops after every step, so each
/// call does at most one chunk of zeroing.
#[kani::proof]
#[kani::unwind(6)]
fn retype_stays_inside_the_untyped() {
    struct Stop;
    impl Budget for Stop {
        fn step(&mut self) -> bool {
            true
        }
    }
    let mut m = Mem {
        slots: [Slot::EMPTY; N],
    };
    init_table(&mut m, 0, TABLE_BITS);
    let size_bits: u8 = kani::any();
    kani::assume((UNTYPED_MIN_BITS..=UNTYPED_MAX_BITS).contains(&size_bits));
    let region = 1u64 << size_bits;
    let base: u64 = kani::any();
    kani::assume(base.is_multiple_of(region) && base <= (1u64 << 52));
    let free: u64 = kani::any();
    kani::assume(free <= region);
    let clean: u64 = kani::any();
    kani::assume(clean >= free && clean <= region);
    let mut cap = Cap::new(Kind::Untyped, base, size_bits);
    cap.a = free;
    cap.b = clean;
    insert_root(&mut m, UT_SLOT, cap);

    let kind = match kani::any::<u8>() % 5 {
        0 => Kind::Table,
        1 => Kind::Endpoint,
        2 => Kind::Notification,
        3 => Kind::Page,
        _ => Kind::Untyped,
    };
    let bits: u8 = kani::any();
    let count: u64 = kani::any();
    kani::assume(count <= 4);
    let dst = Destination {
        table: 0,
        bits: TABLE_BITS,
        index: 2,
    };
    match retype(&mut m, UT_SLOT, kind, bits, count, dst, &mut Stop) {
        Ok(Progress::Done(n)) => {
            let after = m.read(UT_SLOT).cap;
            assert!(n >= 1 && n <= count);
            assert!(after.a > free && after.a <= region);
            assert!(after.b >= after.a && after.b >= clean);
            let first = m.read(dst.table + (dst.index << SLOT_BITS)).cap;
            assert!(first.is_live());
            let (obj_bits, _) = object_bits(kind, bits, size_bits).unwrap();
            assert!(first.object >= base + free);
            assert!(first.object.is_multiple_of(1u64 << obj_bits));
            assert!(first.object + (n << obj_bits) <= base + region);
        }
        Ok(Progress::Interrupted) => {
            let after = m.read(UT_SLOT).cap;
            assert!(after.a == free);
            assert!(after.b > clean && after.b <= region);
            assert!(m.read(dst.table + (dst.index << SLOT_BITS)).cap.is_empty());
        }
        Err(_) => assert!(m.read(UT_SLOT).cap == cap),
    }
}

// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8, step 3: answers instead of data (R8 to R10).

use alaya_store::answers::{PER_DAY_MAX, Question};
use alaya_store::disk::MemDisk;
use alaya_store::grants::Access;
use alaya_store::{Class, Error, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const BOB: [u8; 32] = [2; 32];

const DESKTOP: u64 = 1;
const BUDGET: u64 = 0x108;
const OTHER: u64 = 0x109;
const DAY: u64 = 86_400;

fn fresh() -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(4096), &MACHINE, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.add_person(2, &BOB).unwrap();
    s.commit(10).unwrap();
    s
}

const SPENDING: &[u8] = b"what amount\nrent 700\nfood 212.50\nbus, 31.5\nrefund -20\n";

#[test]
fn the_fixed_questions_answer_one_number() {
    assert_eq!(Question::Lines.ask(SPENDING), 5);
    assert_eq!(Question::Words.ask(SPENDING), 10);
    // "amount" is a heading and is skipped; 700 + 212.50 + 31.5 - 20.
    assert_eq!(Question::Total(2).ask(SPENDING), 92_400);
    assert_eq!(Question::Total(1).ask(SPENDING), 0);
    assert_eq!(Question::Total(3).ask(SPENDING), 0);
    assert_eq!(Question::Total(2).ask(b""), 0);
    // Only the fixed list exists (R8).
    for c in [0, 4, 3, 3 | 33 << 8, 1 | 1 << 8, u64::MAX] {
        assert_eq!(Question::from_code(c), None, "code {c:#x}");
    }
    for q in [Question::Lines, Question::Words, Question::Total(32)] {
        assert_eq!(Question::from_code(q.code()), Some(q));
    }
    // Nothing panics on any bytes.
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    for _ in 0..2000 {
        let n = (x % 80) as usize;
        let bytes: Vec<u8> = (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                b"0123456789.-+ ,;\n\tab"[(x % 20) as usize]
            })
            .collect();
        for q in [
            Question::Lines,
            Question::Words,
            Question::Total(1),
            Question::Total(2),
        ] {
            q.ask(&bytes);
        }
    }
}

#[test]
fn an_app_gets_the_answer_never_the_document() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"spending.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, SPENDING).unwrap();
    s.commit(20).unwrap();
    // No answer grant: nothing.
    assert_eq!(s.answer(1, BUDGET, f, 30), Err(Error::NotFound));
    s.grant_answer(1, BUDGET, f, Question::Total(2), 3).unwrap();
    let a = s.answer(1, BUDGET, f, 30).unwrap();
    assert_eq!((a.value, a.left, a.whole), (92_400, 2, true));
    // It still holds no grant to the file itself.
    assert_eq!(s.access(1, BUDGET, f, 30).unwrap(), Access::No);
    // Another app, or the same app for another person, gets nothing.
    assert_eq!(s.answer(1, OTHER, f, 30), Err(Error::NotFound));
    assert_eq!(s.answer(2, BUDGET, f, 30), Err(Error::NotFound));
    s.commit(40).unwrap();
    // The ledger has the answer, and no read by the app.
    let cn = s.history(1).unwrap().last().unwrap().cn;
    assert!(s.reads(1, cn).unwrap().iter().all(|r| r.app != BUDGET));
    let l = s.answers(1, 0, 10).unwrap();
    assert_eq!(l.len(), 1);
    assert_eq!((l[0].1, l[0].2.app, l[0].2.file), (40, BUDGET, f));
    assert_eq!(
        (l[0].2.question, l[0].2.value),
        (Question::Total(2), 92_400)
    );
}

#[test]
fn the_question_limit_holds_and_starts_again_the_next_day() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"age.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"born 1990\n").unwrap();
    // Too many a day, or none, is not a choice.
    assert_eq!(
        s.grant_answer(1, BUDGET, f, Question::Lines, PER_DAY_MAX + 1),
        Err(Error::BadRequest)
    );
    assert_eq!(
        s.grant_answer(1, BUDGET, f, Question::Lines, 0),
        Err(Error::BadRequest)
    );
    s.grant_answer(1, BUDGET, f, Question::Total(2), 2).unwrap();
    let t = 5 * DAY + 100;
    assert_eq!(s.answer(1, BUDGET, f, t).unwrap().value, 199_000);
    assert_eq!(s.answer(1, BUDGET, f, t + 1).unwrap().left, 0);
    // Asking again changes nothing: the same fixed question gets the same
    // answer, and it runs out (rebuilding a number fails within the limit).
    for i in 0..50 {
        assert_eq!(s.answer(1, BUDGET, f, t + 2 + i), Err(Error::UsedUp));
    }
    // Granting again does not reset today's count.
    s.grant_answer(1, BUDGET, f, Question::Total(2), 2).unwrap();
    assert_eq!(s.answer(1, BUDGET, f, t + 60), Err(Error::UsedUp));
    // The next day it may ask again.
    assert_eq!(s.answer(1, BUDGET, f, 6 * DAY).unwrap().left, 1);
    let g = s.answer_grant(1, BUDGET, f).unwrap().unwrap();
    assert_eq!((g.used, g.asked), (1, 3));
    assert_eq!(s.answer_grants(1, None).unwrap(), vec![g]);
    assert!(s.answer_grants(1, Some(OTHER)).unwrap().is_empty());
    // Taken back.
    assert!(s.revoke_answer(1, BUDGET, f).unwrap());
    assert_eq!(s.answer(1, BUDGET, f, 6 * DAY + 1), Err(Error::NotFound));
}

#[test]
fn an_answer_from_a_private_document_is_private_too() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, DESKTOP, docs, b"salary.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"jan 3000\nfeb 3100\n").unwrap();
    s.set_private(1, f, true).unwrap();
    s.grant_answer(1, BUDGET, f, Question::Total(2), 5).unwrap();
    assert_eq!(s.marked(1, BUDGET).unwrap(), None);
    assert_eq!(s.answer(1, BUDGET, f, 50).unwrap().value, 610_000);
    let m = s.marked(1, BUDGET).unwrap().unwrap();
    assert_eq!((m.file, m.since), (f, 50));
}

#[test]
fn answers_survive_a_restart_and_need_a_file() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let dir = s.create(1, DESKTOP, docs, b"folder", true).unwrap();
    assert_eq!(
        s.grant_answer(1, BUDGET, dir, Question::Lines, 3),
        Err(Error::IsADirectory)
    );
    let f = s.create(1, DESKTOP, docs, b"list.txt", false).unwrap();
    s.write(1, DESKTOP, f, 0, b"a\nb\nc\n").unwrap();
    s.grant_answer(1, BUDGET, f, Question::Lines, 3).unwrap();
    assert_eq!(s.answer(1, BUDGET, f, 70).unwrap().value, 3);
    s.commit(80).unwrap();
    let disk = s.into_disk();
    let mut s = Store::open(disk, &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    assert_eq!(s.answer(1, BUDGET, f, 90).unwrap().left, 1);
    s.commit(100).unwrap();
    let l = s.answers(1, 0, 10).unwrap();
    assert_eq!(l.len(), 2);
    assert!(l[0].0 > l[1].0, "newest first");
    assert_eq!(s.answers(1, l[0].0, 10).unwrap().len(), 1);
}

// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7a, step 2: return points and "since you left".

use alaya_store::disk::MemDisk;
use alaya_store::made::Source;
use alaya_store::resume::LineKind;
use alaya_store::{Class, FileId, Options, Store, what};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const EDITOR: u64 = 0x101;
const CHART: u64 = 0x103;

fn put(s: &mut Store<MemDisk>, app: u64, f: FileId, data: &[u8]) {
    s.truncate(1, app, f, 0).unwrap();
    s.write(1, app, f, 0, data).unwrap();
}

fn read_all(s: &mut Store<MemDisk>, app: u64, f: FileId) {
    let mut buf = [0u8; 64];
    s.read(1, app, f, 0, &mut buf).unwrap();
}

#[test]
fn since_you_left_comes_only_from_the_ledger_and_the_marks() {
    let mut s = Store::format(MemDisk::new(4096), &MACHINE, Options::default()).unwrap();
    s.add_person(1, &ALICE).unwrap();
    s.commit(10).unwrap();
    let docs = s.root(Class::Documents);
    let dir = s.create(1, EDITOR, docs, b"Project", true).unwrap();
    let numbers = s.create(1, EDITOR, dir, b"numbers.txt", false).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 3\n");
    s.commit(20).unwrap();
    read_all(&mut s, CHART, numbers);
    let st = s.stat(1, numbers).unwrap();
    let chart = s.create(1, CHART, dir, b"chart.ppm", false).unwrap();
    put(&mut s, CHART, chart, b"a picture");
    s.made_from(
        1,
        CHART,
        chart,
        &[Source {
            file: numbers,
            version: st.version,
            edits: st.edits,
        }],
    )
    .unwrap();
    let plan = s.create(1, EDITOR, docs, b"plan.txt", false).unwrap();
    let diary = s.create(1, EDITOR, docs, b"diary.txt", false).unwrap();
    s.commit(30).unwrap();

    let g = s.group_new(1, b"Thesis", 0, 30).unwrap();
    s.group_add(1, g, dir).unwrap();
    s.group_add(1, g, plan).unwrap();
    // Never left: nothing to tell.
    assert!(s.since_left(1, g).unwrap().is_empty());
    s.group_leave(1, g, 40, 1 << 1 | 1 << 3).unwrap();
    s.group_note(1, g, b"compare the two models").unwrap();
    s.commit(40).unwrap();
    assert!(
        s.since_left(1, g).unwrap().is_empty(),
        "lines before anything changed"
    );

    // Meanwhile: the numbers change, the plan twice, the diary once.
    put(&mut s, EDITOR, numbers, b"Mon 4\n");
    s.commit(50).unwrap();
    put(&mut s, EDITOR, plan, b"one");
    s.commit(60).unwrap();
    put(&mut s, EDITOR, plan, b"two");
    put(&mut s, EDITOR, diary, b"dear diary");
    s.commit(70).unwrap();

    let r = s.return_point(1, g).unwrap().unwrap();
    assert_eq!(r.note, b"compare the two models");
    assert_eq!((r.left, r.roles), (40, 1 << 1 | 1 << 3));

    let lines = s.since_left(1, g).unwrap();
    let changed: Vec<_> = lines
        .iter()
        .filter(|l| l.kind == LineKind::Changed)
        .collect();
    assert_eq!(changed.len(), 2, "{lines:?}");
    let p = changed.iter().find(|l| l.file == plan).expect("plan.txt");
    assert_eq!((p.count, p.time, p.app), (2, 70, EDITOR));
    let n = changed
        .iter()
        .find(|l| l.file == numbers)
        .expect("numbers.txt");
    assert_eq!((n.count, n.time), (1, 50));
    assert!(
        lines.iter().all(|l| l.file != diary),
        "a file outside the group"
    );
    let look = lines
        .iter()
        .find(|l| l.kind == LineKind::NeedsALook)
        .expect("the chart needs a look");
    assert_eq!(
        (look.file, look.source, look.app),
        (chart, Some(numbers), CHART)
    );

    // Nothing invented: every changed line is a record in the ledger.
    let ledger = s.happenings(1, 0, 64).unwrap();
    for l in &changed {
        let h = ledger.iter().find(|h| h.cn == l.cn).expect("the moment");
        assert_eq!(h.time, l.time);
        assert!(h.changes.iter().any(|c| c.file == l.file
            && c.app == l.app
            && c.what == l.what
            && c.what & what::WROTE != 0));
    }

    // Leaving again starts afresh; the note stays until changed.
    s.group_leave(1, g, 80, 0).unwrap();
    s.commit(80).unwrap();
    assert!(
        s.since_left(1, g)
            .unwrap()
            .iter()
            .all(|l| l.kind == LineKind::NeedsALook)
    );
    assert_eq!(
        s.return_point(1, g).unwrap().unwrap().note,
        b"compare the two models"
    );
    s.group_note(1, g, b"").unwrap();
    assert!(s.return_point(1, g).unwrap().unwrap().note.is_empty());
    // A removed group takes its return point with it.
    s.group_remove(1, g).unwrap();
    assert_eq!(s.return_point(1, g).unwrap(), None);
}

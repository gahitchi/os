// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7, step 3: "Run it again": what an app had read by a past
//! moment, at exactly the versions it read.

use alaya_store::disk::MemDisk;
use alaya_store::{Class, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const EDITOR: u64 = 31;
const CHART: u64 = 33;
const DESKTOP: u64 = 32;

fn fresh() -> Store<MemDisk> {
    let mut s = Store::format(MemDisk::new(8192), &MACHINE, Options::default()).unwrap();
    s.stir(b"test seed");
    s.add_person(1, &ALICE).unwrap();
    s.commit(10).unwrap();
    s
}

#[test]
fn finds_exactly_the_versions_an_app_read() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let n = s.create(1, EDITOR, docs, b"numbers.txt", false).unwrap();
    s.write(1, EDITOR, n, 0, b"Mon 3").unwrap();
    let other = s.create(1, EDITOR, docs, b"other.txt", false).unwrap();
    s.write(1, EDITOR, other, 0, b"x").unwrap();
    let v1 = s.commit(20).unwrap();
    // The chart reads the numbers, then makes its picture.
    let mut b = [0u8; 16];
    s.read(1, CHART, n, 0, &mut b).unwrap();
    let pic = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();
    s.write(1, CHART, pic, 0, b"P6").unwrap();
    let made = s.commit(30).unwrap();
    // Later the numbers change, and the chart reads them again.
    s.write(1, EDITOR, n, 0, b"Mon 9").unwrap();
    s.commit(40).unwrap();
    s.read(1, CHART, n, 0, &mut b).unwrap();
    let later = s.commit(50).unwrap();

    let then = s.read_then(1, CHART, made).unwrap();
    assert_eq!(then.len(), 1, "{then:?}");
    assert_eq!(then[0].file, n);
    assert_eq!(then[0].version, v1);
    assert!(then[0].exact);
    let view = s.past(1, Class::Documents, then[0].moment).unwrap();
    let k = s.past_read(&view, n, 0, &mut b).unwrap();
    assert_eq!(&b[..k], b"Mon 3");
    // As of later: the newer version.
    let now = s.read_then(1, CHART, later).unwrap();
    assert!(now[0].version > v1 && now[0].exact);
    // Other apps' reads are not the chart's.
    assert!(s.read_then(1, DESKTOP, later).unwrap().is_empty());
    // A forgotten file is never brought back.
    s.forget(1, EDITOR, docs, b"numbers.txt").unwrap();
    s.commit(60).unwrap();
    assert!(s.read_then(1, CHART, made).unwrap().is_empty());
}

#[test]
fn after_thinning_says_it_is_the_nearest() {
    let mut s = fresh();
    let docs = s.root(Class::Documents);
    let f = s.create(1, EDITOR, docs, b"diary", false).unwrap();
    let mut t = 0;
    let mut read_at = 0;
    // A change every 20 minutes for three days; the app reads one early
    // version.
    for i in 0..(3 * 24 * 3) {
        s.write(1, EDITOR, f, 0, &(i as u64).to_le_bytes()).unwrap();
        if i == 7 {
            let mut b = [0u8; 8];
            s.read(1, CHART, f, 0, &mut b).unwrap();
        }
        let cn = s.commit(t).unwrap();
        if i == 7 {
            read_at = cn;
        }
        t += 20 * 60;
    }
    assert!(s.read_then(1, CHART, read_at).unwrap()[0].exact);
    s.thin(t + 30 * 24 * 3600).unwrap();
    s.commit(t + 30 * 24 * 3600).unwrap();
    let r = s.read_then(1, CHART, read_at).unwrap();
    assert_eq!(r.len(), 1);
    // That moment was thinned: the nearest kept one stands in, and says
    // it is not exact.
    assert!(!r[0].exact, "{r:?}");
    assert!(s.history(1).unwrap().iter().any(|m| m.cn == r[0].moment));
}

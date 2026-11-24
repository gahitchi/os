// SPDX-License-Identifier: GPL-3.0-only
//! Living validity (milestone 6c): "made from" notes and marks. The rule
//! under test everywhere: nothing is ever marked "up to date" unless the
//! app read every byte of the exact version it names.

use alaya_store::disk::MemDisk;
use alaya_store::made::{Kind, Source, Why};
use alaya_store::{Class, Error, FileId, Options, Store};

const MACHINE: [u8; 32] = [7; 32];
const ALICE: [u8; 32] = [1; 32];
const EDITOR: u64 = 0x101;
const CHART: u64 = 0x103;
const OTHER: u64 = 0x109;

fn fresh() -> (Store<MemDisk>, FileId) {
    let mut s = Store::format(MemDisk::new(4096), &MACHINE, Options::default()).unwrap();
    s.add_person(1, &ALICE).unwrap();
    s.commit(10).unwrap();
    let docs = s.root(Class::Documents);
    (s, docs)
}

fn put(s: &mut Store<MemDisk>, app: u64, f: FileId, data: &[u8]) {
    s.truncate(1, app, f, 0).unwrap();
    s.write(1, app, f, 0, data).unwrap();
}

fn read_all(s: &mut Store<MemDisk>, app: u64, f: FileId) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = [0u8; 7];
    loop {
        let n = s.read(1, app, f, out.len() as u64, &mut buf).unwrap();
        if n == 0 {
            return out;
        }
        out.extend_from_slice(&buf[..n]);
    }
}

fn source(s: &mut Store<MemDisk>, f: FileId) -> Source {
    let st = s.stat(1, f).unwrap();
    Source {
        file: f,
        version: st.version,
        edits: st.edits,
    }
}

/// numbers.txt (by the editor) and chart.ppm (by the chart maker, made
/// from it after reading all of it).
fn pipeline() -> (Store<MemDisk>, FileId, FileId) {
    let (mut s, docs) = fresh();
    let numbers = s.create(1, EDITOR, docs, b"numbers.txt", false).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 3\nTue 5\nWed 2\n");
    s.commit(20).unwrap();
    let src = source(&mut s, numbers);
    let text = read_all(&mut s, CHART, numbers);
    assert_eq!(text.len(), 18);
    let chart = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();
    put(&mut s, CHART, chart, b"a picture");
    s.made_from(1, CHART, chart, &[src]).unwrap();
    s.commit(30).unwrap();
    (s, numbers, chart)
}

#[test]
fn up_to_date_then_needs_a_look() {
    let (mut s, numbers, chart) = pipeline();
    let m = s.mark(1, chart).unwrap();
    assert_eq!(m.kind, Kind::UpToDate);
    assert_eq!((m.maker, m.sources), (CHART, 1));
    // Sources and plain files carry no mark.
    assert_eq!(s.mark(1, numbers).unwrap().kind, Kind::NoMark);

    put(&mut s, EDITOR, numbers, b"Mon 3\nTue 9\nWed 2\n");
    let m = s.mark(1, chart).unwrap();
    assert_eq!(m.kind, Kind::NeedsALook);
    assert_eq!(m.why, Why::SourceChanged);
    assert_eq!(m.source, Some(numbers));
    assert_eq!(m.now_version, s.stat(1, numbers).unwrap().version);
    // It survives a commit and reopening the store.
    s.commit(40).unwrap();
    let mut s = Store::open(s.into_disk(), &MACHINE).unwrap();
    s.unlock(1, &ALICE).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NeedsALook);

    // Made again from the new numbers: up to date again.
    let src = source(&mut s, numbers);
    read_all(&mut s, CHART, numbers);
    put(&mut s, CHART, chart, b"a new picture");
    s.made_from(1, CHART, chart, &[src]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::UpToDate);
    s.commit(90).unwrap();
    let r = s.check().unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

#[test]
fn two_saves_in_one_commit_are_told_apart() {
    // Both saves land before the same commit, so the version number is
    // the same: the change count tells them apart.
    let (mut s, docs) = fresh();
    let numbers = s.create(1, EDITOR, docs, b"numbers.txt", false).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 3\n");
    let src = source(&mut s, numbers);
    read_all(&mut s, CHART, numbers);
    let chart = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();
    put(&mut s, CHART, chart, b"a picture");
    s.made_from(1, CHART, chart, &[src]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::UpToDate);
    put(&mut s, EDITOR, numbers, b"Mon 4\n");
    assert_eq!(s.stat(1, numbers).unwrap().version, src.version);
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NeedsALook);
}

#[test]
fn partial_reads_are_never_up_to_date() {
    let (mut s, docs) = fresh();
    let numbers = s.create(1, EDITOR, docs, b"numbers.txt", false).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 3\nTue 5\nWed 2\n");
    s.commit(20).unwrap();
    let src = source(&mut s, numbers);
    let chart = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();

    // Only the start.
    let mut buf = [0u8; 6];
    s.read(1, CHART, numbers, 0, &mut buf).unwrap();
    put(&mut s, CHART, chart, b"a picture");
    s.made_from(1, CHART, chart, &[src]).unwrap();
    let m = s.mark(1, chart).unwrap();
    assert_eq!(
        (m.kind, m.why, m.source),
        (Kind::NotSure, Why::NotAllRead, Some(numbers))
    );

    // Only the end.
    let (mut s, docs) = fresh();
    let numbers = s.create(1, EDITOR, docs, b"numbers.txt", false).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 3\nTue 5\nWed 2\n");
    let src = source(&mut s, numbers);
    s.read(1, CHART, numbers, 6, &mut [0u8; 100]).unwrap();
    let chart = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();
    put(&mut s, CHART, chart, b"a picture");
    s.made_from(1, CHART, chart, &[src]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NotSure);

    // Someone else read all of it: that does not count for this app.
    s.read(1, OTHER, numbers, 0, &mut [0u8; 100]).unwrap();
    s.made_from(1, CHART, chart, &[src]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NotSure);

    // Never read at all.
    let lone = s.create(1, CHART, docs, b"lone.ppm", false).unwrap();
    put(&mut s, CHART, lone, b"x");
    s.made_from(1, CHART, lone, &[src]).unwrap();
    assert_eq!(s.mark(1, lone).unwrap().kind, Kind::NotSure);
}

#[test]
fn a_save_while_reading_is_not_up_to_date() {
    let (mut s, docs) = fresh();
    let numbers = s.create(1, EDITOR, docs, b"numbers.txt", false).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 3\nTue 5\nWed 2\n");
    s.commit(20).unwrap();
    let old = source(&mut s, numbers);
    // The chart maker reads the first half; the editor saves; it reads
    // the second half.
    s.read(1, CHART, numbers, 0, &mut [0u8; 9]).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 3\nTue 7\nWed 2\n");
    let new = source(&mut s, numbers);
    s.read(1, CHART, numbers, 9, &mut [0u8; 100]).unwrap();
    let chart = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();
    put(&mut s, CHART, chart, b"a mixed picture");
    // Whichever version it names, it never saw all of it.
    s.made_from(1, CHART, chart, &[new]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NotSure);
    s.made_from(1, CHART, chart, &[old]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NeedsALook);
}

#[test]
fn renames_keep_the_mark_honest() {
    let (mut s, numbers, chart) = pipeline();
    let docs = s.root(Class::Documents);
    // Renaming the source or the result changes nothing.
    s.rename(1, EDITOR, docs, b"numbers.txt", docs, b"old.txt")
        .unwrap();
    s.rename(1, EDITOR, docs, b"chart.ppm", docs, b"picture.ppm")
        .unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::UpToDate);
    // A new file under the old name is not the source: the chart still
    // points at the file it was made from.
    let fake = s.create(1, OTHER, docs, b"numbers.txt", false).unwrap();
    put(&mut s, OTHER, fake, b"Mon 100\n");
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::UpToDate);
    // Deleting the real source: not sure any more.
    s.remove(1, EDITOR, docs, b"old.txt").unwrap();
    let m = s.mark(1, chart).unwrap();
    assert_eq!(
        (m.kind, m.why, m.source),
        (Kind::NotSure, Why::SourceGone, Some(numbers))
    );
}

#[test]
fn only_the_maker_may_say_and_changes_after_count() {
    let (mut s, numbers, chart) = pipeline();
    let src = source(&mut s, numbers);
    // Another app did not make chart.ppm: it cannot say what it is made of.
    read_all(&mut s, OTHER, numbers);
    assert_eq!(s.made_from(1, OTHER, chart, &[src]), Err(Error::BadRequest));
    // A result cannot be its own source; there must be one; not too many.
    let itself = source(&mut s, chart);
    assert_eq!(
        s.made_from(1, CHART, chart, &[itself]),
        Err(Error::BadRequest)
    );
    assert_eq!(s.made_from(1, CHART, chart, &[]), Err(Error::BadRequest));
    // Someone changes the picture after it was made.
    s.write(1, OTHER, chart, 0, b"scribble").unwrap();
    let m = s.mark(1, chart).unwrap();
    assert_eq!((m.kind, m.why), (Kind::NotSure, Why::ResultChanged));
}

#[test]
fn as_of_a_date_and_history() {
    let (mut s, numbers, chart) = pipeline();
    let made = s.last_commit();
    s.set_as_of(1, 5, chart, true).unwrap();
    put(&mut s, EDITOR, numbers, b"Mon 1\n");
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::AsOf);
    s.set_as_of(1, 5, chart, false).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NeedsALook);
    // Only a result can be "as of" a date.
    assert_eq!(s.set_as_of(1, 5, numbers, true), Err(Error::NotFound));
    s.commit(50).unwrap();
    // The picture of that moment is still in the history.
    let past = s.past(1, Class::Documents, made).unwrap();
    assert_eq!(s.past_stat(&past, chart).unwrap().size, 9);
    // Deleting the result drops its note.
    let docs = s.root(Class::Documents);
    s.remove(1, CHART, docs, b"chart.ppm").unwrap();
    assert_eq!(s.mark(1, chart), Err(Error::NotFound));
    assert!(s.sources(1, chart).unwrap().is_empty());
    s.commit(90).unwrap();
    let r = s.check().unwrap();
    assert!(r.problems.is_empty(), "{:?}", r.problems);
}

#[test]
fn empty_sources_count_once_asked_for() {
    let (mut s, docs) = fresh();
    let empty = s.create(1, EDITOR, docs, b"empty.txt", false).unwrap();
    let src = source(&mut s, empty);
    let chart = s.create(1, CHART, docs, b"chart.ppm", false).unwrap();
    put(&mut s, CHART, chart, b"nothing");
    s.made_from(1, CHART, chart, &[src]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::NotSure);
    assert!(read_all(&mut s, CHART, empty).is_empty());
    s.made_from(1, CHART, chart, &[src]).unwrap();
    assert_eq!(s.mark(1, chart).unwrap().kind, Kind::UpToDate);
}

/// Milestone 7, step 3: a part handed over by the clipboard carries the
/// mark; "show the difference" shows what changed in the source.
#[test]
fn a_pasted_part_carries_the_mark_and_shows_the_difference() {
    let (mut s, docs) = fresh();
    let src = s.create(1, EDITOR, docs, b"prices.txt", false).unwrap();
    put(&mut s, EDITOR, src, b"apples 3\npears 4\nplums 5\n");
    s.commit(20).unwrap();
    let st = s.stat(1, src).unwrap();
    let source = Source {
        file: src,
        version: st.version,
        edits: st.edits,
    };
    // The editor got one line of it through the clipboard, not by reading.
    let note = s.create(1, EDITOR, docs, b"note.txt", false).unwrap();
    put(&mut s, EDITOR, note, b"pears 4");
    // Without the clipboard's word, a part is "not sure".
    s.made_from(1, EDITOR, note, &[source]).unwrap();
    assert_eq!(s.mark(1, note).unwrap().kind, Kind::NotSure);
    s.note_pasted(1, EDITOR, source).unwrap();
    s.made_from(1, EDITOR, note, &[source]).unwrap();
    s.commit(30).unwrap();
    assert_eq!(s.mark(1, note).unwrap().kind, Kind::UpToDate);
    // Another app was not handed it.
    let other = s.create(1, OTHER, docs, b"other.txt", false).unwrap();
    put(&mut s, OTHER, other, b"pears 4");
    s.made_from(1, OTHER, other, &[source]).unwrap();
    assert_eq!(s.mark(1, other).unwrap().kind, Kind::NotSure);
    // The prices change: needs a look, and the difference shows why.
    put(&mut s, EDITOR, src, b"apples 3\npears 6\nplums 5\n");
    s.commit(40).unwrap();
    assert_eq!(s.mark(1, note).unwrap().kind, Kind::NeedsALook);
    let d = s.what_changed(1, note).unwrap().unwrap();
    assert!(d.exact);
    assert_eq!(d.source, src);
    assert_eq!(d.then, b"apples 3\npears 4\nplums 5\n");
    assert_eq!(d.now, b"apples 3\npears 6\nplums 5\n");
    // Nothing to show for a file that is not a result.
    assert!(s.what_changed(1, src).unwrap().is_none());
}

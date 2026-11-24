// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7 checks, step three: jobs with rules. A program you don't
//! fully trust works on a side copy of one folder the person picked, under
//! rules the person saw on one screen; Alaya runs the checks itself when
//! it is done, and the work reaches the person's files only when the
//! person keeps it. A hostile helper tries every way around the rules.

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, DOCS, st};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask};

const SLOW: u64 = 30_000;
const FOLDER: &str = "Trip notes";
const CHECKLIST: &[u8] = b"# what the trip notes need\nhas packing-list.txt\nsays packing-list.txt socks\nsays contents.txt packing-list.txt\nunder day-1.txt 100\n";

impl Files {
    fn put_all(&self, f: u64, data: &[u8]) -> bool {
        self.buf()[..data.len()].copy_from_slice(data);
        self.call(fs::WRITE, &[f, 0, data.len() as u64])[0] == st::OK
            && self.call(fs::TRUNCATE, &[f, data.len() as u64])[0] == st::OK
    }

    fn make_in(&self, dir: u64, name: &str, dir_too: bool) -> Option<u64> {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
        let r = self.call(fs::CREATE, &[dir, dir_too as u64, name.len() as u64]);
        (r[0] == st::OK).then_some(r[1])
    }

    /// A small file's bytes (up to 512) and how many.
    fn all(&self, f: u64) -> ([u8; 512], usize) {
        let mut b = [0u8; 512];
        let n = self.read(f, &mut b).unwrap_or(0) as usize;
        (b, n.min(512))
    }

    fn says(&self, f: u64, want: &[u8]) -> bool {
        let (b, n) = self.all(f);
        b[..n] == *want
    }

    /// Each entry of `dir`: name, file, whether it is a folder.
    fn each(&self, dir: u64, mut f: impl FnMut(&[u8], u64, bool)) {
        let r = self.call(fs::LIST, &[dir]);
        if r[0] != st::OK {
            return;
        }
        let n = (r[1] as usize).min(64);
        let mut names = [[0u8; 32]; 64];
        let mut lens = [0usize; 64];
        let mut files = [0u64; 64];
        for i in 0..n {
            let b = self.buf();
            let o = 48 * i;
            let w = |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
            files[i] = w(0);
            lens[i] = (w(1) as usize).min(32);
            names[i][..lens[i]].copy_from_slice(&b[o + 16..o + 16 + lens[i]]);
        }
        for i in 0..n {
            let dir = self.call(fs::STAT, &[files[i]])[3] != 0;
            f(&names[i][..lens[i]], files[i], dir);
        }
    }

    /// A fingerprint of every document's name and contents, two folder
    /// levels deep (order does not matter).
    fn snapshot(&self, docs: u64) -> u64 {
        fn fnv(data: &[u8]) -> u64 {
            data.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
                (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
            })
        }
        let mut sum = 0u64;
        let mut dirs = [(0u64, 0u64); 16];
        let mut nd = 0;
        self.each(docs, |name, f, dir| {
            if dir && nd < 16 {
                dirs[nd] = (f, fnv(name));
                nd += 1;
            }
        });
        let mut add = |prefix: u64, name: &[u8], f: u64, dir: bool| {
            let mut h = fnv(name) ^ prefix.rotate_left(7);
            if !dir {
                let (b, n) = self.all(f);
                h ^= fnv(&b[..n]).rotate_left(13);
            }
            sum = sum.wrapping_add(h);
        };
        let mut top = [(0u64, [0u8; 32], 0usize, false); 64];
        let mut nt = 0;
        self.each(docs, |name, f, dir| {
            if nt < 64 {
                top[nt].0 = f;
                top[nt].1[..name.len()].copy_from_slice(name);
                top[nt].2 = name.len();
                top[nt].3 = dir;
                nt += 1;
            }
        });
        for e in &top[..nt] {
            add(0, &e.1[..e.2], e.0, e.3);
        }
        for &(d, h) in &dirs[..nd] {
            let mut inner = [(0u64, [0u8; 32], 0usize, false); 64];
            let mut ni = 0;
            self.each(d, |name, f, dir| {
                if ni < 64 {
                    inner[ni].0 = f;
                    inner[ni].1[..name.len()].copy_from_slice(name);
                    inner[ni].2 = name.len();
                    inner[ni].3 = dir;
                    ni += 1;
                }
            });
            for e in &inner[..ni] {
                add(h, &e.1[..e.2], e.0, e.3);
            }
        }
        sum
    }
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let job = || (ask(ep, dk::STATE, &[])[5] >> 48) & 0xf;

    // The folder, with a checklist and a private file; files outside it.
    let trip = me
        .make_in(docs, FOLDER, true)
        .expect("make the trip folder");
    let packing = me.make_in(trip, "Packing List.txt", false).unwrap();
    assert!(me.put_all(packing, b"socks   \nmap  \n"));
    let day = me.make_in(trip, "Day 1.txt", false).unwrap();
    assert!(me.put_all(day, b"Beach.\n"));
    let list = me.make_in(trip, "checklist.txt", false).unwrap();
    assert!(me.put_all(list, CHECKLIST));
    let passport = me.make_in(trip, "passport.txt", false).unwrap();
    assert!(me.put_all(passport, b"AB123456"));
    assert!(me.call(fs::PRIVATE, &[passport, 1])[0] == st::OK);
    me.call(fs::SAVE, &[]);
    let before = me.snapshot(docs);
    // The folder's row in the rules screen: folders in name order.
    let mut row = 0;
    me.each(docs, |name, _, dir| {
        if dir && name < FOLDER.as_bytes() {
            row += 1;
        }
    });
    let choose = |x: &Session<'_>| {
        for _ in 0..row {
            x.keys("{down}", || false, 800);
        }
    };

    // ---- 93. The tidy-up helper works under rules; Alaya checks; the
    // person keeps it.
    x.start_app(role::HELPER);
    let win = x.wait_window(role::HELPER);
    ask(ep, dk::RAISE, &[win]);
    sleep_until(1500, || false);
    assert!(
        x.keys("{enter}", || job() == 1, SLOW),
        "the helper's ask did not show the rules screen"
    );
    choose(x);
    assert!(x.keys("{enter}", || job() == 2, SLOW), "no rules shown");
    crate::say!("screenshot job rules");
    assert!(
        x.keys("{enter}", || job() == 3 || job() == 4, SLOW * 2),
        "the helper's job did not end with Alaya's checks"
    );
    crate::say!("screenshot job result");
    assert!(
        job() == 3,
        "Alaya's checks of the tidy-up did not pass (state {})",
        job()
    );
    // Nothing reached the person's files yet.
    assert!(
        me.snapshot(docs) == before,
        "the job's work reached the person's files before it was kept"
    );
    assert!(x.keys("{enter}", || job() == 0, SLOW), "Keep did nothing");
    let tidy = me.lookup(trip, "packing-list.txt");
    assert!(
        tidy == Some(packing),
        "the kept work has no packing-list.txt (the same file, renamed)"
    );
    assert!(
        me.says(packing, b"socks\nmap\n"),
        "spaces still at line ends"
    );
    let contents = me.lookup(trip, "contents.txt").expect("contents.txt kept");
    let (c, n) = me.all(contents);
    assert!(c[..n].windows(16).any(|w| w == b"packing-list.txt"));
    assert!(me.says(list, CHECKLIST), "the checklist changed");
    assert!(me.says(passport, b"AB123456"), "the private file changed");
    assert!(me.lookup(docs, "summary.txt").is_some());
    // On record as the helper's work, kept from a job.
    let h = me.call(fs::HISTORY, &[]);
    assert!(h[0] == st::OK && h[1] > 0);
    let newest = u64::from_le_bytes(me.buf()[..8].try_into().unwrap());
    let r = me.call(fs::CHANGES, &[newest]);
    let b = me.buf();
    let kept = (0..r[1] as usize).any(|i| {
        let w = |k: usize| {
            u64::from_le_bytes(b[24 * i + 8 * k..24 * i + 8 * k + 8].try_into().unwrap())
        };
        w(0) == fs::app_id(role::HELPER) && w(2) & fs::what::KEPT != 0
    });
    assert!(kept, "the history does not show the helper's kept work");
    pass(
        "a job with rules: the tidy-up helper changed only its folder, on a side copy; Alaya's checks passed; kept in one step",
    );

    // ---- 94. A hostile helper tries every way around the rules: none
    // works, the checks catch the damage, and Enter never keeps it.
    let before = me.snapshot(docs);
    // Its window takes the tidy-up helper's place (the desktop keeps a
    // few window pictures; a closed window's is used again).
    ask(ep, dk::FORGET, &[role::HELPER]);
    x.start_app(role::HOSTILE);
    let win = x.wait_window(role::HOSTILE);
    ask(ep, dk::RAISE, &[win]);
    sleep_until(1500, || false);
    assert!(
        x.keys("{enter}", || job() == 1, SLOW),
        "the hostile helper's ask showed nothing"
    );
    choose(x);
    assert!(x.keys("{enter}", || job() == 2, SLOW));
    assert!(
        x.keys("{enter}", || job() == 3 || job() == 4, SLOW * 2),
        "the hostile helper's job did not end"
    );
    assert!(
        job() == 4,
        "Alaya's checks passed the hostile helper's damage (state {})",
        job()
    );
    assert!(
        me.snapshot(docs) == before,
        "the hostile helper changed the person's files"
    );
    // As the hostile helper: after "done", nothing more, anywhere.
    let them = x.files_as(x.person, fs::app_id(role::HOSTILE));
    for f in [
        packing,
        list,
        passport,
        me.lookup(docs, "summary.txt").unwrap(),
    ] {
        assert!(
            them.call(fs::WRITE, &[f, 0, 1])[0] != st::OK,
            "a finished job's app could still write"
        );
    }
    assert!(them.call(fs::JOB_KEEP, &[fs::app_id(role::HOSTILE), 1, 0])[0] != st::OK);
    // Enter never keeps held work.
    x.keys("{enter}", || job() != 4, 3000);
    assert!(job() == 4, "Enter kept held work");
    assert!(me.snapshot(docs) == before);
    assert!(x.keys("t", || job() == 0, SLOW), "T did not throw it away");
    assert!(
        me.call(fs::JOB_CHECK, &[fs::app_id(role::HOSTILE)])[0] == st::NOT_FOUND,
        "the thrown-away job is still there"
    );
    assert!(me.snapshot(docs) == before);
    pass(
        "a hostile helper: every way around the rules refused, its damage held by Alaya's checks, thrown away; your files untouched",
    );

    // ---- 95. "Run it again": the chart maker opens again on exactly the
    // numbers it had read, read only, while the numbers moved on.
    let chart = fs::app_id(role::CHART);
    let numbers = me.lookup(docs, "numbers.txt").expect("numbers.txt");
    let (was, n_was) = me.all(numbers);
    let r = me.call(fs::SAVE, &[]);
    assert!(r[0] == st::OK);
    let then = r[1];
    assert!(me.put_all(numbers, b"Mon 1\nTue 1\n"));
    me.call(fs::SAVE, &[]);
    let a = ask(ep, dk::RUN_AGAIN, &[chart, then]);
    assert!(
        a[0] == dk::OK && a[1] >= 1 && a[2] == a[1],
        "run it again found nothing exact: {a:?}"
    );
    let as_chart = x.files_as(x.person, chart);
    let (b, n) = as_chart.all(numbers);
    assert!(
        b[..n] == was[..n_was],
        "run again, the chart maker did not see the numbers it read then"
    );
    assert!(
        as_chart.call(fs::WRITE, &[numbers, 0, 1])[0] != st::OK,
        "run again, the chart maker could change the numbers"
    );
    let mut only = 0;
    as_chart.each(docs, |_, _, _| only += 1);
    assert!(only == a[1], "run again, it saw more than it read then");
    // When it closes, it sees today's numbers again.
    assert!(me.call(fs::APP_CLOSED, &[chart])[0] == st::OK);
    let (b, n) = as_chart.all(numbers);
    assert!(
        b[..n] != was[..n_was],
        "after closing, the chart maker still sees the old numbers"
    );
    x.start_asked();
    pass("run it again: an app opens on exactly what it read then, read only, until it closes");

    // ---- 96. Living validity in full: copy and paste carry the mark (and
    // the private mark), "show the difference", "make again".
    let editor = fs::app_id(role::EDITOR);
    let as_editor = x.files_as(x.person, editor);
    let prices = me.make_in(docs, "prices.txt", false).unwrap();
    assert!(me.put_all(prices, b"apples 3\npears 4\n"));
    me.call(fs::SAVE, &[]);
    assert!(me.call(fs::GRANT, &[editor, prices, 0, 0, 0])[0] == st::OK);
    // One line, through the clipboard: the store knows its exact version.
    assert!(as_editor.call(fs::COPY, &[prices, 9, 7])[0] == st::OK);
    let p = as_editor.call(fs::PASTE, &[]);
    assert!(
        p[0] == st::OK && p[1] == 7 && p[2] == prices,
        "the paste does not know where it came from: {p:?}"
    );
    let (v, e) = (p[3], p[4]);
    let shopping = as_editor.make_in(docs, "shopping.txt", false).unwrap();
    assert!(as_editor.put_all(shopping, b"pears 4"));
    for (i, w) in [prices, v, e].iter().enumerate() {
        as_editor.buf()[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
    }
    assert!(
        as_editor.call(fs::MADE_FROM, &[shopping, 1])[0] == st::OK,
        "a pasted source was refused"
    );
    me.call(fs::SAVE, &[]);
    let kind = |f: u64| me.call(fs::MARK, &[f])[1] & 0xff;
    assert!(kind(shopping) == 1, "pasted, it is not up to date");
    // The prices change: needs a look, and the difference says why.
    assert!(me.put_all(prices, b"apples 3\npears 6\n"));
    me.call(fs::SAVE, &[]);
    assert!(
        kind(shopping) == 2,
        "the prices changed; it does not need a look"
    );
    let d = me.call(fs::WHAT_CHANGED, &[shopping]);
    assert!(
        d[0] == st::OK && d[1] == prices && d[2] == 1,
        "no difference: {d:?}"
    );
    let b = me.buf();
    let (a, n) = (d[3] as usize, d[4] as usize);
    assert!(b[..a] == *b"apples 3\npears 4\n");
    assert!(b[fs::DIFF_AT..fs::DIFF_AT + n] == *b"apples 3\npears 6\n");
    // The private mark travels with a paste (decision 27).
    let (one, two) = (fs::app_id(77), fs::app_id(78));
    let marked = |app: u64| {
        let r = me.call(fs::MARKS, &[]);
        let b = me.buf();
        (0..r[1] as usize)
            .any(|i| u64::from_le_bytes(b[24 * i..24 * i + 8].try_into().unwrap()) == app)
    };
    assert!(me.call(fs::PRIVATE, &[prices, 1])[0] == st::OK);
    assert!(me.call(fs::GRANT, &[one, prices, 0, 0, 0])[0] == st::OK);
    let as_one = x.files_as(x.person, one);
    let as_two = x.files_as(x.person, two);
    assert!(as_one.call(fs::COPY, &[prices, 0, 8])[0] == st::OK);
    assert!(marked(one) && !marked(two));
    assert!(as_two.call(fs::PASTE, &[])[0] == st::OK);
    assert!(
        marked(two),
        "pasting from a private file did not mark the app"
    );
    for app in [one, two] {
        me.call(fs::APP_CLOSED, &[app]);
    }
    me.call(fs::PRIVATE, &[prices, 0]);
    // Make again: the chart maker makes its picture again from today's
    // numbers.
    let picture = me.lookup(docs, "chart.ppm").expect("chart.ppm");
    assert!(kind(picture) == 2, "chart.ppm should need a look now");
    let r = ask(ep, dk::MAKE_AGAIN, &[picture]);
    assert!(r[0] == dk::OK, "make again was refused: {r:?}");
    x.start_asked();
    assert!(
        sleep_until(SLOW * 2, || kind(picture) == 1),
        "the chart maker did not make chart.ppm again (mark {})",
        kind(picture)
    );
    pass(
        "living validity: a paste carries its source's mark and its private mark; the difference shows; the maker makes it again",
    );
}

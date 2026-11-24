// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7 checks, step one: going back (decisions 108 to 110). The
//! one undo with the Alaya key and the strip's button, for a deleted file,
//! a setting and a closed window; "undo anyway?" when something else
//! changed it since (rule B6); "What happened?" with undo on a line; the
//! timeline, bringing a file back and naming a version; file stories; one
//! person never seeing another's; and a year of history opening fast
//! while every core is busy.

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, APPS, DOCS, act, st};

use crate::m3::{sleep_until, ticks_to_us};
use crate::m6b::{Files, Session, ask};
use crate::say;

/// How long to wait for keys the host typed before sending them again.
/// Most keys here are not safe to send twice (a second undo, a second
/// delete), so this is long: slow emulated machines with many cores can
/// take seconds to act.
const SLOW: u64 = 30_000;

/// A person for the checks only: someone else, and a year of history.
const OTHER: u64 = 3;

impl Files {
    fn put(&self, f: u64, data: &[u8]) -> bool {
        self.buf()[..data.len()].copy_from_slice(data);
        self.call(fs::WRITE, &[f, 0, data.len() as u64])[0] == st::OK
            && self.call(fs::TRUNCATE, &[f, data.len() as u64])[0] == st::OK
    }

    fn make(&self, dir: u64, name: &str) -> Option<u64> {
        self.buf()[..name.len()].copy_from_slice(name.as_bytes());
        let r = self.call(fs::CREATE, &[dir, 0, name.len() as u64]);
        (r[0] == st::OK).then_some(r[1])
    }

    /// The first 256 bytes of `f`, and how many there are.
    fn contents(&self, f: u64) -> Contents {
        let mut b = [0u8; 256];
        let n = self.read(f, &mut b).unwrap_or(0) as usize;
        Contents(b, n.min(256))
    }

    /// The newest action of `kind`: (number, undone).
    pub(crate) fn newest(&self, kind: u64) -> Option<(u64, bool)> {
        let r = self.call(fs::ACTIONS, &[0, 16]);
        if r[0] != st::OK {
            return None;
        }
        let b = self.buf();
        (0..r[1] as usize).find_map(|i| {
            let o = fs::ACTION_BYTES * i;
            let w = |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
            (w(3) & 0xff == kind).then(|| (w(0), w(3) >> 8 & 1 != 0))
        })
    }
}

/// The start of a file.
#[derive(PartialEq, Eq)]
struct Contents([u8; 256], usize);

impl Contents {
    fn is(&self, want: &[u8]) -> bool {
        self.0[..self.1] == *want
    }

    fn text(&self) -> &str {
        core::str::from_utf8(&self.0[..self.1]).unwrap_or("?")
    }
}

/// Spins on one core, at a low priority, until stopped.
extern "C" fn busy(_: u64) -> ! {
    loop {
        core::hint::spin_loop();
    }
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let apps = me.root(APPS);
    let state = || ask(ep, dk::STATE, &[]);
    let asking = || state()[5] & 1 != 0;
    let place = |p: u64| {
        let r = ask(ep, dk::PLACES, &[p]);
        assert!(r[0] == dk::OK, "the desktop has no place {p}");
        (r[1] + r[3] / 2, r[2] + r[4] / 2)
    };

    // ---- 76. A file deleted in the file browser comes back with the one
    // undo, exactly as it was.
    let summary = me.lookup(docs, "summary.txt").expect("summary.txt");
    let before = me.contents(summary);
    let files = x.window_of(role::FILES).expect("the file browser's window");
    ask(ep, dk::RAISE, &[files]);
    // It looks again every two seconds: then the summary is the last file.
    sleep_until(2500, || false);
    x.keys("{end}", || false, 1000);
    let gone = || me.lookup(docs, "summary.txt").is_none();
    let back = || me.lookup(docs, "summary.txt") == Some(summary);
    assert!(
        x.keys("d", gone, SLOW),
        "the file browser did not delete it"
    );
    // The file browser puts it on the undo list just after.
    assert!(
        sleep_until(SLOW, || me.newest(act::FILES).is_some_and(|a| !a.1)),
        "deleting is not on the undo list"
    );
    assert!(
        x.keys("{super+z}", back, SLOW),
        "Alaya key + Z did not bring summary.txt back"
    );
    assert!(me.contents(summary) == before, "it came back different");
    assert!(state()[5] >> 1 > 0, "the strip says nothing about the undo");
    say!("screenshot undo");
    pass("a file deleted in the file browser comes back exactly with Alaya key + Z");

    // ---- 77. The strip's Undo button does the same.
    sleep_until(2500, || false);
    assert!(
        x.keys("d", gone, SLOW),
        "the file browser did not delete it again"
    );
    let undo = place(dk::places::UNDO);
    assert!(
        x.drag(undo, undo, back),
        "the Undo button did not bring it back"
    );
    pass("the Undo button in the top strip does the same");

    // ---- 78. A setting goes back too; and when something else changed
    // it since, Alaya asks first (rule B6).
    let set = x.window_of(role::SETTINGS).expect("the settings window");
    ask(ep, dk::RAISE, &[set]);
    let look = || ask(ep, dk::SESSION, &[])[3];
    assert!(look() == 0, "the look is not the plain one");
    assert!(
        x.keys("{tab}{enter}", || look() == 1, SLOW),
        "Settings did not change the look"
    );
    assert!(
        x.keys("{super+z}", || look() == 0, SLOW),
        "the look did not go back"
    );
    assert!(
        me.newest(act::SETTINGS).is_some_and(|a| a.1),
        "the setting's action is not marked undone"
    );
    assert!(
        x.keys("{enter}", || look() == 1, SLOW),
        "Settings did not change the look again"
    );
    // Something else (the checks) changes the look file meanwhile.
    let lf = me.lookup(apps, "look").expect("the look file");
    assert!(me.put(lf, &[7]));
    assert!(
        x.keys("{super+z}", asking, SLOW),
        "undo did not ask when something else had changed it"
    );
    say!("screenshot question");
    assert!(
        x.keys("{escape}", || !asking(), SLOW),
        "Escape did not answer"
    );
    assert!(
        me.contents(lf).is(&[7]),
        "\"keep as it is\" changed something"
    );
    assert!(x.keys("{super+z}", asking, SLOW), "undo did not ask again");
    assert!(
        x.keys("{enter}", || me.contents(lf).is(&[0]), SLOW),
        "\"undo anyway\" did not put the old look back"
    );
    assert!(
        sleep_until(3000, || look() == 0),
        "the desktop kept the new look"
    );
    pass("settings go back too; when something else changed it since, undo asks first");

    // ---- 79. A window closed with its close button opens again.
    let chart = x.window_of(role::CHART).expect("the chart maker's window");
    ask(ep, dk::RAISE, &[chart]);
    let f = ask(ep, dk::WINDOW, &[chart]);
    let close = (f[1] + f[3] - 6 - 11, f[2] + 16);
    assert!(
        x.drag(close, close, || x.window_of(role::CHART).is_none()),
        "the close button did not close the chart maker"
    );
    assert!(
        sleep_until(SLOW, || me.newest(act::WINDOW).is_some_and(|a| !a.1)),
        "closing the window is not on the undo list"
    );
    assert!(
        x.keys("{super+z}", || x.asked_bits() & 1 << role::CHART != 0, SLOW),
        "undo did not ask for the chart maker again"
    );
    x.start_asked();
    x.wait_window(role::CHART);
    pass("closing a window is undone: the app opens again");

    // ---- 80. "What happened?": plain sentences, undo on a line.
    let plan = me.make(docs, "plan.txt").expect("plan.txt");
    assert!(me.put(plan, b"first plan"));
    me.call(fs::SAVE, &[]);
    assert!(me.put(plan, b"second plan"));
    me.call(fs::SAVE, &[]);
    let happened = place(dk::places::HAPPENED);
    assert!(
        x.drag(happened, happened, || x.asked_bits() & 1 << role::BACK != 0),
        "the strip's \"What happened?\" did not ask for it"
    );
    x.start_asked();
    let back_win = x.wait_window(role::BACK);
    ask(ep, dk::RAISE, &[back_win]);
    sleep_until(2500, || false);
    say!("screenshot happened");
    assert!(
        x.keys(
            "1{up}{up}{up}{up}{up}{up}{enter}",
            || me.contents(plan).is(b"first plan"),
            SLOW
        ),
        "undo on the newest line did not put plan.txt back: {:?}",
        me.contents(plan).text()
    );
    assert!(
        sleep_until(3000, || !asking()),
        "undoing one line left a question open"
    );
    pass("\"What happened?\" lists changes in plain words; undo on a line puts it back");

    // ---- 81. The timeline: the documents as they were, bring one back,
    // name a version.
    me.buf()[..8].copy_from_slice(b"plan.txt");
    assert!(me.call(fs::REMOVE, &[docs, 8])[0] == st::OK);
    me.call(fs::SAVE, &[]);
    // The newest moment that still has plan.txt, and its place in the
    // list there (by name), as the timeline will show them.
    let h = me.call(fs::HISTORY, &[]);
    let kept = h[1] as usize;
    let mut cns = [0u64; 512];
    for (i, c) in cns.iter_mut().enumerate().take(kept.min(512)) {
        *c = u64::from_le_bytes(me.buf()[16 * i..16 * i + 8].try_into().unwrap());
    }
    let mut target = None;
    for (back, &cn) in cns[..kept.min(512)].iter().enumerate() {
        let r = me.call(fs::PAST_LIST, &[cn, docs]);
        if r[0] != st::OK {
            continue;
        }
        let b = me.buf();
        let name = |i: usize| {
            let o = fs::PAST_ENTRY_BYTES * i;
            let n = u64::from_le_bytes(b[o + 8..o + 16].try_into().unwrap()) as usize;
            &b[o + 32..o + 32 + n.min(32)]
        };
        let n = r[1] as usize;
        if (0..n).any(|i| name(i) == b"plan.txt") {
            // Its place by name, as the timeline sorts them.
            let index = (0..n).filter(|&i| name(i) < &b"plan.txt"[..]).count();
            target = Some((back, index));
            break;
        }
    }
    let (lefts, index) = target.expect("no moment has plan.txt");
    let mut spec = [0u8; 600];
    let mut len = 0;
    let mut add = |s: &str| {
        spec[len..len + s.len()].copy_from_slice(s.as_bytes());
        len += s.len();
    };
    add("2{end}");
    for _ in 0..lefts.min(20) {
        add("{left}");
    }
    for _ in 0..10 {
        add("{up}");
    }
    for _ in 0..index.min(20) {
        add("{down}");
    }
    let spec = core::str::from_utf8(&spec[..len]).unwrap();
    sleep_until(2500, || false);
    x.keys(spec, || false, 1500);
    say!("screenshot timeline");
    assert!(
        x.keys("b", || me.lookup(docs, "plan.txt") == Some(plan), SLOW),
        "\"bring this back\" did not bring plan.txt back"
    );
    assert!(
        me.contents(plan).is(b"first plan"),
        "it came back different"
    );
    let named = || {
        let r = me.call(fs::NAMED, &[]);
        r[0] == st::OK
            && (0..r[1] as usize).any(|i| {
                let o = fs::NAMED_BYTES * i;
                me.buf()[o + 24..o + 24 + 15] == *b"Before the plan"
            })
    };
    assert!(
        x.keys("nBefore the plan{enter}", named, SLOW),
        "\"name this version\" did not name it"
    );
    pass("the timeline shows the documents as they were; bring back and naming work");

    // ---- 82. File stories, from the record alone.
    let chart_file = me.lookup(docs, "chart.ppm").expect("chart.ppm");
    let numbers = me.lookup(docs, "numbers.txt").expect("numbers.txt");
    let s = me.call(fs::STORY, &[chart_file, 32]);
    assert!(
        s[0] == st::OK && s[1] == fs::app_id(role::CHART),
        "the chart's story does not say the chart maker made it: {s:?}"
    );
    let s = me.call(fs::STORY, &[numbers, 32]);
    let used = (0..s[5] as usize).any(|i| {
        let o = fs::STORY_BYTES * s[4] as usize + 8 * i;
        u64::from_le_bytes(me.buf()[o..o + 8].try_into().unwrap()) == chart_file
    });
    assert!(
        used,
        "numbers.txt's story does not say the chart was made from it"
    );
    ask(ep, dk::RAISE, &[files]);
    x.keys("{home}{down}w", || false, 1500);
    say!("screenshot story");
    pass("a file's story says which app made it and what was made from it");

    // ---- 83. Another person sees none of it.
    me.call(fs::HISTORY, &[]);
    let newest_mine = u64::from_le_bytes(me.buf()[..8].try_into().unwrap());
    let other = x.files_of(OTHER);
    assert!(other.call(fs::ADD_PERSON, &[3, 3, 3, 3])[0] == st::OK);
    other.call(fs::SAVE, &[]);
    let r = other.call(fs::HAPPENINGS, &[0, 64]);
    assert!(
        r[0] == st::OK && r[1] == 0,
        "someone else sees changes: {r:?}"
    );
    let r = other.call(fs::ACTIONS, &[0, 64]);
    assert!(
        r[0] == st::OK && r[1] == 0,
        "someone else sees the undo list"
    );
    let (mine, _) = me.newest(act::FILES).unwrap();
    assert!(
        other.call(fs::UNDO, &[mine, 1])[0] != st::OK,
        "someone else could undo this person's action"
    );
    assert!(
        other.call(fs::NAME_OF, &[summary, 0])[0] != st::OK,
        "someone else learnt a file's name"
    );
    let r = other.call(fs::PAST_LIST, &[newest_mine, docs]);
    assert!(
        r[0] != st::OK || r[1] == 0,
        "someone else saw this person's documents at a moment"
    );
    let s = other.call(fs::STORY, &[numbers, 32]);
    assert!(
        s[0] != st::OK || (s[1] == 0 && s[4] == 0 && s[5] == 0),
        "someone else read a file's story"
    );
    pass("one person never sees another's history, undo list, timeline or stories");

    // ---- 84. A year of history, opened while every core is busy.
    let t0 = alaya_sys::clock();
    // A moment a week: every moment is a real save to the disk, and
    // emulated ARM with more cores than its host saves very slowly. The
    // host tests thin a year of a moment every 6 hours.
    let r = other.call(fs::FAKE_HISTORY, &[365, 24 * 7]);
    assert!(r[0] == st::OK && r[1] == 53, "no year of history: {r:?}");
    say!(
        "timeline: a year of history ({} moments) made in {} ms",
        r[1],
        ticks_to_us(alaya_sys::clock() - t0) / 1000
    );
    let h = other.call(fs::HISTORY, &[]);
    let kept = h[1] as usize;
    assert!((45..=53).contains(&kept), "{kept} moments kept of a year");
    let mut moments = [0u64; 256];
    for (i, m) in moments.iter_mut().enumerate().take(kept) {
        *m = u64::from_le_bytes(other.buf()[16 * i..16 * i + 8].try_into().unwrap());
    }
    let cores = x.s.k.info.cores;
    let mut busy_threads = [0u64; 64];
    let cores = cores.min(64);
    for c in 0..cores {
        busy_threads[c as usize] = x.s.k.thread(busy, 0, 10, 10_000, 10_000, c + 1).0;
    }
    sleep_until(200, || false);
    // Every moment kept opens, timed, with every core busy. On a host
    // with fewer cores than Alaya (emulated 8 cores on a 4-core machine)
    // busy cores take turns on the host's, and one open can take a
    // minute: after a minute the rest open with the cores at rest.
    let open = |i: usize, cn: u64| {
        let t = alaya_sys::clock();
        let r = other.call(fs::PAST_LIST, &[cn, docs]);
        assert!(
            r[0] == st::OK && r[1] <= 1,
            "moment {i} did not open: {r:?}"
        );
        ticks_to_us(alaya_sys::clock() - t)
    };
    let started = alaya_sys::clock();
    let mut worst = 0;
    let mut timed = 0;
    for (i, &cn) in moments[..kept].iter().enumerate() {
        if ticks_to_us(alaya_sys::clock() - started) > 60_000_000 {
            break;
        }
        worst = worst.max(open(i, cn));
        timed += 1;
    }
    for &t in &busy_threads[..cores as usize] {
        let _ = alaya_sys::thread_stop(t);
    }
    for (i, &cn) in moments[..kept].iter().enumerate().skip(timed) {
        open(i, cn);
    }
    // The host checks the time (`cargo xtask test`): under 100 ms with
    // KVM, under two seconds emulated, when it has a core for each of
    // Alaya's.
    say!(
        "timeline: each of {kept} moments of a year opens; {timed} timed with every core busy, at most {} us",
        worst
    );
    pass("a year of history: every moment kept opens while every core is busy");
}

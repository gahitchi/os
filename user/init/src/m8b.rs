// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step two: updates that cannot take more on their
//! own, and a rehearsal on a copy of your files. The boot image carries
//! Postcards in five versions (`xtask/src/bundles.rs`): 2 asks for
//! nothing new, 3 also wants the network, 4 is slow to start, 5 is slow
//! too but a security fix.

use alaya_bundle::promise;
use alaya_programs::desktop::{self as dk, installed as ins, role};
use alaya_programs::files::{DOCS, act};

use crate::m3::sleep_until;
use crate::m6b::{Session, ask};
use crate::m8a::{example, place, put};

/// Close the window of installed place 0 with its close button, as a
/// person would.
fn close_it(x: &Session<'_>) {
    let w = x.window_of(role::INSTALLED).expect("its window");
    ask(x.ep, dk::RAISE, &[w]);
    let f = ask(x.ep, dk::WINDOW, &[w]);
    let close = (f[1] + f[3] - 6 - 11, f[2] + 16);
    assert!(
        x.drag(close, close, || x.window_of(role::INSTALLED).is_none()),
        "the close button did not close it"
    );
}

/// Act on the desktop's end bits until place 0's app has ended (or not).
fn ended(x: &mut Session<'_>) -> bool {
    for _ in 0..40 {
        x.pause_now();
        if !x.app_running(role::INSTALLED) {
            return true;
        }
        sleep_until(250, || false);
    }
    false
}

const SLOW: u64 = 30_000;

/// Place 0's trial, as the desktop sees it: (starts left on trial, what
/// it tried, the version the Guardian went back from, flags, the usual
/// start in ms, the last start in ms, the version kept for undo).
struct Trial {
    left: u64,
    tried: u64,
    went_back_from: u64,
    asks: u64,
    flags: u64,
    usual: u64,
    last: u64,
    kept: u64,
}

fn trial(ep: u64) -> Trial {
    let r = ask(ep, dk::TRIAL, &[0]);
    assert!(r[0] == dk::OK);
    Trial {
        left: r[1] & 0xff,
        tried: (r[1] >> 8) & 0xff,
        went_back_from: (r[1] >> 16) & 0xffff,
        asks: (r[1] >> 32) & 0xffff,
        flags: r[1] >> 48,
        usual: r[2],
        last: r[3],
        kept: r[4],
    }
}

fn version(ep: u64) -> u64 {
    ask(ep, dk::INSTALLED, &[0])[1] >> 32
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let v: [u64; 5] = core::array::from_fn(|i| {
        let name = [
            "Postcards-1.alaya",
            "Postcards-2.alaya",
            "Postcards-3.alaya",
            "Postcards-4.alaya",
            "Postcards-5.alaya",
        ][i];
        put(&me, docs, name, example(x, i), None)
    });

    // ---- 114. An update asking for nothing new is taken at once, and
    // goes on trial: what it tries beyond what you allowed is noted.
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[0]])[0] == dk::OK);
    assert!(sleep_until(SLOW, || place(ep).2 == 1), "no install sheet");
    // Notifications are the second line: untick them.
    assert!(
        x.keys("{down}{space}{enter}", || place(ep).0 == ins::ASIDE, SLOW),
        "not installed on the side: {:?}",
        place(ep)
    );
    assert!(x.keys("{enter}", || place(ep).0 == ins::ON, SLOW));
    let may = place(ep).1;
    assert!(
        may & promise::FILES != 0 && may & promise::NOTIFY == 0,
        "notifications were not unticked: {may:#x}"
    );
    // A few starts, so Alaya knows how long it usually takes.
    for _ in 0..3 {
        x.start_app(role::INSTALLED);
        x.wait_window(role::INSTALLED);
    }
    assert!(sleep_until(SLOW, || trial(ep).usual > 0));
    crate::say!("Postcards 1 usually starts in {} ms", trial(ep).usual);
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[1]])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || version(ep) == 2 && place(ep).2 == 4),
        "the update asking for nothing new was held back: version {}",
        version(ep)
    );
    let t = trial(ep);
    assert!(t.left == u64::from(ins::TRIAL_STARTS) && t.kept == 1 && t.tried == 0);
    assert!(
        place(ep).1 & promise::NOTIFY == 0,
        "the update ticked a line"
    );
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    x.start_app(role::INSTALLED);
    x.wait_window(role::INSTALLED);
    assert!(
        sleep_until(SLOW, || trial(ep).tried >= 1),
        "its refused notification was not noted"
    );
    assert!(trial(ep).left == u64::from(ins::TRIAL_STARTS) - 1);
    // Without the background line, its last window closing ends it.
    close_it(x);
    assert!(ended(x), "it kept running with no window");
    pass(
        "an update asking for nothing new is taken at once and goes on trial; what it tried beyond what you allowed is noted; with no window and no background line, it ends",
    );

    // ---- 115. An update that asks for more is taken at once with what
    // you allowed before (decision 88); the new line stays off until the
    // app first tries it, and then Alaya asks.
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[2]])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || version(ep) == 3 && place(ep).2 == 4),
        "the update was held back: version {}, {:?}",
        version(ep),
        place(ep)
    );
    let may = place(ep).1;
    assert!(
        may & promise::BACKGROUND == 0 && may & promise::FILES != 0 && may & promise::NOTIFY == 0,
        "the update's promise grew by itself: {may:#x}"
    );
    assert!(trial(ep).asks == u64::from(promise::BACKGROUND));
    crate::say!("screenshot update");
    sleep_until(500, || false);
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    x.start_app(role::INSTALLED);
    x.wait_window(role::INSTALLED);
    assert!(
        sleep_until(SLOW, || place(ep).2 == 5),
        "no question when it first tried the new line: {:?}",
        place(ep)
    );
    assert!(
        x.keys("{enter}", || place(ep).1 & promise::BACKGROUND != 0, SLOW),
        "Allow did not give it the line"
    );
    assert!(trial(ep).asks == 0);
    // Now it keeps running with no window.
    close_it(x);
    assert!(
        !ended(x),
        "it was ended although it may run in the background"
    );
    pass(
        "an update that asks for more is taken at once with what you allowed before; the new line is asked about when the app first tries it",
    );

    // ---- 116. One undo takes an update back.
    // (On its line in "What happened?": closing the window came after.)
    let (update, _) = me
        .newest(act::SETTINGS)
        .expect("the update on the undo list");
    assert!(ask(ep, dk::UNDO, &[dk::undo_by::ACTION, update, 0])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || version(ep) == 2),
        "undo did not take the update back: version {}",
        version(ep)
    );
    // The version swapped out ends.
    assert!(ended(x), "the version taken back kept running");
    assert!(trial(ep).kept == 3 && place(ep).0 == ins::ON);
    x.start_app(role::INSTALLED);
    x.wait_window(role::INSTALLED);
    pass("one undo takes an update back to the version before");

    // ---- 117. The Experience Guardian: a clearly slower update goes
    // back by itself; a slower security fix is kept.
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[3]])[0] == dk::OK);
    assert!(sleep_until(SLOW, || version(ep) == 4));
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    x.start_app(role::INSTALLED);
    assert!(
        sleep_until(SLOW * 2, || version(ep) == 2),
        "the slow version was kept: version {}, {} ms (usually {})",
        version(ep),
        trial(ep).last,
        trial(ep).usual
    );
    let t = trial(ep);
    assert!(t.went_back_from == 4 && t.kept == 4);
    crate::say!(
        "the Guardian went back after a start of {} ms (usually {})",
        t.last,
        t.usual
    );
    assert!(sleep_until(SLOW, || x.window_of(role::INSTALLED).is_none()));
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[4]])[0] == dk::OK);
    assert!(sleep_until(SLOW, || version(ep) == 5));
    assert!(trial(ep).flags & u64::from(alaya_bundle::flags::SECURITY_FIX) != 0);
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    x.start_app(role::INSTALLED);
    assert!(
        sleep_until(SLOW * 2, || trial(ep).left < u64::from(ins::TRIAL_STARTS)),
        "the security fix never started"
    );
    let t = trial(ep);
    assert!(
        version(ep) == 5,
        "a security fix was taken back ({} ms, usually {})",
        t.last,
        t.usual
    );
    assert!(t.last > t.usual, "version 5 was not slow: {} ms", t.last);
    pass(
        "the Experience Guardian: a clearly slower update goes back by itself; a security fix is never taken back",
    );

    // ---- 118. A rehearsal: the app runs for real on a side copy of
    // your documents; Alaya shows what it read and would have changed,
    // then throws the copy away. Nothing real changes.
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[4]])[0] == dk::OK);
    assert!(sleep_until(SLOW, || place(ep).2 == 3));
    assert!(
        x.keys("{delete}", || place(ep).2 == 4, SLOW),
        "Delete did not remove it"
    );
    x.pause_now();
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    let card = me
        .lookup(docs, "postcard.txt")
        .expect("the postcard from check 113");
    let mut before = [0u8; 512];
    let n = me.read_at(card, 0, &mut before).unwrap_or(0);
    assert!(n > 0);
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[0]])[0] == dk::OK);
    assert!(sleep_until(SLOW, || place(ep).2 == 1), "no install sheet");
    assert!(
        x.keys("r", || place(ep).2 == 6, SLOW),
        "R did not start a rehearsal: {:?}",
        place(ep)
    );
    assert!(place(ep).0 == ins::REHEARSAL);
    x.store_given = None;
    x.start_app(role::INSTALLED);
    let w = x.wait_window(role::INSTALLED);
    assert!(x.store_given == Some(true));
    ask(ep, dk::RAISE, &[w]);
    sleep_until(1500, || false);
    // It adds a line to the postcard, on the side copy.
    x.keys("{ctrl+s}", || false, 3000);
    sleep_until(3000, || false);
    crate::say!("screenshot rehearsal");
    assert!(
        x.keys("{super+r}", || place(ep).2 == 7, SLOW),
        "the rehearsal did not end: {:?}",
        place(ep)
    );
    x.pause_now();
    assert!(sleep_until(SLOW, || x.window_of(role::INSTALLED).is_none()));
    let r = ask(ep, dk::TRIAL, &[0])[5];
    let (read, changed, gone) = (r & 0xffff, (r >> 16) & 0xffff, r >> 32);
    crate::say!("the rehearsal read {read} files and would have changed {changed}");
    assert!(read >= 1 && changed >= 1, "the rehearsal showed nothing");
    assert!(gone == 1, "the side copy was not thrown away");
    assert!(
        place(ep).0 == ins::EMPTY,
        "the rehearsal installed something"
    );
    let mut after = [0u8; 512];
    let m = me.read_at(card, 0, &mut after).unwrap_or(0);
    assert!(
        m == n && after[..m] == before[..n],
        "the rehearsal reached the real postcard"
    );
    assert!(
        x.keys("{enter}", || place(ep).2 == 1, SLOW),
        "not back to the install sheet"
    );
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    pass(
        "a rehearsal: the app runs on a side copy of your documents; Alaya shows what it read and would have changed, then throws the copy away; nothing real changes",
    );

    // ---- 119. The installer under a fuzzer: bundles changed at random
    // places are all refused, and nothing is installed.
    let f = v[0];
    let good = example(x, 0);
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    const TRIES: usize = 40;
    for t in 0..TRIES {
        // Half in the head (name, promise, flags, key, signature), half
        // anywhere.
        let at = if t % 2 == 0 {
            (next() % alaya_bundle::HEAD as u64) as usize
        } else {
            (next() % good.len() as u64) as usize
        };
        let flip = 1u8 << (next() % 8);
        me.buf()[0] = good[at] ^ flip;
        assert!(
            me.call(alaya_programs::files::WRITE, &[f, at as u64, 1])[0]
                == alaya_programs::files::st::OK
        );
        assert!(ask(ep, dk::OFFER_INSTALL, &[f])[0] == dk::OK);
        assert!(
            sleep_until(SLOW, || place(ep).2 == 4),
            "a bundle changed at byte {at} was not refused: {:?}",
            place(ep)
        );
        assert!(place(ep).0 == ins::EMPTY, "a changed bundle was installed");
        me.buf()[0] = good[at];
        assert!(
            me.call(alaya_programs::files::WRITE, &[f, at as u64, 1])[0]
                == alaya_programs::files::st::OK
        );
    }
    assert!(ask(ep, dk::OFFER_INSTALL, &[f])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || place(ep).2 == 1),
        "the good bundle was refused after"
    );
    pass(
        "the installer under a fuzzer: every bundle changed at a random byte is refused, and nothing is installed",
    );

    // ---- 120. Install, then update and undo many times in a row.
    assert!(x.keys("{enter}", || place(ep).0 == ins::ASIDE, SLOW));
    assert!(x.keys("{enter}", || place(ep).0 == ins::ON, SLOW));
    const ROUNDS: u64 = 50;
    let t0 = alaya_sys::rt::now_ms();
    for i in 0..ROUNDS {
        assert!(ask(ep, dk::OFFER_INSTALL, &[v[1]])[0] == dk::OK);
        assert!(
            sleep_until(SLOW, || version(ep) == 2),
            "round {i}: the update was not taken"
        );
        assert!(ask(ep, dk::UNDO, &[dk::undo_by::LAST, 0, 0])[0] == dk::OK);
        assert!(
            sleep_until(SLOW, || version(ep) == 1),
            "round {i}: undo did not take the update back"
        );
    }
    crate::say!(
        "{ROUNDS} updates and undos took {} ms",
        alaya_sys::rt::now_ms() - t0
    );
    assert!(place(ep).0 == ins::ON && trial(ep).kept == 2);
    x.start_app(role::INSTALLED);
    x.wait_window(role::INSTALLED);
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    pass(
        "an app installed, then updated and undone 50 times in a row: it still starts, at the right version",
    );
}

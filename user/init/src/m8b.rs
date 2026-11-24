// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step two: updates that cannot take more on their
//! own. The boot image carries Postcards in five versions
//! (`xtask/src/bundles.rs`): 2 asks for nothing new, 3 also wants the
//! network, 4 is slow to start, 5 is slow too but a security fix.

use alaya_bundle::promise;
use alaya_programs::desktop::{self as dk, installed as ins, role};
use alaya_programs::files::DOCS;

use crate::m3::sleep_until;
use crate::m6b::{Session, ask};
use crate::m8a::{example, place, put};

const SLOW: u64 = 30_000;

/// Place 0's trial, as the desktop sees it: (starts left on trial, what
/// it tried, the version the Guardian went back from, flags, the usual
/// start in ms, the last start in ms, the version kept for undo).
struct Trial {
    left: u64,
    tried: u64,
    went_back_from: u64,
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
        went_back_from: (r[1] >> 16) & 0xffff_ffff,
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
    pass(
        "an update asking for nothing new is taken at once and goes on trial; what it tried beyond what you allowed is noted",
    );

    // ---- 115. An update that asks for more waits on a difference
    // screen; "not now" keeps the old one, and new lines stay off.
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[2]])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || place(ep).2 == 5),
        "no difference screen: {:?}",
        place(ep)
    );
    assert!(version(ep) == 2, "it updated before you said so");
    crate::say!("screenshot update");
    sleep_until(500, || false);
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    assert!(version(ep) == 2, "not now still updated it");
    assert!(ask(ep, dk::OFFER_INSTALL, &[v[2]])[0] == dk::OK);
    assert!(sleep_until(SLOW, || place(ep).2 == 5));
    assert!(
        x.keys("{enter}", || version(ep) == 3, SLOW),
        "Update did nothing"
    );
    let may = place(ep).1;
    assert!(
        may & promise::NETWORK == 0 && may & promise::FILES != 0 && may & promise::NOTIFY == 0,
        "the update's promise grew by itself: {may:#x}"
    );
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    pass(
        "an update that asks for more waits on a difference screen; not now keeps the old version, and its new lines stay off",
    );

    // ---- 116. One undo takes an update back.
    assert!(
        x.keys("{super+z}", || version(ep) == 2, SLOW),
        "undo did not take the update back: version {}",
        version(ep)
    );
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
}

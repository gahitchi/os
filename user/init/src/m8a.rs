// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step one: signed apps you install and approve.
//! The boot image carries the example app Postcards as a signed bundle
//! (`example-apps`); the checks put it, and a copy changed after signing,
//! among the person's files and go through Alaya's install sheet with the
//! keyboard, as a person would.

use alaya_programs::desktop::{self as dk, installed as ins, role};
use alaya_programs::files::{self as fs, DOCS, st};

use crate::m3::sleep_until;
use crate::m4b::module;
use crate::m6b::{Files, Session, ask};

const SLOW: u64 = 30_000;

/// The first bundle in the boot image's `example-apps`.
fn postcards<'a>(x: &Session<'a>) -> &'a [u8] {
    let all = module(x.s.k.info, "example-apps");
    let n = u32::from_le_bytes(all[..4].try_into().unwrap()) as usize;
    &all[4..4 + n]
}

/// Write `data` as the document `name` (made if missing), in pieces.
fn put(f: &Files, dir: u64, name: &str, data: &[u8], flip: Option<usize>) -> u64 {
    f.buf()[..name.len()].copy_from_slice(name.as_bytes());
    let r = f.call(fs::CREATE, &[dir, 0, name.len() as u64]);
    let file = if r[0] == st::OK {
        r[1]
    } else {
        f.lookup(dir, name).expect("the bundle's file")
    };
    let mut at = 0;
    while at < data.len() {
        let n = (data.len() - at).min(fs::XFER_BYTES);
        f.buf()[..n].copy_from_slice(&data[at..at + n]);
        if let Some(i) = flip
            && (at..at + n).contains(&i)
        {
            f.buf()[i - at] ^= 0x20;
        }
        assert!(f.call(fs::WRITE, &[file, at as u64, n as u64])[0] == st::OK);
        at += n;
    }
    assert!(f.call(fs::TRUNCATE, &[file, data.len() as u64])[0] == st::OK);
    assert!(f.call(fs::SAVE, &[])[0] == st::OK);
    file
}

/// Place 0 as the desktop sees it: (state, what it may, the sheet's stage).
fn place(ep: u64) -> (u8, u32, u64) {
    let r = ask(ep, dk::INSTALLED, &[0]);
    assert!(r[0] == dk::OK);
    (r[1] as u8, (r[1] >> 8) as u32, r[3])
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let bundle = postcards(x);
    say_size(bundle.len());
    let me = x.files();
    let docs = me.root(DOCS);
    let good = put(&me, docs, "Postcards.alaya", bundle, None);
    // One byte of the program changed after the maker signed it.
    let broken = put(&me, docs, "Broken.alaya", bundle, Some(bundle.len() / 2));

    // ---- 110. A changed bundle is refused, and nothing changes.
    assert!(ask(ep, dk::OFFER_INSTALL, &[broken])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || place(ep).2 == 4),
        "the changed bundle was not refused: {:?}",
        place(ep)
    );
    assert!(place(ep).0 == ins::EMPTY, "something was installed");
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    pass("an app bundle changed after its maker signed it is refused, and nothing changes");

    // ---- 111. Install with a line unticked, on the side, then approve.
    assert!(ask(ep, dk::OFFER_INSTALL, &[good])[0] == dk::OK);
    assert!(sleep_until(SLOW, || place(ep).2 == 1), "no install sheet");
    // The first line is files: untick it, then install on the side.
    assert!(
        x.keys("{space}{enter}", || place(ep).0 == ins::ASIDE, SLOW),
        "Enter did not install it on the side: {:?}",
        place(ep)
    );
    let (_, may, _) = place(ep);
    assert!(
        may & alaya_bundle::promise::FILES == 0 && may & alaya_bundle::promise::NOTIFY != 0,
        "the unticked line was kept: {may:#x}"
    );
    // On the side, it is not in the dock and cannot start.
    x.start_app(role::INSTALLED);
    assert!(x.window_of(role::INSTALLED).is_none());
    assert!(
        x.keys("{enter}", || place(ep).0 == ins::ON, SLOW),
        "Approve did not put it in the dock"
    );
    x.store_given = None;
    x.start_app(role::INSTALLED);
    let w = x.wait_window(role::INSTALLED);
    assert!(
        x.store_given == Some(false),
        "the app got a way to the store although files were unticked"
    );
    ask(ep, dk::RAISE, &[w]);
    pass(
        "installed on the side, then approved; an unticked line is really not given: the app has no way to the store",
    );

    // ---- 112. One undo takes the install away.
    assert!(
        x.keys("{super+z}", || place(ep).0 == ins::EMPTY, SLOW),
        "undo did not take the install away: {:?}",
        place(ep)
    );
    assert!(sleep_until(SLOW, || x.window_of(role::INSTALLED).is_none()));
    x.start_app(role::INSTALLED);
    assert!(x.window_of(role::INSTALLED).is_none());
    pass("one undo takes an installed app away: its window closes and it cannot start");

    // ---- 113. Installed with files, it writes; removed, a receipt.
    assert!(ask(ep, dk::OFFER_INSTALL, &[good])[0] == dk::OK);
    assert!(sleep_until(SLOW, || place(ep).2 == 1));
    assert!(x.keys("{enter}", || place(ep).0 == ins::ASIDE, SLOW));
    assert!(x.keys("{enter}", || place(ep).0 == ins::ON, SLOW));
    x.store_given = None;
    x.start_app(role::INSTALLED);
    let w = x.wait_window(role::INSTALLED);
    assert!(
        x.store_given == Some(true),
        "files were allowed, yet no store"
    );
    ask(ep, dk::RAISE, &[w]);
    let card = || me.lookup(docs, "postcard.txt").is_some();
    assert!(
        x.keys("{ctrl+s}", card, SLOW),
        "Postcards did not write its postcard"
    );
    assert!(ask(ep, dk::OFFER_INSTALL, &[good])[0] == dk::OK);
    assert!(
        sleep_until(SLOW, || place(ep).2 == 3),
        "not shown as installed"
    );
    crate::say!("screenshot install");
    sleep_until(500, || false);
    assert!(
        x.keys("{delete}", || place(ep).2 == 4, SLOW),
        "Delete did not remove it"
    );
    let r = ask(ep, dk::INSTALLED, &[0]);
    let (gone, left, made) = (r[4] & 0xffff, r[4] >> 16, r[5] >> 32);
    crate::say!("receipt: role {gone}, {left} left, {made} files it made");
    assert!(gone == role::INSTALLED && left == 0 && made >= 1);
    assert!(place(ep).0 == ins::EMPTY);
    assert!(sleep_until(SLOW, || x.window_of(role::INSTALLED).is_none()));
    assert!(
        me.lookup(docs, "postcard.txt").is_some(),
        "its file went too"
    );
    assert!(x.keys("{escape}", || place(ep).2 == 0, SLOW));
    pass(
        "removing an app ends it, takes back what it was handed and leaves a receipt checked on the store; what it made stays yours",
    );
}

fn say_size(n: usize) {
    crate::say!("the Postcards bundle is {} KiB", n / 1024);
}

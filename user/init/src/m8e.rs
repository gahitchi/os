// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 8 checks, step four, second part: a game written in C on
//! the SDL subset, and saved games with history (decision 161).

use alaya_programs::desktop::{self as dk, role};
use alaya_programs::files::{self as fs, DOCS, st};

use crate::m3::sleep_until;
use crate::m6b::{Files, Session, ask};

const SLOW: u64 = 30_000;

/// The game's save as the person sees it in their documents.
fn save_of(me: &Files, docs: u64, out: &mut [u8; 512]) -> usize {
    let Some(top) = me.lookup(docs, fs::SAVED_GAMES_FOLDER) else {
        return 0;
    };
    let Some(game) = me.lookup(top, "Glowroot") else {
        return 0;
    };
    let Some(f) = me.lookup(game, "save.txt") else {
        return 0;
    };
    me.read_at(f, 0, out).unwrap_or(0)
}

fn save_is(me: &Files, docs: u64, want: &[u8]) -> bool {
    let mut b = [0u8; 512];
    let n = save_of(me, docs, &mut b);
    &b[..n] == want
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let me = x.files();
    let docs = me.root(DOCS);
    let person = x.session()[2];

    // ---- 126. A C game, its saves kept with history.
    x.start_app(role::GAME);
    let w = x.wait_window(role::GAME);
    ask(ep, dk::RAISE, &[w]);
    // The first save: where the garden starts.
    let has_save = || {
        let mut b = [0u8; 512];
        save_of(&me, docs, &mut b) > 0
    };
    assert!(x.keys("s", has_save, SLOW), "the game did not save");
    let mut first = [0u8; 512];
    let n1 = save_of(&me, docs, &mut first);
    let first = &first[..n1];
    assert!(
        first.starts_with(b"glowroot save 1\nlevel 1\npath "),
        "not a save: {:?}",
        core::str::from_utf8(first)
    );
    // Fly (one of the four ways is always open from the start), save
    // again: a new version.
    assert!(
        x.keys(
            "{up}{right}{down}{left}s",
            || {
                let mut b = [0u8; 512];
                let n = save_of(&me, docs, &mut b);
                n > 0 && &b[..n] != first
            },
            SLOW
        ),
        "the second save did not come"
    );
    // The game's saves, as the game sees them: every save is there.
    let game = x.files_as(person, fs::app_id(role::GAME));
    let f = {
        let b = game.buf();
        b[..8].copy_from_slice(b"Glowroot");
        let r = game.call(fs::SAVED_GAMES, &[8]);
        assert!(r[0] == st::OK, "the game's folder: {}", r[0]);
        game.lookup(r[1], "save.txt").expect("its save")
    };
    // (A save is written, then made safe: its version counts once the
    // store has saved it.)
    let versions = || game.call(fs::SAVE_VERSIONS, &[f, 8]);
    assert!(
        sleep_until(SLOW, || {
            let r = versions();
            r[0] == st::OK && r[1] >= 2
        }),
        "saves kept: {:?}",
        versions()
    );
    let r = versions();
    // The oldest version still reads as the first save.
    let n = r[1] as usize;
    let oldest = u64::from_le_bytes(
        game.buf()[16 * (n - 1)..16 * (n - 1) + 8]
            .try_into()
            .unwrap(),
    );
    let r = game.call(fs::READ_PAST, &[f, oldest, 0, 512]);
    assert!(r[0] == st::OK, "reading an older save: {}", r[0]);
    let mut b = [0u8; 512];
    let k = (r[1] as usize).min(512);
    b[..k].copy_from_slice(&game.buf()[..k]);
    // (The oldest may be the empty file before the first write.)
    assert!(
        k == 0 || &b[..k] == first,
        "the oldest save changed: {:?}",
        core::str::from_utf8(&b[..k])
    );
    // In the game: H shows the saves, newest first; the one below is
    // the first save; Enter brings it back, S saves it as the newest.
    assert!(
        x.keys("h{down}{enter}s", || save_is(&me, docs, first), SLOW),
        "the older save did not come back"
    );
    assert!(
        sleep_until(SLOW, || {
            let r = versions();
            r[0] == st::OK && r[1] as usize > n
        }),
        "bringing a save back lost the newer ones: {:?}",
        versions()
    );
    // Another app cannot take the game's folder.
    let other = x.files_as(person, fs::app_id(role::CHART));
    other.buf()[..8].copy_from_slice(b"Glowroot");
    let r = other.call(fs::SAVED_GAMES, &[8]);
    assert!(
        r[0] == st::EXISTS,
        "another app took the game's saves: {}",
        r[0]
    );
    // Nor read its saves' history.
    let r = other.call(fs::SAVE_VERSIONS, &[f, 8]);
    assert!(r[0] != st::OK, "another app read the game's save history");
    // Full screen, chosen by the person, turns game mode on.
    let game_on = || ask(ep, dk::GAME, &[])[1] == 1;
    assert!(x.keys("{super+up}", game_on, SLOW), "no game mode");
    assert!(
        x.keys("{super+down}", || !game_on(), SLOW),
        "game mode stayed on"
    );
    assert!(sleep_until(SLOW, || !game_on()));
    pass(
        "a game written in C runs on the SDL subset; every save is kept in Saved games, an older one comes back without losing the newer ones, and no other app can reach them",
    );
}

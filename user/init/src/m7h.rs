// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7b checks, step three: typing in many languages. Keys go
//! through the desktop's input method; what it hands to the window in
//! front is checked (`dk::TYPED`): a German keyboard with a dead key,
//! Compose, Japanese from romaji, Chinese from pinyin, the emoji picker
//! and the on-screen keyboard.

use alaya_programs::desktop::{self as dk, role};

use crate::m3::sleep_until;
use crate::m6b::{Session, ask};

const SLOW: u64 = 30_000;

/// How many characters the input method handed over, and the last ones.
fn typed(ep: u64) -> (u64, [u8; 32]) {
    let r = ask(ep, dk::TYPED, &[]);
    let mut b = [0u8; 32];
    for k in 0..4 {
        b[k * 8..k * 8 + 8].copy_from_slice(&r[2 + k].to_le_bytes());
    }
    (r[1], b)
}

/// Whether the last typed text ends with `s`.
fn ends(ep: u64, s: &str) -> bool {
    let (_, b) = typed(ep);
    let n = b.iter().position(|&x| x == 0).unwrap_or(32);
    b[..n].ends_with(s.as_bytes())
}

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;
    let editor = x.window_of(role::EDITOR).expect("the editor's window");
    ask(ep, dk::RAISE, &[editor]);
    let mode = || ask(ep, dk::TYPING, &[0, u64::MAX])[2];

    // ---- 107. Keyboards: German (the US y key is z there; ´ then e is
    // é), and Compose (a, e: æ).
    assert!(ask(ep, dk::TYPING, &[0, 2])[1] == 2, "no German keyboard");
    assert!(
        x.keys("y", || ends(ep, "z"), SLOW),
        "the German keyboard did not give z: {:?}",
        typed(ep)
    );
    // The key US calls = is the German dead acute.
    assert!(
        x.keys("=e", || ends(ep, "é"), SLOW),
        "´ then e did not give é: {:?}",
        typed(ep)
    );
    assert!(
        x.keys("{compose}ae", || ends(ep, "æ"), SLOW),
        "Compose a e did not give æ: {:?}",
        typed(ep)
    );
    assert!(ask(ep, dk::TYPING, &[0, 0])[1] == 0);
    pass("keyboards: a German layout with a dead key, and Compose, give the right letters");

    // ---- 108. Japanese from romaji, Chinese from pinyin.
    assert!(
        x.keys("{super+space}", || mode() == 1, SLOW),
        "Alaya key + Space did not switch to Japanese"
    );
    assert!(
        x.keys("nihongo{enter}", || ends(ep, "にほんご"), SLOW),
        "nihongo did not become にほんご: {:?}",
        typed(ep)
    );
    assert!(x.keys("{super+space}", || mode() == 2, SLOW));
    // Space shows the characters, Space again takes the first.
    assert!(
        x.keys("nihao  ", || ends(ep, "你好"), SLOW),
        "nihao did not become 你好: {:?}",
        typed(ep)
    );
    assert!(x.keys("{super+space}", || mode() == 0, SLOW));
    pass("Japanese typed as romaji becomes kana; Chinese pinyin becomes characters");

    // ---- 109. The emoji picker, and the on-screen keyboard.
    let place = |what: u64| {
        let r = ask(ep, dk::PLACES, &[what]);
        (r[0] == dk::OK).then(|| (r[1] + r[3] / 2, r[2] + r[4] / 2))
    };
    assert!(
        x.keys("{super+.}", || place(dk::places::EMOJI).is_some(), SLOW),
        "Alaya key + . did not open the emoji picker"
    );
    assert!(
        x.keys("{right}{enter}", || ends(ep, "😂"), SLOW),
        "the picker did not type its second emoji: {:?}",
        typed(ep)
    );
    assert!(
        x.keys("{super+k}", || place(dk::places::KEY + 16).is_some(), SLOW),
        "Alaya key + K did not show the on-screen keyboard"
    );
    // A click on its Q key types q.
    let q = place(dk::places::KEY + 16).unwrap();
    let before = typed(ep).0;
    assert!(
        x.drag(q, q, || typed(ep).0 > before && ends(ep, "q")),
        "a click on the on-screen Q did not type q: {:?}",
        typed(ep)
    );
    crate::say!("screenshot typing");
    sleep_until(500, || false);
    assert!(x.keys("{super+k}", || place(dk::places::KEY + 16).is_none(), SLOW));
    pass("the emoji picker and the on-screen keyboard type into the window in front");
}

// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7b checks, step one: several screens. A screen is plugged in
//! and out while apps run: no window is lost or left where no screen
//! shows it, and a window whose screen comes back goes back to it.

use alaya_programs::desktop::{self as dk, place, role};

use crate::m3::sleep_until;
use crate::m6b::{Session, ask};

const SLOW: u64 = 30_000;
/// The main screen's width (QEMU's card is set to 1280 by 800).
const MAIN_W: u64 = 1280;

pub(crate) fn run(x: &mut Session<'_>, pass: &mut dyn FnMut(&str)) {
    let ep = x.ep;

    // ---- 103. Plug a second screen in and out while apps run.
    let editor = x.window_of(role::EDITOR).expect("the editor's window");
    ask(ep, dk::RAISE, &[editor]);
    let frame = || {
        let r = ask(ep, dk::WINDOW, &[editor]);
        (r[1], r[2], r[3], r[4], r[5])
    };
    let (n0, layout) = x.s.screens();
    assert!(n0 == 1, "{n0} screens before the test");
    assert!(x.s.plug(0b10, 1024, 768), "no screen card to plug into");
    assert!(
        sleep_until(SLOW, || x.s.screens() == (2, layout + 1)),
        "the second screen did not come"
    );
    // Alaya key + Shift + right: the window goes to the next screen.
    assert!(
        x.keys("{super+shift+right}", || frame().0 >= MAIN_W, SLOW),
        "the window did not move to the second screen: {:?}",
        frame()
    );
    // Full screen there is that screen, nothing more.
    assert!(
        x.keys("{super+up}", || frame().4 == place::FULL, SLOW),
        "no full screen on the second screen"
    );
    let f = frame();
    assert!(
        (f.0, f.1, f.2, f.3) == (MAIN_W, 0, 1024, 768),
        "full screen on the second screen is {f:?}"
    );
    assert!(x.keys("{super+down}", || frame().4 == place::FREE, SLOW));
    let away = frame();
    crate::say!("the editor is on the second screen at {away:?}");
    // Unplugged: the window comes to the main screen, whole.
    assert!(x.s.plug(0, 0, 0));
    assert!(
        sleep_until(SLOW, || {
            let f = frame();
            f.0 + f.2 <= MAIN_W && f.1 + f.3 <= 800
        }),
        "after the screen went, the window is at {:?}",
        frame()
    );
    assert!(x.s.screens().0 == 1);
    crate::say!("the editor came to the main screen at {:?}", frame());
    // Plugged in again: it goes back where it was.
    assert!(x.s.plug(0b10, 1024, 768));
    assert!(
        sleep_until(SLOW, || frame() == away),
        "the window did not go back to its screen: {:?}, not {away:?}",
        frame()
    );
    // Leave one screen for the checks after this one.
    assert!(x.s.plug(0, 0, 0));
    assert!(sleep_until(SLOW, || frame().0 < MAIN_W && x.s.screens().0 == 1));
    pass(
        "plug and unplug a screen while apps run: no window lost, and each goes back to its screen",
    );
}

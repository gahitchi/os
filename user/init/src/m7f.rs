// SPDX-License-Identifier: GPL-3.0-only
//! Milestone 7b checks, step one: several screens. A screen is plugged in
//! and out while apps run: no window is lost or left where no screen
//! shows it, and a window whose screen comes back goes back to it.
//! Then each screen gets its own size for things (scale).

use alaya_programs::desktop::{self as dk, place, role};

use crate::m3::sleep_until;
use crate::m6b::{Session, ask};

const SLOW: u64 = 30_000;
/// The main screen's width (QEMU's card is set to 1280 by 800).
const MAIN_W: u64 = 1280;
/// A window's title bar, in pixels (the window rules' TITLE).
const TITLE_BAR: u64 = 32;

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
    // (The desktop learns of it a moment later.)
    assert!(
        sleep_until(SLOW, || ask(ep, dk::SCREENS, &[])[1] == 2),
        "the desktop did not see the second screen"
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
    assert!(sleep_until(SLOW, || {
        frame().0 < MAIN_W && x.s.screens().0 == 1 && ask(ep, dk::SCREENS, &[])[1] == 1
    }));
    pass(
        "plug and unplug a screen while apps run: no window lost, and each goes back to its screen",
    );

    // ---- 104. Each screen its own scale: a window keeps its size in
    // points on the way across.
    let (_, layout) = x.s.screens();
    assert!(x.s.plug(0b10, 1024, 768));
    assert!(
        sleep_until(SLOW, || x.s.screens() == (2, layout + 1)),
        "the second screen did not come back"
    );
    assert!(sleep_until(SLOW, || ask(ep, dk::SCREENS, &[])[1] == 2));
    if x.window_of(role::SETTINGS).is_none() {
        x.start_app(role::SETTINGS);
        assert!(sleep_until(SLOW, || x.window_of(role::SETTINGS).is_some()));
    }
    let settings = x.window_of(role::SETTINGS).unwrap();
    let at = |w: u64| {
        let r = ask(ep, dk::WINDOW, &[w]);
        (r[1], r[2], r[3], r[4])
    };
    // The editor was on the second screen when it went: it goes back
    // there by itself (check 103), still at 100%. (Each key wait below
    // starts from a state the keys change, so no typing arrives late.)
    assert!(
        sleep_until(SLOW, || at(editor).0 >= MAIN_W),
        "the editor did not go back to the second screen: {:?}",
        at(editor)
    );
    let (ew, eh) = (at(editor).2, at(editor).3);
    // Settings goes there too.
    assert!(at(settings).0 < MAIN_W, "Settings is already there");
    ask(ep, dk::RAISE, &[settings]);
    assert!(
        x.keys("{super+shift+right}", || at(settings).0 >= MAIN_W, SLOW),
        "Settings did not go to the second screen"
    );
    crate::say!("Settings ({settings}) is at {:?}", at(settings));
    // In Settings, S three times: Alaya's pick, 100%, 125%, 150%.
    let title = TITLE_BAR;
    let grown = || {
        let f = at(editor);
        f.2 == ew * 3 / 2 && f.3 - title == (eh - title) * 3 / 2
    };
    assert!(
        x.keys("sss", grown, SLOW),
        "at 150% the editor is {:?}, not {} by {}",
        at(editor),
        ew * 3 / 2,
        title + (eh - title) * 3 / 2
    );
    crate::say!("at 150% the editor is {:?}", at(editor));
    // Back on the main screen (still 100%): as it was.
    ask(ep, dk::RAISE, &[editor]);
    assert!(
        x.keys(
            "{super+shift+left}",
            || {
                let f = at(editor);
                f.0 < MAIN_W && (f.2, f.3) == (ew, eh)
            },
            SLOW
        ),
        "back on the main screen the editor is {:?}",
        at(editor)
    );
    // Settings back to Alaya's pick there (175%, 200%, pick), and the
    // second screen out.
    ask(ep, dk::RAISE, &[settings]);
    // (Wait until Settings has taken the last S: on a slow machine it
    // can still be on its way when the next check types. Only Alaya's
    // pick brings it back to its size at 100%.)
    assert!(
        x.keys("sss", || at(settings).2 <= 640, SLOW),
        "Settings did not go back to Alaya's pick: {:?}",
        at(settings)
    );
    assert!(x.s.plug(0, 0, 0));
    assert!(sleep_until(SLOW, || at(settings).0 < MAIN_W && x.s.screens().0 == 1));
    pass("each screen its own scale: a window keeps its size in points across screens");
}

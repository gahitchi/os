// SPDX-License-Identifier: GPL-3.0-only
//! Settings, first version: the look (decision 90), which first setup
//! asked for. It is kept in the person's own app files (`look`), and the
//! desktop reads it again when Settings says it changed.

use alloc::vec::Vec;

use alaya_apps::store::Store;
use alaya_apps::{self as app, Event, Window};
use alaya_programs::desktop as dk;
use alaya_programs::files::APPS;
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::rt::Mem;
use alaya_sys::{Caps, Msg, say};
use alaya_ui::Rect;
use alaya_ui::login::Look;
use alaya_ui::text::Face;
use alaya_ui::widgets::{Ui, theme};

pub fn run(info: &'static StartInfo, mut mem: Mem, store: Store) -> ! {
    let mut fonts = app::fonts(info);
    let apps = store.root(APPS).expect("app files");
    let file = store.open(apps, "look").expect("the look");
    let mut look = Look::from_code(
        store
            .read_all(file)
            .ok()
            .and_then(|b| b.first().copied())
            .unwrap_or(0),
    );
    let mut win = Window::open(&mut mem, 440, 260, "Settings").expect("open a window");
    let mut focus = 0;
    loop {
        let mut keys = Vec::new();
        while let Some(e) = win.next_event() {
            match e {
                Event::Key(k) => keys.push(k),
                Event::Close => crate::closed(&win),
                _ => {}
            }
        }
        let mut chosen = None;
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.focus = focus;
            ui.key = keys.first().copied();
            ui.text(16, 14, Face::Strong, 15, "The look", theme::INK);
            ui.text(
                16,
                40,
                Face::Text,
                13,
                "What you use Alaya for. Changes the dock at once.",
                theme::MUTED,
            );
            let mut y = 72;
            for l in [Look::Basics, Look::Everything] {
                let (hit, r) = ui.button(16, y, l.name(), l == look);
                if hit {
                    chosen = Some(l);
                }
                ui.text(
                    r.right() + 12,
                    y + 7,
                    Face::Text,
                    12,
                    l.about(),
                    theme::MUTED,
                );
                y += 44;
            }
            focus = ui.finish();
            ui.text(
                16,
                h - 30,
                Face::Text,
                12,
                "Alaya 0.0.1, milestone 6.",
                theme::MUTED,
            );
            win.pointer.released = false;
        }
        if let Some(l) = chosen
            && l != look
        {
            look = l;
            let _ = store.write_all(file, &[l.code()]);
            let _ = store.save();
            let _ = alaya_sys::ipc_call(
                start::FIRST_GIVEN + dk::APP_G_DESKTOP,
                &Msg::new(dk::LOOK_CHANGED, &[]),
                Caps::default(),
            );
            say!("the look is now: {}", l.name());
        }
        win.damage_all();
        win.wait();
    }
}

// SPDX-License-Identifier: GPL-3.0-only
//! The text editor (milestone 6, first version): one window, one text, the
//! keyboard and the mouse. Saving to the store and reopening exactly where
//! you were come with the next step (6b).

#![no_std]
#![no_main]

extern crate alloc;

use alaya_apps::{self as app, Event, Window};
use alaya_sys::abi::StartInfo;
use alaya_sys::rt::Mem;
use alaya_sys::say;
use alaya_ui::Rect;
use alaya_ui::text::Face;
use alaya_ui::widgets::{TextEdit, Ui, theme};

alaya_sys::heap!();
alaya_sys::entry!(main);

fn main(info: &'static StartInfo) -> ! {
    let mut mem = Mem::new(info);
    alaya_sys::heap::init(&mut mem, 2).expect("heap");
    let mut fonts = app::fonts(info);
    let mut win = Window::open(&mut mem, 560, 360, "Text editor").expect("open a window");
    let mut edit = TextEdit::new("Welcome to Alaya.\nType here.");
    let mut focus = 0;
    let mut said = alloc::string::String::new();
    loop {
        {
            let mut cv = win.canvas();
            let (w, h) = (win.width, win.height);
            cv.fill(Rect::new(0, 0, w, h), theme::WIN);
            let mut ui = Ui::new(&mut cv, &mut fonts, win.pointer);
            ui.focus = focus;
            let mut pending = alloc::vec::Vec::new();
            while let Some(e) = win.next_event() {
                pending.push(e);
            }
            let mut close = false;
            for e in &pending {
                match e {
                    // The text box is the only thing that takes keys, so
                    // every key waiting goes to it, in order.
                    Event::Key(k) => {
                        edit.key(*k, true);
                    }
                    Event::Close => close = true,
                    _ => {}
                }
            }
            if close {
                win.close();
                say!("closed");
                loop {
                    alaya_sys::yield_now();
                }
            }
            ui.text_box(Rect::new(12, 12, w - 24, h - 56), "Text", &mut edit, true);
            let (line, col) = edit.position();
            let status = alloc::format!("Line {}, column {}", line + 1, col + 1);
            ui.text(12, h - 34, Face::Text, 13, &status, theme::MUTED);
            focus = ui.finish();
            win.pointer.released = false;
        }
        win.damage_all();
        if edit.text != said {
            said = edit.text.clone();
            let shown: alloc::string::String = said
                .chars()
                .rev()
                .take(40)
                .collect::<alloc::vec::Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            say!("text ends with: {}", shown.replace('\n', "/"));
        }
        win.wait();
    }
}

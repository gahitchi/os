// SPDX-License-Identifier: GPL-3.0-only
//! What an app needs to open a window on the desktop and draw in it with
//! the toolkit: the window's picture, its events, and Alaya's letters.

#![no_std]

extern crate alloc;

pub mod store;

use alaya_sys::abi::{StartInfo, map, start};
use alaya_sys::rt::Mem;
use alaya_sys::{Caps, Msg, MsgInfo};
use alaya_ui::Rect;
use alaya_ui::canvas::Canvas;
use alaya_ui::text::Fonts;
use alaya_ui::widgets::{Key, Pointer, us_key};

use alaya_programs::desktop::{self as dk, event};

/// Where the window's picture is mapped in the app.
const PICTURE: u64 = dk::PICTURES;

/// The fonts the first program shares with every app.
pub fn fonts(info: &StartInfo) -> Fonts<'static> {
    let f = |i: usize| {
        let len = info.args[dk::ARG_FONTS + i] as usize;
        // SAFETY: shared read only at this place before the app starts.
        unsafe {
            core::slice::from_raw_parts((dk::FONTS + i as u64 * dk::FONT_STEP) as *const u8, len)
        }
    };
    Fonts::new(f(0), f(1), f(2), f(3))
}

/// Something that happened to the window, already turned into meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Key(Key),
    /// A letter typed with Ctrl held (Ctrl+S: save).
    Shortcut(char),
    /// Pointer at (x, y) in the content; button 0 for a move.
    Pointer {
        x: i32,
        y: i32,
        button: u8,
        down: bool,
    },
    Resized(i32, i32),
    Close,
    Focus(bool),
}

pub struct Window {
    ep: u64,
    pub ntfn: u64,
    pub id: u64,
    pub width: i32,
    pub height: i32,
    /// Pointer state for the toolkit.
    pub pointer: Pointer,
}

fn call(ep: u64, label: u64, words: &[u64]) -> [u64; 6] {
    alaya_sys::ipc_call(ep, &Msg::new(label, words), Caps::default())
        .map(|m| m.words)
        .unwrap_or([u64::MAX; 6])
}

impl Window {
    /// Open a window with content `w` by `h` and the title `title`.
    pub fn open(mem: &mut Mem, w: i32, h: i32, title: &str) -> Option<Window> {
        let g = start::FIRST_GIVEN;
        let ep = g + dk::APP_G_DESKTOP;
        let ntfn = g + dk::APP_G_NTFN;
        let m = Msg {
            info: MsgInfo::new(dk::OPEN, 2).with_cap(),
            badge: 0,
            words: [w as u64, h as u64, 0, 0, 0, 0],
        };
        let r = alaya_sys::ipc_call(
            ep,
            &m,
            Caps {
                send: ntfn,
                ..Caps::default()
            },
        )
        .ok()?;
        if r.words[0] != dk::OK {
            return None;
        }
        let id = r.words[1];
        for i in 0..dk::PAGES {
            let slot = mem.slot().ok()?;
            let r = alaya_sys::ipc_call(
                ep,
                &Msg::new(dk::PAGE, &[id, i]),
                Caps {
                    recv: slot,
                    ..Caps::default()
                },
            )
            .ok()?;
            if r.words[0] != dk::OK {
                return None;
            }
            alaya_sys::page_map(slot, start::SPACE, PICTURE + i * (2 << 20), map::WRITE).ok()?;
        }
        let mut win = Window {
            ep,
            ntfn,
            id,
            width: w,
            height: h,
            pointer: Pointer::default(),
        };
        win.set_title(title);
        let s = call(ep, dk::SIZE, &[id]);
        if s[0] == dk::OK {
            win.width = s[1] as i32;
            win.height = s[2] as i32;
        }
        Some(win)
    }

    pub fn set_title(&self, title: &str) {
        let mut b = [0u8; 32];
        // Cut at a character boundary.
        let mut n = title.len().min(32);
        while !title.is_char_boundary(n) {
            n -= 1;
        }
        b[..n].copy_from_slice(&title.as_bytes()[..n]);
        let w = |k: usize| u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
        call(
            self.ep,
            dk::TITLE,
            &[self.id, n as u64, w(0), w(1), w(2), w(3)],
        );
    }

    /// The window's picture, as a canvas of its current size.
    pub fn canvas(&self) -> Canvas<'static> {
        // SAFETY: the two picture pages are mapped at PICTURE.
        let px = unsafe {
            core::slice::from_raw_parts_mut(PICTURE as *mut u32, (dk::STRIDE * dk::MAX_H) as usize)
        };
        Canvas::new(px, self.width, self.height, dk::STRIDE as i32).expect("picture")
    }

    /// Tell the desktop a part changed.
    pub fn damage(&self, r: Rect) {
        call(
            self.ep,
            dk::DAMAGE,
            &[self.id, r.x as u64, r.y as u64, r.w as u64, r.h as u64],
        );
    }

    pub fn damage_all(&self) {
        self.damage(Rect::new(0, 0, self.width, self.height));
    }

    pub fn close(&self) {
        call(self.ep, dk::CLOSE, &[self.id]);
    }

    /// The next event, if one is waiting. Keeps size and pointer state.
    pub fn next_event(&mut self) -> Option<Event> {
        loop {
            let r = call(self.ep, dk::EVENT, &[self.id]);
            if r[0] != dk::OK {
                return None;
            }
            let e = match r[1] {
                event::KEY => {
                    // Presses and repeats; releases mean nothing to apps.
                    if r[3] == 0 {
                        continue;
                    }
                    match us_key(r[2] as u16, r[4] != 0) {
                        Some(Key::Char(c)) if r[5] != 0 => Event::Shortcut(c.to_ascii_lowercase()),
                        Some(k) => Event::Key(k),
                        None => continue,
                    }
                }
                event::POINTER => {
                    let (x, y) = (r[2] as i32, r[3] as i32);
                    let button = (r[4] & 0xff) as u8;
                    let down = r[4] >> 8 != 0;
                    let p = &mut self.pointer;
                    p.x = x;
                    p.y = y;
                    if button != 0 {
                        if down {
                            p.down = true;
                            p.down_x = x;
                            p.down_y = y;
                        } else {
                            p.down = false;
                            p.released = true;
                        }
                    }
                    Event::Pointer { x, y, button, down }
                }
                event::RESIZED => {
                    self.width = r[2] as i32;
                    self.height = r[3] as i32;
                    Event::Resized(self.width, self.height)
                }
                event::CLOSE => Event::Close,
                event::FOCUS => Event::Focus(r[2] != 0),
                _ => continue,
            };
            return Some(e);
        }
    }

    /// Wait until events arrive.
    pub fn wait(&self) {
        let _ = alaya_sys::wait(self.ntfn);
    }
}

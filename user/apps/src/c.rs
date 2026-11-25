// SPDX-License-Identifier: GPL-3.0-only
//! The small C interface of Alaya's app SDK (milestone 8, step 3), so
//! that programs written in C (and the POSIX layer of step 8-4) can use
//! the same calls as Rust apps. Its header is `user/apps/include/alaya.h`;
//! `cargo xtask image` checks that the two list the same functions.
//!
//! Every function returns a status word (`ALAYA_OK` is 0; the others are
//! the store's, `alaya_programs::files::st`) and writes results through
//! pointers. Nothing here gives a C program more than a Rust app has: the
//! calls go through the same badged endpoints, and the store and the
//! desktop decide as always.

use alaya_programs::desktop as dk;
use alaya_programs::files::st;
use alaya_sys::abi::start;

use crate::store::Store;

fn store() -> Store {
    Store::new(start::FIRST_GIVEN + dk::APP_G_STORE)
}

/// Write `v` through `out` when it is not null.
///
/// # Safety
/// `out` is null or valid for one write.
unsafe fn put<T>(out: *mut T, v: T) {
    if !out.is_null() {
        // SAFETY: the caller's promise.
        unsafe { out.write(v) };
    }
}

/// # Safety
/// `ptr` is valid for `len` bytes, or `len` is 0.
unsafe fn bytes<'a>(ptr: *const u8, len: usize) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller's promise.
        unsafe { core::slice::from_raw_parts(ptr, len) }
    }
}

fn status<T>(r: Result<T, u64>, out: impl FnOnce(T)) -> u64 {
    match r {
        Ok(v) => {
            out(v);
            st::OK
        }
        Err(e) => e,
    }
}

/// Milliseconds since Alaya started.
#[unsafe(no_mangle)]
pub extern "C" fn alaya_now_ms() -> u64 {
    alaya_sys::rt::now_ms()
}

/// A line in Alaya's log, under the program's name.
///
/// # Safety
/// `text` is valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_say(text: *const u8, len: usize) {
    // SAFETY: the caller's promise.
    let b = unsafe { bytes(text, len) };
    alaya_sys::say!("{}", alloc::string::String::from_utf8_lossy(b));
}

/// The top folder of a class (`ALAYA_DOCUMENTS`, `ALAYA_APP_FILES`).
///
/// # Safety
/// `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_root(class: u64, out: *mut u64) -> u64 {
    // SAFETY: the caller's promise.
    status(store().root(class), |v| unsafe { put(out, v) })
}

/// The file `name` in `dir`, made when it is not there.
///
/// # Safety
/// `name` is valid for `len` bytes; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_open(
    dir: u64,
    name: *const u8,
    len: usize,
    out: *mut u64,
) -> u64 {
    // SAFETY: the caller's promise.
    let Ok(name) = core::str::from_utf8(unsafe { bytes(name, len) }) else {
        return st::BAD_REQUEST;
    };
    // SAFETY: the caller's promise.
    status(store().open(dir, name), |v| unsafe { put(out, v) })
}

/// A file's size in bytes.
///
/// # Safety
/// `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_size(file: u64, out: *mut u64) -> u64 {
    // SAFETY: the caller's promise.
    status(store().stat(file), |v| unsafe { put(out, v.0) })
}

/// Up to `cap` bytes of the whole file into `buf`; `got` is how many.
///
/// # Safety
/// `buf` is valid for `cap` bytes of writes; `got` is null or valid for
/// one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_read(
    file: u64,
    buf: *mut u8,
    cap: usize,
    got: *mut usize,
) -> u64 {
    status(store().read_all(file), |data| {
        let n = data.len().min(cap);
        if n > 0 && !buf.is_null() {
            // SAFETY: the caller's promise; `n` is at most `cap`.
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf, n) };
        }
        // SAFETY: the caller's promise.
        unsafe { put(got, n) };
    })
}

/// Make the file exactly these bytes.
///
/// # Safety
/// `data` is valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_write(file: u64, data: *const u8, len: usize) -> u64 {
    // SAFETY: the caller's promise.
    status(store().write_all(file, unsafe { bytes(data, len) }), |_| {})
}

/// A file that is there, without making it.
///
/// # Safety
/// `name` is valid for `len` bytes; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_lookup(
    dir: u64,
    name: *const u8,
    len: usize,
    out: *mut u64,
) -> u64 {
    // SAFETY: the caller's promise.
    let Ok(name) = core::str::from_utf8(unsafe { bytes(name, len) }) else {
        return st::BAD_REQUEST;
    };
    // SAFETY: the caller's promise.
    status(store().lookup(dir, name), |v| unsafe { put(out, v) })
}

/// Take a file out of its folder (history keeps it).
///
/// # Safety
/// `name` is valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_remove(dir: u64, name: *const u8, len: usize) -> u64 {
    // SAFETY: the caller's promise.
    let Ok(name) = core::str::from_utf8(unsafe { bytes(name, len) }) else {
        return st::BAD_REQUEST;
    };
    status(store().remove(dir, name), |_| {})
}

/// The names in a folder this program may see, one a line, folders
/// ending in '/'; as many whole lines as fit in `cap` bytes.
///
/// # Safety
/// `buf` is valid for `cap` bytes of writes; `got` is null or valid for
/// one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_store_list(
    dir: u64,
    buf: *mut u8,
    cap: usize,
    got: *mut usize,
) -> u64 {
    status(store().list(dir), |mut entries| {
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let mut n = 0;
        for e in entries {
            let line = alloc::format!("{}{}\n", e.name, if e.is_dir { "/" } else { "" });
            if n + line.len() > cap || buf.is_null() {
                break;
            }
            // SAFETY: the caller's promise; it fits in `cap`.
            unsafe { core::ptr::copy_nonoverlapping(line.as_ptr(), buf.add(n), line.len()) };
            n += line.len();
        }
        // SAFETY: the caller's promise.
        unsafe { put(got, n) };
    })
}

/// Make what was written safe now (the store also does it by itself).
#[unsafe(no_mangle)]
pub extern "C" fn alaya_store_save() -> u64 {
    status(store().save(), |_| {})
}

/// Ask this app's question about `file` (answers instead of data): one
/// number, and how many questions are left today.
///
/// # Safety
/// `value` and `left` are null or valid for one write each.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_answer(file: u64, value: *mut i64, left: *mut u64) -> u64 {
    status(store().answer(file), |(v, l)| {
        // SAFETY: the caller's promise.
        unsafe {
            put(value, v);
            put(left, l);
        }
    })
}

// ---- Windows from C (milestone 8, step 4): one window per program, a
// picture the program draws itself, and its events. What the SDL subset
// of Alaya's C library is built on.

use core::cell::RefCell;

use alaya_sys::rt::Mem;
use alaya_ui::widgets::Key;

use crate::{Event, Window};

struct One<T>(RefCell<Option<T>>);
// SAFETY: an app's C code runs on its one main thread.
unsafe impl<T> Sync for One<T> {}

static MEM: One<Mem> = One(RefCell::new(None));
static WINDOW: One<Window> = One(RefCell::new(None));

/// Hand the C side the program's memory, so that it can open its window.
pub fn give_mem(mem: Mem) {
    *MEM.0.borrow_mut() = Some(mem);
}

/// The window the C side opened, if any (to close it after the program
/// ended).
pub fn take_window() -> Option<Window> {
    WINDOW.0.borrow_mut().take()
}

/// What `alaya_window_event` says for keys that are not text.
pub mod keys {
    pub const ENTER: u32 = 13;
    pub const BACKSPACE: u32 = 8;
    pub const TAB: u32 = 9;
    pub const ESCAPE: u32 = 27;
    pub const DELETE: u32 = 127;
    pub const UP: u32 = 0x11_0001;
    pub const DOWN: u32 = 0x11_0002;
    pub const LEFT: u32 = 0x11_0003;
    pub const RIGHT: u32 = 0x11_0004;
    pub const HOME: u32 = 0x11_0005;
    pub const END: u32 = 0x11_0006;
}

/// Kinds of events (`ALAYA_EVENT_*`).
pub mod events {
    pub const KEY: u32 = 1;
    pub const QUIT: u32 = 2;
    pub const RESIZED: u32 = 3;
    pub const FOCUS: u32 = 4;
    pub const POINTER: u32 = 5;
}

fn key_code(k: Key) -> u32 {
    match k {
        Key::Char(c) => c as u32,
        Key::Enter => keys::ENTER,
        Key::Backspace => keys::BACKSPACE,
        Key::Delete => keys::DELETE,
        Key::Left => keys::LEFT,
        Key::Right => keys::RIGHT,
        Key::Up => keys::UP,
        Key::Down => keys::DOWN,
        Key::Home => keys::HOME,
        Key::End => keys::END,
        Key::Tab => keys::TAB,
        Key::Escape => keys::ESCAPE,
    }
}

/// Open the program's window, `w` by `h` points, with a title; a second
/// call only changes the title.
///
/// # Safety
/// `title` is valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_window_open(w: i32, h: i32, title: *const u8, len: usize) -> u64 {
    // SAFETY: the caller's promise.
    let t = alloc::string::String::from_utf8_lossy(unsafe { bytes(title, len) }).into_owned();
    let mut win = WINDOW.0.borrow_mut();
    if let Some(win) = win.as_ref() {
        win.set_title(&t);
        return st::OK;
    }
    let mut mem = MEM.0.borrow_mut();
    let Some(mem) = mem.as_mut() else {
        return st::BAD_REQUEST;
    };
    if !(1..=dk::STRIDE as i32).contains(&w) || !(1..=dk::MAX_H as i32).contains(&h) {
        return st::BAD_REQUEST;
    }
    match Window::open(mem, w, h, &t) {
        Some(o) => {
            *win = Some(o);
            st::OK
        }
        None => st::FULL,
    }
}

/// Show the program's picture: `w` by `h` pixels, 0x00RRGGBB each,
/// scaled up by whole steps to fill the window.
///
/// # Safety
/// `px` is valid for `w * h` reads.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_window_present(px: *const u32, w: i32, h: i32) -> u64 {
    let win = WINDOW.0.borrow();
    let Some(win) = win.as_ref() else {
        return st::NOT_FOUND;
    };
    if px.is_null() || w <= 0 || h <= 0 || w > 4096 || h > 4096 {
        return st::BAD_REQUEST;
    }
    // SAFETY: the caller's promise.
    let src = unsafe { core::slice::from_raw_parts(px, (w * h) as usize) };
    win.present(src, w, h);
    win.damage_all();
    st::OK
}

/// The window's size in points.
///
/// # Safety
/// `w` and `h` are null or valid for one write each.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_window_size(w: *mut i32, h: *mut i32) -> u64 {
    let win = WINDOW.0.borrow();
    let Some(win) = win.as_ref() else {
        return st::NOT_FOUND;
    };
    // SAFETY: the caller's promise.
    unsafe {
        put(w, win.width);
        put(h, win.height);
    }
    st::OK
}

/// The next event, if there is one: 1 and its kind (`ALAYA_EVENT_*`)
/// and two numbers (a key: its code and 1 with Ctrl; a size; a pointer's
/// place), or 0 when there is none now.
///
/// # Safety
/// `kind`, `a` and `b` are null or valid for one write each.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_window_event(kind: *mut u32, a: *mut i32, b: *mut i32) -> u64 {
    let mut win = WINDOW.0.borrow_mut();
    let Some(win) = win.as_mut() else {
        return 0;
    };
    while let Some(e) = win.next_event() {
        let (k, x, y) = match e {
            Event::Key(k) => (events::KEY, key_code(k) as i32, 0),
            Event::Shortcut(c) => (events::KEY, c as i32, 1),
            Event::Close => (events::QUIT, 0, 0),
            Event::Resized(w, h) => (events::RESIZED, w, h),
            Event::Focus(f) => (events::FOCUS, f as i32, 0),
            Event::Pointer { x, y, button, down } if button != 0 && down => (events::POINTER, x, y),
            _ => continue,
        };
        // SAFETY: the caller's promise.
        unsafe {
            put(kind, k);
            put(a, x);
            put(b, y);
        }
        return 1;
    }
    0
}

/// Wait until something happens: an event, or the program's clock.
#[unsafe(no_mangle)]
pub extern "C" fn alaya_window_wait() {
    let ntfn = start::FIRST_GIVEN + dk::APP_G_NTFN;
    let _ = alaya_sys::wait(ntfn);
}

// ---- Saved games (decision 161): saves are documents in
// "Saved games/<game>", so every save is kept like any document, and an
// older one can be read back.

/// This game's folder of saves, made the first time.
///
/// # Safety
/// `game` is valid for `len` bytes; `out` is null or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_saved_games(game: *const u8, len: usize, out: *mut u64) -> u64 {
    // SAFETY: the caller's promise.
    let Ok(name) = core::str::from_utf8(unsafe { bytes(game, len) }) else {
        return st::BAD_REQUEST;
    };
    // SAFETY: the caller's promise.
    status(store().saved_games(name), |v| unsafe { put(out, v) })
}

/// Each moment `file` changed, newest first, at most `max`: the moment
/// and the time.
///
/// # Safety
/// `moments` and `times` are valid for `max` writes each; `got` is null
/// or valid for one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_save_versions(
    file: u64,
    moments: *mut u64,
    times: *mut u64,
    max: usize,
    got: *mut usize,
) -> u64 {
    status(store().save_versions(file, max as u64), |l| {
        let n = l.len().min(max);
        for (i, (cn, t)) in l.into_iter().take(n).enumerate() {
            // SAFETY: the caller's promise; i < max.
            unsafe {
                if !moments.is_null() {
                    moments.add(i).write(cn);
                }
                if !times.is_null() {
                    times.add(i).write(t);
                }
            }
        }
        // SAFETY: the caller's promise.
        unsafe { put(got, n) };
    })
}

/// Up to `cap` bytes of `file` as it was at `moment`; `got` is how many.
///
/// # Safety
/// `buf` is valid for `cap` bytes of writes; `got` is null or valid for
/// one write.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_read_past(
    file: u64,
    moment: u64,
    buf: *mut u8,
    cap: usize,
    got: *mut usize,
) -> u64 {
    if buf.is_null() {
        return st::BAD_REQUEST;
    }
    status(store().read_past(file, moment, cap), |d| {
        let n = d.len().min(cap);
        // SAFETY: the caller's promise; n <= cap.
        unsafe {
            core::ptr::copy_nonoverlapping(d.as_ptr(), buf, n);
            put(got, n);
        }
    })
}

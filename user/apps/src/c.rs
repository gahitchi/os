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

// SPDX-License-Identifier: GPL-3.0-only
//! What Alaya's small C library (`user/libc`, milestone 8, step 4) needs
//! from the Rust side: memory from Alaya's allocator, and where a C
//! program's output goes. The C library itself is linked only into the
//! programs that run C code (the terminal's `apps` program).

use alloc::alloc::{Layout, alloc, dealloc, realloc as grow};
use core::cell::RefCell;

/// Every block starts with its size, so `free` and `realloc` know it.
const HEAD: usize = 16;

fn layout(n: usize) -> Option<Layout> {
    Layout::from_size_align(n.checked_add(HEAD)?, HEAD).ok()
}

/// # Safety
/// The C contract for `malloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(n: usize) -> *mut u8 {
    let Some(l) = layout(n.max(1)) else {
        return core::ptr::null_mut();
    };
    // SAFETY: `l` is not zero-sized.
    let p = unsafe { alloc(l) };
    if p.is_null() {
        return p;
    }
    // SAFETY: the block is at least HEAD bytes, aligned to 16.
    unsafe {
        (p as *mut usize).write(n.max(1));
        p.add(HEAD)
    }
}

/// # Safety
/// The C contract for `free`: null or a block from this allocator.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    // SAFETY: `p` came from `malloc`, so its size is just before it.
    unsafe {
        let base = p.sub(HEAD);
        let n = (base as *const usize).read();
        dealloc(base, layout(n).unwrap());
    }
}

/// # Safety
/// The C contract for `calloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(count: usize, n: usize) -> *mut u8 {
    let Some(total) = count.checked_mul(n) else {
        return core::ptr::null_mut();
    };
    // SAFETY: as malloc.
    let p = unsafe { malloc(total) };
    if !p.is_null() {
        // SAFETY: the block holds `total` bytes.
        unsafe { core::ptr::write_bytes(p, 0, total) };
    }
    p
}

/// # Safety
/// The C contract for `realloc`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(p: *mut u8, n: usize) -> *mut u8 {
    if p.is_null() {
        // SAFETY: as malloc.
        return unsafe { malloc(n) };
    }
    let n = n.max(1);
    let Some(new) = layout(n) else {
        return core::ptr::null_mut();
    };
    // SAFETY: `p` came from `malloc`.
    unsafe {
        let base = p.sub(HEAD);
        let old = (base as *const usize).read();
        let q = grow(base, layout(old).unwrap(), new.size());
        if q.is_null() {
            return q;
        }
        (q as *mut usize).write(n);
        q.add(HEAD)
    }
}

/// Where a C program's standard output and errors go: set by the
/// program that runs C code (the terminal).
/// A sink: the file number (1 or 2) and the bytes.
pub type Sink = fn(i32, &[u8]);

pub struct Out(RefCell<Option<Sink>>);

// SAFETY: Alaya programs that run C code do it on one thread.
unsafe impl Sync for Out {}

pub static OUT: Out = Out(RefCell::new(None));

impl Out {
    pub fn set(&self, f: Sink) {
        *self.0.borrow_mut() = Some(f);
    }
}

/// Output of a C program (fd 1 or 2): to the sink, or Alaya's log.
///
/// # Safety
/// `data` is valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn alaya_out(fd: i32, data: *const u8, len: usize) {
    if data.is_null() || len == 0 {
        return;
    }
    // SAFETY: the caller's promise.
    let b = unsafe { core::slice::from_raw_parts(data, len) };
    let sink = *OUT.0.borrow();
    match sink {
        Some(f) => f(fd, b),
        None => alaya_sys::say!("{}", alloc::string::String::from_utf8_lossy(b).trim_end()),
    }
}

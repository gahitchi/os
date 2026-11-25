// SPDX-License-Identifier: GPL-3.0-only
//! Glowroot (milestone 8, step 4): a small puzzle game written in C on
//! the SDL subset of Alaya's C library (`user/libc/games/glowroot.c`).
//! This side only hands the C program its memory, so that it can open
//! its window, and runs it.

use alaya_apps::c;
use alaya_sys::rt::Mem;

type Main = unsafe extern "C" fn(i32, *mut *mut u8) -> i32;

unsafe extern "C" {
    fn alaya_run_main(main: Main, argc: i32, argv: *mut *mut u8) -> i32;
    fn glowroot_main(argc: i32, argv: *mut *mut u8) -> i32;
}

pub fn run(mem: Mem) -> ! {
    c::give_mem(mem);
    let mut name = *b"glowroot\0";
    let mut argv = [name.as_mut_ptr(), core::ptr::null_mut()];
    // SAFETY: argv holds one C string and a null; the game is C code
    // built against Alaya's C library.
    let code = unsafe { alaya_run_main(glowroot_main, 1, argv.as_mut_ptr()) };
    alaya_sys::say!("glowroot: ended ({code})");
    match c::take_window() {
        Some(w) => crate::closed(&w),
        None => loop {
            let _ = alaya_sys::wait(
                alaya_sys::abi::start::FIRST_GIVEN + alaya_programs::desktop::APP_G_NTFN,
            );
        },
    }
}

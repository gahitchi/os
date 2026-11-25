// SPDX-License-Identifier: GPL-3.0-only
//! The first apps (milestone 6b): the text editor, the file browser, the
//! chart maker and Settings; and "What happened?" with the timeline
//! (milestone 7). One program image; the first program starts
//! one copy per app and says which it is (its role). Each copy is a
//! separate program with its own memory, its own badge on the desktop and
//! on the store, and its own transfer pages.

#![no_std]
#![no_main]

extern crate alloc;

mod back;
mod chart;
mod editor;
mod files;
mod game;
mod helper;
mod settings;
mod terminal;

use alaya_apps::store::Store;
use alaya_programs::desktop::{self as dk, role};
use alaya_sys::abi::{StartInfo, start};
use alaya_sys::rt::{self, Mem};

alaya_sys::heap!();
alaya_sys::entry!(main);

/// The clock's bit on the app's notification (bit 0 is the desktop's).
pub const CLOCK_BIT: u64 = 2;

fn main(info: &'static StartInfo) -> ! {
    let mut mem = Mem::new(info);
    alaya_sys::heap::init(&mut mem, 4).expect("heap");
    let ntfn = start::FIRST_GIVEN + dk::APP_G_NTFN;
    // About once a second; for the game, about twenty times.
    let every = if info.args[dk::ARG_ROLE] == role::GAME {
        5
    } else {
        100
    };
    rt::start_clock(&mut mem, ntfn, CLOCK_BIT, every, info.priority).expect("clock");
    let store = Store::new(start::FIRST_GIVEN + dk::APP_G_STORE);
    match info.args[dk::ARG_ROLE] {
        role::EDITOR => editor::run(info, mem, store),
        role::FILES => files::run(info, mem, store),
        role::CHART => chart::run(info, mem, store),
        role::SETTINGS => settings::run(info, mem, store),
        role::BACK => back::run(info, mem, store),
        role::HELPER => helper::run(info, mem, store, false),
        role::HOSTILE => helper::run(info, mem, store, true),
        role::TERMINAL => terminal::run(info, mem, store),
        role::GAME => game::run(mem),
        r => panic!("no app has role {r}"),
    }
}

/// Stop after the window closed: the program waits for the first program
/// to end it, and starts again from the dock.
pub fn closed(win: &alaya_apps::Window) -> ! {
    win.close();
    loop {
        let _ = alaya_sys::wait(start::FIRST_GIVEN + dk::APP_G_NTFN);
    }
}

// SPDX-License-Identifier: GPL-3.0-only
//! The virtual disk driver, version 2 (see `blk.rs`).

#![no_std]
#![no_main]

#[path = "../blk.rs"]
mod blk;

alaya_sys::entry!(main);

fn main(info: &'static alaya_sys::abi::StartInfo) -> ! {
    blk::run(info, 2)
}

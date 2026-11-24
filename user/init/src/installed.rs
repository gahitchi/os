// SPDX-License-Identifier: GPL-3.0-only
//! Starting installed apps (milestone 8): the first program reads the
//! place's record and bundle from the machine's app files, checks the
//! bundle's signature again (it trusts no copy on the disk), and hands the
//! app only what its promise says and the person left ticked.

use alaya_bundle::{Bundle, promise};
use alaya_programs::desktop::installed as ins;
use alaya_programs::files::{self as fs, APPS};
use alaya_sys::Kind;
use alaya_sys::abi::map;

use crate::m6b::Files;

/// Where the first program reads a bundle into: one 2 MiB page, mapped
/// the first time it is needed.
const LOAD_AT: u64 = 0x2f00_0000_0000;
const LOAD_BYTES: usize = 2 << 20;

/// An installed app ready to start.
pub(crate) struct Ready {
    /// What it may have: its promise, less what the person unticked.
    pub granted: u32,
    pub version: u32,
    pub name: [u8; 32],
    /// The bundle, in the load area.
    pub len: usize,
}

impl Ready {
    pub fn program(&self) -> &'static [u8] {
        let b = area();
        // Checked when it was loaded.
        Bundle::read(&b[..self.len]).map_or(&[][..], |b| b.program)
    }

    pub fn files(&self) -> bool {
        self.granted & promise::FILES != 0
    }
}

fn area() -> &'static mut [u8] {
    // SAFETY: mapped by `map_area` before any use, and used by one thread.
    unsafe { core::slice::from_raw_parts_mut(LOAD_AT as *mut u8, LOAD_BYTES) }
}

pub(crate) fn map_area(k: &mut crate::m3::Kit, space: u64, mapped: &mut bool) {
    if !*mapped {
        let p = k.make(Kind::Page, 21);
        alaya_sys::page_map(p, space, LOAD_AT, map::WRITE).expect("the load area");
        *mapped = true;
    }
}

/// Read person `person`'s place `slot` from the machine's app files.
pub(crate) fn load(m: &Files, person: u64, slot: u64) -> Result<Ready, &'static str> {
    let apps = m.root(APPS);
    let list = m.lookup(apps, ins::FILE).ok_or("no app is installed")?;
    let at = ins::at(person, slot);
    let mut rec = [0u8; ins::RECORD];
    if m.read_at(list, at as u64, &mut rec) != Some(ins::RECORD) {
        return Err("the place is empty");
    }
    if rec[0] != ins::ON && rec[0] != ins::REHEARSAL {
        return Err("the app is not approved");
    }
    let ticked = u32::from_le_bytes(rec[4..8].try_into().unwrap());
    let mut nb = [0u8; 8];
    let f = m
        .lookup(apps, ins::bundle_file(person, slot, &mut nb))
        .ok_or("its bundle is missing")?;
    let size = m.size(f).ok_or("its bundle is missing")? as usize;
    if size > LOAD_BYTES {
        return Err("its bundle is too big");
    }
    let buf = area();
    let mut done = 0;
    while done < size {
        let n = (size - done).min(fs::XFER_BYTES);
        match m.read_at(f, done as u64, &mut buf[done..done + n]) {
            Some(got) if got > 0 => done += got,
            _ => return Err("its bundle could not be read"),
        }
    }
    let b = Bundle::read(&buf[..size]).map_err(|e| e.words())?;
    if rec[16..24] != b.id().to_le_bytes() {
        return Err("the bundle is another app's");
    }
    if rec[8..12] != b.version.to_le_bytes() {
        return Err("the bundle is not the version approved");
    }
    let mut name = [0u8; 32];
    name[..b.name().len()].copy_from_slice(b.name().as_bytes());
    Ok(Ready {
        granted: b.promise & ticked,
        version: b.version,
        name,
        len: size,
    })
}

// SPDX-License-Identifier: GPL-3.0-only
//! The store, as the desktop and the apps use it: whole files by name.
//!
//! Requests are messages on a badged endpoint; the badge says who asks
//! (person, app, transfer channel) and cannot be changed by the app. Data
//! travels in the channel's transfer pages, mapped at
//! [`alaya_programs::desktop::XFER`].

use alloc::string::String;
use alloc::vec::Vec;

use alaya_programs::desktop as dk;
use alaya_programs::files::{self as fs, st};
use alaya_sys::{Caps, Msg};

/// A failed request: the store's status word ([`st`]), or `u64::MAX` when
/// the store did not answer.
pub type Status = u64;

#[derive(Clone, Copy)]
pub struct Store {
    ep: u64,
}

/// One version of a file, exactly: its version and how many times it was
/// changed (several saves can share a version until the store commits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    pub file: u64,
    pub version: u64,
    pub edits: u64,
}

/// What a result's mark says (living validity, see `alaya_store::made`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    NoMark,
    UpToDate,
    NeedsALook,
    AsOf,
    NotSure,
}

/// Why it says so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Nothing,
    SourceChanged,
    NotAllRead,
    SourceGone,
    ResultChanged,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub kind: Kind,
    pub why: Why,
    /// The source the mark is about (0: none), and its version now.
    pub source: u64,
    pub now_version: u64,
    /// The app that made the result, and from how many files.
    pub maker: u64,
    pub sources: u64,
}

/// One entry of a directory.
pub struct Entry {
    pub name: String,
    pub file: u64,
    pub size: u64,
    pub version: u64,
    pub is_dir: bool,
}

fn xfer() -> &'static mut [u8] {
    // SAFETY: the channel's transfer pages are mapped here for good; one
    // request at a time (one thread uses the store).
    unsafe { core::slice::from_raw_parts_mut(dk::XFER as *mut u8, fs::XFER_BYTES) }
}

impl Store {
    pub fn new(ep: u64) -> Store {
        Store { ep }
    }

    fn call(&self, label: u64, words: &[u64]) -> Result<[u64; 6], Status> {
        let r = alaya_sys::ipc_call(self.ep, &Msg::new(label, words), Caps::default())
            .map_err(|_| u64::MAX)?
            .words;
        if r[0] == st::OK { Ok(r) } else { Err(r[0]) }
    }

    pub fn unlock(&self, key: &[u8; 32]) -> Result<(), Status> {
        self.call(fs::UNLOCK, &words(key)).map(|_| ())
    }

    pub fn add_person(&self, key: &[u8; 32]) -> Result<(), Status> {
        self.call(fs::ADD_PERSON, &words(key)).map(|_| ())
    }

    pub fn root(&self, class: u64) -> Result<u64, Status> {
        Ok(self.call(fs::ROOT, &[class])?[1])
    }

    fn name(name: &str) -> Result<u64, Status> {
        if name.is_empty() || name.len() > 32 {
            return Err(st::BAD_REQUEST);
        }
        xfer()[..name.len()].copy_from_slice(name.as_bytes());
        Ok(name.len() as u64)
    }

    pub fn lookup(&self, dir: u64, name: &str) -> Result<u64, Status> {
        let n = Self::name(name)?;
        Ok(self.call(fs::LOOKUP, &[dir, n])?[1])
    }

    /// The file `name` in `dir`, made if it is not there.
    pub fn open(&self, dir: u64, name: &str) -> Result<u64, Status> {
        let n = Self::name(name)?;
        match self.call(fs::CREATE, &[dir, 0, n]) {
            Ok(r) => Ok(r[1]),
            Err(st::EXISTS) => self.lookup(dir, name),
            Err(e) => Err(e),
        }
    }

    /// Size, version, whether it is a directory.
    pub fn stat(&self, f: u64) -> Result<(u64, u64, bool), Status> {
        let r = self.call(fs::STAT, &[f])?;
        Ok((r[1], r[2], r[3] != 0))
    }

    /// Exactly which version of `f` this is now.
    pub fn version(&self, f: u64) -> Result<Version, Status> {
        let r = self.call(fs::STAT, &[f])?;
        Ok(Version {
            file: f,
            version: r[2],
            edits: r[4],
        })
    }

    /// All of `f`, and exactly which version it was. If the file changes
    /// while it is read, it is read again (a few times at most).
    pub fn read_versioned(&self, f: u64) -> Result<(Vec<u8>, Version), Status> {
        let mut tries = 0;
        loop {
            let before = self.version(f)?;
            let data = self.read_all(f)?;
            if self.version(f)? == before || tries == 3 {
                return Ok((data, before));
            }
            tries += 1;
        }
    }

    /// Say that this app made `result` from exactly these versions. The
    /// store checks that this app read all of each one.
    pub fn made_from(&self, result: u64, sources: &[Version]) -> Result<(), Status> {
        let buf = xfer();
        if sources.len() * 24 > buf.len() {
            return Err(st::BAD_REQUEST);
        }
        for (i, v) in sources.iter().enumerate() {
            for (j, w) in [v.file, v.version, v.edits].iter().enumerate() {
                buf[24 * i + 8 * j..24 * i + 8 * j + 8].copy_from_slice(&w.to_le_bytes());
            }
        }
        self.call(fs::MADE_FROM, &[result, sources.len() as u64])
            .map(|_| ())
    }

    /// Is `f` still up to date?
    pub fn mark(&self, f: u64) -> Result<Mark, Status> {
        let r = self.call(fs::MARK, &[f])?;
        let kind = match r[1] & 0xff {
            1 => Kind::UpToDate,
            2 => Kind::NeedsALook,
            3 => Kind::AsOf,
            4 => Kind::NotSure,
            _ => Kind::NoMark,
        };
        let why = match r[1] >> 8 {
            1 => Why::SourceChanged,
            2 => Why::NotAllRead,
            3 => Why::SourceGone,
            4 => Why::ResultChanged,
            _ => Why::Nothing,
        };
        Ok(Mark {
            kind,
            why,
            source: r[2],
            now_version: r[3],
            maker: r[4],
            sources: r[5],
        })
    }

    /// The person says `f` shows a fixed moment ("as of a date"), or not.
    pub fn as_of(&self, f: u64, on: bool) -> Result<(), Status> {
        self.call(fs::AS_OF, &[f, on as u64]).map(|_| ())
    }

    /// Which app changed which file in commit `cn`: (app, file, what).
    pub fn changes(&self, cn: u64) -> Result<Vec<(u64, u64, u64)>, Status> {
        let n = self.call(fs::CHANGES, &[cn])?[1] as usize;
        let b = xfer();
        let w = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
        Ok((0..n)
            .map(|i| (w(3 * i), w(3 * i + 1), w(3 * i + 2)))
            .collect())
    }

    pub fn read_all(&self, f: u64) -> Result<Vec<u8>, Status> {
        let (size, _, _) = self.stat(f)?;
        let mut out = Vec::with_capacity(size as usize);
        while (out.len() as u64) < size {
            let want = ((size - out.len() as u64) as usize).min(fs::XFER_BYTES);
            let r = self.call(fs::READ, &[f, out.len() as u64, want as u64])?;
            let got = r[1] as usize;
            if got == 0 {
                break;
            }
            out.extend_from_slice(&xfer()[..got]);
        }
        Ok(out)
    }

    /// Make the file hold exactly `data`.
    pub fn write_all(&self, f: u64, data: &[u8]) -> Result<(), Status> {
        let mut at = 0;
        while at < data.len() {
            let n = (data.len() - at).min(fs::XFER_BYTES);
            xfer()[..n].copy_from_slice(&data[at..at + n]);
            self.call(fs::WRITE, &[f, at as u64, n as u64])?;
            at += n;
        }
        self.call(fs::TRUNCATE, &[f, data.len() as u64])?;
        Ok(())
    }

    pub fn list(&self, dir: u64) -> Result<Vec<Entry>, Status> {
        let n = self.call(fs::LIST, &[dir])?[1] as usize;
        let mut raw = Vec::with_capacity(n);
        for i in 0..n {
            let b = &xfer()[48 * i..48 * i + 48];
            let file = u64::from_le_bytes(b[..8].try_into().unwrap());
            let len = (u64::from_le_bytes(b[8..16].try_into().unwrap()) as usize).min(32);
            let name = String::from_utf8_lossy(&b[16..16 + len]).into_owned();
            raw.push((name, file));
        }
        let mut out = Vec::with_capacity(n);
        for (name, file) in raw {
            let (size, version, is_dir) = self.stat(file)?;
            out.push(Entry {
                name,
                file,
                size,
                version,
                is_dir,
            });
        }
        Ok(out)
    }

    /// Make everything so far safe on the disk now (it is anyway within
    /// five seconds).
    pub fn save(&self) -> Result<u64, Status> {
        Ok(self.call(fs::SAVE, &[])?[1])
    }
}

fn words(key: &[u8; 32]) -> [u64; 4] {
    let mut w = [0u64; 4];
    for (i, x) in w.iter_mut().enumerate() {
        *x = u64::from_le_bytes(key[8 * i..8 * i + 8].try_into().unwrap());
    }
    w
}

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

/// One action on the undo list.
#[derive(Clone, Copy, Debug)]
pub struct Action {
    pub no: u64,
    pub cn: u64,
    pub app: u64,
    pub kind: u64,
    pub undone: bool,
    pub undoes: u64,
    pub label: [u8; fs::LABEL],
}

impl Action {
    pub fn label(&self) -> String {
        let n = self.label.iter().position(|&b| b == 0).unwrap_or(fs::LABEL);
        String::from_utf8_lossy(&self.label[..n]).into_owned()
    }
}

/// What an undo did.
#[derive(Clone, Debug)]
pub enum Undone {
    /// Done: the kind and label of the action undone, and the files put
    /// back.
    Done {
        kind: u64,
        label: [u8; fs::LABEL],
        files: Vec<u64>,
    },
    /// Nothing done: these files changed since (rule B6).
    Conflicts(Vec<u64>),
}

/// One change in "What happened?".
#[derive(Clone, Copy, Debug)]
pub struct Happening {
    pub cn: u64,
    pub time: u64,
    pub app: u64,
    pub file: u64,
    pub what: u64,
}

/// A file's story.
#[derive(Clone, Debug, Default)]
pub struct Story {
    /// The app that made it, the commit and time (0: not known).
    pub made_by: u64,
    pub made_cn: u64,
    pub made_at: u64,
    /// (commit, time, app, what), newest first.
    pub changes: Vec<(u64, u64, u64, u64)>,
    pub used_by: Vec<u64>,
}

/// One entry of a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
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

    /// Rename or move `name` in `dir` to `to_name` in `to_dir`.
    pub fn rename(&self, dir: u64, name: &str, to_dir: u64, to_name: &str) -> Result<(), Status> {
        if to_name.is_empty() || to_name.len() > 32 {
            return Err(st::BAD_REQUEST);
        }
        let n = Self::name(name)?;
        xfer()[fs::NAME2..fs::NAME2 + to_name.len()].copy_from_slice(to_name.as_bytes());
        self.call(fs::RENAME, &[dir, to_dir, n, to_name.len() as u64])
            .map(|_| ())
    }

    pub fn remove(&self, dir: u64, name: &str) -> Result<(), Status> {
        let n = Self::name(name)?;
        self.call(fs::REMOVE, &[dir, n]).map(|_| ())
    }

    /// The person is about to do something that goes on the undo list:
    /// what was gathered before is saved first.
    pub fn action_begin(&self) -> Result<(), Status> {
        self.call(fs::ACTION_BEGIN, &[]).map(|_| ())
    }

    /// What the person just did goes on the undo list (saved at once).
    pub fn record_action(&self, kind: u64, label: &[u8]) -> Result<u64, Status> {
        let n = label.len().min(fs::LABEL);
        xfer()[..n].copy_from_slice(&label[..n]);
        Ok(self.call(fs::RECORD_ACTION, &[kind, n as u64])?[1])
    }

    /// The undo list, newest first, below action `before` (0: all).
    pub fn actions(&self, before: u64, max: u64) -> Result<Vec<Action>, Status> {
        let n = self.call(fs::ACTIONS, &[before, max])?[1] as usize;
        let b = xfer();
        Ok((0..n)
            .map(|i| {
                let o = fs::ACTION_BYTES * i;
                let w =
                    |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
                Action {
                    no: w(0),
                    cn: w(1),
                    app: w(2),
                    kind: w(3) & 0xff,
                    undone: w(3) >> 8 & 1 != 0,
                    undoes: w(3) >> 16,
                    label: b[o + 32..o + 48].try_into().unwrap(),
                }
            })
            .collect())
    }

    /// The newest action that is not undone (and not itself an undo).
    pub fn last_action(&self) -> Result<Option<Action>, Status> {
        let mut before = 0;
        loop {
            let l = self.actions(before, 32)?;
            let Some(last) = l.last() else {
                return Ok(None);
            };
            if let Some(a) = l.iter().find(|a| !a.undone && a.kind != fs::act::UNDO) {
                return Ok(Some(*a));
            }
            before = last.no;
        }
    }

    fn undone(r: [u64; 6]) -> Undone {
        let n = r[2] as usize;
        let b = xfer();
        let files = (0..n)
            .map(|i| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap()))
            .collect();
        if r[1] == fs::undo::DONE {
            let mut label = [0u8; fs::LABEL];
            label[..8].copy_from_slice(&r[4].to_le_bytes());
            label[8..].copy_from_slice(&r[5].to_le_bytes());
            Undone::Done {
                kind: r[3],
                label,
                files,
            }
        } else {
            Undone::Conflicts(files)
        }
    }

    /// Undo action `no` (`force`: even though something changed since).
    pub fn undo(&self, no: u64, force: bool) -> Result<Undone, Status> {
        Ok(Self::undone(self.call(fs::UNDO, &[no, force as u64])?))
    }

    /// Undo what `app` changed in commit `cn`.
    pub fn undo_change(&self, cn: u64, app: u64, force: bool) -> Result<Undone, Status> {
        Ok(Self::undone(
            self.call(fs::UNDO_CHANGE, &[cn, app, force as u64])?,
        ))
    }

    /// What happened, newest first, in commits below `before` (0: from
    /// the newest), at most `max` commits.
    pub fn happenings(&self, before: u64, max: u64) -> Result<Vec<Happening>, Status> {
        let n = self.call(fs::HAPPENINGS, &[before, max])?[1] as usize;
        let b = xfer();
        Ok((0..n)
            .map(|i| {
                let o = fs::HAPPENING_BYTES * i;
                let w =
                    |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
                Happening {
                    cn: w(0),
                    time: w(1),
                    app: w(2),
                    file: w(3),
                    what: w(4),
                }
            })
            .collect())
    }

    /// A document's name now (`cn` 0) or at moment `cn`.
    pub fn name_of(&self, f: u64, cn: u64) -> Result<String, Status> {
        let r = self.call(fs::NAME_OF, &[f, cn])?;
        let n = (r[1] as usize).min(32);
        Ok(String::from_utf8_lossy(&xfer()[..n]).into_owned())
    }

    /// The moments history keeps: (commit, time), oldest first.
    pub fn history(&self) -> Result<Vec<(u64, u64)>, Status> {
        let r = self.call(fs::HISTORY, &[])?;
        let n = r[1] as usize;
        let b = xfer();
        let w = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
        // The store lists the newest first.
        let mut out: Vec<(u64, u64)> = (0..n).map(|i| (w(2 * i), w(2 * i + 1))).collect();
        out.reverse();
        Ok(out)
    }

    /// Give moment `cn` a name (an empty name takes it away).
    pub fn name_version(&self, cn: u64, name: &str) -> Result<(), Status> {
        let n = if name.is_empty() {
            0
        } else {
            Self::name(name)?
        };
        self.call(fs::NAME_VERSION, &[cn, n]).map(|_| ())
    }

    /// Named versions: (commit, time, name), oldest first.
    pub fn named(&self) -> Result<Vec<(u64, u64, String)>, Status> {
        let n = self.call(fs::NAMED, &[])?[1] as usize;
        let b = xfer();
        Ok((0..n)
            .map(|i| {
                let o = fs::NAMED_BYTES * i;
                let w =
                    |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
                let len = (w(2) as usize).min(32);
                (
                    w(0),
                    w(1),
                    String::from_utf8_lossy(&b[o + 24..o + 24 + len]).into_owned(),
                )
            })
            .collect())
    }

    /// What `dir` held at moment `cn`.
    pub fn past_list(&self, cn: u64, dir: u64) -> Result<Vec<Entry>, Status> {
        let n = self.call(fs::PAST_LIST, &[cn, dir])?[1] as usize;
        let b = xfer();
        Ok((0..n)
            .map(|i| {
                let o = fs::PAST_ENTRY_BYTES * i;
                let w =
                    |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
                let len = (w(1) as usize).min(32);
                Entry {
                    name: String::from_utf8_lossy(&b[o + 32..o + 32 + len]).into_owned(),
                    file: w(0),
                    size: w(2),
                    version: 0,
                    is_dir: w(3) != 0,
                }
            })
            .collect())
    }

    /// Put `files` back as they were at moment `cn`.
    pub fn restore(&self, cn: u64, files: &[u64]) -> Result<Undone, Status> {
        let b = xfer();
        for (i, f) in files.iter().enumerate() {
            b[8 * i..8 * i + 8].copy_from_slice(&f.to_le_bytes());
        }
        let r = self.call(fs::RESTORE, &[cn, files.len() as u64])?;
        Ok(Self::undone([r[0], r[1], r[2], 0, 0, 0]))
    }

    /// Where `f` came from and what was made from it.
    pub fn story(&self, f: u64, max: u64) -> Result<Story, Status> {
        let r = self.call(fs::STORY, &[f, max])?;
        let b = xfer();
        let w = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
        let n = r[4] as usize;
        let changes: Vec<_> = (0..n)
            .map(|i| (w(4 * i), w(4 * i + 1), w(4 * i + 2), w(4 * i + 3)))
            .collect();
        let used_by = (0..r[5] as usize).map(|i| w(4 * n + i)).collect();
        Ok(Story {
            made_by: r[1],
            made_cn: r[2],
            made_at: r[3],
            changes,
            used_by,
        })
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

/// The name people know an app by.
pub fn app_name(app: u64) -> &'static str {
    use alaya_programs::desktop::role;
    match app {
        a if a == fs::app_id(role::EDITOR) => "the text editor",
        a if a == fs::app_id(role::FILES) => "the file browser",
        a if a == fs::app_id(role::CHART) => "the chart maker",
        a if a == fs::app_id(role::SETTINGS) => "Settings",
        a if a == fs::app_id(role::BACK) => "the timeline",
        fs::DESKTOP_APP => "the desktop",
        fs::INIT_APP => "Alaya's checks",
        _ => "another app",
    }
}

/// Labels on the undo list: a letter for what was done, then a name.
pub mod label {
    pub const DELETED: u8 = b'd';
    pub const RENAMED: u8 = b'r';
    pub const LOOK: u8 = b'l';
    pub const WINDOW: u8 = b'W';
    /// A window into a group: then its role, the group it was in, the
    /// group it went to.
    pub const GROUP: u8 = b'G';
}

/// A label for the undo list: what (a [`label`] letter) and the name.
pub fn make_label(what: u8, name: &str) -> String {
    let mut s = String::new();
    s.push(what as char);
    for c in name.chars() {
        if s.len() + c.len_utf8() > fs::LABEL {
            break;
        }
        s.push(c);
    }
    s
}

/// What an action was, in a few plain words ("delete notes.txt").
pub fn describe(label: &[u8]) -> String {
    let n = label.iter().position(|&b| b == 0).unwrap_or(label.len());
    let rest = |from: usize| String::from_utf8_lossy(&label[from.min(n)..n]).into_owned();
    match label.first() {
        Some(&label::DELETED) => alloc::format!("delete {}", rest(1)),
        Some(&label::RENAMED) => alloc::format!("rename to {}", rest(1)),
        Some(&label::LOOK) => String::from("change the look"),
        Some(&label::WINDOW) => alloc::format!("close {}", rest(2)),
        Some(&label::GROUP) => alloc::format!("move {} into a group", rest(4)),
        _ if n == 0 => String::from("a change"),
        _ => rest(0),
    }
}

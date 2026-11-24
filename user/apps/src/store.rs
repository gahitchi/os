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

/// How long a hand-over lasts (as `alaya_store::grants::Lasts`).
/// One search result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub file: u64,
    pub dir: u64,
    pub name: String,
    /// Where the words start; None when the name matched.
    pub at: Option<u64>,
    /// The line around the words.
    pub line: String,
    /// Found only in an older version: (moment, its time in seconds).
    pub past: Option<(u64, u64)>,
    /// The file was removed since (it can be put back).
    pub gone: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lasts {
    Live,
    Pinned(u64),
    Until(u64),
    UntilClosed,
}

impl Lasts {
    pub fn code(self) -> (u64, u64) {
        match self {
            Lasts::Live => (0, 0),
            Lasts::Pinned(cn) => (1, cn),
            Lasts::Until(t) => (2, t),
            Lasts::UntilClosed => (3, 0),
        }
    }

    pub fn from_code(kind: u64, arg: u64) -> Option<Lasts> {
        Some(match kind {
            0 => Lasts::Live,
            1 => Lasts::Pinned(arg),
            2 => Lasts::Until(arg),
            3 => Lasts::UntilClosed,
            _ => return None,
        })
    }

    /// In plain words.
    pub fn words(self) -> &'static str {
        match self {
            Lasts::Live => "until you take it back (in Settings)",
            Lasts::Pinned(_) => "just this version, read only",
            Lasts::Until(_) => "for one hour",
            Lasts::UntilClosed => "until the app closes",
        }
    }
}

/// One hand-over, as GRANTS lists it.
#[derive(Clone, Copy, Debug)]
pub struct GrantRow {
    pub app: u64,
    pub file: u64,
    pub write: bool,
    pub own: bool,
    pub lasts: Lasts,
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

    /// Hand a document to an app (the desktop only).
    pub fn grant(&self, app: u64, f: u64, write: bool, lasts: Lasts) -> Result<(), Status> {
        let (kind, arg) = lasts.code();
        self.call(fs::GRANT, &[app, f, write as u64, kind, arg])
            .map(|_| ())
    }

    /// Take a document back from an app.
    pub fn revoke(&self, app: u64, f: u64) -> Result<bool, Status> {
        Ok(self.call(fs::REVOKE, &[app, f])?[1] != 0)
    }

    /// What has been handed to one app (0: to every app).
    pub fn grants(&self, app: u64) -> Result<Vec<GrantRow>, Status> {
        let n = self.call(fs::GRANTS, &[app])?[1] as usize;
        let b = xfer();
        let w = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
        Ok((0..n)
            .filter_map(|i| {
                let k = 4 * i;
                Some(GrantRow {
                    app: w(k),
                    file: w(k + 1),
                    write: w(k + 2) >> 8 & 1 != 0,
                    own: w(k + 2) >> 9 & 1 != 0,
                    lasts: Lasts::from_code(w(k + 2) & 0xff, w(k + 3))?,
                })
            })
            .collect())
    }

    /// Forget a document after `secs`: it leaves its folder now, and its
    /// key is destroyed then unless it is kept meanwhile.
    pub fn forget_later(&self, dir: u64, name: &str, secs: u64) -> Result<u64, Status> {
        xfer()[..name.len()].copy_from_slice(name.as_bytes());
        Ok(self.call(fs::FORGET_LATER, &[dir, name.len() as u64, secs])?[1])
    }

    /// Documents waiting to be forgotten: (file, seconds left, its name).
    pub fn forgetting(&self) -> Result<Vec<(u64, u64, String)>, Status> {
        let n = self.call(fs::FORGETTING, &[])?[1] as usize;
        let rows: Vec<[u64; 4]> = {
            let b = xfer();
            let w = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
            (0..n)
                .map(|i| [w(4 * i), w(4 * i + 1), w(4 * i + 2), w(4 * i + 3)])
                .collect()
        };
        Ok(rows
            .into_iter()
            .map(|r| (r[0], r[2], self.name_of(r[0], r[3]).unwrap_or_default()))
            .collect())
    }

    /// Keep a document that was waiting to be forgotten.
    pub fn keep_forgotten(&self, f: u64) -> Result<(), Status> {
        self.call(fs::KEEP_FORGOTTEN, &[f]).map(|_| ())
    }

    /// Search the person's documents (Alaya's own programs only): names,
    /// text, and what files said at up to `moments` older moments.
    pub fn search(&self, words: &str, most: u64, moments: u64) -> Result<Vec<Found>, Status> {
        let q = words.trim().as_bytes();
        if q.is_empty() || q.len() > 64 {
            return Err(st::BAD_REQUEST);
        }
        xfer()[..q.len()].copy_from_slice(q);
        let n = self.call(fs::SEARCH, &[q.len() as u64, most, moments])?[1] as usize;
        let b = xfer();
        Ok((0..n)
            .map(|i| {
                let o = fs::SEARCH_BYTES * i;
                let w =
                    |j: usize| u64::from_le_bytes(b[o + 8 * j..o + 8 * j + 8].try_into().unwrap());
                let text = |at: usize, len: u64, most: usize| {
                    String::from_utf8_lossy(&b[o + at..o + at + (len as usize).min(most)])
                        .into_owned()
                };
                Found {
                    file: w(0),
                    dir: w(1),
                    at: (w(2) != u64::MAX).then_some(w(2)),
                    past: (w(3) != 0).then_some((w(3), w(4))),
                    gone: w(5) != 0,
                    name: text(64, w(6), 32),
                    line: text(96, w(7), 64),
                }
            })
            .collect())
    }

    /// Back up to the stick now: (run, written, gone, blocks used, bytes).
    pub fn backup(&self, erase: bool) -> Result<[u64; 5], Status> {
        let r = self.call(fs::BACKUP, &[erase as u64])?;
        Ok([r[1], r[2], r[3], r[4], r[5]])
    }

    /// The person's recovery key, as written down (and whether it is new).
    pub fn recovery_key(&self) -> Result<(String, bool), Status> {
        let r = self.call(fs::RECOVERY_KEY, &[])?;
        Ok((
            String::from_utf8_lossy(&xfer()[..39]).into_owned(),
            r[1] != 0,
        ))
    }

    /// Make the documents on the stick again, with the recovery key as
    /// typed, in folder `into` (or the documents themselves, when empty):
    /// (folders, documents, the folder, run).
    pub fn restore_backup(&self, key: &str, into: &str) -> Result<[u64; 4], Status> {
        if into.is_empty() || into.len() > 32 {
            return Err(st::BAD_REQUEST);
        }
        let b = xfer();
        b[..64].fill(0);
        let k = key.as_bytes();
        let mut n = 0;
        for &c in k.iter().filter(|c| !c.is_ascii_whitespace()) {
            if n < 39 {
                b[n] = c;
                n += 1;
            }
        }
        b[64..64 + into.len()].copy_from_slice(into.as_bytes());
        let r = self.call(fs::RESTORE_BACKUP, &[into.len() as u64])?;
        Ok([r[1], r[2], r[3], r[4]])
    }

    /// Mark a document private (Some(true)), not (Some(false)), or ask.
    pub fn private(&self, f: u64, on: Option<bool>) -> Result<bool, Status> {
        let what = match on {
            Some(true) => 1,
            Some(false) => 0,
            None => 2,
        };
        Ok(self.call(fs::PRIVATE, &[f, what])?[1] != 0)
    }

    /// Apps that read a private document: (app, the document, since).
    pub fn marks(&self) -> Result<Vec<(u64, u64, u64)>, Status> {
        let n = self.call(fs::MARKS, &[])?[1] as usize;
        let b = xfer();
        let w = |i: usize| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
        Ok((0..n)
            .map(|i| (w(3 * i), w(3 * i + 1), w(3 * i + 2)))
            .collect())
    }

    /// The stick: (there is one, its blocks, last run, last moment).
    pub fn stick(&self) -> Result<[u64; 4], Status> {
        let r = self.call(fs::STICK, &[])?;
        Ok([r[1], r[2], r[3], r[4]])
    }

    /// Start a job for `app` under rules the person approved (Alaya's
    /// own programs only): the folder it may change, the checks, the
    /// checklist (0: none) and what must stay unchanged.
    pub fn job_start(
        &self,
        app: u64,
        folder: u64,
        checks: u64,
        checklist: u64,
        keep_same: &[u64],
        label: &[u8],
    ) -> Result<u64, Status> {
        let b = xfer();
        for (i, f) in keep_same.iter().enumerate() {
            b[8 * i..8 * i + 8].copy_from_slice(&f.to_le_bytes());
        }
        let l = label.len().min(24);
        b[fs::JOB_LABEL_AT..fs::JOB_LABEL_AT + l].copy_from_slice(&label[..l]);
        Ok(self.call(
            fs::JOB_START,
            &[
                app,
                folder,
                checks,
                checklist,
                keep_same.len() as u64,
                l as u64,
            ],
        )?[1])
    }

    /// The asking app's own job: (folder, done, checks), if it has one.
    pub fn job(&self) -> Result<Option<(u64, bool, u64)>, Status> {
        let r = self.call(fs::JOB, &[])?;
        Ok((r[1] != 0).then_some((r[2], r[3] != 0, r[4])))
    }

    /// The asking app says its job is done.
    pub fn job_done(&self) -> Result<(), Status> {
        self.call(fs::JOB_DONE, &[]).map(|_| ())
    }

    /// What Alaya's checks of `app`'s job found.
    pub fn job_check(&self, app: u64) -> Result<JobReport, Status> {
        let r = self.call(fs::JOB_CHECK, &[app])?;
        let (n, m) = (r[2] as usize, r[3] as usize);
        let b = xfer();
        let w = |at: usize| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
        Ok(JobReport {
            passed: r[1] != 0,
            checks: (0..n)
                .map(|i| {
                    let o = 32 * i;
                    (w(o), w(o + 8) as u32, w(o + 16) != 0, w(o + 24))
                })
                .collect(),
            stale: (0..m).map(|i| w(32 * n + 8 * i)).collect(),
        })
    }

    /// Keep `app`'s job's work: (`fs::job_kept` kind, files). With
    /// `anyway`, although its checks failed (the person's deliberate
    /// choice, on record); `mine`: conflicts where the person's own
    /// version stays.
    pub fn job_keep(
        &self,
        app: u64,
        anyway: bool,
        mine: &[u64],
    ) -> Result<(u64, Vec<u64>), Status> {
        let b = xfer();
        for (i, f) in mine.iter().enumerate() {
            b[8 * i..8 * i + 8].copy_from_slice(&f.to_le_bytes());
        }
        let r = self.call(fs::JOB_KEEP, &[app, anyway as u64, mine.len() as u64])?;
        let b = xfer();
        let files = (0..r[2] as usize)
            .map(|i| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap()))
            .collect();
        Ok((r[1], files))
    }

    /// Throw `app`'s job's work away.
    pub fn job_discard(&self, app: u64) -> Result<(), Status> {
        self.call(fs::JOB_DISCARD, &[app]).map(|_| ())
    }

    /// "Run it again" (Alaya's own programs): until its last window
    /// closes, `app` sees exactly what it had read by commit `cn`. Returns
    /// (files, how many exact).
    pub fn run_again(&self, app: u64, cn: u64) -> Result<(u64, u64), Status> {
        let r = self.call(fs::RUN_AGAIN, &[app, cn])?;
        Ok((r[1], r[2]))
    }

    /// Copy part of a saved document: the clipboard knows exactly which
    /// version it came from.
    pub fn copy(&self, f: u64, at: u64, len: u64) -> Result<u64, Status> {
        Ok(self.call(fs::COPY, &[f, at, len])?[1])
    }

    /// Copy text that belongs to no saved document.
    pub fn copy_text(&self, text: &[u8]) -> Result<(), Status> {
        let n = text.len().min(fs::CLIP_MAX);
        xfer()[..n].copy_from_slice(&text[..n]);
        self.call(fs::COPY_TEXT, &[n as u64]).map(|_| ())
    }

    /// What the clipboard holds, and the version it came from.
    pub fn paste(&self) -> Result<(Vec<u8>, Option<Version>), Status> {
        let r = self.call(fs::PASTE, &[])?;
        let data = xfer()[..(r[1] as usize).min(fs::CLIP_MAX)].to_vec();
        let src = (r[2] != 0).then_some(Version {
            file: r[2],
            version: r[3],
            edits: r[4],
        });
        Ok((data, src))
    }

    /// "Show the difference" for a result that needs a look.
    pub fn what_changed(&self, result: u64) -> Result<Option<Difference>, Status> {
        let r = self.call(fs::WHAT_CHANGED, &[result])?;
        if r[1] == 0 {
            return Ok(None);
        }
        let b = xfer();
        let then = b[..(r[3] as usize).min(fs::DIFF_AT)].to_vec();
        let now = b[fs::DIFF_AT..fs::DIFF_AT + (r[4] as usize).min(fs::DIFF_AT)].to_vec();
        Ok(Some(Difference {
            source: r[1],
            exact: r[2] != 0,
            then,
            now,
        }))
    }

    /// The person's groups and tasks (Alaya's own programs).
    pub fn groups(&self) -> Result<Vec<GroupRow>, Status> {
        let r = self.call(fs::GROUPS, &[])?;
        let b = xfer();
        Ok((0..r[1] as usize)
            .map(|i| {
                let o = fs::GROUP_BYTES * i;
                let w =
                    |j: usize| u64::from_le_bytes(b[o + 8 * j..o + 8 * j + 8].try_into().unwrap());
                let name = &b[o + 40..o + 56];
                let n = name.iter().position(|&c| c == 0).unwrap_or(16);
                GroupRow {
                    id: w(0) as u32,
                    task: w(1) & fs::GROUP_TASK != 0,
                    paused: w(1) & fs::GROUP_PAUSED != 0,
                    done: w(1) & fs::GROUP_DONE != 0,
                    color: w(2) as u32,
                    made: w(3),
                    left: w(4),
                    name: String::from_utf8_lossy(&name[..n]).into_owned(),
                }
            })
            .collect())
    }

    /// Make a new loose group: its number.
    pub fn group_new(&self, name: &str, color: u32) -> Result<u32, Status> {
        let n = name.len().min(32);
        xfer()[..n].copy_from_slice(&name.as_bytes()[..n]);
        Ok(self.call(fs::GROUP_NEW, &[u64::from(color), n as u64])?[1] as u32)
    }

    /// Change a group's name, colour, task and pause flags, or when it
    /// was left.
    pub fn group_set(&self, g: &GroupRow) -> Result<(), Status> {
        let n = g.name.len().min(32);
        xfer()[..n].copy_from_slice(&g.name.as_bytes()[..n]);
        let flags =
            if g.task { fs::GROUP_TASK } else { 0 } | if g.paused { fs::GROUP_PAUSED } else { 0 };
        self.call(
            fs::GROUP_SET,
            &[u64::from(g.id), flags, u64::from(g.color), g.left, n as u64],
        )
        .map(|_| ())
    }

    pub fn group_remove(&self, id: u32) -> Result<(), Status> {
        self.call(fs::GROUP_REMOVE, &[u64::from(id)]).map(|_| ())
    }

    /// Put a file in a group: true when it was not in it.
    pub fn group_add(&self, id: u32, f: u64) -> Result<bool, Status> {
        Ok(self.call(fs::GROUP_ADD, &[u64::from(id), f])?[1] != 0)
    }

    /// Take a file out of a group: true when it was in it.
    pub fn group_take(&self, id: u32, f: u64) -> Result<bool, Status> {
        Ok(self.call(fs::GROUP_TAKE, &[u64::from(id), f])?[1] != 0)
    }

    /// What is in a group.
    pub fn group_files(&self, id: u32) -> Result<Vec<u64>, Status> {
        let r = self.call(fs::GROUP_FILES, &[u64::from(id)])?;
        let b = xfer();
        Ok((0..r[1] as usize)
            .map(|i| u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap()))
            .collect())
    }

    /// What Finish would do with group `id`.
    pub fn finish_plan(&self, id: u32) -> Result<Vec<FinishRow>, Status> {
        let r = self.call(fs::FINISH_PLAN, &[u64::from(id)])?;
        let b = xfer();
        Ok((0..r[1] as usize)
            .map(|i| {
                let o = fs::FINISH_BYTES * i;
                let w =
                    |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
                FinishRow {
                    fate: w(0),
                    file: w(1),
                    other: w(2),
                    write: w(3) != 0,
                }
            })
            .collect())
    }

    /// Finish group `id`, archiving `archive`: → the work receipt.
    pub fn finish(&self, id: u32, archive: &[u64]) -> Result<u64, Status> {
        let n = archive.len().min(fs::XFER_BYTES / 8);
        for (i, f) in archive.iter().take(n).enumerate() {
            xfer()[8 * i..8 * i + 8].copy_from_slice(&f.to_le_bytes());
        }
        Ok(self.call(fs::FINISH, &[u64::from(id), n as u64])?[1])
    }

    /// Everything Finish did to group `id` comes back.
    pub fn finish_undo(&self, id: u32) -> Result<(), Status> {
        self.call(fs::FINISH_UNDO, &[u64::from(id)]).map(|_| ())
    }

    /// Open `app` in a protected task (or in none).
    pub fn open_in_task(&self, app: u64, task: Option<u32>) -> Result<(), Status> {
        self.call(fs::OPEN_IN_TASK, &[app, task.map_or(0, u64::from)])
            .map(|_| ())
    }

    /// The task `app` is in, if any.
    pub fn task_of(&self, app: u64) -> Result<Option<u32>, Status> {
        let t = self.call(fs::TASK_OF, &[app])?[1];
        Ok((t != 0).then_some(t as u32))
    }

    /// The person left group `id` now, with these apps (bit r) open in it.
    pub fn group_leave(&self, id: u32, roles: u64) -> Result<(), Status> {
        self.call(fs::GROUP_LEAVE, &[u64::from(id), roles])
            .map(|_| ())
    }

    /// The person's own note for group `id` (empty clears it).
    pub fn group_note(&self, id: u32, note: &str) -> Result<(), Status> {
        let n = note.len().min(fs::NOTE_MAX);
        xfer()[..n].copy_from_slice(&note.as_bytes()[..n]);
        self.call(fs::GROUP_NOTE, &[u64::from(id), n as u64])
            .map(|_| ())
    }

    /// Where the person left group `id`: (when, moment, roles, note).
    pub fn return_point(&self, id: u32) -> Result<Option<(u64, u64, u64, String)>, Status> {
        let r = self.call(fs::RETURN_POINT, &[u64::from(id)])?;
        if r[1] == 0 {
            return Ok(None);
        }
        let n = (r[4] as usize).min(fs::NOTE_MAX);
        let note = String::from_utf8_lossy(&xfer()[..n]).into_owned();
        Ok(Some((r[1], r[2], r[3], note)))
    }

    /// "Since you left" for group `id`, each line with its receipt.
    pub fn since_left(&self, id: u32) -> Result<Vec<SinceRow>, Status> {
        let r = self.call(fs::SINCE_LEFT, &[u64::from(id)])?;
        let b = xfer();
        Ok((0..r[1] as usize)
            .map(|i| {
                let o = fs::SINCE_BYTES * i;
                let w =
                    |k: usize| u64::from_le_bytes(b[o + 8 * k..o + 8 * k + 8].try_into().unwrap());
                SinceRow {
                    kind: w(0),
                    file: w(1),
                    cn: w(2),
                    time: w(3),
                    app: w(4),
                    what: w(5),
                    source: w(6),
                    count: w(7),
                }
            })
            .collect())
    }

    /// An app's last window closed (the desktop only).
    pub fn app_closed(&self, app: u64) -> Result<u64, Status> {
        Ok(self.call(fs::APP_CLOSED, &[app])?[1])
    }

    /// Make everything so far safe on the disk now (it is anyway within
    /// five seconds).
    pub fn save(&self) -> Result<u64, Status> {
        Ok(self.call(fs::SAVE, &[])?[1])
    }
}

/// One line of "since you left" and its receipt (see `fs::SINCE_LEFT`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinceRow {
    /// 1: the ledger says it changed; 2: it needs a look.
    pub kind: u64,
    pub file: u64,
    pub cn: u64,
    pub time: u64,
    pub app: u64,
    pub what: u64,
    pub source: u64,
    pub count: u64,
}

/// One group or task, as the desktop sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GroupRow {
    pub id: u32,
    pub name: String,
    pub color: u32,
    pub task: bool,
    pub paused: bool,
    /// Finished (Finish can be undone).
    pub done: bool,
    pub made: u64,
    pub left: u64,
}

/// One line of the Finish screen.
#[derive(Clone, Copy, Debug)]
pub struct FinishRow {
    /// `fs::fate`.
    pub fate: u64,
    pub file: u64,
    /// The other group (elsewhere) or the app (hand back).
    pub other: u64,
    pub write: bool,
}

/// What changed in a source since a result was made from it.
#[derive(Clone, Debug, Default)]
pub struct Difference {
    pub source: u64,
    /// `then` is exactly the version the result used.
    pub exact: bool,
    pub then: Vec<u8>,
    pub now: Vec<u8>,
}

/// What Alaya's checks of a job found.
#[derive(Clone, Debug, Default)]
pub struct JobReport {
    pub passed: bool,
    /// (check bit, checklist line, ok, the file it failed on).
    pub checks: Vec<(u64, u32, bool, u64)>,
    /// Files the job read that the person changed since.
    pub stale: Vec<u64>,
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
    fs::app_label(app)
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
    /// A file into a group or out of it: the group, 1 in / 0 out, the
    /// file's number (4 bytes), then its name.
    pub const IN_GROUP: u8 = b'g';
    /// A group became a task, or paused: the group, the flags before and
    /// after (1 task, 2 paused), then its name.
    pub const TASK: u8 = b't';
    /// A group was finished: the group, then its name.
    pub const FINISH: u8 = b'f';
    /// An app was installed (milestone 8): its place, then its name.
    pub const INSTALL: u8 = b'i';
    /// An app was removed: its place, then its name.
    pub const REMOVE: u8 = b'x';
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
    // The name after a header (which may hold zero bytes: numbers).
    let rest = |from: usize| {
        let from = from.min(label.len());
        let end = label[from..]
            .iter()
            .position(|&b| b == 0)
            .map_or(label.len(), |i| from + i);
        String::from_utf8_lossy(&label[from..end]).into_owned()
    };
    match label.first() {
        Some(&label::DELETED) => alloc::format!("delete {}", rest(1)),
        Some(&label::RENAMED) => alloc::format!("rename to {}", rest(1)),
        Some(&label::LOOK) => String::from("change the look"),
        Some(&label::WINDOW) => alloc::format!("close {}", rest(2)),
        Some(&label::GROUP) => alloc::format!("move {} into a group", rest(4)),
        Some(&label::IN_GROUP) if label.get(2) == Some(&1) => {
            alloc::format!("put {} in a group", rest(7))
        }
        Some(&label::IN_GROUP) => alloc::format!("take {} out of a group", rest(7)),
        Some(&label::FINISH) => alloc::format!("finish {}", rest(2)),
        Some(&label::INSTALL) => alloc::format!("install {}", rest(2)),
        Some(&label::REMOVE) => alloc::format!("remove {}", rest(2)),
        Some(&label::TASK) => match (label.get(2).copied(), label.get(3).copied()) {
            (_, Some(3)) => alloc::format!("pause {}", rest(4)),
            (Some(3), Some(1)) => alloc::format!("go on with {}", rest(4)),
            (_, Some(1)) => alloc::format!("make {} a task", rest(4)),
            _ => alloc::format!("make {} a loose group", rest(4)),
        },
        _ if n == 0 => String::from("a change"),
        _ => rest(0),
    }
}

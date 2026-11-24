// SPDX-License-Identifier: GPL-3.0-only
//! Resume, with "since you left" (milestone 7a, step 2; `continuity.md`
//! section 3).
//!
//! When the person leaves a group, the desktop keeps a **return point**:
//! when, the moment the store was at, which apps were open in it, and an
//! optional note in the person's own words. Coming back, the **briefing**
//! lists what changed since, limited to the group's files:
//!
//! - each file the ledger says changed since that moment (the newest
//!   change, and how many there were), and
//! - each result in the group that now "needs a look".
//!
//! Every line carries its **receipt**: the ledger record (moment, time,
//! app, what) or the mark's source it comes from. Nothing else is ever a
//! line: Alaya never writes a summary of its own, and never a next step
//! for the person (the note is theirs, word for word).

use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{Key, word, words};
use crate::grants::RECORDS;
use crate::made::Kind;
use crate::{App, Class, Error, FileId, Person, Store, what};

/// A note is at most this long (bytes).
pub const NOTE_MAX: usize = 160;
/// A briefing has at most this many lines.
pub const BRIEF_MAX: usize = 24;
/// How far back in the ledger a briefing looks (moments with changes).
const LOOK_BACK: usize = 256;

const K_BACK: u64 = 15;
const K_NOTE: u64 = 16;

fn k_back(id: u32) -> Key {
    Key(RECORDS | K_BACK, u64::from(id))
}

fn k_note(id: u32, chunk: u64) -> Key {
    Key(RECORDS | K_NOTE, u64::from(id) << 8 | chunk)
}

/// Where the person left a group.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReturnPoint {
    /// When (the store's clock).
    pub left: u64,
    /// The last moment (commit) when they left: changes after it are new.
    pub left_cn: u64,
    /// The apps that were open in it (bit r for role r).
    pub roles: u64,
    /// The person's own note, as typed.
    pub note: Vec<u8>,
}

/// What a briefing line is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    /// The ledger says the file changed since.
    Changed = 1,
    /// A result in the group needs a look: a source changed.
    NeedsALook = 2,
}

/// One line of "since you left", with its receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Since {
    pub kind: LineKind,
    pub file: FileId,
    /// Changed: the newest change's moment, time, app and what
    /// ([`what`] bits); NeedsALook: the source that changed, its version
    /// now (the moment that made it) and the result's maker.
    pub cn: u64,
    pub time: u64,
    pub app: App,
    pub what: u64,
    pub source: Option<FileId>,
    /// Changed: how many changes since.
    pub count: u32,
}

impl<D: Disk> Store<D> {
    /// The person left group `id` now, with these apps open in it.
    pub fn group_leave(&mut self, p: Person, id: u32, now: u64, roles: u64) -> Result<(), Error> {
        self.ok()?;
        self.group(p, id)?;
        let len = self.return_point(p, id)?.map_or(0, |r| r.note.len() as u64);
        let cn = self.last_commit();
        self.put_rec(p, k_back(id), words(&[now, cn, roles, len]))
    }

    /// Write (or clear, with an empty one) the person's note for group
    /// `id`: their words, kept as they are.
    pub fn group_note(&mut self, p: Person, id: u32, note: &[u8]) -> Result<(), Error> {
        self.ok()?;
        if note.len() > NOTE_MAX {
            return Err(Error::BadRequest);
        }
        self.group(p, id)?;
        let r = self.return_point(p, id)?.unwrap_or_default();
        for (i, c) in note.chunks(40).enumerate() {
            let mut v = [0u8; 40];
            v[..c.len()].copy_from_slice(c);
            self.put_rec(p, k_note(id, i as u64), v)?;
        }
        for i in note.len().div_ceil(40)..NOTE_MAX.div_ceil(40) {
            self.del_rec(p, k_note(id, i as u64))?;
        }
        self.put_rec(
            p,
            k_back(id),
            words(&[r.left, r.left_cn, r.roles, note.len() as u64]),
        )
    }

    /// Where the person left group `id`, if they did.
    pub fn return_point(&mut self, p: Person, id: u32) -> Result<Option<ReturnPoint>, Error> {
        let Some(v) = self.get_rec(p, k_back(id))? else {
            return Ok(None);
        };
        let len = (word(&v, 3) as usize).min(NOTE_MAX);
        let mut note = Vec::with_capacity(len);
        for i in 0..len.div_ceil(40) {
            let c = self.get_rec(p, k_note(id, i as u64))?.unwrap_or([0; 40]);
            note.extend_from_slice(&c[..(len - 40 * i).min(40)]);
        }
        Ok(Some(ReturnPoint {
            left: word(&v, 0),
            left_cn: word(&v, 1),
            roles: word(&v, 2),
            note,
        }))
    }

    /// A group is gone: so is its return point.
    pub(crate) fn return_point_forget(&mut self, p: Person, id: u32) -> Result<(), Error> {
        self.del_rec(p, k_back(id))?;
        for i in 0..NOTE_MAX.div_ceil(40) {
            self.del_rec(p, k_note(id, i as u64))?;
        }
        Ok(())
    }

    /// Whether `f` is in group `id`: one of its files, or inside one of
    /// its folders.
    pub fn in_group_reach(&mut self, p: Person, id: u32, f: FileId) -> Result<bool, Error> {
        for m in self.group_files(p, id)? {
            if m == f || (self.stat(p, m).is_ok_and(|s| s.is_dir) && self.inside(p, f, m)?) {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// "Since you left" for group `id`: only what the ledger and the
    /// marks say, each line with its receipt. Empty when the person never
    /// left it.
    pub fn since_left(&mut self, p: Person, id: u32) -> Result<Vec<Since>, Error> {
        let Some(r) = self.return_point(p, id)? else {
            return Ok(Vec::new());
        };
        let mut out: Vec<Since> = Vec::new();
        let mut before = 0;
        let mut seen = 0;
        'ledger: while seen < LOOK_BACK {
            let batch = self.happenings(p, before, 16)?;
            let Some(last) = batch.last() else { break };
            before = last.cn;
            for h in batch {
                seen += 1;
                if h.cn <= r.left_cn {
                    break 'ledger;
                }
                for c in h.changes {
                    if c.file.class() != Class::Documents {
                        continue;
                    }
                    if let Some(l) = out.iter_mut().find(|l| l.file == c.file) {
                        // Newest first: this one is older.
                        l.count += 1;
                        continue;
                    }
                    let reach = if c.what & (what::DELETED | what::FORGOTTEN) != 0 {
                        // Gone now: it counts when it was one of the
                        // group's own files.
                        self.group_files(p, id)?.contains(&c.file)
                    } else {
                        self.in_group_reach(p, id, c.file)?
                    };
                    if reach && out.len() < BRIEF_MAX {
                        out.push(Since {
                            kind: LineKind::Changed,
                            file: c.file,
                            cn: h.cn,
                            time: h.time,
                            app: c.app,
                            what: c.what,
                            source: None,
                            count: 1,
                        });
                    }
                }
            }
            if before <= 1 {
                break;
            }
        }
        // Results in the group that need a look now.
        let mut members = Vec::new();
        for m in self.group_files(p, id)? {
            match self.stat(p, m) {
                Ok(s) if s.is_dir => {
                    for (_, f) in self.list(p, m)? {
                        members.push(f);
                    }
                }
                Ok(_) => members.push(m),
                Err(_) => {}
            }
        }
        for f in members {
            if out.len() >= BRIEF_MAX {
                break;
            }
            let Ok(m) = self.mark(p, f) else { continue };
            if m.kind == Kind::NeedsALook {
                out.push(Since {
                    kind: LineKind::NeedsALook,
                    file: f,
                    cn: m.now_version,
                    time: 0,
                    app: m.maker,
                    what: 0,
                    source: m.source,
                    count: 0,
                });
            }
        }
        Ok(out)
    }

    pub(crate) fn get_rec(&mut self, p: Person, k: Key) -> Result<Option<[u8; 40]>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].get(dev, &b.keys, k)
    }

    pub(crate) fn put_rec(&mut self, p: Person, k: Key, v: [u8; 40]) -> Result<(), Error> {
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(dev, &b.keys, k, v)?;
        b.touched = true;
        Ok(())
    }

    pub(crate) fn del_rec(&mut self, p: Person, k: Key) -> Result<(), Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let gone = b.trees[Class::Apps as usize]
            .remove(dev, &b.keys, k)?
            .is_some();
        b.touched |= gone;
        Ok(())
    }
}

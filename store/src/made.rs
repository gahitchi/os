// SPDX-License-Identifier: GPL-3.0-only
//! Living validity, first version (`commitments.md`, milestone 6c): "is
//! this still up to date?"
//!
//! An Alaya app that makes a result from other documents tells the store
//! so: "chart.ppm was made from numbers.txt, version 8". The store keeps
//! that **made from note** with the result, in the person's documents,
//! so it has history like the file itself. Later anyone can ask for the
//! result's **mark**:
//!
//! - **Up to date**: every source is unchanged, and the app read all of
//!   each one.
//! - **Needs a look**: a source changed since.
//! - **As of a date**: the person said the result shows a fixed moment.
//! - **Not sure**: the store cannot vouch for it: the app did not read all
//!   of a source (or the source changed while it read), a source is gone,
//!   or the result itself was changed after it was made.
//!
//! The rule that matters most: nothing is marked "up to date" unless the
//! store saw the app read every byte of the very version it names. The
//! app only names the sources; the store checks them against what it
//! served. A version alone is not enough to tell contents apart (several
//! saves can land in one commit), so each document also counts its
//! changes, and a source is "unchanged" only if both are the same.
//!
//! Renames don't matter: a note points at files, not names (decision 22).

use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{self, word, words};
use crate::{App, Class, Error, FileId, Person, Store, k_made};

/// The most sources one result may name.
pub const MADE_MAX: usize = 16;

/// How much of one version of a document an app has read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub version: u64,
    pub edits: u64,
    pub size: u64,
    /// Every byte before this was read, in this version.
    pub covered: u64,
}

impl Seen {
    pub(crate) fn first(v: (u64, u64, u64), off: u64, n: u64) -> Seen {
        let mut s = Seen {
            version: v.0,
            edits: v.1,
            size: v.2,
            covered: 0,
        };
        s.add(off, n);
        s
    }

    /// A read of `n` bytes at `off` of the version `v` (version, edits,
    /// size). A read of another version starts again from nothing.
    pub(crate) fn read(&mut self, v: (u64, u64, u64), off: u64, n: u64) {
        if (self.version, self.edits, self.size) != v {
            *self = Seen::first(v, off, n);
        } else {
            self.add(off, n);
        }
    }

    fn add(&mut self, off: u64, n: u64) {
        if off <= self.covered {
            self.covered = self.covered.max(off + n);
        }
    }

    /// All of it was read (an empty file counts once it was asked for).
    pub fn whole(&self) -> bool {
        self.covered >= self.size
    }
}

/// One source of a result, as the app names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Source {
    pub file: FileId,
    pub version: u64,
    pub edits: u64,
}

/// What the mark says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Not made from other files (or nobody said so): a source, no mark.
    NoMark = 0,
    UpToDate = 1,
    NeedsALook = 2,
    AsOf = 3,
    NotSure = 4,
}

/// Why a mark is "Not sure" (or which change made it "Needs a look").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Nothing = 0,
    /// A source changed since (Needs a look).
    SourceChanged = 1,
    /// The app did not read all of a source, or it changed while read.
    NotAllRead = 2,
    /// A source was deleted.
    SourceGone = 3,
    /// The result was changed after it was made.
    ResultChanged = 4,
}

/// A result's mark, with the source it is about (if any).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub kind: Kind,
    pub why: Why,
    /// The source the mark is about, and its version now (0 if gone).
    pub source: Option<FileId>,
    pub now_version: u64,
    /// The app that made the result.
    pub maker: App,
    /// How many sources it was made from.
    pub sources: u64,
}

const AS_OF: u64 = 1;
const WHOLE: u64 = 1;

impl<D: Disk> Store<D> {
    /// `app` says it made `result` from `sources`. Only the app that last
    /// changed the result may say so. The store notes, for each source,
    /// whether it saw this app read all of that version.
    pub fn made_from(
        &mut self,
        p: Person,
        app: App,
        result: FileId,
        sources: &[Source],
    ) -> Result<(), Error> {
        self.ok()?;
        if result.class() != Class::Documents
            || sources.is_empty()
            || sources.len() > MADE_MAX
            || sources
                .iter()
                .any(|s| s.file.class() != Class::Documents || s.file == result)
        {
            return Err(Error::BadRequest);
        }
        let st = self.stat(p, result)?;
        if st.is_dir {
            return Err(Error::IsADirectory);
        }
        let b = self.branch(p)?;
        if b.writer.get(&result) != Some(&app) {
            return Err(Error::BadRequest);
        }
        let whole: Vec<bool> = sources
            .iter()
            .map(|s| {
                b.seen
                    .get(&(app, s.file))
                    .is_some_and(|x| x.version == s.version && x.edits == s.edits && x.whole())
            })
            .collect();
        self.room(4 + sources.len() as u64 / 8, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[Class::Documents as usize];
        let obj = result.obj();
        for (k, _) in t.collect(dev, &b.keys, k_made(obj, 0), k_made(obj, u64::MAX))? {
            t.remove(dev, &b.keys, k)?;
        }
        t.insert(
            dev,
            &b.keys,
            k_made(obj, 0),
            words(&[st.version, st.edits, app, sources.len() as u64, 0]),
        )?;
        for (i, (s, w)) in sources.iter().zip(&whole).enumerate() {
            t.insert(
                dev,
                &b.keys,
                k_made(obj, 1 + i as u64),
                words(&[s.file.0, s.version, s.edits, if *w { WHOLE } else { 0 }]),
            )?;
        }
        *b.changes.entry((app, result)).or_insert(0) |= crate::what::MADE;
        b.touched = true;
        b.docs_changed = true;
        Ok(())
    }

    /// The person says `result` shows a fixed moment (or not any more).
    pub fn set_as_of(
        &mut self,
        p: Person,
        app: App,
        result: FileId,
        on: bool,
    ) -> Result<(), Error> {
        self.ok()?;
        if result.class() != Class::Documents {
            return Err(Error::BadRequest);
        }
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[Class::Documents as usize];
        let mut h = t
            .get(dev, &b.keys, k_made(result.obj(), 0))?
            .ok_or(Error::NotFound)?;
        let flags = word(&h, 4);
        let flags = if on { flags | AS_OF } else { flags & !AS_OF };
        format::set_word(&mut h, 4, flags);
        t.insert(dev, &b.keys, k_made(result.obj(), 0), h)?;
        *b.changes.entry((app, result)).or_insert(0) |= crate::what::MADE;
        b.touched = true;
        b.docs_changed = true;
        Ok(())
    }

    /// What a result was made from, as noted.
    pub fn sources(&mut self, p: Person, result: FileId) -> Result<Vec<(Source, bool)>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &b.trees[Class::Documents as usize];
        let obj = result.obj();
        Ok(
            t.collect(dev, &b.keys, k_made(obj, 1), k_made(obj, u64::MAX))?
                .iter()
                .map(|(_, v)| {
                    (
                        Source {
                            file: FileId(word(v, 0)),
                            version: word(v, 1),
                            edits: word(v, 2),
                        },
                        word(v, 3) & WHOLE != 0,
                    )
                })
                .collect(),
        )
    }

    /// Is `result` still up to date?
    pub fn mark(&mut self, p: Person, result: FileId) -> Result<Mark, Error> {
        let st = self.stat(p, result)?;
        let mut m = Mark {
            kind: Kind::NoMark,
            why: Why::Nothing,
            source: None,
            now_version: 0,
            maker: 0,
            sources: 0,
        };
        if result.class() != Class::Documents || st.is_dir {
            return Ok(m);
        }
        let h = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            b.trees[Class::Documents as usize].get(dev, &b.keys, k_made(result.obj(), 0))?
        };
        let Some(h) = h else {
            return Ok(m);
        };
        m.maker = word(&h, 2);
        m.sources = word(&h, 3);
        if word(&h, 4) & AS_OF != 0 {
            m.kind = Kind::AsOf;
            return Ok(m);
        }
        // Gather everything first: a changed source says more than a doubt.
        let mut changed = None;
        let mut doubt = None;
        if (st.version, st.edits) != (word(&h, 0), word(&h, 1)) {
            doubt = Some((Why::ResultChanged, None, 0));
        }
        for (s, whole) in self.sources(p, result)? {
            match self.stat(p, s.file) {
                Ok(now) if (now.version, now.edits) != (s.version, s.edits) => {
                    changed.get_or_insert((s.file, now.version));
                }
                Ok(now) if !whole => {
                    doubt.get_or_insert((Why::NotAllRead, Some(s.file), now.version));
                }
                Ok(_) => {}
                Err(Error::NotFound) => {
                    doubt.get_or_insert((Why::SourceGone, Some(s.file), 0));
                }
                Err(e) => return Err(e),
            }
        }
        if let Some((f, v)) = changed {
            m.kind = Kind::NeedsALook;
            m.why = Why::SourceChanged;
            m.source = Some(f);
            m.now_version = v;
        } else if let Some((why, f, v)) = doubt {
            m.kind = Kind::NotSure;
            m.why = why;
            m.source = f;
            m.now_version = v;
        } else {
            m.kind = Kind::UpToDate;
        }
        Ok(m)
    }
}

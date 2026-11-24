// SPDX-License-Identifier: GPL-3.0-only
//! Grants with time (milestone 7, step 2): an app sees, of a person's
//! documents, only what the person handed it, and what it made itself.
//!
//! Each hand-over says how long it lasts:
//!
//! - **live**: the file as it changes, for good (until taken back);
//! - **pinned**: one exact version, read only;
//! - **until a time**: live until then;
//! - **until the app closes**: live until the app's last window closes.
//!
//! A grant reads only, or reads and writes. A file an app creates is its
//! own: a live grant that reads and writes, marked as its own work.
//!
//! Grants are kept with the person's app files (no history), so they
//! survive a restart, and they are locked with the person's key like
//! everything else. Which apps need grants is the store server's choice:
//! Alaya's own carriers (the desktop, the file browser, "What happened?")
//! see everything of the person they work for.

use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::Key;
use crate::format::{word, words};
use crate::{App, Class, Error, FileId, Person, Store};

/// How long a grant lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lasts {
    Live,
    /// One version: the commit that made it.
    Pinned(u64),
    /// Live until this time (seconds, the store's clock).
    Until(u64),
    /// Live until the app closes.
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
}

/// One hand-over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub app: App,
    pub file: FileId,
    pub write: bool,
    pub lasts: Lasts,
    /// The app made the file itself.
    pub own: bool,
}

/// What an app may do with a file now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    No,
    /// The current version; writes too when true.
    Current(bool),
    /// Only this version, read only.
    Pinned(u64),
}

const K_GRANT: u64 = 4;
/// A document marked private (decision 27): keyed by the document.
const K_PRIVATE: u64 = 8;
/// An app that read a private document: keyed by the app.
const K_MARKED: u64 = 9;
const WRITE: u64 = 1;
const OWN: u64 = 2;

/// Records the store keeps in a person's app-files tree, after every
/// file's keys (which start with the file's number).
pub(crate) const RECORDS: u64 = 0xff << 56;

/// Grants live in the person's app-files tree, keyed by app then file.
fn k_grant(app: App, obj: u64) -> Key {
    Key(RECORDS | app << 8 | K_GRANT, obj)
}

fn k_private(obj: u64) -> Key {
    Key(RECORDS | K_PRIVATE, obj)
}

fn k_marked(app: App) -> Key {
    Key(RECORDS | K_MARKED, app)
}

/// An app that read a private document, and needs the person's OK
/// before it may go online (decision 27).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marked {
    pub app: App,
    /// The first private document it read, and when.
    pub file: FileId,
    pub since: u64,
}

fn decode(app: App, v: &[u8]) -> Option<Grant> {
    let flags = word(v, 2);
    Some(Grant {
        app,
        file: FileId(word(v, 3)),
        write: flags & WRITE != 0,
        lasts: Lasts::from_code(word(v, 0), word(v, 1))?,
        own: flags & OWN != 0,
    })
}

impl<D: Disk> Store<D> {
    /// Hand `file` to `app`. A new grant for the same file replaces the
    /// old one, except that an app's own file stays its own.
    pub fn grant(
        &mut self,
        p: Person,
        app: App,
        file: FileId,
        write: bool,
        lasts: Lasts,
    ) -> Result<(), Error> {
        self.ok()?;
        if file.class() != Class::Documents || app == 0 || app >= 1 << 48 {
            return Err(Error::BadRequest);
        }
        if let Lasts::Pinned(cn) = lasts {
            // The version must be one the person's history keeps.
            let v = self.past(p, Class::Documents, cn)?;
            self.past_stat(&v, file)?;
            if write {
                return Err(Error::BadRequest);
            }
        } else {
            self.stat(p, file)?;
        }
        let own = self
            .grant_of(p, app, file)?
            .is_some_and(|g| g.own && g.lasts == Lasts::Live);
        if own {
            return Ok(());
        }
        self.put_grant(p, app, file, write, lasts, false)
    }

    /// An app made `file`: it is the app's own.
    pub fn own_file(&mut self, p: Person, app: App, file: FileId) -> Result<(), Error> {
        self.task_made(p, app, file)?;
        self.put_grant(p, app, file, true, Lasts::Live, true)
    }

    fn put_grant(
        &mut self,
        p: Person,
        app: App,
        file: FileId,
        write: bool,
        lasts: Lasts,
        own: bool,
    ) -> Result<(), Error> {
        self.room(2, false)?;
        let (kind, arg) = lasts.code();
        let flags = if write { WRITE } else { 0 } | if own { OWN } else { 0 };
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(
            dev,
            &b.keys,
            k_grant(app, file.obj()),
            words(&[kind, arg, flags, file.0]),
        )?;
        b.touched = true;
        Ok(())
    }

    /// Take a file back from an app (its own files too).
    pub fn revoke(&mut self, p: Person, app: App, file: FileId) -> Result<bool, Error> {
        self.ok()?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let gone = b.trees[Class::Apps as usize]
            .remove(dev, &b.keys, k_grant(app, file.obj()))?
            .is_some();
        b.touched |= gone;
        Ok(gone)
    }

    /// The app closed: what was handed to it until then is taken back,
    /// and its private mark ends (it starts again clean).
    pub fn app_closed(&mut self, p: Person, app: App) -> Result<u64, Error> {
        self.unmark(p, app)?;
        let mut n = 0;
        for g in self.grants(p, Some(app))? {
            if g.lasts == Lasts::UntilClosed && self.revoke(p, app, g.file)? {
                n += 1;
            }
        }
        Ok(n)
    }

    /// A file was forgotten or removed for good: no app keeps it.
    pub fn revoke_everywhere(&mut self, p: Person, file: FileId) -> Result<(), Error> {
        for g in self.grants(p, None)? {
            if g.file == file {
                self.revoke(p, g.app, file)?;
            }
        }
        Ok(())
    }

    /// The grants of one app, or of every app.
    pub fn grants(&mut self, p: Person, app: Option<App>) -> Result<Vec<Grant>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &b.trees[Class::Apps as usize];
        let (from, to) = match app {
            Some(a) => (k_grant(a, 0), k_grant(a, u64::MAX)),
            None => (Key(RECORDS, 0), Key(u64::MAX, u64::MAX)),
        };
        Ok(t.collect(dev, &b.keys, from, to)?
            .into_iter()
            .filter(|(k, _)| k.0 & 0xff == K_GRANT)
            .filter_map(|(k, v)| decode((k.0 & !RECORDS) >> 8, &v))
            .collect())
    }

    fn grant_of(&mut self, p: Person, app: App, file: FileId) -> Result<Option<Grant>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.trees[Class::Apps as usize]
            .get(dev, &b.keys, k_grant(app, file.obj()))?
            .and_then(|v| decode(app, &v)))
    }

    /// Mark a document private, or not (the person, in the file browser).
    pub fn set_private(&mut self, p: Person, file: FileId, on: bool) -> Result<(), Error> {
        self.ok()?;
        if file.class() != Class::Documents {
            return Err(Error::BadRequest);
        }
        self.stat(p, file)?;
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[Class::Apps as usize];
        if on {
            t.insert(dev, &b.keys, k_private(file.obj()), words(&[1]))?;
        } else {
            t.remove(dev, &b.keys, k_private(file.obj()))?;
        }
        b.touched = true;
        Ok(())
    }

    pub fn is_private(&mut self, p: Person, file: FileId) -> Result<bool, Error> {
        if file.class() != Class::Documents {
            return Ok(false);
        }
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.trees[Class::Apps as usize]
            .get(dev, &b.keys, k_private(file.obj()))?
            .is_some())
    }

    /// `app` read `file`: if it is private, the app is marked (the first
    /// time only). Returns whether the app is marked now. Alaya's trusted
    /// carriers (the desktop, the store, the clipboard) are never marked
    /// (rule B1): the store server calls this for apps only.
    pub fn note_private_read(
        &mut self,
        p: Person,
        app: App,
        file: FileId,
        now: u64,
    ) -> Result<bool, Error> {
        if !self.is_private(p, file)? {
            return Ok(self.marked(p, app)?.is_some());
        }
        if self.marked(p, app)?.is_some() {
            return Ok(true);
        }
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(dev, &b.keys, k_marked(app), words(&[file.0, now]))?;
        b.touched = true;
        Ok(true)
    }

    /// Whether `app` is marked, and by what.
    pub fn marked(&mut self, p: Person, app: App) -> Result<Option<Marked>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.trees[Class::Apps as usize]
            .get(dev, &b.keys, k_marked(app))?
            .map(|v| Marked {
                app,
                file: FileId(word(&v, 0)),
                since: word(&v, 1),
            }))
    }

    /// Every marked app.
    pub fn marks(&mut self, p: Person) -> Result<Vec<Marked>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.trees[Class::Apps as usize]
            .collect(dev, &b.keys, k_marked(0), k_marked(u64::MAX >> 8))?
            .into_iter()
            .filter(|(k, _)| k.0 == RECORDS | K_MARKED)
            .map(|(k, v)| Marked {
                app: k.1,
                file: FileId(word(&v, 0)),
                since: word(&v, 1),
            })
            .collect())
    }

    /// The mark spreads to an app that takes data from a marked one
    /// (decision 27): `to` is marked by what marked `from`.
    pub fn spread_mark(&mut self, p: Person, from: App, to: App, now: u64) -> Result<bool, Error> {
        let Some(m) = self.marked(p, from)? else {
            return Ok(false);
        };
        if self.marked(p, to)?.is_some() {
            return Ok(true);
        }
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(
            dev,
            &b.keys,
            k_marked(to),
            words(&[m.file.0, now]),
        )?;
        b.touched = true;
        Ok(true)
    }

    fn unmark(&mut self, p: Person, app: App) -> Result<(), Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        if b.trees[Class::Apps as usize]
            .remove(dev, &b.keys, k_marked(app))?
            .is_some()
        {
            b.touched = true;
        }
        Ok(())
    }

    /// What `app` may do with `file` at time `now`.
    pub fn access(&mut self, p: Person, app: App, file: FileId, now: u64) -> Result<Access, Error> {
        // An app opened in a protected task sees the task's files, and
        // nothing else, whatever it was handed before (decision 85).
        if let Some(inside) = self.task_access(p, app, file)? {
            return Ok(if inside {
                Access::Current(true)
            } else {
                Access::No
            });
        }
        let Some(g) = self.grant_of(p, app, file)? else {
            return Ok(Access::No);
        };
        Ok(match g.lasts {
            Lasts::Pinned(cn) => Access::Pinned(cn),
            Lasts::Until(t) if now >= t => Access::No,
            _ => Access::Current(g.write),
        })
    }
}

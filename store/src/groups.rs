// SPDX-License-Identifier: GPL-3.0-only
//! Groups and protected tasks (milestone 7a, step 1; `continuity.md`
//! section 4, decision 85).
//!
//! - A **loose group** is only a label for things that belong together.
//!   A file can be in many. It never changes what an app can see.
//! - A **protected task** is a group the person confirmed as a task. An
//!   app opened in it sees the task's files (and what is inside its
//!   folders), and nothing else of the person's, even files it was handed
//!   before ([`Store::task_access`], asked by [`Store::access`]). What the
//!   app makes while in the task joins the task.
//! - A **paused** task keeps its apps frozen, not closed (rule B3): the
//!   desktop stops them; the store only remembers it.
//!
//! Groups live with the person's app files (no history), so they survive
//! a restart; the desktop puts its own changes on the undo list.

use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{Key, word, words};
use crate::grants::RECORDS;
use crate::{App, Class, Error, FileId, Person, Store};

/// A group's name, at most this long (bytes).
pub const GROUP_NAME_MAX: usize = 16;
/// At most this many groups per person.
pub const GROUPS_MAX: usize = 16;
/// At most this many files (or folders) in one group.
pub const GROUP_FILES_MAX: usize = 128;

const K_GROUP: u64 = 12;
const K_GROUP_FILE: u64 = 13;
const K_IN_TASK: u64 = 14;

const TASK: u64 = 1;
const PAUSED: u64 = 2;

fn k_group(id: u32) -> Key {
    Key(RECORDS | K_GROUP, u64::from(id))
}

fn k_group_file(id: u32, obj: u64) -> Key {
    Key(RECORDS | K_GROUP_FILE, u64::from(id) << 48 | obj)
}

fn k_in_task(app: App) -> Key {
    Key(RECORDS | K_IN_TASK, app)
}

/// One group, or task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    /// From 1.
    pub id: u32,
    pub name: Vec<u8>,
    /// 0xRRGGBB.
    pub color: u32,
    /// A protected task (the person confirmed it).
    pub task: bool,
    pub paused: bool,
    pub made: u64,
    /// When the person last left it (0: not yet).
    pub left: u64,
}

fn encode(g: &Group) -> [u8; 40] {
    let flags = if g.task { TASK } else { 0 } | if g.paused { PAUSED } else { 0 };
    let mut v = words(&[
        flags | u64::from(g.color) << 8 | (g.name.len() as u64) << 40,
        g.made,
        g.left,
    ]);
    v[24..24 + g.name.len()].copy_from_slice(&g.name);
    v
}

fn decode(id: u32, v: &[u8]) -> Group {
    let w = word(v, 0);
    let n = ((w >> 40) as usize).min(GROUP_NAME_MAX);
    Group {
        id,
        name: v[24..24 + n].to_vec(),
        color: (w >> 8) as u32 & 0xff_ffff,
        task: w & TASK != 0,
        paused: w & PAUSED != 0,
        made: word(v, 1),
        left: word(v, 2),
    }
}

impl<D: Disk> Store<D> {
    fn group_rec(&mut self, p: Person, k: Key) -> Result<Option<[u8; 40]>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].get(dev, &b.keys, k)
    }

    fn group_put(&mut self, p: Person, k: Key, v: [u8; 40]) -> Result<(), Error> {
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(dev, &b.keys, k, v)?;
        b.touched = true;
        Ok(())
    }

    fn group_del(&mut self, p: Person, k: Key) -> Result<bool, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let gone = b.trees[Class::Apps as usize]
            .remove(dev, &b.keys, k)?
            .is_some();
        b.touched |= gone;
        Ok(gone)
    }

    fn group_range(
        &mut self,
        p: Person,
        from: Key,
        to: Key,
    ) -> Result<Vec<(Key, [u8; 40])>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].collect(dev, &b.keys, from, to)
    }

    /// Person `p`'s groups and tasks, by number.
    pub fn groups(&mut self, p: Person) -> Result<Vec<Group>, Error> {
        Ok(self
            .group_range(p, k_group(0), k_group(u32::MAX))?
            .into_iter()
            .filter(|(k, _)| k.0 == RECORDS | K_GROUP)
            .map(|(k, v)| decode(k.1 as u32, &v))
            .collect())
    }

    pub fn group(&mut self, p: Person, id: u32) -> Result<Group, Error> {
        self.group_rec(p, k_group(id))?
            .map(|v| decode(id, &v))
            .ok_or(Error::NotFound)
    }

    /// Make a new loose group. Returns its number.
    pub fn group_new(
        &mut self,
        p: Person,
        name: &[u8],
        color: u32,
        now: u64,
    ) -> Result<u32, Error> {
        self.ok()?;
        if name.is_empty() || name.len() > GROUP_NAME_MAX {
            return Err(Error::BadRequest);
        }
        let all = self.groups(p)?;
        if all.len() >= GROUPS_MAX {
            return Err(Error::Full);
        }
        if all.iter().any(|g| g.name == name) {
            return Err(Error::Exists);
        }
        let id = (1..).find(|i| all.iter().all(|g| g.id != *i)).unwrap_or(1);
        let g = Group {
            id,
            name: name.to_vec(),
            color: color & 0xff_ffff,
            task: false,
            paused: false,
            made: now,
            left: 0,
        };
        self.group_put(p, k_group(id), encode(&g))?;
        Ok(id)
    }

    /// Change a group's name, colour, task and pause flags, or when it
    /// was left. A group that stops being a task lets its apps go.
    pub fn group_set(&mut self, p: Person, g: &Group) -> Result<(), Error> {
        self.ok()?;
        if g.name.is_empty() || g.name.len() > GROUP_NAME_MAX || (g.paused && !g.task) {
            return Err(Error::BadRequest);
        }
        let was = self.group(p, g.id)?;
        if self
            .groups(p)?
            .iter()
            .any(|o| o.id != g.id && o.name == g.name)
        {
            return Err(Error::Exists);
        }
        if was.task && !g.task {
            self.task_let_go(p, g.id)?;
        }
        let mut g = g.clone();
        g.made = was.made;
        g.color &= 0xff_ffff;
        self.group_put(p, k_group(g.id), encode(&g))
    }

    /// Remove a group: its files stay where they are; only the label goes.
    pub fn group_remove(&mut self, p: Person, id: u32) -> Result<(), Error> {
        self.ok()?;
        self.group(p, id)?;
        self.task_let_go(p, id)?;
        for f in self.group_files(p, id)? {
            self.group_del(p, k_group_file(id, f.obj()))?;
        }
        self.group_del(p, k_group(id))?;
        self.return_point_forget(p, id)?;
        Ok(())
    }

    /// Put a document (or folder) in a group. True when it was not in it.
    pub fn group_add(&mut self, p: Person, id: u32, f: FileId) -> Result<bool, Error> {
        self.ok()?;
        if f.class() != Class::Documents {
            return Err(Error::BadRequest);
        }
        self.group(p, id)?;
        self.stat(p, f)?;
        if self.group_rec(p, k_group_file(id, f.obj()))?.is_some() {
            return Ok(false);
        }
        if self.group_files(p, id)?.len() >= GROUP_FILES_MAX {
            return Err(Error::Full);
        }
        self.group_put(p, k_group_file(id, f.obj()), words(&[f.0]))?;
        Ok(true)
    }

    /// Take a file out of a group. True when it was in it.
    pub fn group_take(&mut self, p: Person, id: u32, f: FileId) -> Result<bool, Error> {
        self.ok()?;
        self.group_del(p, k_group_file(id, f.obj()))
    }

    /// What is in a group.
    pub fn group_files(&mut self, p: Person, id: u32) -> Result<Vec<FileId>, Error> {
        Ok(self
            .group_range(p, k_group_file(id, 0), k_group_file(id, (1 << 48) - 1))?
            .into_iter()
            .filter(|(k, _)| k.0 == RECORDS | K_GROUP_FILE)
            .map(|(_, v)| FileId(word(&v, 0)))
            .collect())
    }

    /// The groups a file is in (directly).
    pub fn groups_of(&mut self, p: Person, f: FileId) -> Result<Vec<u32>, Error> {
        let mut out = Vec::new();
        for g in self.groups(p)? {
            if self.group_rec(p, k_group_file(g.id, f.obj()))?.is_some() {
                out.push(g.id);
            }
        }
        Ok(out)
    }

    /// Open `app` in task `task` (or in no task, with `None`). Only a
    /// protected task holds apps.
    pub fn open_in_task(&mut self, p: Person, app: App, task: Option<u32>) -> Result<(), Error> {
        self.ok()?;
        if app == 0 || app >= 1 << 48 {
            return Err(Error::BadRequest);
        }
        match task {
            Some(t) => {
                if !self.group(p, t)?.task {
                    return Err(Error::BadRequest);
                }
                self.group_put(p, k_in_task(app), words(&[u64::from(t)]))
            }
            None => self.group_del(p, k_in_task(app)).map(|_| ()),
        }
    }

    /// The task `app` was opened in, if any.
    pub fn task_of(&mut self, p: Person, app: App) -> Result<Option<u32>, Error> {
        Ok(self
            .group_rec(p, k_in_task(app))?
            .map(|v| word(&v, 0) as u32))
    }

    /// The apps opened in a task.
    pub fn task_apps(&mut self, p: Person, task: u32) -> Result<Vec<App>, Error> {
        Ok(self
            .group_range(p, k_in_task(0), k_in_task(u64::MAX))?
            .into_iter()
            .filter(|(k, v)| k.0 == RECORDS | K_IN_TASK && word(v, 0) as u32 == task)
            .map(|(k, _)| k.1)
            .collect())
    }

    fn task_let_go(&mut self, p: Person, task: u32) -> Result<(), Error> {
        for a in self.task_apps(p, task)? {
            self.group_del(p, k_in_task(a))?;
        }
        Ok(())
    }

    /// The wall of a task: `None` when `app` is in no task (grants decide);
    /// otherwise whether `f` is one of the task's files or inside one of
    /// its folders.
    pub fn task_access(&mut self, p: Person, app: App, f: FileId) -> Result<Option<bool>, Error> {
        let Some(t) = self.task_of(p, app)? else {
            return Ok(None);
        };
        if f.class() != Class::Documents {
            return Ok(Some(true));
        }
        for m in self.group_files(p, t)? {
            if m == f || (self.stat(p, m).is_ok_and(|s| s.is_dir) && self.inside(p, f, m)?) {
                return Ok(Some(true));
            }
        }
        Ok(Some(false))
    }

    /// `app` made `f`: when it is in a task, the file joins the task, so
    /// the app keeps seeing what it made.
    pub(crate) fn task_made(&mut self, p: Person, app: App, f: FileId) -> Result<(), Error> {
        if let Some(t) = self.task_of(p, app)?
            && self.task_access(p, app, f)? == Some(false)
        {
            self.group_put(p, k_group_file(t, f.obj()), words(&[f.0]))?;
        }
        Ok(())
    }
}

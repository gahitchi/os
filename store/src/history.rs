// SPDX-License-Identifier: GPL-3.0-only
//! Going back (milestone 7, step 1): putting files back as they were,
//! the one-undo list, named versions, "What happened?", file stories and
//! forgetting a file for good.
//!
//! - **Putting back never deletes history.** A file is put back by a new
//!   change that copies the old contents, name and place, so the putting
//!   back can itself be undone.
//! - **The one-undo list** (decisions 109 and 110) holds the person's
//!   actions: an app that does something the person asked for (move a
//!   file, change a setting, close a window) records it, and the changes
//!   that app made in that commit become one action. Undo puts those
//!   files back as they were just before. If something else changed one
//!   of them since, nothing is done and the store says which (rule B6).
//! - **Named versions** (decision 58) are moments thinning never removes.
//! - **Forget** (decision 62) removes a document and destroys its key.
//!   Everything left of it (old versions, its name in old folders) was
//!   scrambled with that key, and the blocks that held the key are wiped
//!   when they are freed, two commits later.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::disk::Disk;
use crate::format::{self, BLOCK, Key, Ptr, Val, word, words};
use crate::side::{self, FileState};
use crate::tree::Tree;
use crate::{
    App, Change, Class, Error, F_DIR, FileId, H_ACT, H_CHANGE, H_INFO, H_NAME, K_DATA, K_MADE,
    Moment, NAME_MAX, Person, ROOT_OBJ, Store, View, k_data, k_fkey, k_hist, k_pin, what,
};

/// Kinds of action on the one-undo list.
pub mod act {
    /// Files were changed (moved, renamed, deleted, changed).
    pub const FILES: u64 = 1;
    /// A setting was changed (settings live in documents).
    pub const SETTINGS: u64 = 2;
    /// A window was closed (the desktop opens it again).
    pub const WINDOW: u64 = 3;
    /// A window was moved into or out of a loose group.
    pub const GROUP: u64 = 4;
    /// An undo (undoing it is a redo).
    pub const UNDO: u64 = 5;
}

/// Bytes of an action's label.
pub const LABEL: usize = 16;

/// One action on the one-undo list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Action {
    pub no: u64,
    /// The commit its changes are in.
    pub cn: u64,
    pub app: App,
    pub kind: u64,
    pub undone: bool,
    /// For an undo: the action it undid.
    pub undoes: u64,
    /// What the app said it was ("moved notes.txt"); for desktop kinds,
    /// what the desktop needs to put it back.
    pub label: [u8; LABEL],
}

impl Action {
    fn decode(no: u64, v: &Val) -> Action {
        let w = word(v, 2);
        Action {
            no,
            cn: word(v, 0),
            app: word(v, 1),
            kind: w & 0xff,
            undone: w >> 8 & 1 != 0,
            undoes: w >> 16,
            label: v[24..40].try_into().unwrap(),
        }
    }

    fn encode(&self) -> Val {
        let mut v = words(&[
            self.cn,
            self.app,
            self.kind | (self.undone as u64) << 8 | self.undoes << 16,
        ]);
        v[24..40].copy_from_slice(&self.label);
        v
    }
}

fn k_act(no: u64) -> Key {
    Key(H_ACT | no, 0)
}

/// What putting files back did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restored {
    /// These files are back as they were (in the next commit).
    Done(Vec<FileId>),
    /// Nothing was done: these files cannot go back (their old folder is
    /// gone, another file has their name, or a folder to remove has new
    /// files in it).
    Blocked(Vec<FileId>),
}

/// What an undo did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    /// Done: the action is marked undone and these files were put back.
    Done { action: Action, files: Vec<FileId> },
    /// Nothing was done: these files changed since the action (or cannot
    /// go back to their old place). Ask, then undo again with `force`.
    Conflicts(Vec<FileId>),
}

/// One commit with changes, for "What happened?".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Happening {
    pub cn: u64,
    pub time: u64,
    pub changes: Vec<Change>,
}

/// A file's story (right-click, "Where did this come from?").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Story {
    /// The app that made it and when (commit, time), if history still
    /// has it.
    pub made_by: Option<(App, u64, u64)>,
    /// Every recorded change, newest first: (commit, time, app, what).
    pub changes: Vec<(u64, u64, App, u64)>,
    /// Documents whose "made from" note names this file.
    pub used_by: Vec<FileId>,
}

impl<D: Disk> Store<D> {
    // ---- Named versions ----

    /// Name moment `cn` (decision 58: "name this version"). Named moments
    /// are never thinned out.
    pub fn name_version(&mut self, p: Person, cn: u64, name: &[u8]) -> Result<(), Error> {
        self.ok()?;
        if name.is_empty() || name.len() > NAME_MAX || side::is_side(p) {
            return Err(Error::BadName);
        }
        if !self.history(p)?.iter().any(|m| m.cn == cn) {
            return Err(Error::NoSuchMoment);
        }
        self.room(4, false)?;
        self.pin(p, cn, 0, Some(true))?;
        let mut v = [0u8; 40];
        v[..name.len()].copy_from_slice(name);
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.hist.insert(dev, &b.keys, k_hist(cn, H_NAME, 0), v)?;
        b.touched = true;
        Ok(())
    }

    /// Take a version's name away (it may be thinned again).
    pub fn unname_version(&mut self, p: Person, cn: u64) -> Result<(), Error> {
        self.ok()?;
        self.pin(p, cn, 0, Some(false))?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.hist.remove(dev, &b.keys, k_hist(cn, H_NAME, 0))?;
        b.touched = true;
        Ok(())
    }

    /// The named versions, oldest first.
    pub fn named_versions(&mut self, p: Person) -> Result<Vec<(Moment, Vec<u8>)>, Error> {
        let moments = self.history(p)?;
        let named: Vec<u64> = self
            .sys
            .collect(
                &mut self.dev,
                &self.machine,
                k_pin(p, 0),
                k_pin(p, crate::GEN_MAX),
            )?
            .iter()
            .filter(|(_, v)| word(v, 1) != 0)
            .map(|(k, _)| crate::snap_gen(*k))
            .collect();
        let mut out = Vec::new();
        for cn in named {
            let Some(m) = moments.iter().find(|m| m.cn == cn) else {
                continue;
            };
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            if let Some(v) = b.hist.get(dev, &b.keys, k_hist(cn, H_NAME, 0))? {
                let n = v.iter().position(|&c| c == 0).unwrap_or(NAME_MAX);
                out.push((*m, v[..n].to_vec()));
            }
        }
        Ok(out)
    }

    // ---- Forget ----

    /// Forget a document for good (decision 62): it is removed and its key
    /// destroyed, so no old version of it, nor its name in old folders,
    /// can be read again by anyone. A folder is forgotten with everything
    /// in it. The desktop waits the hour of decision 62 before asking.
    pub fn forget(&mut self, p: Person, app: App, dir: FileId, name: &[u8]) -> Result<(), Error> {
        self.ok()?;
        if dir.class() != Class::Documents || side::is_side(p) {
            return Err(Error::BadRequest);
        }
        let f = self.lookup(p, dir, name)?;
        if self.stat(p, f)?.is_dir {
            for (n, _) in self.list(p, f)? {
                self.forget(p, app, f, &n)?;
            }
        }
        self.remove(p, app, dir, name)?;
        self.destroy_key(p, app, f)
    }

    /// Forget a document after a grace time (decision 62): it leaves its
    /// folder at once, and at `at` (seconds) its key is destroyed, unless
    /// it was brought back meanwhile ([`Store::keep_forgotten`], or the
    /// one undo). Returns the file.
    pub fn forget_later(
        &mut self,
        p: Person,
        app: App,
        dir: FileId,
        name: &[u8],
        at: u64,
    ) -> Result<FileId, Error> {
        self.ok()?;
        if dir.class() != Class::Documents || side::is_side(p) {
            return Err(Error::BadRequest);
        }
        let f = self.lookup(p, dir, name)?;
        if self.stat(p, f)?.is_dir {
            return Err(Error::IsADirectory);
        }
        let cn = self.cn;
        self.remove(p, app, dir, name)?;
        self.room(2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(
            dev,
            &b.keys,
            k_forget(f.obj()),
            words(&[at, dir.0, app, cn, f.0]),
        )?;
        b.touched = true;
        Ok(f)
    }

    /// Documents waiting to be forgotten: (file, when, the commit before).
    pub fn forgetting(&mut self, p: Person) -> Result<Vec<(FileId, u64, u64)>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.trees[Class::Apps as usize]
            .collect(dev, &b.keys, k_forget(0), k_forget(u64::MAX))?
            .into_iter()
            .map(|(_, v)| (FileId(word(&v, 4)), word(&v, 0), word(&v, 3)))
            .collect())
    }

    /// Keep a document that was waiting to be forgotten: it goes back
    /// where it was, as it was.
    pub fn keep_forgotten(&mut self, p: Person, app: App, f: FileId) -> Result<Restored, Error> {
        let Some((_, _, cn)) = self.forgetting(p)?.into_iter().find(|x| x.0 == f) else {
            return Err(Error::NotFound);
        };
        let r = if self.name_of(p, f, None)?.is_some() {
            Restored::Done(alloc::vec![f])
        } else {
            self.restore(p, app, &[f], cn)?
        };
        if matches!(r, Restored::Done(_)) {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            b.trees[Class::Apps as usize].remove(dev, &b.keys, k_forget(f.obj()))?;
            b.touched = true;
        }
        Ok(r)
    }

    /// Forget for good what waited long enough and was not brought back.
    /// Returns how many were forgotten.
    pub fn forget_due(&mut self, p: Person, now: u64) -> Result<u64, Error> {
        let mut n = 0;
        for (f, at, _) in self.forgetting(p)? {
            if at > now {
                continue;
            }
            let app = {
                let Store { dev, branches, .. } = self;
                let b = branches.get_mut(&p).ok_or(Error::Locked)?;
                let v = b.trees[Class::Apps as usize].remove(dev, &b.keys, k_forget(f.obj()))?;
                b.touched = true;
                v.map_or(0, |v| word(&v, 2))
            };
            // Brought back meanwhile: nothing to forget.
            if self.name_of(p, f, None)?.is_some() {
                continue;
            }
            self.destroy_key(p, app, f)?;
            n += 1;
        }
        Ok(n)
    }

    fn destroy_key(&mut self, p: Person, app: App, f: FileId) -> Result<(), Error> {
        self.revoke_everywhere(p, f)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.ktab.remove(dev, &b.keys, k_fkey(f.obj()))?;
        b.fkeys.remove(&f.obj());
        b.writer.remove(&f);
        b.seen.retain(|(_, g), _| *g != f);
        *b.changes.entry((app, f)).or_insert(0) |= what::FORGOTTEN;
        b.touched = true;
        Ok(())
    }

    // ---- Putting back ----

    /// A file's state in a view (its records and its directory entry).
    fn state_in(
        &mut self,
        p: Person,
        view: View,
        f: FileId,
        hint: &[u64],
    ) -> Result<FileState, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let keys = b.keys.clone();
        match view {
            View::Live => side::file_state(&b.trees[0], dev, &keys, f.obj(), hint),
            View::At(root) => side::file_state(&Tree::open(root, false), dev, &keys, f.obj(), hint),
        }
    }

    /// Are two states of file `f` the same to the person: same name and
    /// place, same contents, same "made from" note? Version numbers and
    /// change counts are left out (putting a file back gives it a new
    /// version with the old contents), and blocks are compared by what is
    /// in them.
    fn same_state(
        &mut self,
        p: Person,
        f: FileId,
        a: &FileState,
        b: &FileState,
    ) -> Result<bool, Error> {
        if a.inode().is_some() != b.inode().is_some() || a.own.len() != b.own.len() {
            return Ok(false);
        }
        match (a.entry, b.entry) {
            (None, None) => {}
            (Some((ka, va)), Some((kb, vb))) => {
                if ka.0 != kb.0 {
                    return Ok(false);
                }
                if (ka, va) != (kb, vb)
                    && self.entry_name(p, Class::Documents, ka, &va)?
                        != self.entry_name(p, Class::Documents, kb, &vb)?
                {
                    return Ok(false);
                }
            }
            _ => return Ok(false),
        }
        let mut blocks = Vec::new();
        for ((ka, va), (kb, vb)) in a.own.iter().zip(&b.own) {
            if ka != kb {
                return Ok(false);
            }
            match ka.0 & 0xff {
                crate::K_INODE => {
                    let flags = (1u64 << crate::USED_SHIFT) - 1;
                    if word(va, 0) != word(vb, 0)
                        || word(va, 2) != word(vb, 2)
                        || word(va, 3) != word(vb, 3)
                        || word(va, 4) & flags != word(vb, 4) & flags
                    {
                        return Ok(false);
                    }
                }
                K_DATA if va != vb => blocks.push((Ptr::decode(va), Ptr::decode(vb))),
                _ if va != vb => return Ok(false),
                _ => {}
            }
        }
        if !blocks.is_empty() {
            let fk = self.file_keys(p, f.obj())?;
            let mut x = [0u8; BLOCK];
            let mut y = [0u8; BLOCK];
            for (pa, pb) in blocks {
                self.dev.read_block(&fk, &pa, &mut x)?;
                self.dev.read_block(&fk, &pb, &mut y)?;
                if x != y {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// The root of the documents at moment `cn`.
    fn moment_root(&mut self, p: Person, cn: u64) -> Result<Ptr, Error> {
        let v = self.past(p, Class::Documents, cn)?;
        Ok(v.root)
    }

    /// Put files back as they were at moment `cn`, as `app`. Checks
    /// everything first: if one file cannot go back (its old folder is
    /// gone, another file now has its name, or a folder to remove has new
    /// files in it), nothing is done and those files are returned.
    pub fn restore(
        &mut self,
        p: Person,
        app: App,
        files: &[FileId],
        cn: u64,
    ) -> Result<Restored, Error> {
        self.ok()?;
        if side::is_side(p) {
            return Err(Error::BadRequest);
        }
        let root = self.moment_root(p, cn)?;
        let mut plan: Vec<(FileId, FileState, FileState)> = Vec::new();
        for &f in files {
            if f.class() != Class::Documents || f.obj() == ROOT_OBJ {
                continue;
            }
            let old = self.state_in(p, View::At(root), f, &[])?;
            let hint: Vec<u64> = old.parent().into_iter().collect();
            let now = self.state_in(p, View::Live, f, &hint)?;
            if old.inode().is_some() && self.file_keys(p, f.obj()).is_err() {
                return Err(Error::Forgotten);
            }
            if old != now && !self.same_state(p, f, &old, &now)? {
                plan.push((f, old, now));
            }
        }
        // What each file's parent will be.
        let mut parent_of: BTreeMap<u64, Option<u64>> = BTreeMap::new();
        for (f, old, _) in &plan {
            parent_of.insert(f.obj(), old.parent());
        }
        let mut bad = BTreeSet::new();
        for (f, old, now) in &plan {
            match old.entry {
                Some((k, v)) => {
                    let dir = k.0 >> 8;
                    let dir_ok = match parent_of.get(&dir) {
                        Some(p) => p.is_some(),
                        None => self
                            .inode(p, FileId::new(dir, Class::Documents))
                            .is_ok_and(|v| word(&v, 4) & F_DIR != 0),
                    };
                    if !dir_ok {
                        bad.insert(*f);
                        continue;
                    }
                    let name = self
                        .entry_name(p, Class::Documents, k, &v)?
                        .ok_or(Error::Forgotten)?;
                    if let Some((_, ov)) =
                        self.find_named(p, View::Live, FileId::new(dir, Class::Documents), &name)?
                    {
                        let other = word(&ov, 0);
                        let moves = parent_of.get(&other).is_some_and(|_| other != f.obj());
                        if other != f.obj() && !moves {
                            bad.insert(*f);
                        }
                    }
                }
                None => {
                    // Made after the moment: it goes, but a folder only if
                    // everything in it goes or moves out too.
                    if now.is_dir() {
                        let kids = self.raw_entries(p, View::Live, *f, 0, u64::MAX)?;
                        if kids.iter().any(|(_, v)| {
                            parent_of
                                .get(&word(v, 0))
                                .is_none_or(|np| *np == Some(f.obj()))
                        }) {
                            bad.insert(*f);
                        }
                    }
                }
            }
        }
        if !bad.is_empty() {
            return Ok(Restored::Blocked(bad.into_iter().collect()));
        }
        self.room(plan.len() as u64 * 8 + 8, true)?;
        // Folders that come back go first (parents before children);
        // things that go away last (children first).
        let depth = |m: &BTreeMap<u64, Option<u64>>, mut o: u64| {
            let mut d = 0;
            while let Some(Some(up)) = m.get(&o) {
                o = *up;
                d += 1;
                if d > 4096 {
                    break;
                }
            }
            d
        };
        plan.sort_by_key(|(f, old, _)| {
            let d = depth(&parent_of, f.obj()) as i64;
            if old.inode().is_some() {
                d
            } else {
                1_000_000 - d
            }
        });
        let pg = self.pending_gen();
        let mut done = Vec::new();
        for (f, old, now) in plan {
            self.put_back(p, app, f, &old, &now, pg)?;
            done.push(f);
        }
        Ok(Restored::Done(done))
    }

    /// Make file `f` what `old` says (from the moment at `root`).
    #[allow(clippy::too_many_arguments)]
    fn put_back(
        &mut self,
        p: Person,
        app: App,
        f: FileId,
        old: &FileState,
        now: &FileState,
        pg: u64,
    ) -> Result<(), Error> {
        let obj = f.obj();
        // Blocks to copy back.
        let mut blocks = Vec::new();
        if old.inode().is_some() {
            let fk = self.file_keys(p, obj)?;
            for (k, v) in &old.own {
                if k.0 & 0xff == K_DATA && !now.own.iter().any(|(nk, nv)| nk == k && nv == v) {
                    let mut blk = alloc::boxed::Box::new([0u8; BLOCK]);
                    self.dev.read_block(&fk, &Ptr::decode(v), &mut blk)?;
                    blocks.push((k.1, blk));
                }
            }
        }
        let entry = match old.entry {
            Some((k, v)) => {
                let name = self
                    .entry_name(p, Class::Documents, k, &v)?
                    .ok_or(Error::Forgotten)?;
                let dir = FileId::new(k.0 >> 8, Class::Documents);
                let slot = if now.entry.is_some_and(|(nk, _)| nk == k) {
                    k
                } else {
                    let h = crate::name_hash(&name);
                    let used = self.raw_entries(p, View::Live, dir, h, h | 0xff)?;
                    let s = (0..256u64)
                        .find(|i| {
                            !used
                                .iter()
                                .any(|(uk, uv)| uk.1 == h | i && word(uv, 0) != obj)
                        })
                        .ok_or(Error::Full)?;
                    crate::k_dirent(dir.obj(), h | s)
                };
                Some((
                    slot,
                    self.seal_entry(p, Class::Documents, slot, obj, &name)?,
                ))
            }
            None => None,
        };
        let edits = now.inode().map_or(0, |v| word(v, 4) >> crate::USED_SHIFT);
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[0];
        for (k, v) in &now.own {
            if !old.own.iter().any(|(ok, _)| ok == k) {
                t.remove(dev, &b.keys, *k)?;
                if k.0 & 0xff == K_DATA {
                    t.released.push(Ptr::decode(v));
                }
            }
        }
        if let Some((k, _)) = now.entry
            && entry.is_none_or(|(nk, _)| nk != k)
        {
            t.remove(dev, &b.keys, k)?;
        }
        for (k, v) in &old.own {
            match k.0 & 0xff {
                K_DATA => {}
                crate::K_INODE => {
                    // A new version with the old contents: the version
                    // and the count of changes move on, so marks and
                    // other checks see that it changed.
                    let mut v = *v;
                    format::set_word(&mut v, 1, pg);
                    let flags = word(&v, 4) & ((1 << crate::USED_SHIFT) - 1);
                    format::set_word(&mut v, 4, flags | (edits + 1) << crate::USED_SHIFT);
                    t.insert(dev, &b.keys, *k, v)?;
                }
                _ => {
                    t.insert(dev, &b.keys, *k, *v)?;
                }
            }
        }
        if let Some((k, v)) = entry {
            t.insert(dev, &b.keys, k, v)?;
        }
        for (idx, blk) in blocks {
            if let Some(o) = t.remove(dev, &b.keys, k_data(obj, idx))? {
                t.released.push(Ptr::decode(&o));
            }
            b.dirty.insert((f, idx), blk);
        }
        b.dirty
            .retain(|(g, i), _| *g != f || old.own.iter().any(|(k, _)| *k == k_data(obj, *i)));
        let mut w = what::RESTORED;
        if old.inode().is_none() {
            w |= what::DELETED;
            b.writer.remove(&f);
        } else {
            b.writer.insert(f, app);
            if now.inode().is_none() {
                w |= what::CREATED;
            }
        }
        if old.own.iter().any(|(k, _)| k.0 & 0xff == K_MADE)
            != now.own.iter().any(|(k, _)| k.0 & 0xff == K_MADE)
        {
            w |= what::MADE;
        }
        *b.changes.entry((app, f)).or_insert(0) |= w;
        b.touched = true;
        b.docs_changed = true;
        Ok(())
    }

    // ---- The one-undo list ----

    /// Record an action the person asked `app` for: the changes `app`
    /// made since its last commit become one action. The action is in the
    /// next commit, with them; commit right after.
    pub fn record_action(
        &mut self,
        p: Person,
        app: App,
        kind: u64,
        label: &[u8],
    ) -> Result<u64, Error> {
        self.record_action_inner(p, app, kind, label, 0)
    }

    fn record_action_inner(
        &mut self,
        p: Person,
        app: App,
        kind: u64,
        label: &[u8],
        undoes: u64,
    ) -> Result<u64, Error> {
        self.ok()?;
        if side::is_side(p) || kind == 0 || kind > 0xff {
            return Err(Error::BadRequest);
        }
        let cn = self.pending_gen();
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let no = b
            .hist
            .last_in(dev, &b.keys, k_act(0), k_act(u64::MAX >> 2))?
            .map_or(1, |(k, _)| (k.0 & !H_ACT) + 1);
        let mut l = [0u8; LABEL];
        let n = label.len().min(LABEL);
        l[..n].copy_from_slice(&label[..n]);
        let a = Action {
            no,
            cn,
            app,
            kind,
            undone: false,
            undoes,
            label: l,
        };
        b.hist.insert(dev, &b.keys, k_act(no), a.encode())?;
        b.touched = true;
        Ok(no)
    }

    /// The person's actions, newest first, starting below `before` (0:
    /// from the newest).
    pub fn actions(&mut self, p: Person, before: u64, max: usize) -> Result<Vec<Action>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let mut out = Vec::new();
        let mut hi = if before == 0 {
            u64::MAX >> 2
        } else {
            before - 1
        };
        while out.len() < max && hi > 0 {
            match b.hist.last_in(dev, &b.keys, k_act(1), k_act(hi))? {
                Some((k, v)) => {
                    let no = k.0 & !H_ACT;
                    out.push(Action::decode(no, &v));
                    hi = no - 1;
                }
                None => break,
            }
        }
        Ok(out)
    }

    /// What one press of undo undoes (decision 110): the person's newest
    /// action that is not undone and is not itself an undo.
    pub fn last_action(&mut self, p: Person) -> Result<Option<Action>, Error> {
        let mut before = 0;
        loop {
            let batch = self.actions(p, before, 32)?;
            if batch.is_empty() {
                return Ok(None);
            }
            if let Some(a) = batch.iter().find(|a| !a.undone && a.kind != act::UNDO) {
                return Ok(Some(*a));
            }
            before = batch.last().unwrap().no;
        }
    }

    fn set_undone(&mut self, p: Person, no: u64, undone: bool) -> Result<Action, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let v = b
            .hist
            .get(dev, &b.keys, k_act(no))?
            .ok_or(Error::NotFound)?;
        let mut a = Action::decode(no, &v);
        a.undone = undone;
        b.hist.insert(dev, &b.keys, k_act(no), a.encode())?;
        b.touched = true;
        Ok(a)
    }

    /// Undo what app `of` changed in commit `cn` ("What happened?": a line
    /// that is not on the undo list, such as an app saving by itself), as
    /// `app`. The change goes on the undo list as an action first (or the
    /// action already there is used), so the undo can be undone like any
    /// other. Same rules as [`Store::undo`].
    pub fn undo_change(
        &mut self,
        p: Person,
        app: App,
        cn: u64,
        of: App,
        force: bool,
        now: u64,
    ) -> Result<Undo, Error> {
        self.ok()?;
        if side::is_side(p) || cn == 0 || cn >= self.pending_gen() {
            return Err(Error::BadRequest);
        }
        if !self.changes(p, cn)?.iter().any(|c| c.app == of) {
            return Err(Error::NotFound);
        }
        // An action already recorded for exactly this change.
        let mut before = 0;
        let mut found = None;
        'look: loop {
            let batch = self.actions(p, before, 64)?;
            for a in &batch {
                if a.cn == cn && a.app == of && a.kind != act::UNDO {
                    found = Some(*a);
                    break 'look;
                }
                if a.cn < cn {
                    break 'look;
                }
            }
            match batch.last() {
                Some(a) => before = a.no,
                None => break,
            }
        }
        let no = match found {
            Some(a) if a.undone => return Err(Error::BadRequest),
            Some(a) => a.no,
            None => {
                let no = self.record_action_inner(p, of, act::FILES, b"", 0)?;
                // It is about commit `cn`, not the one being gathered.
                let Store { dev, branches, .. } = self;
                let b = branches.get_mut(&p).ok_or(Error::Locked)?;
                let v = b
                    .hist
                    .get(dev, &b.keys, k_act(no))?
                    .ok_or(Error::NotFound)?;
                let mut a = Action::decode(no, &v);
                a.cn = cn;
                b.hist.insert(dev, &b.keys, k_act(no), a.encode())?;
                no
            }
        };
        // On a conflict nothing is undone; an action written here stays on
        // the list (it describes a real change) and is saved with the next
        // commit.
        self.undo(p, app, no, force, now)
    }

    /// Undo action `no` as `app` (the desktop), and commit. Its files go
    /// back to how they were just before it. If one of them changed since
    /// (by anyone), nothing is done unless `force` (rule B6: show both and
    /// ask first). Undoing an undo is a redo. Desktop actions (a closed
    /// window, a group) have no files: the store only marks them, and the
    /// desktop puts them back.
    pub fn undo(
        &mut self,
        p: Person,
        app: App,
        no: u64,
        force: bool,
        now: u64,
    ) -> Result<Undo, Error> {
        self.ok()?;
        let a = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            let v = b
                .hist
                .get(dev, &b.keys, k_act(no))?
                .ok_or(Error::NotFound)?;
            Action::decode(no, &v)
        };
        if a.undone {
            return Err(Error::BadRequest);
        }
        let files: Vec<FileId> = self
            .changes(p, a.cn)?
            .into_iter()
            .filter(|c| c.app == a.app && c.file.class() == Class::Documents)
            .map(|c| c.file)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut done = Vec::new();
        if !files.is_empty() {
            let moments = self.history(p)?;
            let before = moments
                .iter()
                .rev()
                .find(|m| m.cn < a.cn)
                .ok_or(Error::NoSuchMoment)?
                .cn;
            // Rule B6: each file must still be as the action left it.
            if !force {
                let mut changed = Vec::new();
                let after = moments.iter().find(|m| m.cn == a.cn).map(|m| m.cn);
                for &f in &files {
                    let same = match after {
                        Some(cn) => {
                            let root = self.moment_root(p, cn)?;
                            let then = self.state_in(p, View::At(root), f, &[])?;
                            let hint: Vec<u64> = then.parent().into_iter().collect();
                            let now = self.state_in(p, View::Live, f, &hint)?;
                            !self.branch(p)?.dirty.keys().any(|(g, _)| *g == f)
                                && self.same_state(p, f, &then, &now)?
                        }
                        None => false,
                    };
                    if !same {
                        changed.push(f);
                    }
                }
                if !changed.is_empty() {
                    return Ok(Undo::Conflicts(changed));
                }
            }
            match self.restore(p, app, &files, before)? {
                Restored::Done(d) => done = d,
                Restored::Blocked(b) => return Ok(Undo::Conflicts(b)),
            }
        }
        let a = self.set_undone(p, no, true)?;
        if a.kind == act::UNDO && a.undoes != 0 {
            // A redo: the action that was undone counts again.
            self.set_undone(p, a.undoes, false)?;
        }
        self.record_action_inner(p, app, act::UNDO, &a.label, no)?;
        self.commit(now)?;
        Ok(Undo::Done {
            action: a,
            files: done,
        })
    }

    /// A test aid: `days` of history for person `p`, a moment every
    /// `hours`, as if it had been lived from time 1 on (one small file,
    /// "year.txt", changed each time), thinned as it goes as the store
    /// would have. Commits what was gathered first, at `now`. Returns how
    /// many moments were made. Only for a person used by nothing else:
    /// their time labels run ahead of the clock.
    pub fn fake_history(
        &mut self,
        p: Person,
        app: App,
        days: u64,
        hours: u64,
        now: u64,
    ) -> Result<u64, Error> {
        self.ok()?;
        if side::is_side(p) || hours == 0 || days > 3660 {
            return Err(Error::BadRequest);
        }
        self.branch(p)?;
        if self.has_changes() {
            self.commit(now)?;
        }
        let saved = self.now;
        let docs = self.root(Class::Documents);
        let f = match self.lookup(p, docs, b"year.txt") {
            Ok(f) => f,
            Err(Error::NotFound) => self.create(p, app, docs, b"year.txt", false)?,
            Err(e) => return Err(e),
        };
        let step = hours * 3600;
        let mut t = 1;
        let mut made = 0;
        while t <= days * 24 * 3600 {
            let line = alloc::format!("day {:4}, hour {:2}\n", t / 86400, t / 3600 % 24);
            self.write(p, app, f, 0, line.as_bytes())?;
            self.commit(t)?;
            made += 1;
            // Thinning runs once a day in Alaya.
            if made % (24 / hours.min(24)).max(1) == 0 {
                self.thin_one(p, t)?;
            }
            t += step;
        }
        self.thin_one(p, t)?;
        self.commit(t)?;
        self.now = saved;
        Ok(made)
    }

    // ---- What happened? and file stories ----

    /// Commits with changes, newest first, below commit `before` (0: from
    /// the newest).
    pub fn happenings(
        &mut self,
        p: Person,
        before: u64,
        max: usize,
    ) -> Result<Vec<Happening>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let mut out = Vec::new();
        let mut hi = if before == 0 {
            crate::GEN_MAX
        } else {
            before - 1
        };
        while out.len() < max {
            let Some((k, _)) =
                b.hist
                    .last_in(dev, &b.keys, Key::MIN, Key(hi << 8 | 0xff, u64::MAX))?
            else {
                break;
            };
            let cn = k.0 >> 8;
            let mut h = Happening {
                cn,
                time: 0,
                changes: Vec::new(),
            };
            b.hist.range(
                dev,
                &b.keys,
                Key(cn << 8, 0),
                Key(cn << 8 | 0xff, u64::MAX),
                &mut |k, v| {
                    match k.0 & 0xff {
                        H_CHANGE => h.changes.push(Change {
                            app: word(v, 0),
                            file: FileId(word(v, 1)),
                            what: word(v, 2),
                        }),
                        H_INFO => h.time = word(v, 0),
                        _ => {}
                    }
                    true
                },
            )?;
            if !h.changes.is_empty() {
                out.push(h);
            }
            if cn == 0 {
                break;
            }
            hi = cn - 1;
        }
        Ok(out)
    }

    /// A document's name and folder, now or at moment `cn` (None: it does
    /// not exist there, or it was forgotten).
    pub fn name_of(
        &mut self,
        p: Person,
        f: FileId,
        cn: Option<u64>,
    ) -> Result<Option<(Vec<u8>, FileId)>, Error> {
        let view = match cn {
            Some(cn) => View::At(self.moment_root(p, cn)?),
            None => View::Live,
        };
        let st = self.state_in(p, view, f, &[])?;
        match st.entry {
            Some((k, v)) => Ok(self
                .entry_name(p, Class::Documents, k, &v)?
                .map(|n| (n, FileId::new(k.0 >> 8, Class::Documents)))),
            None => Ok(None),
        }
    }

    /// A document's story: who made it, every recorded change (newest
    /// first, at most `max`), and what was made from it.
    pub fn story(&mut self, p: Person, f: FileId, max: usize) -> Result<Story, Error> {
        let mut st = Story::default();
        let mut before = 0;
        loop {
            let batch = self.happenings(p, before, 64)?;
            if batch.is_empty() {
                break;
            }
            for h in &batch {
                for c in &h.changes {
                    if c.file == f {
                        if st.changes.len() < max {
                            st.changes.push((h.cn, h.time, c.app, c.what));
                        }
                        if c.what & what::CREATED != 0 {
                            st.made_by = Some((c.app, h.cn, h.time));
                        }
                    }
                }
            }
            if st.made_by.is_some() {
                break;
            }
            before = batch.last().unwrap().cn;
        }
        // What names it as a source.
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let mut used = BTreeSet::new();
        b.trees[0].range(dev, &b.keys, Key::MIN, Key::MAX, &mut |k, v| {
            if k.0 & 0xff == K_MADE && k.1 >= 1 && word(v, 0) == f.0 {
                used.insert(FileId::new(k.0 >> 8, Class::Documents));
            }
            true
        })?;
        st.used_by = used.into_iter().collect();
        Ok(st)
    }
}

/// A document waiting to be forgotten, in the person's app-files tree.
const K_FORGET: u64 = 5;
fn k_forget(obj: u64) -> Key {
    Key(crate::grants::RECORDS | K_FORGET, obj)
}

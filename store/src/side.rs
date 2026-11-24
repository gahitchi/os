// SPDX-License-Identifier: GPL-3.0-only
//! Side branches (decision 22, milestone 7): a copy of a person's
//! documents where work can happen without touching the real files, then
//! be kept in one go or thrown away.
//!
//! A side branch starts from a moment of its owner's history and costs
//! nothing until something changes: it shares every block with that
//! moment. The moment is pinned, so thinning keeps it (and with it every
//! shared block) while the side branch lives. Blocks the side branch
//! writes are its own; they are all younger than the moment it starts
//! from, which is how the store tells them apart.
//!
//! - **Keep** compares, file by file, the side branch and the owner's
//!   current files with the moment it started from. Files only the side
//!   branch changed are copied over; a file both changed is a conflict
//!   (rule B6), and then nothing is kept until the person chooses. A
//!   keep is one commit: a power cut leaves all of it or none of it.
//! - **Throw away** gives back every block the side branch owns.
//!
//! In Alaya a side branch is used like a person's own files (its handle
//! is a [`Person`] number with [`SIDE_FLAG`] set), for documents only.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crate::crypto::Keys;
use crate::disk::Disk;
use crate::format::{self, BLOCK, Key, Ptr, Val, word, words};
use crate::tree::Tree;
use crate::{
    App, Branch, Class, Error, F_DIR, FileId, H_CHANGE, H_INFO, H_READ, H_ROOT, K_DATA, K_DIRENT,
    K_INODE, Person, ROOT_OBJ, Store, View, k_data, k_dirent, k_hist, k_pend, k_side, k_sidenext,
    name_hash, write_file_blocks,
};

/// Set in the handle of a side branch.
pub const SIDE_FLAG: Person = 1 << 32;
/// "SIDE".
const SIDE_MAGIC: u64 = 0x4544_4953;
/// Longest label of a side branch, in bytes.
pub const LABEL_MAX: usize = 24;

pub fn is_side(id: Person) -> bool {
    id & SIDE_FLAG != 0
}

/// What a side branch is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SideInfo {
    pub owner: Person,
    pub id: u64,
    /// The owner's moment it starts from.
    pub fork: u64,
    /// The owner's next file number when it started.
    pub first_obj: u64,
    pub created: u64,
    pub label: [u8; LABEL_MAX],
}

impl SideInfo {
    pub(crate) fn decode(b: &[u8; BLOCK]) -> SideInfo {
        SideInfo {
            id: word(b, 1),
            owner: word(b, 2),
            fork: word(b, 3),
            first_obj: word(b, 4),
            created: word(b, 5),
            label: b[80..80 + LABEL_MAX].try_into().unwrap(),
        }
    }

    fn encode(&self, b: &mut [u8; BLOCK]) {
        format::set_word(b, 0, SIDE_MAGIC);
        format::set_word(b, 1, self.id);
        format::set_word(b, 2, self.owner);
        format::set_word(b, 3, self.fork);
        format::set_word(b, 4, self.first_obj);
        format::set_word(b, 5, self.created);
        b[80..80 + LABEL_MAX].copy_from_slice(&self.label);
    }

    /// The documents root and the history root on a side page.
    pub(crate) fn roots(b: &[u8; BLOCK]) -> (Ptr, Ptr) {
        (
            Ptr::decode(b[128..168].try_into().unwrap()),
            Ptr::decode(b[168..208].try_into().unwrap()),
        )
    }

    /// The label, without the padding.
    pub fn label(&self) -> &[u8] {
        let n = self.label.iter().position(|&b| b == 0).unwrap_or(LABEL_MAX);
        &self.label[..n]
    }
}

/// What a keep did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Keep {
    /// These files were copied over; the side branch is gone.
    Kept(Vec<FileId>),
    /// Nothing was done: these files were also changed by the owner since
    /// (or clash with them). Ask the person, then keep again saying for
    /// each whose version wins.
    Conflicts(Vec<FileId>),
}

/// The records of one file in one state: its own records (inode, blocks,
/// "made from" note) and its directory entry.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FileState {
    pub(crate) own: Vec<(Key, Val)>,
    pub(crate) entry: Option<(Key, Val)>,
}

impl FileState {
    pub(crate) fn inode(&self) -> Option<&Val> {
        self.own
            .iter()
            .find(|(k, _)| k.0 & 0xff == K_INODE)
            .map(|(_, v)| v)
    }
    pub(crate) fn parent(&self) -> Option<u64> {
        self.inode().map(|v| word(v, 3))
    }
    pub(crate) fn is_dir(&self) -> bool {
        self.inode().is_some_and(|v| word(v, 4) & F_DIR != 0)
    }
}

/// Every record of file `obj` in `t`, plus its directory entry.
pub(crate) fn file_state(
    t: &Tree,
    io: &mut dyn crate::tree::Io,
    keys: &Keys,
    obj: u64,
    parent_hint: &[u64],
) -> Result<FileState, Error> {
    let own: Vec<(Key, Val)> = t
        .collect(io, keys, Key(obj << 8, 0), Key(obj << 8 | 0xff, u64::MAX))?
        .into_iter()
        .filter(|(k, _)| k.0 & 0xff != K_DIRENT)
        .collect();
    let mut st = FileState { own, entry: None };
    let mut dirs: Vec<u64> = st.parent().into_iter().collect();
    dirs.extend(parent_hint.iter().copied().filter(|d| *d != 0));
    for d in dirs {
        let mut found = None;
        t.range(
            io,
            keys,
            k_dirent(d, 0),
            k_dirent(d, u64::MAX),
            &mut |k, v| {
                if word(v, 0) == obj {
                    found = Some((k, *v));
                    false
                } else {
                    true
                }
            },
        )?;
        if found.is_some() {
            st.entry = found;
            break;
        }
    }
    Ok(st)
}

impl<D: Disk> Store<D> {
    /// Start a side branch of person `p`'s documents, from their last
    /// saved moment. Returns its handle, used like a person number.
    pub fn side_open(&mut self, p: Person, label: &[u8], now: u64) -> Result<Person, Error> {
        self.ok()?;
        if is_side(p) || label.len() > LABEL_MAX {
            return Err(Error::BadRequest);
        }
        let owner_next = self.branch(p)?.next_obj;
        let fork = self.history(p)?.last().ok_or(Error::NoSuchMoment)?.cn;
        let root = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            Ptr::decode(
                &b.hist
                    .get(dev, &b.keys, k_hist(fork, H_ROOT, 0))?
                    .ok_or(Error::NoSuchMoment)?,
            )
        };
        self.room(8, false)?;
        let id = {
            let Store {
                dev, machine, sys, ..
            } = self;
            let next = sys
                .get(dev, machine, k_sidenext())?
                .map_or(1, |v| word(&v, 0));
            sys.insert(dev, machine, k_sidenext(), words(&[next + 1]))?;
            next
        };
        self.pin(p, fork, 1, None)?;
        let spare = self.spare_map();
        let keys = self.branch(p)?.keys.clone();
        let mut b = Branch::new(keys, spare);
        b.trees[Class::Documents as usize] = Tree::open(root, spare);
        b.trees[Class::Apps as usize] = Tree::new(spare);
        b.trees[Class::Caches as usize] = Tree::new(spare);
        let mut l = [0u8; LABEL_MAX];
        l[..label.len()].copy_from_slice(label);
        b.side = Some(SideInfo {
            owner: p,
            id,
            fork,
            first_obj: owner_next,
            created: now,
            label: l,
        });
        b.touched = true;
        b.docs_changed = false;
        self.branches.insert(SIDE_FLAG | id, b);
        Ok(SIDE_FLAG | id)
    }

    /// The side branches of `p` (their handles and what they are).
    pub fn sides(&mut self, p: Person) -> Result<Vec<(Person, SideInfo)>, Error> {
        self.branch(p)?;
        Ok(self
            .branches
            .iter()
            .filter_map(|(id, b)| b.side.filter(|s| s.owner == p).map(|s| (*id, s)))
            .collect())
    }

    /// Load `p`'s side branches (when their files are opened).
    pub(crate) fn load_sides(&mut self, p: Person) -> Result<(), Error> {
        let pages = self.sys.collect(
            &mut self.dev,
            &self.machine,
            k_side(p, 0),
            k_side(p, u64::MAX),
        )?;
        let spare = self.spare_map();
        for (_, v) in pages {
            let ptr = Ptr::decode(&v);
            let keys = self.branch(p)?.keys.clone();
            let mut blk = [0u8; BLOCK];
            self.dev.read_block(&keys, &ptr, &mut blk)?;
            if word(&blk, 0) != SIDE_MAGIC {
                return Err(Error::Damaged(ptr.addr));
            }
            let info = SideInfo::decode(&blk);
            let (docs, hist) = SideInfo::roots(&blk);
            let mut b = Branch::new(keys, spare);
            b.trees[Class::Documents as usize] = Tree::open(docs, spare);
            b.hist = Tree::open(hist, spare);
            b.page = ptr;
            b.side = Some(info);
            b.touched = false;
            b.docs_changed = false;
            self.branches.insert(SIDE_FLAG | info.id, b);
        }
        Ok(())
    }

    /// Commit a side branch: its own blocks, its records, its page.
    pub(crate) fn commit_side(
        &mut self,
        id: Person,
        cn: u64,
        now: u64,
        docs_spare: bool,
        fk: &BTreeMap<u64, Keys>,
    ) -> Result<(), Error> {
        let spare = self.spare_map();
        let Store {
            dev,
            machine,
            sys,
            space,
            branches,
            ..
        } = self;
        let b = branches.get_mut(&id).unwrap();
        let info = b.side.unwrap();
        write_file_blocks(dev, space, b, cn, docs_spare, fk)?;
        let docs = b.trees[0].write(dev, &b.keys, cn, &mut |s| space.alloc(s))?;
        for (seq, ((app, f), w)) in core::mem::take(&mut b.changes).into_iter().enumerate() {
            b.hist.insert(
                dev,
                &b.keys,
                k_hist(cn, H_CHANGE, seq as u64),
                words(&[app, f.0, w]),
            )?;
        }
        for (seq, r) in core::mem::take(&mut b.reads).into_iter().enumerate() {
            b.hist.insert(
                dev,
                &b.keys,
                k_hist(cn, H_READ, seq as u64),
                words(&[r.app, r.file.0, r.version]),
            )?;
        }
        b.hist
            .insert(dev, &b.keys, k_hist(cn, H_INFO, 0), words(&[now]))?;
        let hist = b.hist.write(dev, &b.keys, cn, &mut |s| space.alloc(s))?;
        let mut page = [0u8; BLOCK];
        info.encode(&mut page);
        page[128..168].copy_from_slice(&docs.encode());
        page[168..208].copy_from_slice(&hist.encode());
        let (a, a2) = space.alloc(spare)?;
        let ptr = dev.write_block(&b.keys, a, a2, cn, &mut page)?;
        let old_page = core::mem::replace(&mut b.page, ptr);
        sys.insert(dev, machine, k_side(info.owner, info.id), ptr.encode())?;
        // Blocks given up: the side branch's own are freed; the ones it
        // shares with its owner (no younger than its start) stay theirs.
        let mut pend: Vec<Ptr> = Vec::new();
        if !old_page.is_null() {
            pend.push(old_page);
        }
        pend.append(&mut b.hist.released);
        for t in b.trees.iter_mut() {
            for r in core::mem::take(&mut t.released) {
                if r.birth > info.fork {
                    pend.push(r);
                }
            }
        }
        for r in pend {
            sys.insert(dev, machine, k_pend(cn, r.addr), words(&[r.addr2]))?;
        }
        b.touched = false;
        b.docs_changed = false;
        Ok(())
    }

    /// Every file the side branch changed, with what happened to it.
    pub fn side_changes(&mut self, id: Person) -> Result<Vec<(FileId, u64)>, Error> {
        if !is_side(id) {
            return Err(Error::BadRequest);
        }
        let mut out: BTreeMap<FileId, u64> = BTreeMap::new();
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&id).ok_or(Error::Locked)?;
        b.hist.range(
            dev,
            &b.keys,
            Key::MIN,
            Key(crate::H_ACT - 1, u64::MAX),
            &mut |k, v| {
                if k.0 & 0xff == H_CHANGE {
                    *out.entry(FileId(word(v, 1))).or_insert(0) |= word(v, 2);
                }
                true
            },
        )?;
        for ((_, f), w) in &b.changes {
            *out.entry(*f).or_insert(0) |= *w;
        }
        Ok(out.into_iter().collect())
    }

    /// Throw a side branch away: every block it owns is given back (two
    /// commits from now, like any other).
    pub fn side_discard(&mut self, id: Person) -> Result<(), Error> {
        self.ok()?;
        let info = self.branch(id)?.side.ok_or(Error::BadRequest)?;
        // Keys made for files that exist only in the side branch go too.
        let created: Vec<u64> = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&id).unwrap();
            let mut v = Vec::new();
            b.trees[0].range(dev, &b.keys, Key::MIN, Key::MAX, &mut |k, _| {
                if k.0 & 0xff == K_INODE && k.1 == 0 && k.0 >> 8 >= info.first_obj {
                    v.push(k.0 >> 8);
                }
                true
            })?;
            v
        };
        for obj in created {
            if self
                .inode(info.owner, FileId::new(obj, Class::Documents))
                .is_err()
            {
                let Store { dev, branches, .. } = self;
                let o = branches.get_mut(&info.owner).ok_or(Error::Locked)?;
                o.ktab.remove(dev, &o.keys, crate::k_fkey(obj))?;
                o.fkeys.remove(&obj);
                o.touched = true;
            }
        }
        let b = self.branches.remove(&id).unwrap();
        let pg = self.pending_gen();
        let mut own: BTreeSet<(u64, u64)> = BTreeSet::new();
        if !b.page.is_null() {
            own.insert((b.page.addr, b.page.addr2));
            let mut blk = [0u8; BLOCK];
            self.dev.read_block(&b.keys, &b.page, &mut blk)?;
            let (docs, hist) = SideInfo::roots(&blk);
            let fork = info.fork;
            Tree::walk(&mut self.dev, &b.keys, &docs, &mut |p, n| {
                if p.birth > fork {
                    own.insert((p.addr, p.addr2));
                }
                if n.level == 0 {
                    for (k, v) in n.keys.iter().zip(&n.vals) {
                        let d = Ptr::decode(v);
                        if k.0 & 0xff == K_DATA && d.birth > fork {
                            own.insert((d.addr, d.addr2));
                        }
                    }
                }
            })?;
            Tree::walk(&mut self.dev, &b.keys, &hist, &mut |p, _| {
                own.insert((p.addr, p.addr2));
            })?;
        }
        let Store {
            dev, machine, sys, ..
        } = self;
        for (a, a2) in own {
            sys.insert(dev, machine, k_pend(pg, a), words(&[a2]))?;
        }
        sys.remove(dev, machine, k_side(info.owner, info.id))?;
        self.pin(info.owner, info.fork, -1, None)?;
        Ok(())
    }

    /// Keep a side branch's work: copy every file it changed into its
    /// owner's documents, as `app`, and throw the side branch away. If the
    /// owner changed one of those files too since the side branch started
    /// (or a name now clashes), nothing is done and the conflicts are
    /// returned; keep again with each conflict in `side_wins` or
    /// `owner_wins`. The side branch must be saved (committed) first.
    pub fn side_keep(
        &mut self,
        id: Person,
        app: App,
        side_wins: &[FileId],
        owner_wins: &[FileId],
    ) -> Result<Keep, Error> {
        self.side_keep_as(id, app, 0, side_wins, owner_wins)
    }

    /// [`Store::side_keep`], adding `extra` to what each kept change
    /// records (a job kept although its checks failed says so).
    pub(crate) fn side_keep_as(
        &mut self,
        id: Person,
        app: App,
        extra: u64,
        side_wins: &[FileId],
        owner_wins: &[FileId],
    ) -> Result<Keep, Error> {
        self.ok()?;
        let info = self.branch(id)?.side.ok_or(Error::BadRequest)?;
        {
            // Saved first (reads alone don't matter).
            let b = self.branch(id)?;
            if !b.changes.is_empty() || !b.dirty.is_empty() || b.trees[0].is_dirty() {
                return Err(Error::BadRequest);
            }
        }
        let owner = info.owner;
        let changed = self.side_changes(id)?;
        // The three states of each changed file: where it started, the
        // side branch, the owner now.
        let fork_root = {
            let Store { dev, branches, .. } = self;
            let o = branches.get_mut(&owner).ok_or(Error::Locked)?;
            Ptr::decode(
                &o.hist
                    .get(dev, &o.keys, k_hist(info.fork, H_ROOT, 0))?
                    .ok_or(Error::NoSuchMoment)?,
            )
        };
        let fork_tree = Tree::open(fork_root, false);
        let mut plan: Vec<(FileId, u64, FileState, FileState)> = Vec::new();
        let mut conflicts: BTreeSet<FileId> = BTreeSet::new();
        for &(f, what) in &changed {
            if f.class() != Class::Documents || f.obj() == ROOT_OBJ {
                continue;
            }
            let obj = f.obj();
            let (st_f, st_s, st_o) = {
                let Store { dev, branches, .. } = self;
                let s = branches.get(&id).unwrap();
                let keys = s.keys.clone();
                let st_f = file_state(&fork_tree, dev, &keys, obj, &[])?;
                let hint: Vec<u64> = st_f.parent().into_iter().collect();
                let st_s = file_state(&s.trees[0], dev, &keys, obj, &hint)?;
                let o = branches.get(&owner).unwrap();
                let mut hint2 = hint.clone();
                hint2.extend(st_s.parent());
                let st_o = file_state(&o.trees[0], dev, &keys, obj, &hint2)?;
                (st_f, st_s, st_o)
            };
            if st_s == st_f {
                continue;
            }
            if owner_wins.contains(&f) {
                continue;
            }
            if st_o != st_f && !side_wins.contains(&f) {
                conflicts.insert(f);
                continue;
            }
            plan.push((f, what, st_s, st_o));
        }
        // Names and places must still work in the owner's files: the new
        // directory exists, no other file has the name, no directory ends
        // up inside itself, and a directory the side branch removed has no
        // new files in it.
        let mut parent_of: BTreeMap<u64, Option<u64>> = BTreeMap::new();
        for (f, _, st_s, _) in &plan {
            parent_of.insert(f.obj(), st_s.parent());
        }
        for (f, _, st_s, _) in &plan {
            let obj = f.obj();
            if let Some((k, v)) = st_s.entry {
                let dir = FileId::new(k.0 >> 8, Class::Documents);
                let dir_ok = match parent_of.get(&dir.obj()) {
                    Some(p) => p.is_some(),
                    None => self
                        .inode(owner, dir)
                        .is_ok_and(|v| word(&v, 4) & F_DIR != 0),
                };
                if !dir_ok {
                    conflicts.insert(*f);
                    continue;
                }
                if let Some(name) = self.entry_name(id, Class::Documents, k, &v)?
                    && let Some((_, ov)) = self.find_named(owner, View::Live, dir, &name)?
                    && word(&ov, 0) != obj
                    && !plan.iter().any(|(g, _, s, _)| {
                        g.obj() == word(&ov, 0)
                            && s.entry.is_none_or(|(ek, _)| ek.0 >> 8 != dir.obj())
                    })
                {
                    conflicts.insert(*f);
                    continue;
                }
                // Up the new parents: never back to this file.
                let mut up = dir.obj();
                let mut steps = 0;
                while up != ROOT_OBJ {
                    if up == obj || steps > 4096 {
                        conflicts.insert(*f);
                        break;
                    }
                    up = match parent_of.get(&up) {
                        Some(Some(p)) => *p,
                        Some(None) => {
                            conflicts.insert(*f);
                            break;
                        }
                        None => match self.inode(owner, FileId::new(up, Class::Documents)) {
                            Ok(v) => word(&v, 3),
                            Err(_) => {
                                conflicts.insert(*f);
                                break;
                            }
                        },
                    };
                    steps += 1;
                }
            } else if self
                .raw_entries(owner, View::Live, *f, 0, u64::MAX)?
                .iter()
                .any(|(_, v)| !parent_of.contains_key(&word(v, 0)))
            {
                // Removed in the side branch, but the owner put new files
                // in it.
                conflicts.insert(*f);
            }
        }
        if !conflicts.is_empty() {
            return Ok(Keep::Conflicts(conflicts.into_iter().collect()));
        }
        self.room(plan.len() as u64 * 8 + 16, false)?;
        // Apply. Contents are copied, so every block the owner then holds
        // is the owner's own.
        let mut kept = Vec::new();
        for (f, what, st_s, st_o) in plan {
            let obj = f.obj();
            // Blocks to copy: those the side branch has and the owner
            // does not have the same.
            let mut copy = Vec::new();
            for (k, v) in &st_s.own {
                if k.0 & 0xff == K_DATA && !st_o.own.iter().any(|(ok, ov)| ok == k && ov == v) {
                    copy.push(k.1);
                }
            }
            let mut blocks = Vec::new();
            for idx in copy {
                let mut blk = alloc::boxed::Box::new([0u8; BLOCK]);
                match self.read_block_of(id, f, idx, &mut blk) {
                    Ok(()) => blocks.push((idx, blk)),
                    Err(Error::Forgotten) => break,
                    Err(e) => return Err(e),
                }
            }
            let entry_name = match st_s.entry {
                Some((k, v)) => self.entry_name(id, Class::Documents, k, &v)?,
                None => None,
            };
            if st_s.inode().is_some() && self.file_keys(owner, obj).is_err() {
                // Forgotten meanwhile: stays forgotten.
                continue;
            }
            let new_slot = match (&st_s.entry, &entry_name) {
                (Some((k, _)), Some(name)) => {
                    let dir = FileId::new(k.0 >> 8, Class::Documents);
                    if st_o.entry.is_some_and(|(ok, _)| ok == *k) {
                        Some((*k, name.clone()))
                    } else {
                        // Free it first if the owner's own entry for this
                        // file sits there.
                        Some((self.free_slot_except(owner, dir, name, obj)?, name.clone()))
                    }
                }
                _ => None,
            };
            let new_entry = match &new_slot {
                Some((k, name)) => {
                    Some((*k, self.seal_entry(owner, Class::Documents, *k, obj, name)?))
                }
                None => None,
            };
            let Store { dev, branches, .. } = self;
            let o = branches.get_mut(&owner).ok_or(Error::Locked)?;
            let t = &mut o.trees[0];
            // Take away what the owner has and the side branch does not.
            for (k, v) in &st_o.own {
                if !st_s.own.iter().any(|(sk, _)| sk == k) {
                    t.remove(dev, &o.keys, *k)?;
                    if k.0 & 0xff == K_DATA {
                        t.released.push(Ptr::decode(v));
                    }
                }
            }
            if let Some((k, _)) = st_o.entry
                && new_entry.is_none_or(|(nk, _)| nk != k)
            {
                t.remove(dev, &o.keys, k)?;
            }
            for (k, v) in &st_s.own {
                if k.0 & 0xff != K_DATA {
                    t.insert(dev, &o.keys, *k, *v)?;
                }
            }
            if let Some((k, v)) = new_entry {
                t.insert(dev, &o.keys, k, v)?;
            }
            for (idx, blk) in blocks {
                // Drop the owner's old block now; the copy is written at
                // the commit.
                if let Some(old) = t.remove(dev, &o.keys, k_data(obj, idx))? {
                    t.released.push(Ptr::decode(&old));
                }
                o.dirty.insert((f, idx), blk);
            }
            o.dirty
                .retain(|(g, i), _| *g != f || st_s.own.iter().any(|(k, _)| *k == k_data(obj, *i)));
            if st_s.inode().is_some() {
                o.writer.insert(f, app);
            } else {
                o.writer.remove(&f);
            }
            *o.changes.entry((app, f)).or_insert(0) |= what | crate::what::KEPT | extra;
            o.touched = true;
            o.docs_changed = true;
            kept.push(f);
        }
        self.side_discard(id)?;
        Ok(Keep::Kept(kept))
    }

    /// A free place for `name` in `dir`, where an entry of `obj` itself
    /// counts as free (it is about to be replaced).
    fn free_slot_except(
        &mut self,
        id: Person,
        dir: FileId,
        name: &[u8],
        obj: u64,
    ) -> Result<Key, Error> {
        let h = name_hash(name);
        let used = self.raw_entries(id, View::Live, dir, h, h | 0xff)?;
        let slot = (0..256u64)
            .find(|i| !used.iter().any(|(k, v)| k.1 == h | i && word(v, 0) != obj))
            .ok_or(Error::Full)?;
        Ok(k_dirent(dir.obj(), h | slot))
    }

    /// Walk a side branch's documents for the checker: blocks younger
    /// than its start are its own, the rest are shared with its owner.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn walk_side(
        &mut self,
        _sid: Person,
        owner: Person,
        keys: &Keys,
        root: &Ptr,
        fork: u64,
        own: &mut BTreeSet<u64>,
        shared: &mut BTreeSet<u64>,
        r: &mut crate::Report,
    ) -> Result<(), Error> {
        let mut data = Vec::new();
        Tree::walk(&mut self.dev, keys, root, &mut |p, n| {
            let set = if p.birth > fork {
                &mut *own
            } else {
                &mut *shared
            };
            set.insert(p.addr);
            if p.addr2 != 0 {
                set.insert(p.addr2);
            }
            if n.level == 0 {
                for (k, v) in n.keys.iter().zip(&n.vals) {
                    if k.0 & 0xff == K_DATA {
                        data.push((k.0 >> 8, Ptr::decode(v)));
                    }
                }
            }
        })?;
        let (mine, theirs): (Vec<_>, Vec<_>) = data.into_iter().partition(|(_, p)| p.birth > fork);
        for (_, p) in &theirs {
            shared.insert(p.addr);
            if p.addr2 != 0 {
                shared.insert(p.addr2);
            }
        }
        // Read and check its own blocks with the owner's file keys.
        let mut seen = BTreeSet::new();
        self.check_data(owner, Class::Documents, mine.clone(), &mut seen, r)?;
        for (_, p) in mine {
            own.insert(p.addr);
            if p.addr2 != 0 {
                own.insert(p.addr2);
            }
        }
        Ok(())
    }
}

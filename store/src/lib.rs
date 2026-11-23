// SPDX-License-Identifier: GPL-3.0-only
//! The Alaya store (milestone 5): files that survive any power cut, with
//! history, attribution, storage classes, separate people and encryption.
//!
//! - **Never overwrite** (decision 16). Changes are gathered in memory and
//!   written at a commit to free blocks; only when they are all on the disk
//!   does a front page switch to them. There are two front pages, written
//!   in turn.
//! - **A fingerprint on every block**, kept by whoever points at it, up to
//!   the front page (a Merkle tree). Damage is noticed, never returned as
//!   data. The store's own map has a spare copy of every block.
//! - **Encrypted** (decision 29): AES-256 and keyed SHA-256 fingerprints.
//!   The machine key protects the store's map; each person's files are
//!   under their own key (decision 30).
//! - **Attribution and reads** (decision 22, `commitments.md`): each commit
//!   records which app changed which file, and which app read which
//!   version of a document.
//! - **Storage classes** (decision 33): documents keep their history,
//!   apps and games the current version plus one copy from before an
//!   update, caches none.
//!
//! The layout and the rules are explained in `docs/store.md`.

#![no_std]

extern crate alloc;

pub use alaya_crypto as crypto;
pub mod disk;
pub mod format;
pub mod made;
pub mod space;
pub mod tree;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

use crypto::Keys;
use disk::{Disk, IoError};
use format::{BLOCK, Block, FRONT, Front, FrontError, Key, Ptr, RawNode, Val, word, words};
use space::Space;
use tree::{Io, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The disk failed or the power was cut.
    Io(IoError),
    /// A block's fingerprint did not match and no spare copy did either.
    Damaged(u64),
    /// Not an Alaya store.
    NotAStore,
    /// The key does not open it.
    WrongKey,
    /// A format this version does not know.
    Format,
    NotFound,
    Exists,
    /// No room left (the reserve is only for deleting).
    Full,
    /// The person's files are locked (their key was not given).
    Locked,
    BadName,
    NotADirectory,
    IsADirectory,
    NotEmpty,
    /// Past views are read-only, and so is a moment history no longer has.
    NoSuchMoment,
    /// A commit failed half way; the store must be opened again.
    MustReopen,
    BadRequest,
}

impl From<IoError> for Error {
    fn from(e: IoError) -> Error {
        Error::Io(e)
    }
}

/// Storage classes (decision 33).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Full history, thinned over time; reads are recorded.
    Documents = 0,
    /// The current version plus one copy from before an update.
    Apps = 1,
    /// No history.
    Caches = 2,
}

impl Class {
    pub const ALL: [Class; 3] = [Class::Documents, Class::Apps, Class::Caches];

    pub fn from(n: u64) -> Option<Class> {
        match n {
            0 => Some(Class::Documents),
            1 => Some(Class::Apps),
            2 => Some(Class::Caches),
            _ => None,
        }
    }
}

/// Spare copies on the same disk (milestone 5, choice C).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copies {
    None = 0,
    /// Every block of the store's own map (trees, bitmaps, person pages).
    Map = 1,
    /// The map and the contents of documents.
    MapAndDocuments = 2,
}

/// A person (decision 30). Person 0 holds what is shared, such as
/// installed apps.
pub type Person = u64;
/// The app a change or a read is recorded for (in Alaya, the badge of the
/// capability it used).
pub type App = u64;

/// A file or directory: its number and class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u64);

impl FileId {
    pub fn new(obj: u64, class: Class) -> FileId {
        FileId(obj << 2 | class as u64)
    }
    pub fn obj(self) -> u64 {
        self.0 >> 2
    }
    pub fn class(self) -> Class {
        Class::from(self.0 & 3).unwrap_or(Class::Caches)
    }
}

/// The root directory of a class.
pub const ROOT_OBJ: u64 = 1;
/// Longest name, in bytes.
pub const NAME_MAX: usize = 32;
/// The largest person number.
pub const PERSON_MAX: Person = 0xfff;

/// What a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stat {
    pub size: u64,
    /// The commit that made this version: with the file's number, a
    /// version's permanent identity.
    pub version: u64,
    pub created: u64,
    pub parent: u64,
    pub is_dir: bool,
    /// For documents, how many times the file was changed: with the
    /// version, it tells apart changes made before the same commit.
    pub edits: u64,
}

/// A document that might be worth cleaning up (see
/// [`Store::cleanup_suggestions`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub file: FileId,
    pub size: u64,
    pub days_unchanged: u64,
}

/// One recorded change: which app did what to which file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Change {
    pub app: App,
    pub file: FileId,
    pub what: u64,
}

/// Bits of [`Change::what`].
pub mod what {
    pub const CREATED: u64 = 1;
    pub const WROTE: u64 = 2;
    pub const DELETED: u64 = 4;
    pub const RENAMED: u64 = 8;
}

/// One recorded read: an app opened this version of a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Read {
    pub app: App,
    pub file: FileId,
    pub version: u64,
}

/// A moment in a person's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Moment {
    pub cn: u64,
    /// The wall-clock label (seconds), only a label (decision 47).
    pub time: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Damaged blocks fixed from their spare copy.
    pub repaired: u64,
    /// Blocks read from the disk (not the cache).
    pub disk_reads: u64,
    pub commits: u64,
    /// Snapshots given up to stay within budget or make room.
    pub dropped: u64,
}

// Keys of the system tree.
const T_SPACE: u64 = 1;
const T_PERSON: u64 = 2;
const T_PEND: u64 = 3;
const T_DEAD: u64 = 4;
const T_SNAP: u64 = 5;
const T_KEYCHECK: u64 = 6;
const GEN_BITS: u64 = 40;
const GEN_MAX: u64 = (1 << GEN_BITS) - 1;

fn k_space(chunk: u64) -> Key {
    Key(T_SPACE << 56 | chunk, 0)
}
fn k_person(p: Person) -> Key {
    Key(T_PERSON << 56 | p, 0)
}
fn k_keycheck(p: Person) -> Key {
    Key(T_KEYCHECK << 56 | p, 0)
}
fn k_pend(cn: u64, addr: u64) -> Key {
    Key(T_PEND << 56 | cn, addr)
}
fn k_dead(p: Person, c: Class, died: u64, addr: u64) -> Key {
    Key(
        T_DEAD << 56 | p << 44 | (c as u64) << GEN_BITS | died.min(GEN_MAX),
        addr,
    )
}
fn k_snap(p: Person, c: Class, cn: u64) -> Key {
    Key(
        T_SNAP << 56 | p << 44 | (c as u64) << GEN_BITS | cn.min(GEN_MAX),
        0,
    )
}
fn snap_gen(k: Key) -> u64 {
    k.0 & GEN_MAX
}

// Keys of a person's class trees.
const K_INODE: u64 = 0;
const K_DATA: u64 = 1;
const K_DIRENT: u64 = 2;
/// The "made from" note of a result (see `made.rs`).
const K_MADE: u64 = 3;
fn k_inode(obj: u64) -> Key {
    Key(obj << 8 | K_INODE, 0)
}
fn k_data(obj: u64, idx: u64) -> Key {
    Key(obj << 8 | K_DATA, idx)
}
fn k_dirent(dir: u64, slot: u64) -> Key {
    Key(dir << 8 | K_DIRENT, slot)
}
fn k_made(obj: u64, slot: u64) -> Key {
    Key(obj << 8 | K_MADE, slot)
}

// Keys of a person's history tree.
const H_ROOT: u64 = 0;
const H_CHANGE: u64 = 1;
const H_READ: u64 = 2;
const H_INFO: u64 = 3;
const H_APPS: u64 = 4;
fn k_hist(cn: u64, kind: u64, seq: u64) -> Key {
    Key(cn << 8 | kind, seq)
}

const F_DIR: u64 = 1;
/// Inode word 4 holds the flags in its low byte and, for caches, the day
/// the file was last used plus one (0: not known yet) above them.
const USED_SHIFT: u32 = 8;
/// The app number the store itself uses for its own changes (cleaning).
pub const STORE_APP: App = 0;
const DAY: u64 = 24 * 3600;

fn name_hash(name: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name {
        h = (h ^ *b as u64).wrapping_mul(0x0100_0000_01b3);
    }
    h & !0xff
}

fn check_name(name: &[u8]) -> Result<(), Error> {
    if name.is_empty() || name.len() > NAME_MAX || name.iter().any(|&b| b == 0 || b == b'/') {
        return Err(Error::BadName);
    }
    Ok(())
}

/// "PERS".
const PERSON_MAGIC: u64 = 0x5352_4550;

/// How blocks reach the disk: reading with checks, writing scrambled.
pub struct Dev<D: Disk> {
    pub disk: D,
    cache: BTreeMap<(u64, u64), RawNode>,
    pub stats: Stats,
    /// Skip the cache (the checker reads everything from the disk).
    pub uncached: bool,
    /// Most tree nodes kept in memory.
    pub cache_nodes: usize,
}

const CACHE_NODES: usize = 1024;

impl<D: Disk> Dev<D> {
    /// Read, check and unscramble the block `p` points at. A damaged
    /// first copy is fixed from the spare.
    pub fn read_block(&mut self, keys: &Keys, p: &Ptr, out: &mut Block) -> Result<(), Error> {
        let mut bad_first = false;
        for (n, addr) in [p.addr, p.addr2].into_iter().enumerate() {
            if addr == 0 {
                continue;
            }
            self.stats.disk_reads += 1;
            match self.disk.read(addr, out) {
                Ok(()) => {}
                Err(IoError::PowerCut) => return Err(Error::Io(IoError::PowerCut)),
                Err(IoError::Failed) => {
                    bad_first |= n == 0;
                    continue;
                }
            }
            if !keys.open(p.addr, p.birth, &p.mac, out) {
                bad_first |= n == 0;
                continue;
            }
            if bad_first {
                // Put the good copy back where the damaged one was. The
                // bytes are exactly what belongs there, so this is safe.
                keys.reseal(&p.mac, out);
                let _ = self.disk.write(p.addr, out);
                keys.reseal(&p.mac, out);
                self.stats.repaired += 1;
            }
            return Ok(());
        }
        Err(Error::Damaged(p.addr))
    }

    fn write_block(
        &mut self,
        keys: &Keys,
        addr: u64,
        addr2: u64,
        cn: u64,
        plain: &mut Block,
    ) -> Result<Ptr, Error> {
        let mac = keys.seal(addr, cn, plain);
        self.disk.write(addr, plain)?;
        if addr2 != 0 {
            self.disk.write(addr2, plain)?;
        }
        Ok(Ptr {
            addr,
            addr2,
            birth: cn,
            mac,
        })
    }
}

impl<D: Disk> Io for Dev<D> {
    fn load(&mut self, keys: &Keys, p: &Ptr) -> Result<RawNode, Error> {
        if !self.uncached
            && let Some(n) = self.cache.get(&(p.addr, p.birth))
        {
            return Ok(n.clone());
        }
        let mut b = [0u8; BLOCK];
        self.read_block(keys, p, &mut b)?;
        let n = RawNode::decode(&b).ok_or(Error::Damaged(p.addr))?;
        if !self.uncached {
            if self.cache.len() >= self.cache_nodes.max(1) {
                self.cache.pop_first();
            }
            self.cache.insert((p.addr, p.birth), n.clone());
        }
        Ok(n)
    }

    fn store(
        &mut self,
        keys: &Keys,
        addr: u64,
        addr2: u64,
        cn: u64,
        plain: &mut Block,
    ) -> Result<Ptr, Error> {
        let node = RawNode::decode(plain);
        let p = self.write_block(keys, addr, addr2, cn, plain)?;
        if let Some(n) = node {
            if self.cache.len() >= self.cache_nodes.max(1) {
                self.cache.pop_first();
            }
            self.cache.insert((addr, cn), n);
        }
        Ok(p)
    }
}

/// One person's open branch.
struct Branch {
    keys: Keys,
    trees: [Tree; 3],
    hist: Tree,
    page: Ptr,
    next_obj: u64,
    /// Changes and reads since the last commit.
    changes: BTreeMap<(App, FileId), u64>,
    reads: BTreeSet<Read>,
    /// File blocks written since the last commit.
    dirty: BTreeMap<(FileId, u64), Box<Block>>,
    /// Something changed since the last commit.
    touched: bool,
    docs_changed: bool,
    /// Keep the apps as they are now, as the copy from before an update.
    apps_snapshot: bool,
    /// How much of which document version each app has read since the
    /// person's files were opened (living validity; kept in memory only).
    seen: BTreeMap<(App, FileId), made::Seen>,
    /// Which app last changed each document since they were opened.
    writer: BTreeMap<FileId, App>,
}

impl Branch {
    fn new(keys: Keys, spare: bool) -> Branch {
        Branch {
            keys,
            trees: [Tree::new(spare), Tree::new(spare), Tree::new(spare)],
            hist: Tree::new(spare),
            page: Ptr::default(),
            next_obj: ROOT_OBJ + 1,
            changes: BTreeMap::new(),
            reads: BTreeSet::new(),
            dirty: BTreeMap::new(),
            touched: true,
            docs_changed: true,
            apps_snapshot: false,
            seen: BTreeMap::new(),
            writer: BTreeMap::new(),
        }
    }
}

/// Options when formatting.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub copies: Copies,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            copies: Copies::Map,
        }
    }
}

pub struct Store<D: Disk> {
    pub dev: Dev<D>,
    machine: Keys,
    /// The last commit that is safely on the disk.
    cn: u64,
    sys: Tree,
    branches: BTreeMap<Person, Branch>,
    space: Space,
    copies: Copies,
    /// Blocks kept only for past moments.
    history_blocks: u64,
    /// Set when a commit failed half way.
    broken: bool,
    /// Old bitmap blocks replaced in this commit.
    released_bitmaps: Vec<Ptr>,
    /// The wall-clock label of the newest commit (seconds; 0 until the
    /// first commit of this mount).
    now: u64,
    /// Record which app read which version of a document (on unless a
    /// measurement turns it off to see what it costs).
    pub track_reads: bool,
}

/// The default history budget (decision 33): 15 % of the disk.
pub const HISTORY_BUDGET_PERCENT: u64 = 15;
/// Moments never given up to make room (choice B): the last day.
pub const KEEP_SECS: u64 = 24 * 3600;

impl<D: Disk> Store<D> {
    /// Make a new, empty store on `disk`.
    pub fn format(
        mut disk: D,
        machine_secret: &[u8; 32],
        opts: Options,
    ) -> Result<Store<D>, Error> {
        let blocks = disk.blocks();
        if blocks < 256 {
            return Err(Error::BadRequest);
        }
        // Wipe both front pages first, so no older store on this disk can
        // be mistaken for this one.
        let zero = [0u8; BLOCK];
        disk.write(FRONT[0], &zero)?;
        disk.write(FRONT[1], &zero)?;
        disk.flush()?;
        let mut space = Space::new(blocks);
        space.mark(FRONT[0]);
        space.mark(FRONT[1]);
        let spare = opts.copies != Copies::None;
        let mut s = Store {
            dev: Dev {
                disk,
                cache: BTreeMap::new(),
                stats: Stats::default(),
                uncached: false,
                cache_nodes: CACHE_NODES,
            },
            machine: Keys::new(machine_secret),
            cn: 0,
            sys: Tree::new(spare),
            branches: BTreeMap::new(),
            space,
            copies: opts.copies,
            history_blocks: 0,
            broken: false,
            released_bitmaps: Vec::new(),
            track_reads: true,
            now: 0,
        };
        // Every chunk gets its bitmap block in the first commit.
        for c in 0..s.space.chunks() {
            s.space.dirty.insert(c);
        }
        s.commit(0)?;
        Ok(s)
    }

    /// Open the store on `disk` at its last safe commit.
    pub fn open(mut disk: D, machine_secret: &[u8; 32]) -> Result<Store<D>, Error> {
        let machine = Keys::new(machine_secret);
        let mut fronts = Vec::new();
        let mut wrong_key = false;
        for &a in FRONT.iter() {
            let mut b = [0u8; BLOCK];
            if disk.read(a, &mut b).is_err() {
                continue;
            }
            match Front::decode(&machine, &b) {
                Ok(f) => fronts.push(f),
                Err(FrontError::Fingerprint) => wrong_key = true,
                Err(FrontError::Format) => return Err(Error::Format),
                Err(FrontError::NotAStore) => {}
            }
        }
        fronts.sort_by_key(|f| core::cmp::Reverse(f.cn));
        let highest = fronts.first().map(|f| f.cn);
        let mut dev = Dev {
            disk,
            cache: BTreeMap::new(),
            stats: Stats::default(),
            uncached: false,
            cache_nodes: CACHE_NODES,
        };
        for f in fronts {
            // The newest front page whose system tree is whole.
            if dev.load(&machine, &f.root).is_err() {
                continue;
            }
            let copies = match f.options >> format::opt::COPIES_SHIFT & format::opt::COPIES_MASK {
                0 => Copies::None,
                1 => Copies::Map,
                _ => Copies::MapAndDocuments,
            };
            let spare = copies != Copies::None;
            let mut s = Store {
                dev,
                machine,
                cn: f.cn,
                sys: Tree::open(f.root, spare),
                branches: BTreeMap::new(),
                space: Space::new(f.blocks),
                copies,
                history_blocks: f.history_blocks,
                broken: false,
                released_bitmaps: Vec::new(),
                track_reads: true,
                now: 0,
            };
            s.load_space()?;
            // Start every new mount past every commit number the front
            // pages show, and write its front page on the other page than
            // the one just opened, so that one stays as the older page
            // (checked in formal/commit). A newer page with a bad spot may
            // hide the last number used; that is harmless, because blocks
            // are scrambled from their own fingerprints (crypto::Keys::seal).
            let mut bump = highest.unwrap_or(f.cn).max(f.cn) + 1;
            if bump % 2 == f.cn % 2 {
                bump += 1;
            }
            s.write_front(bump, f.root)?;
            s.cn = bump;
            return Ok(s);
        }
        Err(if wrong_key {
            Error::WrongKey
        } else {
            Error::NotAStore
        })
    }

    fn load_space(&mut self) -> Result<(), Error> {
        for c in 0..self.space.chunks() {
            let v = self
                .sys
                .get(&mut self.dev, &self.machine, k_space(c))?
                .ok_or(Error::Damaged(0))?;
            let p = Ptr::decode(&v);
            let mut b = [0u8; BLOCK];
            self.dev.read_block(&self.machine, &p, &mut b)?;
            self.space.load_chunk(c, &b);
        }
        self.space.dirty.clear();
        Ok(())
    }

    fn write_front(&mut self, cn: u64, root: Ptr) -> Result<(), Error> {
        let f = Front {
            cn,
            blocks: self.space.blocks,
            root,
            history_blocks: self.history_blocks,
            used: self.space.used,
            options: (self.copies as u64) << format::opt::COPIES_SHIFT,
        };
        let mut b = [0u8; BLOCK];
        f.encode(&self.machine, &mut b);
        self.dev.disk.write(FRONT[(cn % 2) as usize], &b)?;
        self.dev.disk.flush()?;
        Ok(())
    }

    /// The last commit safely on the disk.
    pub fn last_commit(&self) -> u64 {
        self.cn
    }

    /// The commit the changes being gathered now will get.
    pub fn pending_gen(&self) -> u64 {
        self.cn + 1
    }

    /// A commit failed half way: the store must be opened again.
    pub fn broken(&self) -> bool {
        self.broken
    }

    pub fn into_disk(self) -> D {
        self.dev.disk
    }

    pub fn blocks(&self) -> u64 {
        self.space.blocks
    }

    pub fn used_blocks(&self) -> u64 {
        self.space.used
    }

    pub fn history_blocks(&self) -> u64 {
        self.history_blocks
    }

    /// Blocks held back so deleting always works (2 %, at least 64).
    pub fn reserve(&self) -> u64 {
        (self.space.blocks * 2 / 100).max(64)
    }

    fn spare_map(&self) -> bool {
        self.copies != Copies::None
    }

    fn ok(&self) -> Result<(), Error> {
        if self.broken {
            Err(Error::MustReopen)
        } else {
            Ok(())
        }
    }

    // ---- People ----

    /// Add a person with their own key. Their root directories are made
    /// in the next commit.
    pub fn add_person(&mut self, p: Person, secret: &[u8; 32]) -> Result<(), Error> {
        self.ok()?;
        if p > PERSON_MAX {
            return Err(Error::BadRequest);
        }
        if self.branches.contains_key(&p)
            || self
                .sys
                .get(&mut self.dev, &self.machine, k_person(p))?
                .is_some()
        {
            return Err(Error::Exists);
        }
        self.room(16, false)?;
        let spare = self.spare_map();
        let mut b = Branch::new(Keys::new(secret), spare);
        // A check value, so a wrong key is told apart from damage.
        let check = b.keys.mac(&[b"key check", &p.to_le_bytes()]);
        let mut v = [0u8; 40];
        v[..32].copy_from_slice(&check);
        self.sys
            .insert(&mut self.dev, &self.machine, k_keycheck(p), v)?;
        let pg = self.pending_gen();
        for c in Class::ALL {
            b.trees[c as usize].insert(
                &mut self.dev,
                &b.keys,
                k_inode(ROOT_OBJ),
                words(&[0, pg, pg, ROOT_OBJ, F_DIR]),
            )?;
        }
        self.branches.insert(p, b);
        Ok(())
    }

    /// Open a person's files with their key.
    pub fn unlock(&mut self, p: Person, secret: &[u8; 32]) -> Result<(), Error> {
        self.ok()?;
        if self.branches.contains_key(&p) {
            return Ok(());
        }
        let v = self
            .sys
            .get(&mut self.dev, &self.machine, k_person(p))?
            .ok_or(Error::NotFound)?;
        let ptr = Ptr::decode(&v);
        let keys = Keys::new(secret);
        let check = self
            .sys
            .get(&mut self.dev, &self.machine, k_keycheck(p))?
            .ok_or(Error::Damaged(0))?;
        if !crypto::same(&check[..32], &keys.mac(&[b"key check", &p.to_le_bytes()])) {
            return Err(Error::WrongKey);
        }
        let mut b = [0u8; BLOCK];
        self.dev.read_block(&keys, &ptr, &mut b)?;
        if word(&b, 0) != PERSON_MAGIC || word(&b, 1) != p {
            return Err(Error::Damaged(ptr.addr));
        }
        let spare = self.spare_map();
        let root = |i: usize| Ptr::decode(b[64 + 40 * i..104 + 40 * i].try_into().unwrap());
        let br = Branch {
            keys,
            trees: [
                Tree::open(root(0), spare),
                Tree::open(root(1), spare),
                Tree::open(root(2), spare),
            ],
            hist: Tree::open(root(3), spare),
            page: ptr,
            next_obj: word(&b, 2),
            changes: BTreeMap::new(),
            reads: BTreeSet::new(),
            dirty: BTreeMap::new(),
            touched: false,
            docs_changed: false,
            apps_snapshot: false,
            seen: BTreeMap::new(),
            writer: BTreeMap::new(),
        };
        self.branches.insert(p, br);
        Ok(())
    }

    /// Close a person's files. Their changes must be committed first.
    pub fn lock(&mut self, p: Person) -> Result<(), Error> {
        match self.branches.get(&p) {
            None => Ok(()),
            Some(b) if b.touched => Err(Error::BadRequest),
            Some(_) => {
                self.branches.remove(&p);
                Ok(())
            }
        }
    }

    /// The people on this disk.
    pub fn people(&mut self) -> Result<Vec<Person>, Error> {
        let mut out: Vec<Person> = self
            .sys
            .collect(
                &mut self.dev,
                &self.machine,
                k_person(0),
                k_person(PERSON_MAX),
            )?
            .iter()
            .map(|(k, _)| k.0 & PERSON_MAX)
            .collect();
        for p in self.branches.keys() {
            if !out.contains(p) {
                out.push(*p);
            }
        }
        out.sort();
        Ok(out)
    }

    // ---- Room ----

    /// A safe upper bound on the blocks the next commit needs.
    fn need(&self) -> u64 {
        let map = if self.spare_map() { 2 } else { 1 };
        let docs = if self.copies == Copies::MapAndDocuments {
            2
        } else {
            1
        };
        let mut n = 0;
        let mut released = self.sys.released.len() as u64;
        for b in self.branches.values() {
            if !b.touched {
                continue;
            }
            for (f, _) in b.dirty.keys() {
                n += if f.class() == Class::Documents {
                    docs
                } else {
                    1
                };
            }
            let nodes: usize = b.trees.iter().map(|t| t.mem).sum::<usize>() + b.hist.mem;
            let extra = (b.changes.len() + b.reads.len()) as u64 / 30 + 4;
            n += (nodes as u64 + extra + 1) * map;
            released += b.trees.iter().map(|t| t.released.len()).sum::<usize>() as u64;
            released += b.dirty.len() as u64 + 2;
        }
        // The system tree: records for released blocks, the bitmaps, paths.
        n += (self.sys.mem as u64 + released / 30 + 8) * map;
        n += self.space.chunks() * map * 2;
        n + 16
    }

    /// Is there room for a change needing about `extra` more blocks?
    /// Deleting may use the reserve.
    fn room(&self, extra: u64, deleting: bool) -> Result<(), Error> {
        let keep = if deleting { 0 } else { self.reserve() };
        if self.space.free_blocks() < self.need() + extra + keep {
            return Err(Error::Full);
        }
        Ok(())
    }

    // ---- Files ----

    fn branch(&mut self, p: Person) -> Result<&mut Branch, Error> {
        self.branches.get_mut(&p).ok_or(Error::Locked)
    }

    fn inode(&mut self, p: Person, f: FileId) -> Result<Val, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[f.class() as usize]
            .get(dev, &b.keys, k_inode(f.obj()))?
            .ok_or(Error::NotFound)
    }

    fn put_inode(&mut self, p: Person, f: FileId, v: Val) -> Result<(), Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[f.class() as usize].insert(dev, &b.keys, k_inode(f.obj()), v)?;
        b.touched = true;
        if f.class() == Class::Documents {
            b.docs_changed = true;
        }
        Ok(())
    }

    /// A document changed: count the change and remember who made it.
    fn edited(&mut self, p: Person, app: App, f: FileId, st: &mut Val) -> Result<(), Error> {
        if f.class() == Class::Documents {
            let w = word(st, 4) + (1 << USED_SHIFT);
            format::set_word(st, 4, w);
            self.branch(p)?.writer.insert(f, app);
        }
        Ok(())
    }

    fn note(&mut self, p: Person, app: App, f: FileId, w: u64) -> Result<(), Error> {
        let b = self.branch(p)?;
        *b.changes.entry((app, f)).or_insert(0) |= w;
        b.touched = true;
        if f.class() == Class::Documents {
            b.docs_changed = true;
        }
        Ok(())
    }

    pub fn root(&self, class: Class) -> FileId {
        FileId::new(ROOT_OBJ, class)
    }

    pub fn stat(&mut self, p: Person, f: FileId) -> Result<Stat, Error> {
        let v = self.inode(p, f)?;
        let mut st = stat_of(&v);
        if f.class() == Class::Documents {
            st.edits = word(&v, 4) >> USED_SHIFT;
        }
        Ok(st)
    }

    fn find_entry(
        &mut self,
        p: Person,
        dir: FileId,
        name: &[u8],
    ) -> Result<Option<(Key, Val)>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        find_in(
            &b.trees[dir.class() as usize],
            dev,
            &b.keys,
            dir.obj(),
            name,
        )
    }

    pub fn lookup(&mut self, p: Person, dir: FileId, name: &[u8]) -> Result<FileId, Error> {
        check_name(name)?;
        match self.find_entry(p, dir, name)? {
            Some((_, v)) => Ok(FileId::new(word(&v, 0), dir.class())),
            None => Err(Error::NotFound),
        }
    }

    /// The names in a directory, with their files.
    pub fn list(&mut self, p: Person, dir: FileId) -> Result<Vec<(Vec<u8>, FileId)>, Error> {
        if !stat_of(&self.inode(p, dir)?).is_dir {
            return Err(Error::NotADirectory);
        }
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        list_in(&b.trees[dir.class() as usize], dev, &b.keys, dir)
    }

    /// Make a file (or directory) called `name` in `dir`.
    pub fn create(
        &mut self,
        p: Person,
        app: App,
        dir: FileId,
        name: &[u8],
        is_dir: bool,
    ) -> Result<FileId, Error> {
        self.ok()?;
        check_name(name)?;
        if !stat_of(&self.inode(p, dir)?).is_dir {
            return Err(Error::NotADirectory);
        }
        if self.find_entry(p, dir, name)?.is_some() {
            return Err(Error::Exists);
        }
        self.room(8, false)?;
        let pg = self.pending_gen();
        let last_used = if dir.class() == Class::Caches {
            self.today_bits()
        } else {
            0
        };
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[dir.class() as usize];
        // A free slot among the names that share this name's hash.
        let h = name_hash(name);
        let used = t.collect(
            dev,
            &b.keys,
            k_dirent(dir.obj(), h),
            k_dirent(dir.obj(), h | 0xff),
        )?;
        let slot = (0..256u64)
            .find(|i| !used.iter().any(|(k, _)| k.1 == h | i))
            .ok_or(Error::Full)?;
        let obj = b.next_obj;
        b.next_obj += 1;
        let f = FileId::new(obj, dir.class());
        t.insert(
            dev,
            &b.keys,
            k_inode(obj),
            words(&[
                0,
                pg,
                pg,
                dir.obj(),
                (if is_dir { F_DIR } else { 0 }) | last_used,
            ]),
        )?;
        let mut v = [0u8; 40];
        format::set_word(&mut v, 0, obj);
        v[8..8 + name.len()].copy_from_slice(name);
        t.insert(dev, &b.keys, k_dirent(dir.obj(), h | slot), v)?;
        if f.class() == Class::Documents {
            b.writer.insert(f, app);
        }
        self.note(p, app, f, what::CREATED)?;
        Ok(f)
    }

    /// Write `data` at `off`.
    pub fn write(
        &mut self,
        p: Person,
        app: App,
        f: FileId,
        off: u64,
        data: &[u8],
    ) -> Result<(), Error> {
        self.ok()?;
        let mut st = self.inode(p, f)?;
        if word(&st, 4) & F_DIR != 0 {
            return Err(Error::IsADirectory);
        }
        if data.is_empty() {
            return Ok(());
        }
        let end = off
            .checked_add(data.len() as u64)
            .ok_or(Error::BadRequest)?;
        let first = off / BLOCK as u64;
        let last = (end - 1) / BLOCK as u64;
        let fresh = {
            let b = self.branch(p)?;
            (first..=last)
                .filter(|i| !b.dirty.contains_key(&(f, *i)))
                .count() as u64
        };
        let per = if f.class() == Class::Documents && self.copies == Copies::MapAndDocuments {
            2
        } else {
            1
        };
        self.room(fresh * per + 8, false)?;
        let size = word(&st, 0);
        for idx in first..=last {
            let mut blk = match self.branch(p)?.dirty.remove(&(f, idx)) {
                Some(b) => b,
                None => {
                    let mut b = Box::new([0u8; BLOCK]);
                    // A block written only in part keeps the rest.
                    let whole = off <= idx * BLOCK as u64 && end >= (idx + 1) * BLOCK as u64;
                    if !whole && idx * (BLOCK as u64) < size {
                        self.read_block_of(p, f, idx, &mut b)?;
                    }
                    b
                }
            };
            let bs = idx * BLOCK as u64;
            let from = off.max(bs);
            let to = end.min(bs + BLOCK as u64);
            blk[(from - bs) as usize..(to - bs) as usize]
                .copy_from_slice(&data[(from - off) as usize..(to - off) as usize]);
            self.branch(p)?.dirty.insert((f, idx), blk);
        }
        format::set_word(&mut st, 0, size.max(end));
        format::set_word(&mut st, 1, self.pending_gen());
        if f.class() == Class::Caches {
            self.stamp(&mut st);
        }
        self.edited(p, app, f, &mut st)?;
        self.put_inode(p, f, st)?;
        self.note(p, app, f, what::WROTE)?;
        Ok(())
    }

    fn read_block_of(
        &mut self,
        p: Person,
        f: FileId,
        idx: u64,
        out: &mut Block,
    ) -> Result<(), Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        if let Some(d) = b.dirty.get(&(f, idx)) {
            *out = **d;
            return Ok(());
        }
        match b.trees[f.class() as usize].get(dev, &b.keys, k_data(f.obj(), idx))? {
            Some(v) => dev.read_block(&b.keys, &Ptr::decode(&v), out),
            None => {
                out.fill(0);
                Ok(())
            }
        }
    }

    /// Read from `off` into `buf`; how many bytes were read. For documents
    /// the read is recorded: which app saw which version.
    pub fn read(
        &mut self,
        p: Person,
        app: App,
        f: FileId,
        off: u64,
        buf: &mut [u8],
    ) -> Result<usize, Error> {
        let st = self.inode(p, f)?;
        if word(&st, 4) & F_DIR != 0 {
            return Err(Error::IsADirectory);
        }
        let size = word(&st, 0);
        if f.class() == Class::Caches {
            self.touch_cache(p, f, st)?;
        }
        if f.class() == Class::Documents && self.track_reads {
            let r = Read {
                app,
                file: f,
                version: word(&st, 1),
            };
            let b = self.branch(p)?;
            if b.reads.insert(r) {
                b.touched = true;
            }
        }
        let n = if off >= size {
            0
        } else {
            (buf.len() as u64).min(size - off) as usize
        };
        if f.class() == Class::Documents {
            let v = (word(&st, 1), word(&st, 4) >> USED_SHIFT, size);
            self.branch(p)?
                .seen
                .entry((app, f))
                .and_modify(|s| s.read(v, off, n as u64))
                .or_insert_with(|| made::Seen::first(v, off, n as u64));
        }
        if n == 0 {
            return Ok(0);
        }
        let mut done = 0;
        let mut blk = [0u8; BLOCK];
        while done < n {
            let pos = off + done as u64;
            let idx = pos / BLOCK as u64;
            let within = (pos % BLOCK as u64) as usize;
            let take = (BLOCK - within).min(n - done);
            self.read_block_of(p, f, idx, &mut blk)?;
            buf[done..done + take].copy_from_slice(&blk[within..within + take]);
            done += take;
        }
        Ok(n)
    }

    /// Cut a file to `size` bytes (or grow it with zeros).
    pub fn truncate(&mut self, p: Person, app: App, f: FileId, size: u64) -> Result<(), Error> {
        self.ok()?;
        let mut st = self.inode(p, f)?;
        if word(&st, 4) & F_DIR != 0 {
            return Err(Error::IsADirectory);
        }
        let old = word(&st, 0);
        self.room(8, size < old)?;
        if size < old {
            let keep = size.div_ceil(BLOCK as u64);
            self.drop_blocks(p, f, keep)?;
            // Zero the tail of the last kept block.
            if !size.is_multiple_of(BLOCK as u64) {
                let idx = size / BLOCK as u64;
                let mut b = Box::new([0u8; BLOCK]);
                self.read_block_of(p, f, idx, &mut b)?;
                b[(size % BLOCK as u64) as usize..].fill(0);
                self.branch(p)?.dirty.insert((f, idx), b);
            }
        }
        format::set_word(&mut st, 0, size);
        format::set_word(&mut st, 1, self.pending_gen());
        if f.class() == Class::Caches {
            self.stamp(&mut st);
        }
        self.edited(p, app, f, &mut st)?;
        self.put_inode(p, f, st)?;
        self.note(p, app, f, what::WROTE)
    }

    /// Forget every block of `f` from index `from` on.
    fn drop_blocks(&mut self, p: Person, f: FileId, from: u64) -> Result<(), Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.dirty.retain(|(g, i), _| *g != f || *i < from);
        let t = &mut b.trees[f.class() as usize];
        let gone = t.collect(
            dev,
            &b.keys,
            k_data(f.obj(), from),
            k_data(f.obj(), u64::MAX),
        )?;
        for (k, _) in gone {
            if let Some(v) = t.remove(dev, &b.keys, k)? {
                t.released.push(Ptr::decode(&v));
            }
        }
        Ok(())
    }

    /// Delete `name` from `dir`. A directory must be empty. Works on a
    /// full disk.
    pub fn remove(&mut self, p: Person, app: App, dir: FileId, name: &[u8]) -> Result<(), Error> {
        self.ok()?;
        check_name(name)?;
        let (k, v) = self.find_entry(p, dir, name)?.ok_or(Error::NotFound)?;
        let f = FileId::new(word(&v, 0), dir.class());
        let st = stat_of(&self.inode(p, f)?);
        if st.is_dir && !self.list(p, f)?.is_empty() {
            return Err(Error::NotEmpty);
        }
        self.room(4, true)?;
        self.drop_blocks(p, f, 0)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[dir.class() as usize];
        t.remove(dev, &b.keys, k)?;
        t.remove(dev, &b.keys, k_inode(f.obj()))?;
        for (mk, _) in t.collect(dev, &b.keys, k_made(f.obj(), 0), k_made(f.obj(), u64::MAX))? {
            t.remove(dev, &b.keys, mk)?;
        }
        b.writer.remove(&f);
        self.note(p, app, f, what::DELETED)
    }

    /// Rename or move within one class.
    pub fn rename(
        &mut self,
        p: Person,
        app: App,
        dir: FileId,
        name: &[u8],
        to_dir: FileId,
        to_name: &[u8],
    ) -> Result<(), Error> {
        self.ok()?;
        check_name(name)?;
        check_name(to_name)?;
        if dir.class() != to_dir.class() {
            return Err(Error::BadRequest);
        }
        if !stat_of(&self.inode(p, to_dir)?).is_dir {
            return Err(Error::NotADirectory);
        }
        let (k, v) = self.find_entry(p, dir, name)?.ok_or(Error::NotFound)?;
        let f = FileId::new(word(&v, 0), dir.class());
        if self.find_entry(p, to_dir, to_name)?.is_some() {
            return Err(Error::Exists);
        }
        // A directory cannot move inside itself.
        let mut up = to_dir.obj();
        while up != ROOT_OBJ {
            if up == f.obj() {
                return Err(Error::BadRequest);
            }
            up = stat_of(&self.inode(p, FileId::new(up, dir.class()))?).parent;
        }
        self.room(8, false)?;
        let mut st = self.inode(p, f)?;
        format::set_word(&mut st, 3, to_dir.obj());
        self.put_inode(p, f, st)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &mut b.trees[dir.class() as usize];
        t.remove(dev, &b.keys, k)?;
        let h = name_hash(to_name);
        let used = t.collect(
            dev,
            &b.keys,
            k_dirent(to_dir.obj(), h),
            k_dirent(to_dir.obj(), h | 0xff),
        )?;
        let slot = (0..256u64)
            .find(|i| !used.iter().any(|(k, _)| k.1 == h | i))
            .ok_or(Error::Full)?;
        let mut nv = [0u8; 40];
        format::set_word(&mut nv, 0, f.obj());
        nv[8..8 + to_name.len()].copy_from_slice(to_name);
        t.insert(dev, &b.keys, k_dirent(to_dir.obj(), h | slot), nv)?;
        self.note(p, app, f, what::RENAMED)
    }

    /// Keep the person's apps as they are at the next commit, as the copy
    /// from before an update (replacing any earlier copy).
    pub fn keep_apps(&mut self, p: Person) -> Result<(), Error> {
        let b = self.branch(p)?;
        b.apps_snapshot = true;
        b.touched = true;
        Ok(())
    }

    /// Give up the copy of the apps from before an update.
    pub fn release_apps(&mut self, p: Person) -> Result<(), Error> {
        let snaps = self.snapshots(p, Class::Apps)?;
        for s in snaps {
            self.drop_snapshot(p, Class::Apps, s.cn)?;
        }
        Ok(())
    }

    // ---- History ----

    fn snapshots(&mut self, p: Person, c: Class) -> Result<Vec<Moment>, Error> {
        Ok(self
            .sys
            .collect(
                &mut self.dev,
                &self.machine,
                k_snap(p, c, 0),
                k_snap(p, c, GEN_MAX),
            )?
            .iter()
            .map(|(k, v)| Moment {
                cn: snap_gen(*k),
                time: word(v, 0),
            })
            .collect())
    }

    /// The moments of a person's documents that history still keeps.
    pub fn history(&mut self, p: Person) -> Result<Vec<Moment>, Error> {
        self.snapshots(p, Class::Documents)
    }

    /// The copy of the apps from before an update, if there is one.
    pub fn apps_copy(&mut self, p: Person) -> Result<Option<Moment>, Error> {
        Ok(self.snapshots(p, Class::Apps)?.pop())
    }

    /// Which app changed what in commit `cn`.
    pub fn changes(&mut self, p: Person, cn: u64) -> Result<Vec<Change>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.hist
            .collect(
                dev,
                &b.keys,
                k_hist(cn, H_CHANGE, 0),
                k_hist(cn, H_CHANGE, u64::MAX),
            )?
            .iter()
            .map(|(_, v)| Change {
                app: word(v, 0),
                file: FileId(word(v, 1)),
                what: word(v, 2),
            })
            .collect())
    }

    /// Which app read which version of which document in commit `cn`.
    pub fn reads(&mut self, p: Person, cn: u64) -> Result<Vec<Read>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.hist
            .collect(
                dev,
                &b.keys,
                k_hist(cn, H_READ, 0),
                k_hist(cn, H_READ, u64::MAX),
            )?
            .iter()
            .map(|(_, v)| Read {
                app: word(v, 0),
                file: FileId(word(v, 1)),
                version: word(v, 2),
            })
            .collect())
    }

    /// A read-only view of a person's documents (or apps copy) at a moment
    /// history keeps.
    pub fn past(&mut self, p: Person, class: Class, cn: u64) -> Result<Past, Error> {
        if class == Class::Caches {
            return Err(Error::NoSuchMoment);
        }
        if self
            .sys
            .get(&mut self.dev, &self.machine, k_snap(p, class, cn))?
            .is_none()
        {
            return Err(Error::NoSuchMoment);
        }
        let kind = if class == Class::Documents {
            H_ROOT
        } else {
            H_APPS
        };
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let v = b
            .hist
            .get(dev, &b.keys, k_hist(cn, kind, 0))?
            .ok_or(Error::NoSuchMoment)?;
        Ok(Past {
            person: p,
            class,
            cn,
            root: Ptr::decode(&v),
        })
    }

    pub fn past_stat(&mut self, v: &Past, f: FileId) -> Result<Stat, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&v.person).ok_or(Error::Locked)?;
        let t = Tree::open(v.root, false);
        Ok(stat_of(
            &t.get(dev, &b.keys, k_inode(f.obj()))?
                .ok_or(Error::NotFound)?,
        ))
    }

    pub fn past_lookup(&mut self, v: &Past, dir: FileId, name: &[u8]) -> Result<FileId, Error> {
        check_name(name)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&v.person).ok_or(Error::Locked)?;
        let t = Tree::open(v.root, false);
        match find_in(&t, dev, &b.keys, dir.obj(), name)? {
            Some((_, e)) => Ok(FileId::new(word(&e, 0), v.class)),
            None => Err(Error::NotFound),
        }
    }

    pub fn past_list(&mut self, v: &Past, dir: FileId) -> Result<Vec<(Vec<u8>, FileId)>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&v.person).ok_or(Error::Locked)?;
        let t = Tree::open(v.root, false);
        list_in(&t, dev, &b.keys, FileId::new(dir.obj(), v.class))
    }

    pub fn past_read(
        &mut self,
        v: &Past,
        f: FileId,
        off: u64,
        buf: &mut [u8],
    ) -> Result<usize, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&v.person).ok_or(Error::Locked)?;
        let t = Tree::open(v.root, false);
        let st = stat_of(
            &t.get(dev, &b.keys, k_inode(f.obj()))?
                .ok_or(Error::NotFound)?,
        );
        if st.is_dir {
            return Err(Error::IsADirectory);
        }
        if off >= st.size {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(st.size - off) as usize;
        let mut done = 0;
        let mut blk = [0u8; BLOCK];
        while done < n {
            let pos = off + done as u64;
            let idx = pos / BLOCK as u64;
            let within = (pos % BLOCK as u64) as usize;
            let take = (BLOCK - within).min(n - done);
            match t.get(dev, &b.keys, k_data(f.obj(), idx))? {
                Some(pv) => dev.read_block(&b.keys, &Ptr::decode(&pv), &mut blk)?,
                None => blk.fill(0),
            }
            buf[done..done + take].copy_from_slice(&blk[within..within + take]);
            done += take;
        }
        Ok(n)
    }

    /// Give up the moment `cn` of class `c`; blocks only it needed become
    /// free two commits later.
    fn drop_snapshot(&mut self, p: Person, c: Class, s: u64) -> Result<(), Error> {
        let Store {
            dev, machine, sys, ..
        } = self;
        if sys.get(dev, machine, k_snap(p, c, s))?.is_none() {
            return Ok(());
        }
        let prev = sys
            .last_in(
                dev,
                machine,
                k_snap(p, c, 0),
                k_snap(p, c, s.saturating_sub(1)),
            )?
            .map(|(k, _)| snap_gen(k))
            .filter(|_| s > 0)
            .unwrap_or(0);
        let mut next = GEN_MAX;
        sys.range(
            dev,
            machine,
            k_snap(p, c, s + 1),
            k_snap(p, c, GEN_MAX),
            &mut |k, _| {
                next = snap_gen(k);
                false
            },
        )?;
        // Kept only for `s`: born after the moment before it, at or before
        // `s`, and gone by the moment after it.
        let dead = sys.collect(
            dev,
            machine,
            k_dead(p, c, s + 1, 0),
            k_dead(p, c, next, u64::MAX),
        )?;
        let pg = self.cn + 1;
        for (k, v) in dead {
            let birth = word(&v, 0);
            if birth > prev && birth <= s {
                let addr2 = word(&v, 1);
                sys.remove(dev, machine, k)?;
                sys.insert(dev, machine, k_pend(pg, k.1), words(&[addr2]))?;
                self.history_blocks -= if addr2 != 0 { 2 } else { 1 };
            }
        }
        sys.remove(dev, machine, k_snap(p, c, s))?;
        self.dev.stats.dropped += 1;
        Ok(())
    }

    /// Thin history by decision 33: every moment for a day, one an hour
    /// for a week, one a day for a month, one a week after that; then keep
    /// history within its budget (15 % of the disk), oldest first, never
    /// giving up the last day.
    pub fn thin(&mut self, now: u64) -> Result<(), Error> {
        self.ok()?;
        let people = self.people()?;
        for p in people {
            let moments = self.history(p)?;
            let mut keep_bucket: BTreeSet<(u64, u64)> = BTreeSet::new();
            // Newest first, so the newest moment of each bucket is kept.
            for m in moments.iter().rev().skip(1) {
                let age = now.saturating_sub(m.time);
                let bucket = if age < 24 * 3600 {
                    continue;
                } else if age < 7 * 24 * 3600 {
                    (1, m.time / 3600)
                } else if age < 30 * 24 * 3600 {
                    (2, m.time / (24 * 3600))
                } else {
                    (3, m.time / (7 * 24 * 3600))
                };
                if !keep_bucket.insert(bucket) {
                    self.drop_snapshot(p, Class::Documents, m.cn)?;
                }
            }
        }
        let budget = self.space.blocks * HISTORY_BUDGET_PERCENT / 100;
        while self.history_blocks > budget {
            if !self.drop_oldest(now)? {
                break;
            }
        }
        Ok(())
    }

    /// Give up the oldest moment, of anyone, that is more than a day old
    /// and not their newest. False when there is none.
    fn drop_oldest(&mut self, now: u64) -> Result<bool, Error> {
        let mut best: Option<(u64, Person, u64)> = None;
        for p in self.people()? {
            let h = self.history(p)?;
            if h.len() < 2 {
                continue;
            }
            let m = h[0];
            if now.saturating_sub(m.time) < KEEP_SECS {
                continue;
            }
            if best.is_none_or(|(g, _, _)| m.cn < g) {
                best = Some((m.cn, p, m.cn));
            }
        }
        match best {
            Some((_, p, g)) => {
                self.drop_snapshot(p, Class::Documents, g)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Make room on a full disk (choices B and D): empty caches, least
    /// recently used first, then give up the oldest moments more than a
    /// day old until `blocks` are free or none are left, and
    /// commit so the room is really free. True when it made enough room.
    pub fn make_room(&mut self, now: u64, blocks: u64) -> Result<bool, Error> {
        self.ok()?;
        let mut tries = 0;
        loop {
            let target = blocks + self.reserve() + self.need();
            if self.space.free_blocks() >= target {
                return Ok(true);
            }
            let short = target - self.space.free_blocks();
            // Caches go first (choice D), least recently used first.
            let freed = self.free_caches(short)?;
            let held = self.history_blocks;
            let mut dropped = freed > 0;
            while freed + (held - self.history_blocks) < short {
                if !self.drop_oldest(now)? {
                    break;
                }
                dropped = true;
            }
            // Nothing more to give up: one more round frees what is
            // still waiting, then stop.
            if !dropped {
                tries += 1;
                if tries > 1 {
                    return Ok(false);
                }
            }
            // Blocks given up become free two commits later.
            for _ in 0..3 {
                self.commit(now)?;
            }
        }
    }

    // ---- Cleaning (choice D) ----

    /// Today's "last used" bits for a cache inode (0 before the first
    /// commit of this mount, when the date is not known).
    fn today_bits(&self) -> u64 {
        if self.now == 0 {
            0
        } else {
            (self.now / DAY + 1) << USED_SHIFT
        }
    }

    fn stamp(&self, st: &mut Val) {
        let today = self.today_bits();
        if today != 0 {
            let flags = word(st, 4) & ((1 << USED_SHIFT) - 1);
            format::set_word(st, 4, flags | today);
        }
    }

    /// Note that a cache file was used today. Writes its inode at most
    /// once a day, so reading caches costs nearly nothing.
    fn touch_cache(&mut self, p: Person, f: FileId, mut st: Val) -> Result<(), Error> {
        let today = self.today_bits();
        if today != 0 && word(&st, 4) >> USED_SHIFT << USED_SHIFT != today {
            self.stamp(&mut st);
            self.put_inode(p, f, st)?;
        }
        Ok(())
    }

    /// Every non-empty cache file of the people whose files are open:
    /// (day last used, or 0 if not known, person, file, blocks).
    fn cache_files(&mut self) -> Result<Vec<(u64, Person, FileId, u64)>, Error> {
        let mut out = Vec::new();
        let Store { dev, branches, .. } = self;
        for (&p, b) in branches.iter_mut() {
            let t = &b.trees[Class::Caches as usize];
            for (k, v) in t.collect(dev, &b.keys, Key(0, 0), Key(u64::MAX, u64::MAX))? {
                if k.0 & 0xff != K_INODE || k.1 != 0 || word(&v, 4) & F_DIR != 0 {
                    continue;
                }
                let size = word(&v, 0);
                if size == 0 {
                    continue;
                }
                let day = (word(&v, 4) >> USED_SHIFT).saturating_sub(1);
                let f = FileId::new(k.0 >> 8, Class::Caches);
                out.push((day, p, f, size.div_ceil(BLOCK as u64)));
            }
        }
        Ok(out)
    }

    /// Empty the cache files nobody used for `unused_days` days (choice D:
    /// 30). The files stay, empty, so apps simply make them again. Returns
    /// how many blocks that gives back (once committed). Works only on the
    /// branches that are open (a locked person's caches wait for them).
    pub fn clean_caches(&mut self, now: u64, unused_days: u64) -> Result<u64, Error> {
        self.ok()?;
        self.now = self.now.max(now);
        let today = now / DAY;
        let mut freed = 0;
        for (day, p, f, blocks) in self.cache_files()? {
            if day == 0 {
                // Not known yet (made before this mount knew the date):
                // start counting from today.
                let st = self.inode(p, f)?;
                self.touch_cache(p, f, st)?;
            } else if today.saturating_sub(day) >= unused_days {
                self.truncate(p, STORE_APP, f, 0)?;
                freed += blocks;
            }
        }
        Ok(freed)
    }

    /// Empty cache files, least recently used first, until about `blocks`
    /// blocks are given back. Returns how many it gave back.
    fn free_caches(&mut self, blocks: u64) -> Result<u64, Error> {
        let mut files = self.cache_files()?;
        files.sort();
        let mut freed = 0;
        for (_, p, f, n) in files {
            if freed >= blocks {
                break;
            }
            self.truncate(p, STORE_APP, f, 0)?;
            freed += n;
        }
        Ok(freed)
    }

    /// Documents that might be worth cleaning up: at least `min_bytes`
    /// big and not changed for `unused_days` days, biggest first. The
    /// store never removes a person's own files by itself; this list is
    /// for asking them (the question screen comes with the desktop).
    pub fn cleanup_suggestions(
        &mut self,
        p: Person,
        now: u64,
        min_bytes: u64,
        unused_days: u64,
    ) -> Result<Vec<Suggestion>, Error> {
        let moments = self.history(p)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        let t = &b.trees[Class::Documents as usize];
        let mut out = Vec::new();
        for (k, v) in t.collect(dev, &b.keys, Key(0, 0), Key(u64::MAX, u64::MAX))? {
            if k.0 & 0xff != K_INODE || k.1 != 0 || word(&v, 4) & F_DIR != 0 {
                continue;
            }
            let size = word(&v, 0);
            if size < min_bytes {
                continue;
            }
            // When its version was made: the moment of that commit, or
            // the newest kept moment before it (history may be thinned).
            let version = word(&v, 1);
            let Some(m) = moments.iter().rev().find(|m| m.cn <= version) else {
                continue;
            };
            let days = now.saturating_sub(m.time) / DAY;
            if days >= unused_days {
                out.push(Suggestion {
                    file: FileId::new(k.0 >> 8, Class::Documents),
                    size,
                    days_unchanged: days,
                });
            }
        }
        out.sort_by_key(|s| core::cmp::Reverse(s.size));
        Ok(out)
    }

    /// Changes gathered and not yet committed?
    pub fn has_changes(&self) -> bool {
        self.branches.values().any(|b| b.touched)
    }

    // ---- Commit ----

    /// Write everything gathered since the last commit, at once and
    /// safely. `now` is the wall-clock label of this moment (seconds).
    /// Returns the commit number. Once it returns, the commit survives
    /// any power cut.
    pub fn commit(&mut self, now: u64) -> Result<u64, Error> {
        self.ok()?;
        self.now = self.now.max(now);
        match self.commit_inner(now) {
            Ok(g) => {
                self.dev.stats.commits += 1;
                Ok(g)
            }
            Err(e) => {
                self.broken = true;
                Err(e)
            }
        }
    }

    fn commit_inner(&mut self, now: u64) -> Result<u64, Error> {
        let cn = self.cn + 1;
        let spare = self.spare_map();
        let docs_spare = self.copies == Copies::MapAndDocuments;

        // 1. Blocks given up two commits ago are free now: no front page
        // on the disk can still point at them.
        if cn >= 2 {
            let Store {
                dev,
                machine,
                sys,
                space,
                ..
            } = self;
            let pend = sys.collect(dev, machine, k_pend(0, 0), k_pend(cn - 2, u64::MAX))?;
            for (k, v) in pend {
                space.free(k.1);
                if word(&v, 0) != 0 {
                    space.free(word(&v, 0));
                }
                sys.remove(dev, machine, k)?;
            }
        }

        // 2. Each person's changes.
        let people: Vec<Person> = self.branches.keys().copied().collect();
        for p in people {
            self.commit_branch(p, cn, now, spare, docs_spare)?;
        }

        // 3. The system tree and the bitmaps. Placing blocks changes the
        // bitmaps, whose new places change the system tree, so repeat
        // until nothing new needs a place.
        let mut placed_chunks: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
        loop {
            let Store {
                dev,
                machine,
                sys,
                space,
                released_bitmaps,
                ..
            } = self;
            let mut changed = false;
            let released: Vec<Ptr> = core::mem::take(&mut sys.released)
                .into_iter()
                .chain(core::mem::take(released_bitmaps))
                .collect();
            for r in released {
                sys.insert(dev, machine, k_pend(cn, r.addr), words(&[r.addr2]))?;
                changed = true;
            }
            let dirty: Vec<u64> = space.dirty.iter().copied().collect();
            for c in dirty {
                if placed_chunks.contains_key(&c) {
                    continue;
                }
                let (a, a2) = space.alloc(spare)?;
                placed_chunks.insert(c, (a, a2));
                let placeholder = Ptr {
                    addr: a,
                    addr2: a2,
                    birth: cn,
                    mac: [0; 16],
                };
                if let Some(old) = sys.insert(dev, machine, k_space(c), placeholder.encode())? {
                    released_bitmaps.push(Ptr::decode(&old));
                }
                changed = true;
            }
            let n = sys.place(&mut |s| space.alloc(s))?;
            if n > 0 {
                changed = true;
            }
            if !changed && self.released_bitmaps.is_empty() && self.sys.released.is_empty() {
                break;
            }
        }

        // 4. Now that every place is chosen, the bitmaps are final.
        let mem_before = self.sys.mem;
        for (&c, &(a, a2)) in &placed_chunks {
            let mut b = [0u8; BLOCK];
            self.space.chunk_bytes(c, &mut b);
            let p = self.dev.write_block(&self.machine, a, a2, cn, &mut b)?;
            self.sys
                .insert(&mut self.dev, &self.machine, k_space(c), p.encode())?;
        }
        assert!(
            self.sys.mem == mem_before && self.sys.released.is_empty(),
            "bitmap update changed the tree's shape"
        );
        let Store {
            dev, machine, sys, ..
        } = self;
        let root = sys.write(dev, machine, cn, &mut |_| {
            panic!("system tree block without a place")
        })?;

        // 5. Everything is written; make it safe, then switch.
        self.dev.disk.flush()?;
        self.write_front(cn, root)?;
        self.cn = cn;
        self.space.dirty.clear();
        Ok(cn)
    }

    fn commit_branch(
        &mut self,
        p: Person,
        cn: u64,
        now: u64,
        spare: bool,
        docs_spare: bool,
    ) -> Result<(), Error> {
        let Store {
            dev,
            machine,
            sys,
            space,
            branches,
            history_blocks,
            ..
        } = self;
        let b = branches.get_mut(&p).unwrap();
        if !b.touched {
            return Ok(());
        }
        // File blocks.
        let dirty = core::mem::take(&mut b.dirty);
        for ((f, idx), mut blk) in dirty {
            let s = docs_spare && f.class() == Class::Documents;
            let (a, a2) = space.alloc(s)?;
            let ptr = dev.write_block(&b.keys, a, a2, cn, &mut blk)?;
            let t = &mut b.trees[f.class() as usize];
            // The file may have been deleted after the write.
            if t.get(dev, &b.keys, k_inode(f.obj()))?.is_none() {
                t.released.push(ptr);
                continue;
            }
            if let Some(old) = t.insert(dev, &b.keys, k_data(f.obj(), idx), ptr.encode())? {
                t.released.push(Ptr::decode(&old));
            }
        }
        // The three class trees.
        let mut roots = [Ptr::default(); 3];
        for c in Class::ALL {
            roots[c as usize] =
                b.trees[c as usize].write(dev, &b.keys, cn, &mut |s| space.alloc(s))?;
        }
        // History: the documents root, what changed, what was read.
        if b.docs_changed {
            b.hist
                .insert(dev, &b.keys, k_hist(cn, H_ROOT, 0), roots[0].encode())?;
            sys.insert(dev, machine, k_snap(p, Class::Documents, cn), words(&[now]))?;
        }
        if b.apps_snapshot {
            b.hist
                .insert(dev, &b.keys, k_hist(cn, H_APPS, 0), roots[1].encode())?;
        }
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
        let hist_root = b.hist.write(dev, &b.keys, cn, &mut |s| space.alloc(s))?;

        // The person page.
        let mut page = [0u8; BLOCK];
        format::set_word(&mut page, 0, PERSON_MAGIC);
        format::set_word(&mut page, 1, p);
        format::set_word(&mut page, 2, b.next_obj);
        for (i, r) in roots.iter().chain([&hist_root]).enumerate() {
            page[64 + 40 * i..104 + 40 * i].copy_from_slice(&r.encode());
        }
        let (a, a2) = space.alloc(spare)?;
        let ptr = dev.write_block(&b.keys, a, a2, cn, &mut page)?;
        let old_page = core::mem::replace(&mut b.page, ptr);
        sys.insert(dev, machine, k_person(p), ptr.encode())?;

        // What the commit gave up: kept for history if a kept moment still
        // needs it, otherwise freed two commits from now.
        let mut pend: Vec<Ptr> = Vec::new();
        if !old_page.is_null() {
            pend.push(old_page);
        }
        pend.append(&mut b.hist.released);
        pend.append(&mut b.trees[Class::Caches as usize].released);
        for c in [Class::Documents, Class::Apps] {
            for r in core::mem::take(&mut b.trees[c as usize].released) {
                let needed = r.birth < cn
                    && sys
                        .last_in(dev, machine, k_snap(p, c, r.birth), k_snap(p, c, cn - 1))?
                        .is_some();
                if needed {
                    sys.insert(
                        dev,
                        machine,
                        k_dead(p, c, cn, r.addr),
                        words(&[r.birth, r.addr2]),
                    )?;
                    *history_blocks += r.count();
                } else {
                    pend.push(r);
                }
            }
        }
        for r in pend {
            sys.insert(dev, machine, k_pend(cn, r.addr), words(&[r.addr2]))?;
        }
        let snap_apps = b.apps_snapshot;
        b.touched = false;
        b.docs_changed = false;
        b.apps_snapshot = false;
        if snap_apps {
            // Only one copy from before an update: the new one replaces it.
            let old: Vec<u64> = self
                .snapshots(p, Class::Apps)?
                .iter()
                .map(|m| m.cn)
                .collect();
            for g in old {
                self.drop_snapshot(p, Class::Apps, g)?;
            }
            self.sys.insert(
                &mut self.dev,
                &self.machine,
                k_snap(p, Class::Apps, cn),
                words(&[now]),
            )?;
        }
        Ok(())
    }

    // ---- Checking ----

    /// Read and check everything reachable: every fingerprint, every
    /// block's place in the bitmap, the history records. Needs every
    /// person unlocked to check their files.
    pub fn check(&mut self) -> Result<Report, Error> {
        let mut r = Report::default();
        self.dev.uncached = true;
        let res = self.check_inner(&mut r);
        self.dev.uncached = false;
        res?;
        Ok(r)
    }

    fn check_inner(&mut self, r: &mut Report) -> Result<(), Error> {
        if self.has_changes() {
            return Err(Error::BadRequest);
        }
        let mut live: BTreeSet<u64> = BTreeSet::new();
        let mut past: BTreeSet<u64> = BTreeSet::new();
        let mark = |set: &mut BTreeSet<u64>, p: &Ptr| {
            set.insert(p.addr);
            if p.addr2 != 0 {
                set.insert(p.addr2);
            }
        };
        mark(
            &mut live,
            &Ptr {
                addr: FRONT[0],
                ..Default::default()
            },
        );
        mark(
            &mut live,
            &Ptr {
                addr: FRONT[1],
                ..Default::default()
            },
        );
        let root = self.sys.root_ptr();
        let mut sys_nodes = Vec::new();
        Tree::walk(&mut self.dev, &self.machine, &root, &mut |p, n| {
            sys_nodes.push((*p, n.clone()))
        })?;
        let mut people = Vec::new();
        let mut dead_records = Vec::new();
        let mut pend_records = Vec::new();
        for (p, n) in &sys_nodes {
            mark(&mut live, p);
            if n.level != 0 {
                continue;
            }
            for (k, v) in n.keys.iter().zip(&n.vals) {
                match k.0 >> 56 {
                    T_SPACE => {
                        let bp = Ptr::decode(v);
                        let mut b = [0u8; BLOCK];
                        self.dev.read_block(&self.machine, &bp, &mut b)?;
                        mark(&mut live, &bp);
                    }
                    T_PERSON => people.push((k.0 & PERSON_MAX, Ptr::decode(v))),
                    T_DEAD => dead_records.push((*k, *v)),
                    T_PEND => pend_records.push((*k, *v)),
                    _ => {}
                }
            }
        }
        r.people = people.len() as u64;
        let mut dead_blocks = 0;
        for (k, v) in &dead_records {
            let p = Ptr {
                addr: k.1,
                addr2: word(v, 1),
                ..Default::default()
            };
            dead_blocks += p.count();
        }
        if dead_blocks != self.history_blocks {
            r.problems.push(Problem::HistoryCount);
        }
        for (pid, page) in people {
            mark(&mut live, &page);
            let Some(b) = self.branches.get(&pid) else {
                r.locked += 1;
                continue;
            };
            let keys = b.keys.clone();
            let mut blk = [0u8; BLOCK];
            self.dev.read_block(&keys, &page, &mut blk)?;
            let roots: Vec<Ptr> = (0..4)
                .map(|i| Ptr::decode(blk[64 + 40 * i..104 + 40 * i].try_into().unwrap()))
                .collect();
            for (i, root) in roots.iter().enumerate() {
                // The history tree (the fourth) holds records, not files.
                self.walk_files(&keys, root, &mut live, r, i < 3)?;
            }
            // Every moment history keeps must be whole.
            let hist = Tree::open(roots[3], false);
            for (c, kind) in [(Class::Documents, H_ROOT), (Class::Apps, H_APPS)] {
                for m in self.snapshots(pid, c)? {
                    let v = hist
                        .get(&mut self.dev, &keys, k_hist(m.cn, kind, 0))?
                        .ok_or(Error::NoSuchMoment)?;
                    self.walk_files(&keys, &Ptr::decode(&v), &mut past, r, true)?;
                    r.moments += 1;
                }
            }
        }
        // Every block kept for history must be needed by a kept moment.
        for (k, v) in &dead_records {
            let p = Ptr {
                addr: k.1,
                addr2: word(v, 1),
                ..Default::default()
            };
            if r.locked == 0 && !past.contains(&p.addr) {
                r.problems.push(Problem::UnneededHistory(p.addr));
            }
            if live.contains(&p.addr) {
                r.problems.push(Problem::DeadButLive(p.addr));
            }
            mark(&mut past, &p);
        }
        for (k, v) in &pend_records {
            let p = Ptr {
                addr: k.1,
                addr2: word(v, 0),
                ..Default::default()
            };
            if live.contains(&p.addr) || past.contains(&p.addr) {
                r.problems.push(Problem::FreedButUsed(p.addr));
            }
            mark(&mut live, &p);
        }
        let all: BTreeSet<u64> = live.union(&past).copied().collect();
        for &a in &all {
            if !self.space.is_used(a) {
                r.problems.push(Problem::NotMarked(a));
            }
        }
        if r.locked == 0 {
            let mut used = 0;
            for a in 0..self.space.blocks {
                if self.space.is_used(a) {
                    used += 1;
                    if !all.contains(&a) {
                        r.problems.push(Problem::Leaked(a));
                    }
                }
            }
            if used != self.space.used {
                r.problems.push(Problem::UsedCount);
            }
        }
        r.blocks = all.len() as u64;
        Ok(())
    }

    fn walk_files(
        &mut self,
        keys: &Keys,
        root: &Ptr,
        set: &mut BTreeSet<u64>,
        r: &mut Report,
        files: bool,
    ) -> Result<(), Error> {
        let mut data = Vec::new();
        Tree::walk(&mut self.dev, keys, root, &mut |p, n| {
            set.insert(p.addr);
            if p.addr2 != 0 {
                set.insert(p.addr2);
            }
            if n.level == 0 && files {
                for (k, v) in n.keys.iter().zip(&n.vals) {
                    if k.0 & 0xff == K_DATA {
                        data.push(Ptr::decode(v));
                    }
                }
            }
        })?;
        let mut b = [0u8; BLOCK];
        for p in data {
            // Shared with the live tree or another moment: checked once.
            if set.contains(&p.addr) {
                continue;
            }
            set.insert(p.addr);
            if p.addr2 != 0 {
                set.insert(p.addr2);
            }
            match self.dev.read_block(keys, &p, &mut b) {
                Ok(()) => r.data_blocks += 1,
                Err(Error::Damaged(a)) => r.problems.push(Problem::Damaged(a)),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// A read-only view of a past moment.
#[derive(Clone, Copy, Debug)]
pub struct Past {
    pub person: Person,
    pub class: Class,
    pub cn: u64,
    root: Ptr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Problem {
    /// In use but not marked in the bitmap.
    NotMarked(u64),
    /// Marked in the bitmap but used by nothing.
    Leaked(u64),
    /// Kept for history that no kept moment needs.
    UnneededHistory(u64),
    DeadButLive(u64),
    FreedButUsed(u64),
    Damaged(u64),
    HistoryCount,
    UsedCount,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub blocks: u64,
    pub data_blocks: u64,
    pub moments: u64,
    pub people: u64,
    pub locked: u64,
    pub problems: Vec<Problem>,
}

fn stat_of(v: &Val) -> Stat {
    Stat {
        size: word(v, 0),
        version: word(v, 1),
        created: word(v, 2),
        parent: word(v, 3),
        is_dir: word(v, 4) & F_DIR != 0,
        edits: 0,
    }
}

fn find_in(
    t: &Tree,
    io: &mut dyn Io,
    keys: &Keys,
    dir: u64,
    name: &[u8],
) -> Result<Option<(Key, Val)>, Error> {
    let h = name_hash(name);
    let mut found = None;
    t.range(
        io,
        keys,
        k_dirent(dir, h),
        k_dirent(dir, h | 0xff),
        &mut |k, v| {
            let n = v[8..].iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
            if &v[8..8 + n] == name {
                found = Some((k, *v));
                false
            } else {
                true
            }
        },
    )?;
    Ok(found)
}

fn list_in(
    t: &Tree,
    io: &mut dyn Io,
    keys: &Keys,
    dir: FileId,
) -> Result<Vec<(Vec<u8>, FileId)>, Error> {
    let mut out = Vec::new();
    t.range(
        io,
        keys,
        k_dirent(dir.obj(), 0),
        k_dirent(dir.obj(), u64::MAX),
        &mut |_, v| {
            let n = v[8..].iter().position(|&b| b == 0).unwrap_or(NAME_MAX);
            out.push((v[8..8 + n].to_vec(), FileId::new(word(v, 0), dir.class())));
            true
        },
    )?;
    out.sort();
    Ok(out)
}

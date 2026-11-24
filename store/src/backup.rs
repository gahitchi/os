// SPDX-License-Identifier: GPL-3.0-only
//! Backup (milestone 7, step 2; decision 36): a person's documents to an
//! external drive (a "stick"), locked end to end, only what changed since
//! the last time, and restorable on a fresh Alaya with the person's
//! recovery key.
//!
//! **The recovery key.** Keys never leave the machine, so a backup is
//! locked with a key made from a recovery key: 20 random bytes the person
//! is shown once, as 32 letters and digits (`ABCD-EFGH-...`), to write
//! down. The machine keeps it with the person's app files, locked with the
//! person's key like everything else.
//!
//! **The stick's layout** (blocks of 4 KiB). Every block carries its own
//! 16-byte seal in its last bytes, made with its place on the stick and
//! the run that wrote it, so a block moved, replayed or changed does not
//! open.
//!
//! - Blocks 0 and 1: two copies of the header, used in turn (the newest
//!   that opens wins, so a run cut off half way leaves the last one).
//!   Plain: the magic, the stick's salt and the run number. Locked: the
//!   person, the last moment backed up, where the records end, and the
//!   key table's place and length.
//! - Two key tables, used in turn: each document's own backup key. A
//!   document's records are locked with its own key, so **forgetting a
//!   document makes it unreadable on the stick too**: the next run leaves
//!   its key out of the table and writes both tables, so no copy of the
//!   key is left.
//! - Then the records, only ever added: a folder (locked with the backup
//!   key: its name and place), a document (locked with its own key: its
//!   name, place and contents), or "gone" (a document or folder removed
//!   since).
//!
//! **What changed** is known from the machine: for every document and
//! folder it keeps what the stick last got (version, changes, folder,
//! name). A different stick gets everything.
//!
//! **Restoring** reads the newest header, the key table and every record
//! in order, and makes the folders and documents again as they were at
//! the last run, in a folder of their own when the person's documents are
//! not empty.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::crypto::{Hmac, Keys, sha256};
use crate::disk::Disk;
use crate::format::{BLOCK, Block, Key, Val, word, words};
use crate::{Class, Error, FileId, NAME_MAX, Person, ROOT_OBJ, Store};

const MAGIC: u64 = 0x314b_4241_5941_4c41; // "ALAYABK1"
/// The seal at the end of every block.
const SEAL: usize = 16;
/// What a block holds besides its seal.
const PAYLOAD: usize = BLOCK - SEAL;
/// Bytes of a record's first block before its contents start.
const META: usize = 128;
/// Plain bytes at the start of a record's first block: kind, document,
/// blocks.
const PLAIN: usize = 24;
/// One key table: blocks (each entry 40 bytes: document, key).
const TABLE_BLOCKS: u64 = 32;
const PER_TABLE_BLOCK: usize = PAYLOAD / 40;
/// Where the records start.
const RECORDS_AT: u64 = 2 + 2 * TABLE_BLOCKS;

mod kind {
    pub const FOLDER: u64 = 1;
    pub const DOC: u64 = 2;
    pub const GONE: u64 = 3;
    /// Everything before is out of date: a whole run follows.
    pub const AFRESH: u64 = 4;
}

/// The machine's notes, in the person's app files (no history).
const K_BACKUP: u64 = 6;
const K_RECOVERY: u64 = 7;
fn k_backup(obj: u64) -> Key {
    Key(crate::grants::RECORDS | K_BACKUP, obj)
}
fn k_recovery() -> Key {
    Key(crate::grants::RECORDS | K_RECOVERY, 0)
}

/// A recovery key, as the person writes it down.
pub type Recovery = [u8; 20];

const LETTERS: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// `ABCD-EFGH-JKLM-NPQR-STUV-WXYZ-2345-6789`: 32 letters and digits (no
/// I, O, 0 or 1, which are easy to misread).
pub fn recovery_text(r: &Recovery) -> [u8; 39] {
    let mut out = [b'-'; 39];
    let mut acc = 0u64;
    let mut bits = 0;
    let mut n = 0;
    for &b in r {
        acc = acc << 8 | u64::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            let c = LETTERS[(acc >> bits & 31) as usize];
            out[n + n / 4] = c;
            n += 1;
        }
    }
    out
}

/// Read a recovery key as typed: case, spaces and dashes do not matter.
pub fn recovery_from(text: &[u8]) -> Option<Recovery> {
    let mut r = [0u8; 20];
    let mut acc = 0u64;
    let mut bits = 0;
    let mut n = 0;
    for &c in text {
        let c = c.to_ascii_uppercase();
        if c == b'-' || c == b' ' || c == 0 {
            continue;
        }
        let v = LETTERS.iter().position(|&l| l == c)? as u64;
        acc = acc << 5 | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if n == 20 {
                return None;
            }
            r[n] = (acc >> bits) as u8;
            n += 1;
        }
    }
    (n == 20).then_some(r)
}

/// What one run did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ran {
    pub run: u64,
    /// Documents and folders written (changed since the last run).
    pub written: u64,
    /// Removed since (and forgotten).
    pub gone: u64,
    /// Blocks used on the stick, all runs together.
    pub used: u64,
    /// Bytes of documents written this time.
    pub bytes: u64,
}

/// What a restore made.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Restored {
    pub folders: u64,
    pub documents: u64,
    /// The folder it went into (the documents root when it was empty).
    pub into: u64,
    /// The run and the moment it came from.
    pub run: u64,
    pub moment: u64,
}

/// The stick's newest header.
#[derive(Clone, Copy, Debug, Default)]
struct Head {
    run: u64,
    salt: [u8; 32],
    person: u64,
    moment: u64,
    end: u64,
    table_len: u64,
}

fn backup_keys(r: &Recovery, salt: &[u8; 32]) -> Keys {
    let s = Hmac::new(r).mac(&[b"alaya backup", salt]);
    Keys::new(&s)
}

/// Seal `b`'s payload with `k`, for place `at` and run `run`.
fn seal(k: &Keys, at: u64, run: u64, b: &mut Block, plain: usize) {
    let tag = k.seal(at, run, &mut b[plain..PAYLOAD]);
    b[PAYLOAD..].copy_from_slice(&tag);
}

fn open(k: &Keys, at: u64, run: u64, b: &mut Block, plain: usize) -> bool {
    let tag: [u8; 16] = b[PAYLOAD..].try_into().unwrap();
    k.open(at, run, &tag, &mut b[plain..PAYLOAD])
}

fn io(e: crate::disk::IoError) -> Error {
    Error::Io(e)
}

/// The run a record block was written by is not known when reading in
/// order, so record blocks are sealed with run 0 and their place: places
/// are never written twice.
const RECORD_RUN: u64 = 0;

fn read_head<S: Disk>(stick: &mut S, r: &Recovery) -> Result<Option<Head>, Error> {
    let mut best: Option<Head> = None;
    let mut seen_magic = false;
    for at in 0..2 {
        let mut b = [0u8; BLOCK];
        stick.read(at, &mut b).map_err(io)?;
        if word(&b, 0) != MAGIC {
            continue;
        }
        seen_magic = true;
        let run = word(&b, 1);
        let salt: [u8; 32] = b[16..48].try_into().unwrap();
        let k = backup_keys(r, &salt);
        if !open(&k, at, run, &mut b, 48) {
            continue;
        }
        let h = Head {
            run,
            salt,
            person: word(&b, 6),
            moment: word(&b, 7),
            end: word(&b, 8),
            table_len: word(&b, 9),
        };
        if best.is_none_or(|x| h.run > x.run) {
            best = Some(h);
        }
    }
    if seen_magic && best.is_none() {
        // A backup is there, but this key does not open it.
        return Err(Error::WrongKey);
    }
    Ok(best)
}

fn write_head<S: Disk>(stick: &mut S, r: &Recovery, h: &Head) -> Result<(), Error> {
    let at = h.run % 2;
    let mut b = [0u8; BLOCK];
    b[..8].copy_from_slice(&MAGIC.to_le_bytes());
    b[8..16].copy_from_slice(&h.run.to_le_bytes());
    b[16..48].copy_from_slice(&h.salt);
    for (i, w) in [h.person, h.moment, h.end, h.table_len].iter().enumerate() {
        b[48 + 8 * i..56 + 8 * i].copy_from_slice(&w.to_le_bytes());
    }
    seal(&backup_keys(r, &h.salt), at, h.run, &mut b, 48);
    stick.write(at, &b).map_err(io)?;
    stick.flush().map_err(io)
}

fn table_at(run: u64) -> u64 {
    2 + (run % 2) * TABLE_BLOCKS
}

fn write_table<S: Disk>(
    stick: &mut S,
    k: &Keys,
    run: u64,
    slot: u64,
    t: &[(u64, [u8; 32])],
) -> Result<(), Error> {
    if t.len() > PER_TABLE_BLOCK * TABLE_BLOCKS as usize {
        return Err(Error::Full);
    }
    let base = table_at(slot);
    for i in 0..TABLE_BLOCKS {
        let mut b = [0u8; BLOCK];
        let from = i as usize * PER_TABLE_BLOCK;
        for (j, (obj, key)) in t.iter().skip(from).take(PER_TABLE_BLOCK).enumerate() {
            b[40 * j..40 * j + 8].copy_from_slice(&obj.to_le_bytes());
            b[40 * j + 8..40 * j + 40].copy_from_slice(key);
        }
        // Every block of the table is written, so an older table's keys
        // are overwritten even where this one is shorter.
        seal(k, base + i, run, &mut b, 0);
        stick.write(base + i, &b).map_err(io)?;
    }
    Ok(())
}

fn read_table<S: Disk>(
    stick: &mut S,
    k: &Keys,
    h: &Head,
) -> Result<BTreeMap<u64, [u8; 32]>, Error> {
    let base = table_at(h.run);
    let mut t = BTreeMap::new();
    let mut left = h.table_len as usize;
    for i in 0..TABLE_BLOCKS {
        if left == 0 {
            break;
        }
        let mut b = [0u8; BLOCK];
        stick.read(base + i, &mut b).map_err(io)?;
        if !open(k, base + i, h.run, &mut b, 0) {
            return Err(Error::Damaged(base + i));
        }
        for j in 0..left.min(PER_TABLE_BLOCK) {
            t.insert(
                word(&b, 5 * j),
                b[40 * j + 8..40 * j + 40].try_into().unwrap(),
            );
        }
        left -= left.min(PER_TABLE_BLOCK);
    }
    Ok(t)
}

/// One record as written: what it is about and its meta words.
struct Rec {
    kind: u64,
    obj: u64,
    parent: u64,
    name: Vec<u8>,
    data: Vec<u8>,
}

/// Write a record at `at`; the next free place.
fn put_record<S: Disk>(stick: &mut S, k: &Keys, at: u64, r: &Rec) -> Result<u64, Error> {
    let first = PAYLOAD - META;
    let rest = r.data.len().saturating_sub(first);
    let blocks = 1 + rest.div_ceil(PAYLOAD) as u64;
    if at + blocks > stick.blocks() {
        return Err(Error::Full);
    }
    let mut b = [0u8; BLOCK];
    for (i, w) in [r.kind, r.obj, blocks].iter().enumerate() {
        b[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
    }
    for (i, w) in [r.parent, r.data.len() as u64, r.name.len() as u64]
        .iter()
        .enumerate()
    {
        b[PLAIN + 8 * i..PLAIN + 8 * i + 8].copy_from_slice(&w.to_le_bytes());
    }
    b[PLAIN + 24..PLAIN + 24 + r.name.len()].copy_from_slice(&r.name);
    let n = r.data.len().min(first);
    b[META..META + n].copy_from_slice(&r.data[..n]);
    seal(k, at, RECORD_RUN, &mut b, PLAIN);
    stick.write(at, &b).map_err(io)?;
    for (i, chunk) in r.data[n..].chunks(PAYLOAD).enumerate() {
        let mut b = [0u8; BLOCK];
        b[..chunk.len()].copy_from_slice(chunk);
        let place = at + 1 + i as u64;
        seal(k, place, RECORD_RUN, &mut b, 0);
        stick.write(place, &b).map_err(io)?;
    }
    Ok(at + blocks)
}

/// The record at `at`, if its key is known and it opens; and where the
/// next one starts.
fn get_record<S: Disk>(
    stick: &mut S,
    at: u64,
    key_of: impl Fn(u64, u64) -> Option<Keys>,
) -> Result<(Option<Rec>, u64), Error> {
    let mut b = [0u8; BLOCK];
    stick.read(at, &mut b).map_err(io)?;
    let (kind, obj, blocks) = (word(&b, 0), word(&b, 1), word(&b, 2));
    if blocks == 0 || blocks > stick.blocks() {
        return Err(Error::Damaged(at));
    }
    let next = at + blocks;
    let Some(k) = key_of(kind, obj) else {
        // Forgotten: its key is gone, so it is skipped unread.
        return Ok((None, next));
    };
    if !open(&k, at, RECORD_RUN, &mut b, PLAIN) {
        return Err(Error::Damaged(at));
    }
    let parent = word(&b[PLAIN..], 0);
    let len = word(&b[PLAIN..], 1) as usize;
    let nlen = (word(&b[PLAIN..], 2) as usize).min(NAME_MAX);
    let name = b[PLAIN + 24..PLAIN + 24 + nlen].to_vec();
    let mut data = Vec::with_capacity(len);
    let first = (PAYLOAD - META).min(len);
    data.extend_from_slice(&b[META..META + first]);
    let mut place = at + 1;
    while data.len() < len {
        stick.read(place, &mut b).map_err(io)?;
        if !open(&k, place, RECORD_RUN, &mut b, 0) {
            return Err(Error::Damaged(place));
        }
        let take = (len - data.len()).min(PAYLOAD);
        data.extend_from_slice(&b[..take]);
        place += 1;
    }
    Ok((
        Some(Rec {
            kind,
            obj,
            parent,
            name,
            data,
        }),
        next,
    ))
}

/// What the machine remembers of one document or folder on the stick:
/// version, changes, folder, the name's fingerprint, the stick.
fn note_of(version: u64, edits: u64, parent: u64, name: &[u8], stick: u64) -> Val {
    let h = sha256(name);
    words(&[
        version,
        edits,
        parent,
        u64::from_le_bytes(h[..8].try_into().unwrap()),
        stick,
    ])
}

/// Look for `needle` anywhere on `stick`, as raw bytes (tests: nothing
/// of a document may be there in the clear).
pub fn raw_find<S: Disk>(stick: &mut S, needle: &[u8]) -> Result<bool, Error> {
    if needle.is_empty() || needle.len() > 64 {
        return Err(Error::BadRequest);
    }
    let mut b = [0u8; BLOCK];
    let mut carry: Vec<u8> = Vec::new();
    for a in 0..stick.blocks() {
        stick.read(a, &mut b).map_err(io)?;
        carry.extend_from_slice(&b);
        if carry.windows(needle.len()).any(|w| w == needle) {
            return Ok(true);
        }
        let keep = carry.len() - (needle.len() - 1);
        carry.drain(..keep);
    }
    Ok(false)
}

impl<D: Disk> Store<D> {
    /// What the machine knows of person `p`'s last backup: the stick's
    /// number, its run, and the moment it covered.
    pub fn last_backup(&mut self, p: Person) -> Result<Option<(u64, u64, u64)>, Error> {
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        Ok(b.trees[Class::Apps as usize]
            .get(dev, &b.keys, k_backup(0))?
            .map(|v| (word(&v, 0), word(&v, 1), word(&v, 2))))
    }

    /// The person's recovery key, made the first time it is asked for.
    /// The second value is true when it was just made (show it to the
    /// person, once).
    pub fn recovery_key(&mut self, p: Person) -> Result<(Recovery, bool), Error> {
        self.ok()?;
        let old = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            b.trees[Class::Apps as usize].get(dev, &b.keys, k_recovery())?
        };
        if let Some(v) = old {
            return Ok((v[..20].try_into().unwrap(), false));
        }
        let r: Recovery = self.rng.next(b"recovery key")[..20].try_into().unwrap();
        self.room(2, false)?;
        let mut v = [0u8; 40];
        v[..20].copy_from_slice(&r);
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        b.trees[Class::Apps as usize].insert(dev, &b.keys, k_recovery(), v)?;
        b.touched = true;
        Ok((r, true))
    }

    /// Back up person `p`'s documents to `stick`: only what changed since
    /// the stick's last run. A stick that holds another backup (another
    /// person's, or one this key does not open) is refused unless `erase`:
    /// then it starts again, empty.
    pub fn backup<S: Disk>(&mut self, p: Person, stick: &mut S, erase: bool) -> Result<Ran, Error> {
        self.ok()?;
        let (r, _) = self.recovery_key(p)?;
        if stick.blocks() < RECORDS_AT + 8 {
            return Err(Error::Full);
        }
        let found = match read_head(stick, &r) {
            Err(Error::WrongKey) if erase => None,
            other => other?,
        };
        let head = match found {
            Some(h) if h.person == p && !erase => h,
            Some(_) if !erase => return Err(Error::Exists),
            _ => {
                // A new stick: everything goes on it.
                let mut salt = [0u8; 32];
                salt.copy_from_slice(&self.rng.next(b"backup salt"));
                Head {
                    run: 0,
                    salt,
                    person: p,
                    moment: 0,
                    end: RECORDS_AT,
                    table_len: 0,
                }
            }
        };
        let id = u64::from_le_bytes(head.salt[..8].try_into().unwrap());
        let bk = backup_keys(&r, &head.salt);
        let run = head.run + 1;
        let mut ran = Ran {
            run,
            ..Ran::default()
        };
        // Everything there is now.
        let mut now = BTreeMap::new();
        let mut dirs = alloc::vec![self.root(Class::Documents)];
        while let Some(d) = dirs.pop() {
            for (name, f) in self.list(p, d)? {
                let st = self.stat(p, f)?;
                if st.is_dir {
                    dirs.push(f);
                }
                now.insert(f.obj(), (f, d, name, st));
            }
        }
        // What the machine says this stick already has.
        let had: BTreeMap<u64, Val> = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            b.trees[Class::Apps as usize]
                .collect(dev, &b.keys, k_backup(0), k_backup(u64::MAX))?
                .into_iter()
                .collect::<Vec<_>>()
                .into_iter()
                .filter(|(k, v)| k.1 != 0 && word(v, 4) == id)
                .map(|(k, v)| (k.1, v))
                .collect()
        };
        // The machine's notes count only if this stick got the run they
        // were made with (not, say, an older copy of it).
        let last = {
            let Store { dev, branches, .. } = self;
            let b = branches.get_mut(&p).ok_or(Error::Locked)?;
            b.trees[Class::Apps as usize].get(dev, &b.keys, k_backup(0))?
        };
        let had = match last {
            Some(v) if word(&v, 0) == id && word(&v, 1) == head.run => had,
            _ => BTreeMap::new(),
        };
        let mut end = head.end;
        if had.is_empty() && head.run > 0 {
            // The stick missed what the machine thought it had: a whole
            // run, and restoring starts from here.
            let rec = Rec {
                kind: kind::AFRESH,
                obj: 0,
                parent: 0,
                name: Vec::new(),
                data: Vec::new(),
            };
            end = put_record(stick, &bk, end, &rec)?;
        }
        let mut table: Vec<(u64, [u8; 32])> = Vec::new();
        let mut notes: Vec<(u64, Option<Val>)> = Vec::new();
        // Folders first, then documents, each folder before what is in it
        // (restore makes them in this order too).
        let mut order: Vec<_> = now.values().cloned().collect();
        order.sort_by_key(|(f, _, _, st)| (!st.is_dir, st.created, f.obj()));
        for (f, d, name, st) in order {
            let note = note_of(st.version, st.edits, d.obj(), &name, id);
            let fk = if st.is_dir {
                None
            } else {
                let k = self
                    .file_keys(p, f.obj())?
                    .mac(&[b"backup key", &head.salt]);
                table.push((f.obj(), k));
                Some(k)
            };
            if had.get(&f.obj()) == Some(&note) {
                continue;
            }
            let data = match fk {
                None => Vec::new(),
                Some(_) => {
                    let mut buf = alloc::vec![0u8; st.size as usize];
                    let mut blk = [0u8; BLOCK];
                    let mut off = 0;
                    while off < buf.len() {
                        let take = (buf.len() - off).min(BLOCK);
                        self.read_block_of(p, f, (off / BLOCK) as u64, &mut blk)?;
                        buf[off..off + take].copy_from_slice(&blk[..take]);
                        off += take;
                    }
                    buf
                }
            };
            ran.bytes += data.len() as u64;
            let rec = Rec {
                kind: if st.is_dir { kind::FOLDER } else { kind::DOC },
                obj: f.obj(),
                parent: d.obj(),
                name,
                data,
            };
            let k = fk.map_or_else(|| bk.clone(), |k| Keys::new(&k));
            end = put_record(stick, &k, end, &rec)?;
            ran.written += 1;
            notes.push((f.obj(), Some(note)));
        }
        // What is gone since (removed, or forgotten).
        let mut forgot = false;
        for &obj in had.keys() {
            if now.contains_key(&obj) {
                continue;
            }
            let rec = Rec {
                kind: kind::GONE,
                obj,
                parent: 0,
                name: Vec::new(),
                data: Vec::new(),
            };
            end = put_record(stick, &bk, end, &rec)?;
            ran.gone += 1;
            forgot = true;
            notes.push((obj, None));
        }
        stick.flush().map_err(io)?;
        // The key table, then the header that names it: a run cut off
        // before the header leaves the last run whole.
        write_table(stick, &bk, run, run, &table)?;
        stick.flush().map_err(io)?;
        let h = Head {
            run,
            salt: head.salt,
            person: p,
            moment: self.last_commit(),
            end,
            table_len: table.len() as u64,
        };
        write_head(stick, &r, &h)?;
        if forgot {
            // No older table may keep a key that is gone: the other table
            // gets this one's keys too (the older header no longer opens
            // its table, which is fine: this header is newer).
            write_table(stick, &bk, run + 1, run + 1, &table)?;
            stick.flush().map_err(io)?;
        }
        // Only now does the machine note what the stick has.
        self.room(notes.len() as u64 / 8 + 2, false)?;
        let Store { dev, branches, .. } = self;
        let b = branches.get_mut(&p).ok_or(Error::Locked)?;
        for (obj, v) in notes {
            match v {
                Some(v) => {
                    b.trees[Class::Apps as usize].insert(dev, &b.keys, k_backup(obj), v)?;
                }
                None => {
                    b.trees[Class::Apps as usize].remove(dev, &b.keys, k_backup(obj))?;
                }
            }
        }
        let moment = h.moment;
        b.trees[Class::Apps as usize].insert(
            dev,
            &b.keys,
            k_backup(0),
            words(&[id, run, moment]),
        )?;
        b.touched = true;
        ran.used = end;
        Ok(ran)
    }

    /// Make person `p`'s documents again from `stick`, opened with the
    /// recovery key `r`. They go into the documents root when it is empty,
    /// else into a new folder called `into` there.
    pub fn restore_backup<S: Disk>(
        &mut self,
        p: Person,
        app: crate::App,
        stick: &mut S,
        r: &Recovery,
        into: &[u8],
    ) -> Result<Restored, Error> {
        self.ok()?;
        let h = read_head(stick, r)?.ok_or(Error::NotFound)?;
        let bk = backup_keys(r, &h.salt);
        let table = read_table(stick, &bk, &h)?;
        // Every record in order: the newest of each wins; "gone" removes.
        let mut last: BTreeMap<u64, Rec> = BTreeMap::new();
        let mut at = RECORDS_AT;
        while at < h.end {
            let (rec, next) = get_record(stick, at, |kind, obj| match kind {
                kind::DOC => table.get(&obj).map(Keys::new),
                _ => Some(bk.clone()),
            })?;
            at = next;
            let Some(rec) = rec else { continue };
            match rec.kind {
                kind::GONE => {
                    last.remove(&rec.obj);
                }
                kind::AFRESH => last.clear(),
                _ => {
                    last.insert(rec.obj, rec);
                }
            }
        }
        // Documents whose key is not in the table were forgotten.
        last.retain(|obj, r| r.kind != kind::DOC || table.contains_key(obj));
        let docs = self.root(Class::Documents);
        let base = if self.list(p, docs)?.is_empty() {
            docs
        } else {
            self.create(p, app, docs, into, true)?
        };
        let mut done = Restored {
            into: base.0,
            run: h.run,
            moment: h.moment,
            ..Restored::default()
        };
        // The stick's folders become new ones here: parents first.
        let mut made: BTreeMap<u64, FileId> = BTreeMap::new();
        made.insert(ROOT_OBJ, base);
        let mut left: Vec<&Rec> = last.values().collect();
        while !left.is_empty() {
            let before = left.len();
            let mut later = Vec::new();
            for rec in left {
                let Some(&parent) = made.get(&rec.parent) else {
                    later.push(rec);
                    continue;
                };
                let f = self.create(p, app, parent, &rec.name, rec.kind == kind::FOLDER)?;
                if rec.kind == kind::FOLDER {
                    made.insert(rec.obj, f);
                    done.folders += 1;
                } else {
                    if !rec.data.is_empty() {
                        self.write(p, app, f, 0, &rec.data)?;
                    }
                    done.documents += 1;
                }
            }
            if later.len() == before {
                // Folders whose parent is not on the stick: none should be.
                return Err(Error::Damaged(0));
            }
            left = later;
        }
        Ok(done)
    }
}

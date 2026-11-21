// SPDX-License-Identifier: GPL-3.0-only
//! What the store needs from a disk, and a disk in memory that can lose
//! power, lose writes it never promised to keep, tear blocks and grow bad
//! spots, for the tests.

use alloc::collections::BTreeMap;
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

use crate::format::{BLOCK, Block};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IoError {
    /// The device reported an error for this block.
    Failed,
    /// The power was cut (tests only): nothing more reaches the disk.
    PowerCut,
}

/// A disk of 4 KiB blocks. A write may sit in the disk's own cache until
/// the next [`Disk::flush`]; only a finished flush promises that every
/// earlier write is kept.
pub trait Disk {
    fn blocks(&self) -> u64;
    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError>;
    fn write(&mut self, addr: u64, buf: &Block) -> Result<(), IoError>;
    fn flush(&mut self) -> Result<(), IoError>;
}

impl<D: Disk + ?Sized> Disk for &mut D {
    fn blocks(&self) -> u64 {
        (**self).blocks()
    }
    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        (**self).read(addr, buf)
    }
    fn write(&mut self, addr: u64, buf: &Block) -> Result<(), IoError> {
        (**self).write(addr, buf)
    }
    fn flush(&mut self) -> Result<(), IoError> {
        (**self).flush()
    }
}

/// A disk in memory with a write cache that the power can take away.
#[derive(Clone)]
pub struct MemDisk {
    blocks: u64,
    /// What survives a power cut.
    kept: BTreeMap<u64, Block>,
    /// Writes since the last flush, in order.
    cached: Vec<(u64, Block)>,
    /// Writes so far (the power-cut tests count these).
    pub writes: u64,
    pub flushes: u64,
    pub reads: u64,
    /// Writes and flushes so far.
    pub events: u64,
    /// Cut the power when this many writes and flushes have been made:
    /// the next one fails, and so does everything after it.
    pub cut_after: Option<u64>,
    cut: bool,
    /// Blocks that fail to read.
    pub bad: BTreeSet<u64>,
}

impl MemDisk {
    pub fn new(blocks: u64) -> MemDisk {
        MemDisk {
            blocks,
            kept: BTreeMap::new(),
            cached: Vec::new(),
            writes: 0,
            flushes: 0,
            reads: 0,
            events: 0,
            cut_after: None,
            cut: false,
            bad: BTreeSet::new(),
        }
    }

    pub fn is_cut(&self) -> bool {
        self.cut
    }

    /// The disk after the power came back. Of the writes that were still
    /// in the cache, `keep(i, n)` decides for write `i` of `n` how much
    /// reached the disk: 0 none, 8 all of it, 1 to 7 only its first sectors
    /// (a torn write). Writes reach the disk in any order the disk likes,
    /// which `keep` models by keeping a later write and dropping an
    /// earlier one.
    pub fn after_power_cut(&self, mut keep: impl FnMut(usize, usize) -> u8) -> MemDisk {
        let mut d = MemDisk::new(self.blocks);
        d.kept = self.kept.clone();
        d.bad = self.bad.clone();
        let n = self.cached.len();
        for (i, (addr, data)) in self.cached.iter().enumerate() {
            let k = keep(i, n).min(8) as usize;
            if k == 0 {
                continue;
            }
            let b = d.kept.entry(*addr).or_insert([0; BLOCK]);
            b[..k * 512].copy_from_slice(&data[..k * 512]);
        }
        d
    }

    /// Change bytes of a kept block behind the store's back.
    pub fn damage(&mut self, addr: u64, at: usize) {
        let b = self.kept.entry(addr).or_insert([0; BLOCK]);
        b[at % BLOCK] ^= 0x5a;
    }

    /// Every kept block (to search the raw disk for readable data).
    pub fn raw(&self) -> impl Iterator<Item = (&u64, &Block)> {
        self.kept.iter()
    }
}

impl Disk for MemDisk {
    fn blocks(&self) -> u64 {
        self.blocks
    }

    fn read(&mut self, addr: u64, buf: &mut Block) -> Result<(), IoError> {
        assert!(addr < self.blocks, "read past the end: {addr}");
        self.reads += 1;
        if self.bad.contains(&addr) {
            return Err(IoError::Failed);
        }
        // The disk answers from its cache first.
        if let Some((_, b)) = self.cached.iter().rev().find(|(a, _)| *a == addr) {
            *buf = *b;
        } else if let Some(b) = self.kept.get(&addr) {
            *buf = *b;
        } else {
            buf.fill(0);
        }
        Ok(())
    }

    fn write(&mut self, addr: u64, buf: &Block) -> Result<(), IoError> {
        assert!(addr < self.blocks, "write past the end: {addr}");
        if self.cut {
            return Err(IoError::PowerCut);
        }
        if self.cut_after == Some(self.events) {
            self.cut = true;
            return Err(IoError::PowerCut);
        }
        self.events += 1;
        self.writes += 1;
        self.cached.push((addr, *buf));
        Ok(())
    }

    fn flush(&mut self) -> Result<(), IoError> {
        if self.cut {
            return Err(IoError::PowerCut);
        }
        if self.cut_after == Some(self.events) {
            self.cut = true;
            return Err(IoError::PowerCut);
        }
        self.events += 1;
        self.flushes += 1;
        for (a, b) in self.cached.drain(..) {
            self.kept.insert(a, b);
        }
        Ok(())
    }
}

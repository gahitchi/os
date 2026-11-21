// SPDX-License-Identifier: GPL-3.0-only
//! Which blocks are in use: one bit per block, kept in memory and written
//! as part of every commit, one 4 KiB bitmap block per chunk of blocks
//! that changed.

use alloc::collections::BTreeSet;
use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::format::BLOCK;

/// Blocks one bitmap block covers.
pub const CHUNK: u64 = (BLOCK * 8) as u64;

pub struct Space {
    pub blocks: u64,
    bits: Vec<u64>,
    /// Chunks changed since the last commit.
    pub dirty: BTreeSet<u64>,
    pub used: u64,
    /// Where the next search starts, for blocks and for spare copies
    /// (spare copies are kept in the other half of the disk).
    cursor: [u64; 2],
}

impl Space {
    pub fn new(blocks: u64) -> Space {
        Space {
            blocks,
            bits: vec![0; blocks.div_ceil(64) as usize],
            dirty: BTreeSet::new(),
            used: 0,
            cursor: [2, blocks / 2],
        }
    }

    pub fn chunks(&self) -> u64 {
        self.blocks.div_ceil(CHUNK)
    }

    pub fn is_used(&self, b: u64) -> bool {
        self.bits[(b / 64) as usize] >> (b % 64) & 1 != 0
    }

    pub fn mark(&mut self, b: u64) {
        assert!(b < self.blocks);
        assert!(!self.is_used(b), "block {b} is already in use");
        self.bits[(b / 64) as usize] |= 1 << (b % 64);
        self.used += 1;
        self.dirty.insert(b / CHUNK);
    }

    pub fn free(&mut self, b: u64) {
        assert!(self.is_used(b), "block {b} is already free");
        self.bits[(b / 64) as usize] &= !(1 << (b % 64));
        self.used -= 1;
        self.dirty.insert(b / CHUNK);
    }

    fn find(&mut self, which: usize, avoid: u64) -> Result<u64, Error> {
        let start = self.cursor[which].clamp(2, self.blocks - 1);
        let mut b = start;
        loop {
            if !self.is_used(b) && b != avoid {
                self.cursor[which] = b + 1;
                return Ok(b);
            }
            b += 1;
            if b >= self.blocks {
                b = 2;
            }
            if b == start {
                return Err(Error::Full);
            }
        }
    }

    /// A free block, and a second one far from it when `spare`.
    pub fn alloc(&mut self, spare: bool) -> Result<(u64, u64), Error> {
        let a = self.find(0, 0)?;
        self.mark(a);
        if !spare {
            return Ok((a, 0));
        }
        match self.find(1, a) {
            Ok(b) => {
                self.mark(b);
                Ok((a, b))
            }
            Err(e) => {
                self.free(a);
                Err(e)
            }
        }
    }

    /// The bitmap block of `chunk`.
    pub fn chunk_bytes(&self, chunk: u64, out: &mut [u8; BLOCK]) {
        let words = BLOCK / 8;
        let first = chunk as usize * words;
        for i in 0..words {
            let w = self.bits.get(first + i).copied().unwrap_or(0);
            out[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
        }
    }

    /// Load the bitmap block of `chunk`.
    pub fn load_chunk(&mut self, chunk: u64, b: &[u8; BLOCK]) {
        let words = BLOCK / 8;
        let first = chunk as usize * words;
        for i in 0..words {
            if first + i >= self.bits.len() {
                break;
            }
            let w = u64::from_le_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
            // Bits past the end of the disk are never set.
            let w = if (first + i + 1) * 64 > self.blocks as usize {
                let valid = self.blocks as usize - (first + i) * 64;
                w & ((1u64 << valid) - 1)
            } else {
                w
            };
            self.used += w.count_ones() as u64;
            self.bits[first + i] = w;
        }
    }

    pub fn free_blocks(&self) -> u64 {
        self.blocks - self.used
    }
}

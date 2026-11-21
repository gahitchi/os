// SPDX-License-Identifier: GPL-3.0-only
//! The bytes on the disk: block pointers, tree nodes and the front pages.
//!
//! Every number is little-endian. Format changes bump [`FORMAT`]; a store
//! refuses a format it does not know.

use alloc::vec::Vec;

/// Bytes in a block.
pub const BLOCK: usize = 4096;
pub type Block = [u8; BLOCK];

/// "ALAYASTO".
pub const MAGIC: u64 = 0x4f54_5341_5941_4c41;
pub const FORMAT: u64 = 1;

/// The two front pages live in blocks 0 and 1; commit `g` writes page
/// `g % 2`, so a cut while writing one leaves the other whole.
pub const FRONT: [u64; 2] = [0, 1];

/// Where a block is and how to check it: its address, the address of its
/// spare copy (0 when it has none), the commit that wrote it (its birth)
/// and its fingerprint.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Ptr {
    pub addr: u64,
    pub addr2: u64,
    pub birth: u64,
    pub mac: [u8; 16],
}

impl Ptr {
    pub const SIZE: usize = 40;

    pub fn encode(&self) -> Val {
        let mut v = [0u8; 40];
        v[0..8].copy_from_slice(&self.addr.to_le_bytes());
        v[8..16].copy_from_slice(&self.addr2.to_le_bytes());
        v[16..24].copy_from_slice(&self.birth.to_le_bytes());
        v[24..40].copy_from_slice(&self.mac);
        v
    }

    pub fn decode(v: &Val) -> Ptr {
        Ptr {
            addr: word(v, 0),
            addr2: word(v, 1),
            birth: word(v, 2),
            mac: v[24..40].try_into().unwrap(),
        }
    }

    pub fn is_null(&self) -> bool {
        self.addr == 0
    }

    /// The blocks it takes: one, or two with a spare copy.
    pub fn count(&self) -> u64 {
        if self.addr2 != 0 { 2 } else { 1 }
    }
}

/// A tree key: two words, compared in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct Key(pub u64, pub u64);

impl Key {
    pub const MIN: Key = Key(0, 0);
    pub const MAX: Key = Key(u64::MAX, u64::MAX);
}

/// A tree value: 40 bytes, either a [`Ptr`] or a small record of words.
pub type Val = [u8; 40];

pub fn word(v: &[u8], i: usize) -> u64 {
    u64::from_le_bytes(v[8 * i..8 * i + 8].try_into().unwrap())
}

pub fn set_word(v: &mut [u8], i: usize, x: u64) {
    v[8 * i..8 * i + 8].copy_from_slice(&x.to_le_bytes());
}

/// A value made of up to five words.
pub fn words(w: &[u64]) -> Val {
    let mut v = [0u8; 40];
    for (i, x) in w.iter().enumerate() {
        set_word(&mut v, i, *x);
    }
    v
}

/// "NODE".
pub const NODE_MAGIC: u32 = 0x4544_4f4e;
const NODE_HEADER: usize = 16;
const ENTRY: usize = 16 + 40;
/// Entries a node holds.
pub const FANOUT: usize = (BLOCK - NODE_HEADER) / ENTRY;

/// A decoded tree node: a leaf (level 0) holds values, an inner node holds
/// pointers to the nodes below, each keyed by the smallest key under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawNode {
    pub level: u8,
    pub keys: Vec<Key>,
    pub vals: Vec<Val>,
}

impl RawNode {
    pub fn encode(&self, out: &mut Block) {
        assert!(self.keys.len() <= FANOUT && self.keys.len() == self.vals.len());
        out.fill(0);
        out[0..4].copy_from_slice(&NODE_MAGIC.to_le_bytes());
        out[4] = self.level;
        out[6..8].copy_from_slice(&(self.keys.len() as u16).to_le_bytes());
        for (i, (k, v)) in self.keys.iter().zip(&self.vals).enumerate() {
            let o = NODE_HEADER + i * ENTRY;
            out[o..o + 8].copy_from_slice(&k.0.to_le_bytes());
            out[o + 8..o + 16].copy_from_slice(&k.1.to_le_bytes());
            out[o + 16..o + 56].copy_from_slice(v);
        }
    }

    /// `None` when the bytes are not a node (they always are once the
    /// fingerprint matched; this guards against bugs).
    pub fn decode(b: &Block) -> Option<RawNode> {
        if u32::from_le_bytes(b[0..4].try_into().unwrap()) != NODE_MAGIC {
            return None;
        }
        let level = b[4];
        let n = u16::from_le_bytes(b[6..8].try_into().unwrap()) as usize;
        if n > FANOUT {
            return None;
        }
        let mut keys = Vec::with_capacity(n);
        let mut vals = Vec::with_capacity(n);
        for i in 0..n {
            let o = NODE_HEADER + i * ENTRY;
            keys.push(Key(word(&b[o..], 0), word(&b[o..], 1)));
            vals.push(b[o + 16..o + 56].try_into().unwrap());
        }
        if keys.windows(2).any(|w| w[0] >= w[1]) {
            return None;
        }
        Some(RawNode { level, keys, vals })
    }
}

/// A front page: which commit the disk is at and where its system tree is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Front {
    /// The commit number. Commit numbers only go up (decision 47).
    pub cn: u64,
    /// Blocks on the disk when it was formatted.
    pub blocks: u64,
    /// The root of the system tree.
    pub root: Ptr,
    /// Blocks held back for history (kept for past moments only).
    pub history_blocks: u64,
    /// Blocks in use.
    pub used: u64,
    /// Store options ([`opt`]).
    pub options: u64,
}

/// Store options in [`Front::options`].
pub mod opt {
    /// Spare copies: 0 none, 1 the store's map, 2 the map and documents.
    pub const COPIES_SHIFT: u64 = 0;
    pub const COPIES_MASK: u64 = 3;
}

impl Front {
    const BODY: usize = 8 * 8 + Ptr::SIZE;

    pub fn encode(&self, keys: &crate::crypto::Keys, out: &mut Block) {
        out.fill(0);
        set_word(out, 0, MAGIC);
        set_word(out, 1, FORMAT);
        set_word(out, 2, self.cn);
        set_word(out, 3, self.blocks);
        set_word(out, 4, self.history_blocks);
        set_word(out, 5, self.used);
        set_word(out, 6, self.options);
        out[64..64 + Ptr::SIZE].copy_from_slice(&self.root.encode());
        let mac = keys.mac(&[b"front", &out[..Self::BODY]]);
        out[Self::BODY..Self::BODY + 32].copy_from_slice(&mac);
    }

    /// The front page in `b`, if it is whole and made with these keys.
    pub fn decode(keys: &crate::crypto::Keys, b: &Block) -> Result<Front, FrontError> {
        if word(b, 0) != MAGIC {
            return Err(FrontError::NotAStore);
        }
        let mac = keys.mac(&[b"front", &b[..Self::BODY]]);
        if !crate::crypto::same(&mac, &b[Self::BODY..Self::BODY + 32]) {
            return Err(FrontError::Fingerprint);
        }
        if word(b, 1) != FORMAT {
            return Err(FrontError::Format);
        }
        Ok(Front {
            cn: word(b, 2),
            blocks: word(b, 3),
            history_blocks: word(b, 4),
            used: word(b, 5),
            options: word(b, 6),
            root: Ptr::decode(b[64..64 + Ptr::SIZE].try_into().unwrap()),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrontError {
    NotAStore,
    /// Torn, damaged, or made with another key.
    Fingerprint,
    Format,
}

// SPDX-License-Identifier: GPL-3.0-only
//! A copy-on-write B+ tree.
//!
//! Nodes on the disk are never changed. Changing an entry copies the path
//! from the root down to it into memory; the copies are written to fresh
//! blocks at the next commit, bottom up, each parent recording its
//! children's new addresses and fingerprints. The blocks the copies
//! replaced are handed back in [`Tree::released`] for the store to free or
//! keep for history.

use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::Error;
use crate::crypto::Keys;
use crate::format::{BLOCK, FANOUT, Key, Ptr, RawNode, Val};

/// How the tree reaches the disk.
pub trait Io {
    /// Read, check and unscramble the node at `ptr`.
    fn load(&mut self, keys: &Keys, ptr: &Ptr) -> Result<RawNode, Error>;
    /// Scramble, fingerprint and write `plain` at `addr` (and its spare
    /// copy at `addr2`, when not 0) as part of commit `cn`.
    fn store(
        &mut self,
        keys: &Keys,
        addr: u64,
        addr2: u64,
        cn: u64,
        plain: &mut [u8; BLOCK],
    ) -> Result<Ptr, Error>;
}

pub enum Link {
    Disk(Ptr),
    Mem(Box<Node>),
}

/// A node copied into memory.
pub struct Node {
    level: u8,
    keys: Vec<Key>,
    /// Values, in a leaf.
    vals: Vec<Val>,
    /// Children, in an inner node.
    kids: Vec<Link>,
    /// Where it will be written, once the commit has picked a place.
    addr: Option<(u64, u64)>,
}

impl Node {
    fn from_raw(r: RawNode) -> Node {
        if r.level == 0 {
            Node {
                level: 0,
                keys: r.keys,
                vals: r.vals,
                kids: Vec::new(),
                addr: None,
            }
        } else {
            Node {
                level: r.level,
                keys: r.keys,
                vals: Vec::new(),
                kids: r.vals.iter().map(|v| Link::Disk(Ptr::decode(v))).collect(),
                addr: None,
            }
        }
    }

    fn len(&self) -> usize {
        self.keys.len()
    }

    /// The child an entry with key `k` belongs under.
    fn route(&self, k: Key) -> usize {
        match self.keys.binary_search(&k) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        }
    }

    /// Split off the upper half.
    fn split(&mut self) -> Box<Node> {
        let half = self.len() / 2;
        let keys = self.keys.split_off(half);
        let (vals, kids) = if self.level == 0 {
            (self.vals.split_off(half), Vec::new())
        } else {
            (Vec::new(), self.kids.split_off(half))
        };
        Box::new(Node {
            level: self.level,
            keys,
            vals,
            kids,
            addr: None,
        })
    }
}

fn route_raw(r: &RawNode, k: Key) -> Option<usize> {
    match r.keys.binary_search(&k) {
        Ok(i) => Some(i),
        Err(0) => None,
        Err(i) => Some(i - 1),
    }
}

#[derive(Default)]
pub struct Tree {
    root: Option<Link>,
    /// Blocks whose contents were copied into memory since the last commit.
    pub released: Vec<Ptr>,
    /// Nodes in memory (written at the next commit).
    pub mem: usize,
    /// Whether this tree's nodes get a spare copy.
    pub spare: bool,
}

impl Tree {
    pub fn new(spare: bool) -> Tree {
        Tree {
            spare,
            ..Tree::default()
        }
    }

    /// A tree whose root is on the disk (or empty, for a null pointer).
    pub fn open(root: Ptr, spare: bool) -> Tree {
        Tree {
            root: if root.is_null() {
                None
            } else {
                Some(Link::Disk(root))
            },
            spare,
            ..Tree::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    /// The root's pointer, once the tree has been written.
    pub fn root_ptr(&self) -> Ptr {
        match &self.root {
            None => Ptr::default(),
            Some(Link::Disk(p)) => *p,
            Some(Link::Mem(_)) => panic!("tree not written yet"),
        }
    }

    pub fn is_dirty(&self) -> bool {
        matches!(self.root, Some(Link::Mem(_))) || !self.released.is_empty()
    }

    fn mem_of<'a>(
        io: &mut dyn Io,
        keys: &Keys,
        link: &'a mut Link,
        released: &mut Vec<Ptr>,
        mem: &mut usize,
    ) -> Result<&'a mut Node, Error> {
        if let Link::Disk(p) = link {
            let p = *p;
            let raw = io.load(keys, &p)?;
            released.push(p);
            *mem += 1;
            *link = Link::Mem(Box::new(Node::from_raw(raw)));
        }
        match link {
            Link::Mem(n) => Ok(n),
            Link::Disk(_) => unreachable!(),
        }
    }

    pub fn get(&self, io: &mut dyn Io, keys: &Keys, k: Key) -> Result<Option<Val>, Error> {
        let mut link = match &self.root {
            None => return Ok(None),
            Some(l) => l,
        };
        // Through the nodes in memory first.
        loop {
            match link {
                Link::Mem(n) => {
                    if n.level == 0 {
                        return Ok(n.keys.binary_search(&k).ok().map(|i| n.vals[i]));
                    }
                    if n.keys.is_empty() || k < n.keys[0] {
                        return Ok(None);
                    }
                    link = &n.kids[n.route(k)];
                }
                Link::Disk(p) => {
                    let mut p = *p;
                    loop {
                        let r = io.load(keys, &p)?;
                        if r.level == 0 {
                            return Ok(r.keys.binary_search(&k).ok().map(|i| r.vals[i]));
                        }
                        match route_raw(&r, k) {
                            None => return Ok(None),
                            Some(i) => p = Ptr::decode(&r.vals[i]),
                        }
                    }
                }
            }
        }
    }

    /// Visit the entries with `lo <= key <= hi` in order until `f` says stop.
    pub fn range(
        &self,
        io: &mut dyn Io,
        keys: &Keys,
        lo: Key,
        hi: Key,
        f: &mut dyn FnMut(Key, &Val) -> bool,
    ) -> Result<(), Error> {
        if let Some(l) = &self.root {
            visit(io, keys, l, lo, hi, f)?;
        }
        Ok(())
    }

    /// The entries with `lo <= key <= hi`.
    pub fn collect(
        &self,
        io: &mut dyn Io,
        keys: &Keys,
        lo: Key,
        hi: Key,
    ) -> Result<Vec<(Key, Val)>, Error> {
        let mut out = Vec::new();
        self.range(io, keys, lo, hi, &mut |k, v| {
            out.push((k, *v));
            true
        })?;
        Ok(out)
    }

    /// The last entry with key in `lo..=hi`.
    pub fn last_in(
        &self,
        io: &mut dyn Io,
        keys: &Keys,
        lo: Key,
        hi: Key,
    ) -> Result<Option<(Key, Val)>, Error> {
        match &self.root {
            None => Ok(None),
            Some(l) => last(io, keys, l, lo, hi),
        }
    }

    /// Set `k` to `v`; the old value, if there was one.
    pub fn insert(
        &mut self,
        io: &mut dyn Io,
        keys: &Keys,
        k: Key,
        v: Val,
    ) -> Result<Option<Val>, Error> {
        let Tree {
            root,
            released,
            mem,
            ..
        } = self;
        let Some(link) = root else {
            *mem += 1;
            *root = Some(Link::Mem(Box::new(Node {
                level: 0,
                keys: alloc::vec![k],
                vals: alloc::vec![v],
                kids: Vec::new(),
                addr: None,
            })));
            return Ok(None);
        };
        let node = Self::mem_of(io, keys, link, released, mem)?;
        let (old, split) = ins(io, keys, node, k, v, released, mem)?;
        if let Some(right) = split {
            let left = match root.take() {
                Some(Link::Mem(n)) => n,
                _ => unreachable!(),
            };
            let level = left.level + 1;
            *mem += 1;
            *root = Some(Link::Mem(Box::new(Node {
                level,
                keys: alloc::vec![left.keys[0], right.keys[0]],
                vals: Vec::new(),
                kids: alloc::vec![Link::Mem(left), Link::Mem(right)],
                addr: None,
            })));
        }
        Ok(old)
    }

    /// Remove `k`; its value, if it was there.
    pub fn remove(&mut self, io: &mut dyn Io, keys: &Keys, k: Key) -> Result<Option<Val>, Error> {
        if self.get(io, keys, k)?.is_none() {
            return Ok(None);
        }
        let Tree {
            root,
            released,
            mem,
            ..
        } = self;
        let link = root.as_mut().unwrap();
        let node = Self::mem_of(io, keys, link, released, mem)?;
        let old = rem(io, keys, node, k, released, mem)?;
        // Shrink the root while it is an inner node with one child, or empty.
        while let Some(Link::Mem(n)) = root {
            if n.keys.is_empty() {
                assert!(n.addr.is_none(), "removed a node already placed");
                *root = None;
                *mem -= 1;
                break;
            }
            if n.level > 0 && n.kids.len() == 1 {
                assert!(n.addr.is_none(), "removed a node already placed");
                let kid = n.kids.pop().unwrap();
                *root = Some(kid);
                *mem -= 1;
                continue;
            }
            break;
        }
        Ok(old)
    }

    /// Pick places for the nodes in memory that have none yet; how many.
    pub fn place(
        &mut self,
        alloc: &mut dyn FnMut(bool) -> Result<(u64, u64), Error>,
    ) -> Result<usize, Error> {
        let spare = self.spare;
        match &mut self.root {
            Some(Link::Mem(n)) => place(n, spare, alloc),
            _ => Ok(0),
        }
    }

    /// Write every node in memory (bottom up) as part of commit `cn`,
    /// picking places for nodes that have none. Returns the root pointer.
    pub fn write(
        &mut self,
        io: &mut dyn Io,
        keys: &Keys,
        cn: u64,
        alloc: &mut dyn FnMut(bool) -> Result<(u64, u64), Error>,
    ) -> Result<Ptr, Error> {
        self.place(alloc)?;
        if let Some(link) = &mut self.root
            && let Link::Mem(_) = link
        {
            let p = seal(io, keys, cn, link)?;
            *link = Link::Disk(p);
        }
        self.mem = 0;
        Ok(self.root_ptr())
    }

    /// Visit every block of a tree that is on the disk, with its level.
    pub fn walk(
        io: &mut dyn Io,
        keys: &Keys,
        root: &Ptr,
        f: &mut dyn FnMut(&Ptr, &RawNode),
    ) -> Result<(), Error> {
        if root.is_null() {
            return Ok(());
        }
        let r = io.load(keys, root)?;
        f(root, &r);
        if r.level > 0 {
            for v in &r.vals {
                Self::walk(io, keys, &Ptr::decode(v), f)?;
            }
        }
        Ok(())
    }
}

fn visit(
    io: &mut dyn Io,
    keys: &Keys,
    link: &Link,
    lo: Key,
    hi: Key,
    f: &mut dyn FnMut(Key, &Val) -> bool,
) -> Result<bool, Error> {
    match link {
        Link::Mem(n) => {
            if n.level == 0 {
                for (k, v) in n.keys.iter().zip(&n.vals) {
                    if *k > hi {
                        return Ok(false);
                    }
                    if *k >= lo && !f(*k, v) {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            let start = n.route(lo);
            for i in start..n.kids.len() {
                if n.keys[i] > hi {
                    return Ok(false);
                }
                if !visit(io, keys, &n.kids[i], lo, hi, f)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Link::Disk(p) => {
            let r = io.load(keys, p)?;
            if r.level == 0 {
                for (k, v) in r.keys.iter().zip(&r.vals) {
                    if *k > hi {
                        return Ok(false);
                    }
                    if *k >= lo && !f(*k, v) {
                        return Ok(false);
                    }
                }
                return Ok(true);
            }
            let start = route_raw(&r, lo).unwrap_or(0);
            for i in start..r.keys.len() {
                if r.keys[i] > hi {
                    return Ok(false);
                }
                let kid = Link::Disk(Ptr::decode(&r.vals[i]));
                if !visit(io, keys, &kid, lo, hi, f)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    }
}

fn last(
    io: &mut dyn Io,
    keys: &Keys,
    link: &Link,
    lo: Key,
    hi: Key,
) -> Result<Option<(Key, Val)>, Error> {
    match link {
        Link::Mem(n) => {
            if n.level == 0 {
                return Ok(n
                    .keys
                    .iter()
                    .zip(&n.vals)
                    .rev()
                    .find(|(k, _)| **k >= lo && **k <= hi)
                    .map(|(k, v)| (*k, *v)));
            }
            let mut i = n.route(hi) as isize;
            while i >= 0 {
                let iu = i as usize;
                if n.keys[iu] > hi {
                    i -= 1;
                    continue;
                }
                if let Some(x) = last(io, keys, &n.kids[iu], lo, hi)? {
                    return Ok(Some(x));
                }
                if n.keys[iu] <= lo {
                    break;
                }
                i -= 1;
            }
            Ok(None)
        }
        Link::Disk(p) => {
            let r = io.load(keys, p)?;
            if r.level == 0 {
                return Ok(r
                    .keys
                    .iter()
                    .zip(&r.vals)
                    .rev()
                    .find(|(k, _)| **k >= lo && **k <= hi)
                    .map(|(k, v)| (*k, *v)));
            }
            let Some(start) = route_raw(&r, hi) else {
                return Ok(None);
            };
            let mut i = start as isize;
            while i >= 0 {
                let iu = i as usize;
                let kid = Link::Disk(Ptr::decode(&r.vals[iu]));
                if let Some(x) = last(io, keys, &kid, lo, hi)? {
                    return Ok(Some(x));
                }
                if r.keys[iu] <= lo {
                    break;
                }
                i -= 1;
            }
            Ok(None)
        }
    }
}

type Split = Option<Box<Node>>;

fn ins(
    io: &mut dyn Io,
    keys: &Keys,
    node: &mut Node,
    k: Key,
    v: Val,
    released: &mut Vec<Ptr>,
    mem: &mut usize,
) -> Result<(Option<Val>, Split), Error> {
    let old = if node.level == 0 {
        match node.keys.binary_search(&k) {
            Ok(i) => Some(core::mem::replace(&mut node.vals[i], v)),
            Err(i) => {
                node.keys.insert(i, k);
                node.vals.insert(i, v);
                None
            }
        }
    } else {
        let i = node.route(k);
        if k < node.keys[i] {
            node.keys[i] = k;
        }
        let kid = Tree::mem_of(io, keys, &mut node.kids[i], released, mem)?;
        let (old, split) = ins(io, keys, kid, k, v, released, mem)?;
        if let Some(right) = split {
            node.keys.insert(i + 1, right.keys[0]);
            node.kids.insert(i + 1, Link::Mem(right));
        }
        old
    };
    let split = if node.len() > FANOUT {
        *mem += 1;
        Some(node.split())
    } else {
        None
    };
    Ok((old, split))
}

fn rem(
    io: &mut dyn Io,
    keys: &Keys,
    node: &mut Node,
    k: Key,
    released: &mut Vec<Ptr>,
    mem: &mut usize,
) -> Result<Option<Val>, Error> {
    if node.level == 0 {
        return Ok(match node.keys.binary_search(&k) {
            Ok(i) => {
                node.keys.remove(i);
                Some(node.vals.remove(i))
            }
            Err(_) => None,
        });
    }
    let i = node.route(k);
    let kid = Tree::mem_of(io, keys, &mut node.kids[i], released, mem)?;
    let old = rem(io, keys, kid, k, released, mem)?;
    if kid.keys.is_empty() {
        assert!(kid.addr.is_none(), "removed a node already placed");
        node.keys.remove(i);
        node.kids.remove(i);
        *mem -= 1;
    } else {
        node.keys[i] = kid.keys[0];
    }
    Ok(old)
}

fn place(
    n: &mut Node,
    spare: bool,
    alloc: &mut dyn FnMut(bool) -> Result<(u64, u64), Error>,
) -> Result<usize, Error> {
    let mut count = 0;
    for kid in n.kids.iter_mut() {
        if let Link::Mem(k) = kid {
            count += place(k, spare, alloc)?;
        }
    }
    if n.addr.is_none() {
        n.addr = Some(alloc(spare)?);
        count += 1;
    }
    Ok(count)
}

fn seal(io: &mut dyn Io, keys: &Keys, cn: u64, link: &mut Link) -> Result<Ptr, Error> {
    let n = match link {
        Link::Disk(p) => return Ok(*p),
        Link::Mem(n) => n,
    };
    let mut vals = Vec::with_capacity(n.len());
    if n.level == 0 {
        vals.extend_from_slice(&n.vals);
    } else {
        for kid in n.kids.iter_mut() {
            let p = seal(io, keys, cn, kid)?;
            *kid = Link::Disk(p);
            vals.push(p.encode());
        }
    }
    let raw = RawNode {
        level: n.level,
        keys: core::mem::take(&mut n.keys),
        vals,
    };
    let mut plain = [0u8; BLOCK];
    raw.encode(&mut plain);
    let (a, a2) = n.addr.expect("node has no place");
    io.store(keys, a, a2, cn, &mut plain)
}

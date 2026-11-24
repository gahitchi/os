// SPDX-License-Identifier: GPL-3.0-only
//! Scrambling and fingerprints: AES-256 (FIPS 197) in counter mode, SHA-256
//! (FIPS 180-4) and HMAC-SHA-256 (RFC 2104), plus the slow PIN step that
//! turns a short PIN into a key ([`pin_key`]).
//!
//! It never allocates memory, so even the first program can use it.
//!
//! Every block is scrambled with AES-256-CTR and then fingerprinted with
//! HMAC-SHA-256 over its place, its birth and the scrambled bytes
//! ("encrypt, then MAC"). The fingerprint is kept by whoever points at the
//! block, so a block cannot be changed, swapped with another or moved
//! without the store noticing.
//!
//! This is plain Rust that runs on every processor. It uses lookup tables,
//! so its timing can depend on the data; the processor's own AES
//! instructions (decision 29) replace it as a later speed and safety step.

#![no_std]

pub mod ed25519;

/// The AES S-box.
const SBOX: [u8; 256] = {
    let mut s = [0u8; 256];
    // Build it from the multiplicative inverse in GF(2^8) and the affine map.
    let mut p: u8 = 1;
    let mut q: u8 = 1;
    loop {
        // p * 3
        p = p ^ (p << 1) ^ if p & 0x80 != 0 { 0x1b } else { 0 };
        // q / 3
        q ^= q << 1;
        q ^= q << 2;
        q ^= q << 4;
        if q & 0x80 != 0 {
            q ^= 0x09;
        }
        let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
        s[p as usize] = x ^ 0x63;
        if p == 1 {
            break;
        }
    }
    s[0] = 0x63;
    s
};

const fn xtime(x: u8) -> u8 {
    (x << 1) ^ if x & 0x80 != 0 { 0x1b } else { 0 }
}

/// The round table: SubBytes and MixColumns of one byte, as a column.
const TE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let s = SBOX[i];
        let s2 = xtime(s);
        let s3 = s2 ^ s;
        t[i] = (s2 as u32) << 24 | (s as u32) << 16 | (s as u32) << 8 | s3 as u32;
        i += 1;
    }
    t
};

/// An expanded AES-256 key.
#[derive(Clone)]
pub struct Aes256 {
    rk: [u32; 60],
}

impl Aes256 {
    pub fn new(key: &[u8; 32]) -> Aes256 {
        let mut rk = [0u32; 60];
        for (i, w) in rk.iter_mut().take(8).enumerate() {
            *w = u32::from_be_bytes([key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]]);
        }
        let mut rcon: u8 = 1;
        for i in 8..60 {
            let mut t = rk[i - 1];
            if i % 8 == 0 {
                t = sub_word(t.rotate_left(8)) ^ (rcon as u32) << 24;
                rcon = xtime(rcon);
            } else if i % 8 == 4 {
                t = sub_word(t);
            }
            rk[i] = rk[i - 8] ^ t;
        }
        Aes256 { rk }
    }

    /// Scramble one 16-byte block.
    pub fn encrypt(&self, block: &mut [u8; 16]) {
        let rk = &self.rk;
        let mut s = [0u32; 4];
        for (i, w) in s.iter_mut().enumerate() {
            *w = u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]) ^ rk[i];
        }
        for round in 1..14 {
            let mut t = [0u32; 4];
            for (c, w) in t.iter_mut().enumerate() {
                *w = TE[(s[c] >> 24) as usize]
                    ^ TE[(s[(c + 1) % 4] >> 16 & 0xff) as usize].rotate_right(8)
                    ^ TE[(s[(c + 2) % 4] >> 8 & 0xff) as usize].rotate_right(16)
                    ^ TE[(s[(c + 3) % 4] & 0xff) as usize].rotate_right(24)
                    ^ rk[4 * round + c];
            }
            s = t;
        }
        for c in 0..4 {
            let w = (SBOX[(s[c] >> 24) as usize] as u32) << 24
                | (SBOX[(s[(c + 1) % 4] >> 16 & 0xff) as usize] as u32) << 16
                | (SBOX[(s[(c + 2) % 4] >> 8 & 0xff) as usize] as u32) << 8
                | SBOX[(s[(c + 3) % 4] & 0xff) as usize] as u32;
            block[4 * c..4 * c + 4].copy_from_slice(&(w ^ rk[56 + c]).to_be_bytes());
        }
    }

    /// Counter mode: XOR `data` with the key stream that starts at the
    /// counter block `iv` (its last 4 bytes count up). Scrambling and
    /// unscrambling are the same.
    pub fn ctr(&self, iv: &[u8; 16], data: &mut [u8]) {
        let base = u32::from_be_bytes([iv[12], iv[13], iv[14], iv[15]]);
        for (n, chunk) in data.chunks_mut(16).enumerate() {
            let mut ks = *iv;
            ks[12..].copy_from_slice(&base.wrapping_add(n as u32).to_be_bytes());
            self.encrypt(&mut ks);
            for (d, k) in chunk.iter_mut().zip(ks.iter()) {
                *d ^= k;
            }
        }
    }
}

fn sub_word(w: u32) -> u32 {
    let b = w.to_be_bytes();
    u32::from_be_bytes([
        SBOX[b[0] as usize],
        SBOX[b[1] as usize],
        SBOX[b[2] as usize],
        SBOX[b[3] as usize],
    ])
}

const K256: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H256: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// SHA-256, fed in pieces.
#[derive(Clone)]
pub struct Sha256 {
    h: [u32; 8],
    buf: [u8; 64],
    fill: usize,
    len: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Sha256 {
        Sha256 {
            h: H256,
            buf: [0; 64],
            fill: 0,
            len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if self.fill > 0 {
            let n = (64 - self.fill).min(data.len());
            self.buf[self.fill..self.fill + n].copy_from_slice(&data[..n]);
            self.fill += n;
            data = &data[n..];
            if self.fill < 64 {
                return;
            }
            let b = self.buf;
            self.block(&b);
            self.fill = 0;
        }
        while data.len() >= 64 {
            self.block(data[..64].try_into().unwrap());
            data = &data[64..];
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.fill = data.len();
    }

    pub fn finish(mut self) -> [u8; 32] {
        let bits = self.len * 8;
        self.update(&[0x80]);
        while self.fill != 56 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 32];
        for (i, w) in self.h.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn block(&mut self, b: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b_, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K256[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b_) ^ (a & c) ^ (b_ & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b_;
            b_ = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in self.h.iter_mut().zip([a, b_, c, d, e, f, g, h]) {
            *x = x.wrapping_add(y);
        }
    }
}

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut s = Sha256::new();
    s.update(data);
    s.finish()
}

/// HMAC-SHA-256 with the key's inner and outer states prepared once.
#[derive(Clone)]
pub struct Hmac {
    inner: Sha256,
    outer: Sha256,
}

impl Hmac {
    pub fn new(key: &[u8]) -> Hmac {
        let mut k = [0u8; 64];
        if key.len() > 64 {
            k[..32].copy_from_slice(&sha256(key));
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut inner = Sha256::new();
        let mut outer = Sha256::new();
        let mut ipad = [0u8; 64];
        let mut opad = [0u8; 64];
        for i in 0..64 {
            ipad[i] = k[i] ^ 0x36;
            opad[i] = k[i] ^ 0x5c;
        }
        inner.update(&ipad);
        outer.update(&opad);
        Hmac { inner, outer }
    }

    pub fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        let mut i = self.inner.clone();
        for p in parts {
            i.update(p);
        }
        let mut o = self.outer.clone();
        o.update(&i.finish());
        o.finish()
    }
}

/// Compare without stopping at the first difference.
pub fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

/// The keys of one key domain (the machine, or one person), derived from
/// its secret: one for scrambling, one for fingerprints.
#[derive(Clone)]
pub struct Keys {
    aes: Aes256,
    mac: Hmac,
}

impl Keys {
    pub fn new(secret: &[u8; 32]) -> Keys {
        let derive = Hmac::new(secret);
        let enc = derive.mac(&[b"alaya store scramble key"]);
        let mac = derive.mac(&[b"alaya store fingerprint key"]);
        Keys {
            aes: Aes256::new(&enc),
            mac: Hmac::new(&mac),
        }
    }

    /// The fingerprint of a block's plain contents, written at `addr` in
    /// commit `birth`.
    pub fn fingerprint(&self, addr: u64, birth: u64, plain: &[u8]) -> [u8; 16] {
        let m = self
            .mac
            .mac(&[b"block", &addr.to_le_bytes(), &birth.to_le_bytes(), plain]);
        m[..16].try_into().unwrap()
    }

    /// Scramble a block in place and return its fingerprint.
    ///
    /// The key stream starts from the fingerprint of the plain contents
    /// (the "synthetic IV" construction of RFC 5297, with HMAC-SHA-256 and
    /// AES-256-CTR). Two blocks share a key stream only if their address,
    /// commit and contents are all the same, and then they are the same
    /// bytes anyway. So even a commit number used twice (a newer front page
    /// with a bad spot can hide the last number used) leaks nothing.
    pub fn seal(&self, addr: u64, birth: u64, data: &mut [u8]) -> [u8; 16] {
        let mac = self.fingerprint(addr, birth, data);
        self.aes.ctr(&mac, data);
        mac
    }

    /// Unscramble a block in place and check it against `mac`. On a
    /// mismatch the block is damaged (or not what the pointer expects) and
    /// `data` holds garbage.
    pub fn open(&self, addr: u64, birth: u64, mac: &[u8; 16], data: &mut [u8]) -> bool {
        self.aes.ctr(mac, data);
        same(&self.fingerprint(addr, birth, data), mac)
    }

    /// Scramble again what `open` unscrambled (to write a repair).
    pub fn reseal(&self, mac: &[u8; 16], data: &mut [u8]) {
        self.aes.ctr(mac, data);
    }

    /// A full fingerprint of anything else (the front page).
    pub fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        self.mac.mac(parts)
    }

    /// Scramble (or unscramble: it is its own inverse) a short field, such
    /// as a file's name, with a key stream that depends on `tweak`. A field
    /// is only ever scrambled once for each tweak (the store uses the
    /// field's place, which a new value never shares with an old one
    /// under the same key unless the value is the same too).
    pub fn scramble_field(&self, tweak: &[&[u8]], data: &mut [u8]) {
        let mut parts: [&[u8]; 4] = [b"field", &[], &[], &[]];
        for (i, t) in tweak.iter().take(3).enumerate() {
            parts[1 + i] = t;
        }
        let iv = self.mac.mac(&parts[..1 + tweak.len().min(3)]);
        self.aes.ctr(iv[..16].try_into().unwrap(), data);
    }
}

/// A small generator of secret random numbers (HMAC-DRBG style, NIST SP
/// 800-90A with SHA-256, without the reseed counter): what it gives is as
/// good as the randomness stirred in.
#[derive(Clone)]
pub struct Rng {
    state: [u8; 32],
}

impl Rng {
    pub fn new(seed: &[u8]) -> Rng {
        Rng {
            state: Hmac::new(b"alaya rng").mac(&[seed]),
        }
    }

    /// Mix in more randomness.
    pub fn stir(&mut self, extra: &[u8]) {
        self.state = Hmac::new(&self.state).mac(&[b"stir", extra]);
    }

    /// 32 fresh random bytes. The state moves on, so earlier outputs
    /// cannot be worked out from a later state.
    pub fn next(&mut self, context: &[u8]) -> [u8; 32] {
        let h = Hmac::new(&self.state);
        let out = h.mac(&[b"out", context]);
        self.state = h.mac(&[b"next"]);
        out
    }
}

/// The slow PIN step: turn a short PIN into a 32-byte key.
///
/// A PIN has few possibilities, so what protects a copied disk is how much
/// each guess costs. This step fills `work` (all of it: its size is the
/// memory cost) with a chain of SHA-256 fingerprints, then walks it in an
/// order that depends on everything before, as scrypt's ROMix does
/// (RFC 7914, with SHA-256 for the mixing step). Guessing many PINs at
/// once then needs `work`'s memory for each guess, which makes graphics
/// cards and special chips far less of a help. `salt` makes the same PIN
/// give a different key on every machine and for every person.
///
/// The chip (TPM, milestone 13) limits the number of guesses for real; this
/// step is what stands until then.
pub fn pin_key(pin: &[u8], salt: &[u8], work: &mut [[u8; 32]]) -> [u8; 32] {
    assert!(!work.is_empty(), "the PIN step needs memory");
    let h = Hmac::new(salt);
    let mut x = h.mac(&[b"alaya pin", pin]);
    let n = work.len();
    let step = |x: &[u8; 32], i: usize| {
        let mut s = Sha256::new();
        s.update(x);
        s.update(&(i as u64).to_le_bytes());
        s.finish()
    };
    for (i, w) in work.iter_mut().enumerate() {
        *w = x;
        x = step(&x, i);
    }
    for i in 0..n {
        let j = (u64::from_le_bytes(x[..8].try_into().unwrap()) % n as u64) as usize;
        for (a, b) in x.iter_mut().zip(&work[j]) {
            *a ^= b;
        }
        x = step(&x, n + i);
    }
    h.mac(&[b"alaya pin key", &x, pin])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_key_depends_on_everything() {
        let mut w = [[0u8; 32]; 1024];
        let a = pin_key(b"2468", b"salt one", &mut w);
        assert_eq!(a, pin_key(b"2468", b"salt one", &mut w));
        assert_ne!(a, pin_key(b"2469", b"salt one", &mut w));
        assert_ne!(a, pin_key(b"2468", b"salt two", &mut w));
        let mut small = [[0u8; 32]; 512];
        assert_ne!(a, pin_key(b"2468", b"salt one", &mut small));
    }

    #[test]
    fn pin_key_uses_all_its_memory() {
        // The second walk reads all over the table: changing one entry
        // between the walks would change the key. Here: every entry of a
        // fresh table differs from the others (a chain, not a constant).
        let mut w = [[0u8; 32]; 256];
        pin_key(b"1", b"s", &mut w);
        for i in 1..w.len() {
            assert_ne!(w[i], w[i - 1]);
        }
    }
}

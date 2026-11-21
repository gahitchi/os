// SPDX-License-Identifier: GPL-3.0-only
//! Scrambling and fingerprints: AES-256 (FIPS 197) in counter mode, SHA-256
//! (FIPS 180-4) and HMAC-SHA-256 (RFC 2104).
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

    fn iv(addr: u64, birth: u64) -> [u8; 16] {
        let mut iv = [0u8; 16];
        iv[..8].copy_from_slice(&addr.to_le_bytes());
        iv[8..14].copy_from_slice(&birth.to_le_bytes()[..6]);
        iv
    }

    /// Scramble or unscramble a block written at `addr` in commit `birth`.
    /// Each block is written once per commit and commit numbers never
    /// repeat, so no two blocks share a key stream.
    pub fn crypt(&self, addr: u64, birth: u64, data: &mut [u8]) {
        self.aes.ctr(&Self::iv(addr, birth), data);
    }

    /// The fingerprint of a scrambled block.
    pub fn fingerprint(&self, addr: u64, birth: u64, data: &[u8]) -> [u8; 16] {
        let m = self
            .mac
            .mac(&[b"block", &addr.to_le_bytes(), &birth.to_le_bytes(), data]);
        m[..16].try_into().unwrap()
    }

    /// A full fingerprint of anything else (the front page).
    pub fn mac(&self, parts: &[&[u8]]) -> [u8; 32] {
        self.mac.mac(parts)
    }
}

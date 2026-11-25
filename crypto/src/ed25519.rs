// SPDX-License-Identifier: GPL-3.0-only
//! Ed25519 signatures (RFC 8032), and the SHA-512 they are built on
//! (milestone 8, decision 146): app bundles are signed by their maker,
//! and Alaya checks the signature before anything is installed.
//!
//! Field elements are five 51-bit limbs; points are in extended
//! coordinates. [`verify`] is what Alaya runs. [`sign`] and
//! [`public_key`] exist for tools and tests: they are not written to hide
//! their timing, so a secret key must never be used with them on a
//! machine someone else can watch.

const K512: [u64; 80] = [
    0x428a2f98d728ae22,
    0x7137449123ef65cd,
    0xb5c0fbcfec4d3b2f,
    0xe9b5dba58189dbbc,
    0x3956c25bf348b538,
    0x59f111f1b605d019,
    0x923f82a4af194f9b,
    0xab1c5ed5da6d8118,
    0xd807aa98a3030242,
    0x12835b0145706fbe,
    0x243185be4ee4b28c,
    0x550c7dc3d5ffb4e2,
    0x72be5d74f27b896f,
    0x80deb1fe3b1696b1,
    0x9bdc06a725c71235,
    0xc19bf174cf692694,
    0xe49b69c19ef14ad2,
    0xefbe4786384f25e3,
    0x0fc19dc68b8cd5b5,
    0x240ca1cc77ac9c65,
    0x2de92c6f592b0275,
    0x4a7484aa6ea6e483,
    0x5cb0a9dcbd41fbd4,
    0x76f988da831153b5,
    0x983e5152ee66dfab,
    0xa831c66d2db43210,
    0xb00327c898fb213f,
    0xbf597fc7beef0ee4,
    0xc6e00bf33da88fc2,
    0xd5a79147930aa725,
    0x06ca6351e003826f,
    0x142929670a0e6e70,
    0x27b70a8546d22ffc,
    0x2e1b21385c26c926,
    0x4d2c6dfc5ac42aed,
    0x53380d139d95b3df,
    0x650a73548baf63de,
    0x766a0abb3c77b2a8,
    0x81c2c92e47edaee6,
    0x92722c851482353b,
    0xa2bfe8a14cf10364,
    0xa81a664bbc423001,
    0xc24b8b70d0f89791,
    0xc76c51a30654be30,
    0xd192e819d6ef5218,
    0xd69906245565a910,
    0xf40e35855771202a,
    0x106aa07032bbd1b8,
    0x19a4c116b8d2d0c8,
    0x1e376c085141ab53,
    0x2748774cdf8eeb99,
    0x34b0bcb5e19b48a8,
    0x391c0cb3c5c95a63,
    0x4ed8aa4ae3418acb,
    0x5b9cca4f7763e373,
    0x682e6ff3d6b2b8a3,
    0x748f82ee5defb2fc,
    0x78a5636f43172f60,
    0x84c87814a1f0ab72,
    0x8cc702081a6439ec,
    0x90befffa23631e28,
    0xa4506cebde82bde9,
    0xbef9a3f7b2c67915,
    0xc67178f2e372532b,
    0xca273eceea26619c,
    0xd186b8c721c0c207,
    0xeada7dd6cde0eb1e,
    0xf57d4f7fee6ed178,
    0x06f067aa72176fba,
    0x0a637dc5a2c898a6,
    0x113f9804bef90dae,
    0x1b710b35131c471b,
    0x28db77f523047d84,
    0x32caab7b40c72493,
    0x3c9ebe0a15c9bebc,
    0x431d67c49c100d4c,
    0x4cc5d4becb3e42b6,
    0x597f299cfc657e2a,
    0x5fcb6fab3ad6faec,
    0x6c44198c4a475817,
];

const H512: [u64; 8] = [
    0x6a09e667f3bcc908,
    0xbb67ae8584caa73b,
    0x3c6ef372fe94f82b,
    0xa54ff53a5f1d36f1,
    0x510e527fade682d1,
    0x9b05688c2b3e6c1f,
    0x1f83d9abfb41bd6b,
    0x5be0cd19137e2179,
];

/// SHA-512, fed in pieces.
#[derive(Clone)]
pub struct Sha512 {
    h: [u64; 8],
    buf: [u8; 128],
    fill: usize,
    len: u64,
}

impl Default for Sha512 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha512 {
    pub fn new() -> Sha512 {
        Sha512 {
            h: H512,
            buf: [0; 128],
            fill: 0,
            len: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.len += data.len() as u64;
        if self.fill > 0 {
            let n = (128 - self.fill).min(data.len());
            self.buf[self.fill..self.fill + n].copy_from_slice(&data[..n]);
            self.fill += n;
            data = &data[n..];
            if self.fill < 128 {
                return;
            }
            let b = self.buf;
            self.block(&b);
            self.fill = 0;
        }
        while data.len() >= 128 {
            self.block(data[..128].try_into().unwrap());
            data = &data[128..];
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.fill = data.len();
    }

    pub fn finish(mut self) -> [u8; 64] {
        let bits = u128::from(self.len) * 8;
        self.update(&[0x80]);
        while self.fill != 112 {
            self.update(&[0]);
        }
        self.update(&bits.to_be_bytes());
        let mut out = [0u8; 64];
        for (i, w) in self.h.iter().enumerate() {
            out[8 * i..8 * i + 8].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn block(&mut self, b: &[u8; 128]) {
        let mut w = [0u64; 80];
        for i in 0..16 {
            w[i] = u64::from_be_bytes(b[8 * i..8 * i + 8].try_into().unwrap());
        }
        for i in 16..80 {
            let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
            let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b_, mut c, mut d, mut e, mut f, mut g, mut h] = self.h;
        for i in 0..80 {
            let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K512[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
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

pub fn sha512(data: &[u8]) -> [u8; 64] {
    let mut s = Sha512::new();
    s.update(data);
    s.finish()
}

// ---- The field: numbers modulo 2^255 - 19, in five 51-bit limbs.

const MASK: u64 = (1 << 51) - 1;

#[derive(Clone, Copy)]
struct Fe([u64; 5]);

const ZERO: Fe = Fe([0; 5]);
const ONE: Fe = Fe([1, 0, 0, 0, 0]);
const D: Fe = Fe([
    0x34dca135978a3,
    0x1a8283b156ebd,
    0x5e7a26001c029,
    0x739c663a03cbb,
    0x52036cee2b6ff,
]);
const D2: Fe = Fe([
    0x69b9426b2f159,
    0x35050762add7a,
    0x3cf44c0038052,
    0x6738cc7407977,
    0x2406d9dc56dff,
]);
const SQRT_M1: Fe = Fe([
    0x61b274a0ea0b0,
    0xd5a5fc8f189d,
    0x7ef5e9cbd0c60,
    0x78595a6804c9e,
    0x2b8324804fc1d,
]);
const BX: Fe = Fe([
    0x62d608f25d51a,
    0x412a4b4f6592a,
    0x75b7171a4b31d,
    0x1ff60527118fe,
    0x216936d3cd6e5,
]);
const BY: Fe = Fe([
    0x6666666666658,
    0x4cccccccccccc,
    0x1999999999999,
    0x3333333333333,
    0x6666666666666,
]);

impl Fe {
    fn carry(mut l: [u64; 5]) -> Fe {
        for _ in 0..2 {
            let mut c;
            for i in 0..4 {
                c = l[i] >> 51;
                l[i] &= MASK;
                l[i + 1] += c;
            }
            c = l[4] >> 51;
            l[4] &= MASK;
            l[0] += c * 19;
        }
        Fe(l)
    }

    fn add(self, o: Fe) -> Fe {
        let mut l = [0; 5];
        for (i, x) in l.iter_mut().enumerate() {
            *x = self.0[i] + o.0[i];
        }
        Fe::carry(l)
    }

    fn sub(self, o: Fe) -> Fe {
        // Add 4p first so nothing goes below zero.
        const P4: [u64; 5] = [
            0x1f_ffff_ffff_ffb4,
            0x1f_ffff_ffff_fffc,
            0x1f_ffff_ffff_fffc,
            0x1f_ffff_ffff_fffc,
            0x1f_ffff_ffff_fffc,
        ];
        let mut l = [0; 5];
        for (i, x) in l.iter_mut().enumerate() {
            *x = self.0[i] + P4[i] - o.0[i];
        }
        Fe::carry(l)
    }

    fn neg(self) -> Fe {
        ZERO.sub(self)
    }

    fn mul(self, o: Fe) -> Fe {
        let a = self.0.map(u128::from);
        let b = o.0.map(u128::from);
        let b19 = [b[1] * 19, b[2] * 19, b[3] * 19, b[4] * 19];
        let r = [
            a[0] * b[0] + a[1] * b19[3] + a[2] * b19[2] + a[3] * b19[1] + a[4] * b19[0],
            a[0] * b[1] + a[1] * b[0] + a[2] * b19[3] + a[3] * b19[2] + a[4] * b19[1],
            a[0] * b[2] + a[1] * b[1] + a[2] * b[0] + a[3] * b19[3] + a[4] * b19[2],
            a[0] * b[3] + a[1] * b[2] + a[2] * b[1] + a[3] * b[0] + a[4] * b19[3],
            a[0] * b[4] + a[1] * b[3] + a[2] * b[2] + a[3] * b[1] + a[4] * b[0],
        ];
        let mut l = [0u64; 5];
        let mut c: u128 = 0;
        for i in 0..5 {
            let v = r[i] + c;
            l[i] = (v as u64) & MASK;
            c = v >> 51;
        }
        l[0] += (c as u64) * 19;
        Fe::carry(l)
    }

    fn sq(self) -> Fe {
        self.mul(self)
    }

    fn pow2k(self, k: u32) -> Fe {
        let mut x = self;
        for _ in 0..k {
            x = x.sq();
        }
        x
    }

    /// self^(2^250 - 1), the common start of inversion and square roots;
    /// also returns self^11.
    fn pow250(self) -> (Fe, Fe) {
        let z2 = self.sq();
        let z9 = z2.pow2k(2).mul(self);
        let z11 = z9.mul(z2);
        let z5 = z11.sq().mul(z9); // 2^5 - 1
        let z10 = z5.pow2k(5).mul(z5);
        let z20 = z10.pow2k(10).mul(z10);
        let z40 = z20.pow2k(20).mul(z20);
        let z50 = z40.pow2k(10).mul(z10);
        let z100 = z50.pow2k(50).mul(z50);
        let z200 = z100.pow2k(100).mul(z100);
        let z250 = z200.pow2k(50).mul(z50);
        (z250, z11)
    }

    fn invert(self) -> Fe {
        // p - 2 = 2^255 - 21.
        let (z250, z11) = self.pow250();
        z250.pow2k(5).mul(z11)
    }

    /// self^((p - 5) / 8) = self^(2^252 - 3).
    fn pow22523(self) -> Fe {
        let (z250, _) = self.pow250();
        z250.pow2k(2).mul(self)
    }

    fn from_bytes(b: &[u8; 32]) -> Fe {
        let w = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        Fe([
            w(0) & MASK,
            (w(6) >> 3) & MASK,
            (w(12) >> 6) & MASK,
            (w(19) >> 1) & MASK,
            (w(24) >> 12) & MASK,
        ])
    }

    fn to_bytes(self) -> [u8; 32] {
        let mut l = Fe::carry(self.0).0;
        // Fully reduce: subtract p if the value is at least p.
        let mut q = (l[0] + 19) >> 51;
        for x in &l[1..] {
            q = (x + q) >> 51;
        }
        l[0] += 19 * q;
        for i in 0..4 {
            l[i + 1] += l[i] >> 51;
            l[i] &= MASK;
        }
        l[4] &= MASK;
        let mut out = [0u8; 32];
        let mut acc: u128 = 0;
        let mut bits = 0;
        let mut at = 0;
        for x in l {
            acc |= u128::from(x) << bits;
            bits += 51;
            while bits >= 8 && at < 32 {
                out[at] = acc as u8;
                acc >>= 8;
                bits -= 8;
                at += 1;
            }
        }
        if at < 32 {
            out[at] = acc as u8;
        }
        out
    }

    fn is_zero(self) -> bool {
        self.to_bytes() == [0; 32]
    }

    fn is_negative(self) -> bool {
        self.to_bytes()[0] & 1 == 1
    }

    fn eq(self, o: Fe) -> bool {
        self.to_bytes() == o.to_bytes()
    }
}

// ---- Points on the twisted Edwards curve, extended coordinates.

#[derive(Clone, Copy)]
struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    const IDENTITY: Point = Point {
        x: ZERO,
        y: ONE,
        z: ONE,
        t: ZERO,
    };

    fn base() -> Point {
        Point {
            x: BX,
            y: BY,
            z: ONE,
            t: BX.mul(BY),
        }
    }

    fn add(self, q: Point) -> Point {
        let a = self.y.sub(self.x).mul(q.y.sub(q.x));
        let b = self.y.add(self.x).mul(q.y.add(q.x));
        let c = self.t.mul(D2).mul(q.t);
        let d = self.z.add(self.z).mul(q.z);
        let e = b.sub(a);
        let f = d.sub(c);
        let g = d.add(c);
        let h = b.add(a);
        Point {
            x: e.mul(f),
            y: g.mul(h),
            z: f.mul(g),
            t: e.mul(h),
        }
    }

    fn double(self) -> Point {
        self.add(self)
    }

    fn neg(self) -> Point {
        Point {
            x: self.x.neg(),
            t: self.t.neg(),
            ..self
        }
    }

    /// `k` (little-endian) times this point.
    fn mul(self, k: &[u8; 32]) -> Point {
        let mut r = Point::IDENTITY;
        for i in (0..256).rev() {
            r = r.double();
            if (k[i / 8] >> (i % 8)) & 1 == 1 {
                r = r.add(self);
            }
        }
        r
    }

    fn encode(self) -> [u8; 32] {
        let zi = self.z.invert();
        let x = self.x.mul(zi);
        let y = self.y.mul(zi);
        let mut out = y.to_bytes();
        out[31] |= u8::from(x.is_negative()) << 7;
        out
    }

    fn decode(b: &[u8; 32]) -> Option<Point> {
        let sign = b[31] >> 7;
        let mut yb = *b;
        yb[31] &= 0x7f;
        let y = Fe::from_bytes(&yb);
        // y must be below p.
        if y.to_bytes() != yb {
            return None;
        }
        let y2 = y.sq();
        let u = y2.sub(ONE);
        let v = D.mul(y2).add(ONE);
        let v3 = v.sq().mul(v);
        let v7 = v3.sq().mul(v);
        let mut x = u.mul(v3).mul(u.mul(v7).pow22523());
        let vx2 = v.mul(x.sq());
        if !vx2.eq(u) {
            if vx2.eq(u.neg()) {
                x = x.mul(SQRT_M1);
            } else {
                return None;
            }
        }
        if x.is_zero() && sign == 1 {
            return None;
        }
        if u8::from(x.is_negative()) != sign {
            x = x.neg();
        }
        Some(Point {
            x,
            y,
            z: ONE,
            t: x.mul(y),
        })
    }
}

// ---- Scalars: numbers modulo L, the order of the base point.

/// L = 2^252 + 27742317777372353535851937790883648493, little-endian.
const L: [u64; 4] = [
    0x5812631a5cf5d3ed,
    0x14def9dea2f79cd6,
    0,
    0x1000000000000000,
];

/// A number of up to 512 bits (eight little-endian words) modulo L.
fn reduce(n: [u64; 8]) -> [u8; 32] {
    // Long division one bit at a time: slow but plain, and only run once
    // or twice per signature.
    let mut r = [0u64; 5];
    for i in (0..512).rev() {
        // r = 2r + bit
        let mut c = (n[i / 64] >> (i % 64)) & 1;
        for w in r.iter_mut() {
            let top = *w >> 63;
            *w = (*w << 1) | c;
            c = top;
        }
        if !less(&r, &L) {
            sub_l(&mut r);
        }
    }
    let mut out = [0u8; 32];
    for (i, w) in r[..4].iter().enumerate() {
        out[8 * i..8 * i + 8].copy_from_slice(&w.to_le_bytes());
    }
    out
}

fn less(r: &[u64; 5], l: &[u64; 4]) -> bool {
    if r[4] != 0 {
        return false;
    }
    for i in (0..4).rev() {
        if r[i] != l[i] {
            return r[i] < l[i];
        }
    }
    false
}

fn sub_l(r: &mut [u64; 5]) {
    let mut borrow = 0u64;
    for i in 0..5 {
        let l = if i < 4 { L[i] } else { 0 };
        let (a, b1) = r[i].overflowing_sub(l);
        let (a, b2) = a.overflowing_sub(borrow);
        r[i] = a;
        borrow = u64::from(b1 || b2);
    }
}

fn words(b: &[u8]) -> [u64; 8] {
    let mut w = [0u64; 8];
    for (i, c) in b.chunks(8).enumerate() {
        let mut x = [0u8; 8];
        x[..c.len()].copy_from_slice(c);
        w[i] = u64::from_le_bytes(x);
    }
    w
}

/// a * b + c modulo L (all little-endian 32-byte numbers).
fn mul_add(a: &[u8; 32], b: &[u8; 32], c: &[u8; 32]) -> [u8; 32] {
    let (a, b, c) = (words(a), words(b), words(c));
    let mut p = [0u128; 9];
    for i in 0..4 {
        for j in 0..4 {
            let m = u128::from(a[i]) * u128::from(b[j]);
            p[i + j] += m & u128::from(u64::MAX);
            p[i + j + 1] += m >> 64;
        }
    }
    for i in 0..4 {
        p[i] += u128::from(c[i]);
    }
    let mut n = [0u64; 8];
    let mut carry = 0u128;
    for i in 0..8 {
        let v = p[i] + carry;
        n[i] = v as u64;
        carry = v >> 64;
    }
    reduce(n)
}

fn scalar_ok(s: &[u8; 32]) -> bool {
    let w = words(s);
    less(&[w[0], w[1], w[2], w[3], 0], &L)
}

fn expand(secret: &[u8; 32]) -> ([u8; 32], [u8; 32]) {
    let h = sha512(secret);
    let mut a: [u8; 32] = h[..32].try_into().unwrap();
    a[0] &= 248;
    a[31] &= 63;
    a[31] |= 64;
    (a, h[32..].try_into().unwrap())
}

fn hram(r: &[u8; 32], public: &[u8; 32], msg: &[u8]) -> [u8; 32] {
    let mut h = Sha512::new();
    h.update(r);
    h.update(public);
    h.update(msg);
    reduce(words(&h.finish()))
}

/// The public key for a 32-byte secret key (tools and tests only).
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    let (a, _) = expand(secret);
    Point::base().mul(&a).encode()
}

/// Sign `msg` (tools and tests only; see the module notes).
pub fn sign(secret: &[u8; 32], msg: &[u8]) -> [u8; 64] {
    let (a, prefix) = expand(secret);
    let public = Point::base().mul(&a).encode();
    let mut h = Sha512::new();
    h.update(&prefix);
    h.update(msg);
    let r = reduce(words(&h.finish()));
    let rb = Point::base().mul(&r).encode();
    let k = hram(&rb, &public, msg);
    let s = mul_add(&k, &a, &r);
    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&rb);
    sig[32..].copy_from_slice(&s);
    sig
}

/// Whether `sig` is `public`'s signature on `msg`.
pub fn verify(public: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
    let Some(a) = Point::decode(public) else {
        return false;
    };
    let r: [u8; 32] = sig[..32].try_into().unwrap();
    let s: [u8; 32] = sig[32..].try_into().unwrap();
    if !scalar_ok(&s) || Point::decode(&r).is_none() {
        return false;
    }
    let k = hram(&r, public, msg);
    // [s]B - [k]A must be R.
    let p = Point::base().mul(&s).add(a.neg().mul(&k));
    p.encode() == r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, o) in out.iter_mut().enumerate() {
            *o = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        out
    }

    #[test]
    fn sha512_known_answers() {
        assert_eq!(
            sha512(b"abc"),
            hex::<64>(
                "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
                 2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
            )
        );
        let mut s = Sha512::new();
        for _ in 0..1000 {
            s.update(&[b'a'; 1000]);
        }
        assert_eq!(
            s.finish(),
            hex::<64>(
                "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973eb\
                 de0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"
            )
        );
    }

    // RFC 8032, section 7.1, tests 1 to 3.
    const VECTORS: [(&str, &str, &str, &str); 3] = [
        (
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
            "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "",
            "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
        ),
        (
            "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
            "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
            "72",
            "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
        ),
        (
            "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
            "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
            "af82",
            "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
        ),
    ];

    #[test]
    fn rfc8032_signatures_come_out_and_check() {
        for (sk, pk, msg, sig) in VECTORS {
            let sk: [u8; 32] = hex(sk);
            let pk: [u8; 32] = hex(pk);
            let mut m = [0u8; 2];
            for (i, b) in m.iter_mut().enumerate().take(msg.len() / 2) {
                *b = u8::from_str_radix(&msg[2 * i..2 * i + 2], 16).unwrap();
            }
            let m = &m[..msg.len() / 2];
            let sig: [u8; 64] = hex(sig);
            assert_eq!(public_key(&sk), pk);
            assert_eq!(sign(&sk, m), sig);
            assert!(verify(&pk, m, &sig));
        }
    }

    #[test]
    fn any_changed_bit_is_refused() {
        let sk = [7u8; 32];
        let pk = public_key(&sk);
        let msg = b"an app bundle";
        let sig = sign(&sk, msg);
        assert!(verify(&pk, msg, &sig));
        for i in 0..64 {
            let mut bad = sig;
            bad[i] ^= 1 << (i % 8);
            assert!(!verify(&pk, msg, &bad), "signature byte {i}");
        }
        let mut m = *msg;
        m[3] ^= 1;
        assert!(!verify(&pk, &m, &sig));
        let other = public_key(&[8u8; 32]);
        assert!(!verify(&other, msg, &sig));
        // s at or above L is refused (no second signature from s + L).
        let mut big = sig;
        big[32..].copy_from_slice(&[0xff; 32]);
        assert!(!verify(&pk, msg, &big));
    }
}

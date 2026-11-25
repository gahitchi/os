// SPDX-License-Identifier: GPL-3.0-only
//! App bundles (milestone 8, decision 146): one file with an app's name,
//! its maker, its version, its promise (what it may ever ask for) and its
//! program, signed by the maker's Ed25519 key.
//!
//! The layout, all numbers little-endian:
//!
//! | bytes | what |
//! | --- | --- |
//! | 0 to 8 | `ALAYAAP1` |
//! | 8 to 40 | the maker's public key |
//! | 40 to 104 | the signature, over every byte from 104 to the end |
//! | 104 to 108 | the version |
//! | 108 to 112 | the promise ([`promise`] bits) |
//! | 112 to 144 | the name, UTF-8, padded with zeros |
//! | 144 to 148 | the program's length |
//! | 148 to 152 | flags ([`flags`] bits) |
//! | 152 on | the program (an ELF file) |
//!
//! Nothing may follow the program, every byte after the signature is
//! signed, and a promise bit Alaya does not know is refused: an app can
//! never ask for something it could not show you at install.

#![no_std]

use alaya_crypto::{ed25519, sha256};

pub const MAGIC: [u8; 8] = *b"ALAYAAP1";
/// Where the program starts.
pub const HEAD: usize = 152;
/// The largest program a bundle may carry.
pub const MAX_PROGRAM: usize = 4 << 20;
/// The longest name, in bytes.
pub const MAX_NAME: usize = 32;

/// What an app may ever ask for. Each line can be unticked at install
/// (decision 60), and an update never adds one by itself (R11).
pub mod promise {
    /// Open and save the files you hand it (the store).
    pub const FILES: u32 = 1;
    /// Keep running with no window open.
    pub const BACKGROUND: u32 = 2;
    /// Send you notifications (they land in the decision inbox).
    pub const NOTIFY: u32 = 4;
    /// Use the network (milestone 10).
    pub const NETWORK: u32 = 8;
    /// Capture the screen (with the red light, decision 60).
    pub const CAPTURE: u32 = 16;
    pub const ALL: u32 = 31;

    /// Each line as the install sheet shows it.
    pub const LINES: [(u32, &str); 5] = [
        (FILES, "Open and save the files you hand it"),
        (BACKGROUND, "Keep running with no window open"),
        (NOTIFY, "Send you notifications"),
        (NETWORK, "Use the network"),
        (CAPTURE, "Capture the screen"),
    ];
}

/// What the maker says about this version.
pub mod flags {
    /// A security fix: it is never held back or taken back for being
    /// slower (the Experience Guardian, `commitments.md` section 4).
    pub const SECURITY_FIX: u32 = 1;
    pub const ALL: u32 = 1;
}

/// Why a bundle was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bad {
    /// Shorter than its own header says.
    Short,
    /// Not a bundle at all.
    Magic,
    /// The name is empty, too long or not UTF-8.
    Name,
    /// The program is empty or too big, or bytes follow it.
    Length,
    /// It promises something Alaya does not know.
    Promise,
    /// A flag Alaya does not know.
    Reserved,
    /// The signature does not match the maker's key and the contents.
    Signature,
    /// The space given to [`make`] is too small.
    NoRoom,
}

impl Bad {
    /// In plain words, for the install sheet.
    pub fn words(self) -> &'static str {
        match self {
            Bad::Short | Bad::Length => "the file is cut short or has extra bytes",
            Bad::Magic => "this is not an Alaya app",
            Bad::Name => "its name is not readable",
            Bad::Promise => "it asks for something Alaya does not know",
            Bad::Reserved => "it was made for a newer Alaya",
            Bad::Signature => "it was changed after its maker signed it",
            Bad::NoRoom => "there is no room for it",
        }
    }
}

/// A bundle whose signature checked out.
#[derive(Clone, Copy, Debug)]
pub struct Bundle<'a> {
    pub maker: [u8; 32],
    pub version: u32,
    pub promise: u32,
    pub flags: u32,
    name: &'a str,
    pub program: &'a [u8],
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

impl<'a> Bundle<'a> {
    /// Read and check a bundle: its shape first, then its signature.
    pub fn read(b: &'a [u8]) -> Result<Bundle<'a>, Bad> {
        if b.len() < HEAD {
            return Err(Bad::Short);
        }
        if b[..8] != MAGIC {
            return Err(Bad::Magic);
        }
        let len = u32_at(b, 144) as usize;
        if len == 0 || len > MAX_PROGRAM {
            return Err(Bad::Length);
        }
        if b.len() < HEAD + len {
            return Err(Bad::Short);
        }
        if b.len() > HEAD + len {
            return Err(Bad::Length);
        }
        let promise = u32_at(b, 108);
        if promise & !promise::ALL != 0 {
            return Err(Bad::Promise);
        }
        if u32_at(b, 148) & !flags::ALL != 0 {
            return Err(Bad::Reserved);
        }
        let raw = &b[112..144];
        let n = raw.iter().position(|&c| c == 0).unwrap_or(MAX_NAME);
        if n == 0 || raw[n..].iter().any(|&c| c != 0) {
            return Err(Bad::Name);
        }
        let name = core::str::from_utf8(&raw[..n]).map_err(|_| Bad::Name)?;
        let maker: [u8; 32] = b[8..40].try_into().unwrap();
        let sig: [u8; 64] = b[40..104].try_into().unwrap();
        if !ed25519::verify(&maker, &b[104..], &sig) {
            return Err(Bad::Signature);
        }
        Ok(Bundle {
            maker,
            version: u32_at(b, 104),
            promise,
            flags: u32_at(b, 148),
            name,
            program: &b[HEAD..],
        })
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    /// The app's lasting number: the same maker and name are the same
    /// app, so a later version is an update of it, never a stranger.
    pub fn id(&self) -> u64 {
        id_of(&self.maker, self.name)
    }

    /// The maker's key in a short form people can compare: four groups
    /// of four letters and digits from its fingerprint.
    pub fn maker_code(&self) -> [u8; 19] {
        maker_code(&self.maker)
    }
}

/// The lasting number of the app `name` by `maker`.
pub fn id_of(maker: &[u8; 32], name: &str) -> u64 {
    let mut b = [0u8; 32 + MAX_NAME];
    b[..32].copy_from_slice(maker);
    let n = name.len().min(MAX_NAME);
    b[32..32 + n].copy_from_slice(&name.as_bytes()[..n]);
    let h = sha256(&b[..32 + n]);
    u64::from_le_bytes(h[..8].try_into().unwrap())
}

/// A maker key's short form, such as `K4TR-9QMX-2B7D-WNHE`.
pub fn maker_code(maker: &[u8; 32]) -> [u8; 19] {
    // No 0, O, 1, I, L or U: easy to read aloud and to compare.
    const LETTERS: &[u8; 30] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";
    let h = sha256(maker);
    let mut out = [b'-'; 19];
    let mut k = 0;
    for (i, o) in out.iter_mut().enumerate() {
        if i % 5 == 4 {
            continue;
        }
        *o = LETTERS[usize::from(h[k]) % LETTERS.len()];
        k += 1;
    }
    out
}

/// Make a signed bundle in `out` (tools and tests); its length.
pub fn make(
    secret: &[u8; 32],
    name: &str,
    version: u32,
    promise: u32,
    program: &[u8],
    out: &mut [u8],
) -> Result<usize, Bad> {
    make_flagged(secret, name, version, promise, 0, program, out)
}

/// [`make`], with [`flags`].
pub fn make_flagged(
    secret: &[u8; 32],
    name: &str,
    version: u32,
    promise: u32,
    flags: u32,
    program: &[u8],
    out: &mut [u8],
) -> Result<usize, Bad> {
    if flags & !flags::ALL != 0 {
        return Err(Bad::Reserved);
    }
    if name.is_empty() || name.len() > MAX_NAME || name.as_bytes().contains(&0) {
        return Err(Bad::Name);
    }
    if program.is_empty() || program.len() > MAX_PROGRAM {
        return Err(Bad::Length);
    }
    if promise & !promise::ALL != 0 {
        return Err(Bad::Promise);
    }
    let total = HEAD + program.len();
    if out.len() < total {
        return Err(Bad::NoRoom);
    }
    let out = &mut out[..total];
    out.fill(0);
    out[..8].copy_from_slice(&MAGIC);
    out[8..40].copy_from_slice(&ed25519::public_key(secret));
    out[104..108].copy_from_slice(&version.to_le_bytes());
    out[108..112].copy_from_slice(&promise.to_le_bytes());
    out[112..112 + name.len()].copy_from_slice(name.as_bytes());
    out[144..148].copy_from_slice(&(program.len() as u32).to_le_bytes());
    out[148..152].copy_from_slice(&flags.to_le_bytes());
    out[HEAD..].copy_from_slice(program);
    let sig = ed25519::sign(secret, &out[104..]);
    out[40..104].copy_from_slice(&sig);
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: [u8; 32] = [42; 32];
    const PROGRAM: &[u8] = b"\x7fELF pretend program bytes";

    fn good(out: &mut [u8; 512]) -> usize {
        make(
            &KEY,
            "Postcards",
            3,
            promise::FILES | promise::NOTIFY,
            PROGRAM,
            out,
        )
        .unwrap()
    }

    #[test]
    fn a_good_bundle_reads_back() {
        let mut b = [0u8; 512];
        let n = good(&mut b);
        let a = Bundle::read(&b[..n]).unwrap();
        assert_eq!(a.name(), "Postcards");
        assert_eq!(a.version, 3);
        assert_eq!(a.promise, promise::FILES | promise::NOTIFY);
        assert_eq!(a.program, PROGRAM);
        assert_eq!(a.maker, ed25519::public_key(&KEY));
        assert_eq!(a.id(), id_of(&a.maker, "Postcards"));
        let code = a.maker_code();
        assert!(
            code.iter()
                .enumerate()
                .all(|(i, &c)| (i % 5 == 4) == (c == b'-'))
        );
    }

    #[test]
    fn every_changed_byte_is_refused() {
        let mut b = [0u8; 512];
        let n = good(&mut b);
        for i in 0..n {
            for bit in [0x01, 0x80] {
                let mut c = b;
                c[i] ^= bit;
                assert!(Bundle::read(&c[..n]).is_err(), "byte {i} bit {bit:#x}");
            }
        }
    }

    #[test]
    fn a_promise_edited_after_signing_is_refused() {
        let mut b = [0u8; 512];
        let n = good(&mut b);
        b[108] |= promise::CAPTURE as u8;
        assert_eq!(Bundle::read(&b[..n]).unwrap_err(), Bad::Signature);
        // Unknown asks are refused even before the signature.
        b[110] = 1;
        assert_eq!(Bundle::read(&b[..n]).unwrap_err(), Bad::Promise);
    }

    #[test]
    fn another_makers_key_is_refused() {
        let mut b = [0u8; 512];
        let n = good(&mut b);
        b[8..40].copy_from_slice(&ed25519::public_key(&[7; 32]));
        assert_eq!(Bundle::read(&b[..n]).unwrap_err(), Bad::Signature);
    }

    #[test]
    fn cut_short_or_with_extra_bytes_is_refused() {
        let mut b = [0u8; 512];
        let n = good(&mut b);
        for cut in [0, 7, HEAD - 1, HEAD, n - 1] {
            assert!(Bundle::read(&b[..cut]).is_err(), "cut at {cut}");
        }
        assert_eq!(Bundle::read(&b[..n + 1]).unwrap_err(), Bad::Length);
        assert_eq!(
            Bundle::read(b"not a bundle at all").unwrap_err(),
            Bad::Short
        );
    }

    #[test]
    fn a_security_fix_is_signed_and_unknown_flags_refused() {
        let mut b = [0u8; 512];
        let n = make_flagged(
            &KEY,
            "Postcards",
            4,
            promise::FILES,
            flags::SECURITY_FIX,
            PROGRAM,
            &mut b,
        )
        .unwrap();
        assert_eq!(Bundle::read(&b[..n]).unwrap().flags, flags::SECURITY_FIX);
        // Marking a version a security fix after signing is refused.
        let m = good(&mut b);
        b[148] = flags::SECURITY_FIX as u8;
        assert_eq!(Bundle::read(&b[..m]).unwrap_err(), Bad::Signature);
        b[148] = 0x80;
        assert_eq!(Bundle::read(&b[..m]).unwrap_err(), Bad::Reserved);
        assert_eq!(
            make_flagged(&KEY, "Postcards", 4, 0, 2, PROGRAM, &mut [0u8; 512]).unwrap_err(),
            Bad::Reserved
        );
    }

    #[test]
    fn random_bytes_never_read_as_a_bundle() {
        // A small fuzzer: shapes near a real bundle, from a fixed seed.
        let mut b = [0u8; 512];
        let n = good(&mut b);
        let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for _ in 0..2000 {
            let mut c = b;
            let changes = 1 + next() % 6;
            for _ in 0..changes {
                let i = (next() % n as u64) as usize;
                c[i] = next() as u8;
            }
            let len = if next() % 4 == 0 {
                (next() % 512) as usize
            } else {
                n
            };
            if c[..n] == b[..n] && len == n {
                continue;
            }
            assert!(Bundle::read(&c[..len]).is_err());
        }
    }
}

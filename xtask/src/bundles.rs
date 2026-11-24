// SPDX-License-Identifier: GPL-3.0-only
//! The example apps the boot image carries (milestone 8): each program is
//! cut down to what is loaded, wrapped in a bundle and signed with the
//! example maker's key. They go into one boot module, `example-apps`:
//! for each bundle, its length (four bytes, little-endian), then the
//! bundle.
//!
//! The example maker's secret key is public on purpose (it is right
//! here): it signs test apps only, and a real maker keeps theirs secret.

use alaya_bundle::promise;
use alaya_crypto::sha256;

/// The example maker's secret key.
pub fn example_key() -> [u8; 32] {
    sha256(b"Alaya example maker: a public key for test apps only")
}

/// One example app: its name, version, promise and program.
pub struct Example<'a> {
    pub name: &'a str,
    pub version: u32,
    pub promise: u32,
    pub program: &'a [u8],
}

/// An ELF file cut down to its header, its program headers and what they
/// load: no symbols or debugging sections.
pub fn loaded_only(elf: &[u8]) -> Result<Vec<u8>, String> {
    let u16_at = |at: usize| u16::from_le_bytes(elf[at..at + 2].try_into().unwrap());
    let u32_at = |at: usize| u32::from_le_bytes(elf[at..at + 4].try_into().unwrap());
    let u64_at = |at: usize| u64::from_le_bytes(elf[at..at + 8].try_into().unwrap());
    if elf.len() < 64 || elf[..4] != *b"\x7fELF" || elf[4] != 2 || elf[5] != 1 {
        return Err("not a 64-bit little-endian ELF file".into());
    }
    let phoff = u64_at(0x20) as usize;
    let phsize = usize::from(u16_at(0x36));
    let phnum = usize::from(u16_at(0x38));
    let headers_end = 64 + phsize * phnum;
    let mut out = vec![0u8; headers_end];
    out[..64].copy_from_slice(&elf[..64]);
    // Program headers right after the header; no section headers.
    out[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
    out[0x28..0x30].copy_from_slice(&0u64.to_le_bytes());
    out[0x3a..0x3c].copy_from_slice(&0u16.to_le_bytes());
    out[0x3c..0x3e].copy_from_slice(&0u16.to_le_bytes());
    out[0x3e..0x40].copy_from_slice(&0u16.to_le_bytes());
    for i in 0..phnum {
        let ph = phoff + i * phsize;
        let mut h = elf[ph..ph + phsize].to_vec();
        let kind = u32_at(ph);
        let (offset, vaddr, filesz) = (
            u64_at(ph + 8) as usize,
            u64_at(ph + 16),
            u64_at(ph + 32) as usize,
        );
        if kind == 1 && filesz > 0 {
            // Keep the offset in step with the address within its page.
            let mut at = out.len().next_multiple_of(4096) + (vaddr % 4096) as usize;
            if at < out.len() {
                at += 4096;
            }
            out.resize(at, 0);
            out.extend_from_slice(&elf[offset..offset + filesz]);
            h[8..16].copy_from_slice(&(at as u64).to_le_bytes());
        } else {
            h[8..16].copy_from_slice(&0u64.to_le_bytes());
            if kind != 1 {
                h[32..40].copy_from_slice(&0u64.to_le_bytes());
            }
        }
        out[64 + i * phsize..64 + (i + 1) * phsize].copy_from_slice(&h);
    }
    Ok(out)
}

/// The `example-apps` module.
pub fn pack(apps: &[Example]) -> Result<Vec<u8>, String> {
    let key = example_key();
    let mut out = Vec::new();
    for a in apps {
        let mut b = vec![0u8; alaya_bundle::HEAD + a.program.len()];
        let n = alaya_bundle::make(&key, a.name, a.version, a.promise, a.program, &mut b)
            .map_err(|e| format!("bundle {}: {e:?}", a.name))?;
        out.extend_from_slice(&(n as u32).to_le_bytes());
        out.extend_from_slice(&b[..n]);
    }
    Ok(out)
}

/// What Postcards promises in its first version.
pub const POSTCARDS_PROMISE: u32 = promise::FILES | promise::NOTIFY;

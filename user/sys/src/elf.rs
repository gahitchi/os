// SPDX-License-Identifier: GPL-3.0-only
//! Reading static ELF executables, to start programs.

/// A loadable segment.
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub vaddr: u64,
    pub mem_size: u64,
    /// Where its bytes are in the file, and how many.
    pub offset: usize,
    pub file_size: usize,
    pub write: bool,
    pub execute: bool,
}

/// A checked 64-bit x86 executable.
pub struct Elf<'a> {
    pub bytes: &'a [u8],
    pub entry: u64,
    phoff: usize,
    phentsize: usize,
    phnum: usize,
}

/// Highest address a program may use.
const USER_END: u64 = 0x0000_7fff_ffff_f000;

impl<'a> Elf<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Elf<'a>> {
        if bytes.len() < 64 || bytes[..4] != *b"\x7fELF" || bytes[4] != 2 || bytes[5] != 1 {
            return None;
        }
        let e = Elf {
            bytes,
            entry: u64_at(bytes, 24)?,
            phoff: u64_at(bytes, 32)? as usize,
            phentsize: u16_at(bytes, 54)? as usize,
            phnum: u16_at(bytes, 56)? as usize,
        };
        if u16_at(bytes, 18)? != 62 || e.phentsize < 56 {
            return None;
        }
        // Every segment must be sane before anything is loaded.
        for i in 0..e.phnum {
            e.segment(i)?;
        }
        Some(e)
    }

    /// Segment `i`, or None if it is not loadable (or broken).
    fn segment(&self, i: usize) -> Option<Option<Segment>> {
        let ph = self.phoff.checked_add(i.checked_mul(self.phentsize)?)?;
        let b = self.bytes;
        if u32_at(b, ph)? != 1 {
            return Some(None);
        }
        let flags = u32_at(b, ph + 4)?;
        let s = Segment {
            offset: u64_at(b, ph + 8)? as usize,
            vaddr: u64_at(b, ph + 16)?,
            file_size: u64_at(b, ph + 32)? as usize,
            mem_size: u64_at(b, ph + 40)?,
            write: flags & 2 != 0,
            execute: flags & 1 != 0,
        };
        let end = s.vaddr.checked_add(s.mem_size)?;
        if s.vaddr < 4096
            || end > USER_END
            || s.file_size as u64 > s.mem_size
            || s.offset.checked_add(s.file_size)? > b.len()
        {
            return None;
        }
        Some(Some(s))
    }

    pub fn segments(&self) -> impl Iterator<Item = Segment> + '_ {
        (0..self.phnum).filter_map(|i| self.segment(i).flatten())
    }
}

fn u16_at(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

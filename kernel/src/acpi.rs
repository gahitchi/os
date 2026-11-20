// SPDX-License-Identifier: GPL-3.0-only
//! The few ACPI tables the kernel reads itself, at boot:
//!
//! - **MCFG**: where PCI configuration space is (ECAM). The kernel only
//!   passes it on; the device manager in user space walks the bus.
//! - **DMAR** (PC) and **IORT** (ARM): where the device memory guard (the
//!   VT-d IOMMU, or ARM's SMMU) has its registers. The kernel drives it
//!   itself (`iommu.rs`), because it decides what memory a device may
//!   reach.
//!
//! Everything else in ACPI is left to user space later.

use crate::{kprintln, limine, mm};

/// PCI configuration space: one segment's ECAM window.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ecam {
    pub base: u64,
    pub segment: u16,
    pub bus_start: u8,
    pub bus_end: u8,
}

/// What the kernel learned from ACPI.
#[derive(Clone, Copy, Debug, Default)]
pub struct Platform {
    pub ecam: Option<Ecam>,
    /// Register base of the IOMMU unit that covers every PCI device of
    /// segment 0 (DMAR "include all").
    pub iommu: Option<u64>,
    /// The devices that unit covers, when it lists them (None: all).
    pub iommu_scope: Option<Scope>,
    /// The machine's address width for DMA, from DMAR.
    pub dma_width: u8,
}

/// PCI functions an IOMMU unit lists as its own.
#[derive(Clone, Copy, Debug, Default)]
pub struct Scope {
    pub devices: [u16; 32],
    pub count: usize,
    /// It also covers a bridge's whole bus range.
    pub all: bool,
}

impl Scope {
    fn add(&mut self, bdf: u16) {
        if self.count < self.devices.len() {
            self.devices[self.count] = bdf;
            self.count += 1;
        }
    }

    #[allow(dead_code)] // used by one processor family only
    pub fn covers(&self, bdf: u16) -> bool {
        self.all || self.devices[..self.count].contains(&bdf)
    }
}

fn read<T: Copy>(phys: u64) -> T {
    let virt = mm::map_physical(phys, core::mem::size_of::<T>() as u64, false);
    // SAFETY: just mapped; ACPI tables are firmware memory nobody writes.
    unsafe { core::ptr::read_unaligned(virt as *const T) }
}

fn bytes(phys: u64, len: u64) -> &'static [u8] {
    let virt = mm::map_physical(phys, len, false);
    // SAFETY: as for `read`.
    unsafe { core::slice::from_raw_parts(virt as *const u8, len as usize) }
}

pub fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

pub fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

pub fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

/// A whole table at `phys`, if its checksum is right.
fn table(phys: u64) -> Option<&'static [u8]> {
    let header = bytes(phys, 36);
    let len = u64::from(u32_at(header, 4));
    if !(36..=1 << 20).contains(&len) {
        return None;
    }
    let t = bytes(phys, len);
    let sum = t.iter().fold(0u8, |a, &b| a.wrapping_add(b));
    (sum == 0).then_some(t)
}

/// The firmware table with signature `sig`, if there is one.
#[allow(dead_code)] // used by one processor family only
pub fn find(sig: &[u8; 4]) -> Option<&'static [u8]> {
    let rsdp = limine::rsdp()?;
    let r: [u8; 36] = read(rsdp);
    if &r[..8] != b"RSD PTR " {
        return None;
    }
    let (root, wide) = if r[15] >= 2 && u64_at(&r, 24) != 0 {
        (u64_at(&r, 24), true)
    } else {
        (u64::from(u32_at(&r, 16)), false)
    };
    let rt = table(root)?;
    let entry = if wide { 8 } else { 4 };
    (36..rt.len()).step_by(entry).find_map(|i| {
        let at = if wide {
            u64_at(rt, i)
        } else {
            u64::from(u32_at(rt, i))
        };
        table(at).filter(|t| &t[..4] == sig)
    })
}

/// Read the tables. Must run before the first program starts (it maps
/// firmware memory with the boot allocator's page tables).
pub fn init() -> Platform {
    let mut p = Platform::default();
    let Some(rsdp) = limine::rsdp() else {
        kprintln!("acpi: no root pointer; no PCI and no IOMMU");
        return p;
    };
    let r: [u8; 36] = read(rsdp);
    if &r[..8] != b"RSD PTR " {
        kprintln!("acpi: bad root pointer at {rsdp:#x}");
        return p;
    }
    let (root, wide) = if r[15] >= 2 && u64_at(&r, 24) != 0 {
        (u64_at(&r, 24), true)
    } else {
        (u64::from(u32_at(&r, 16)), false)
    };
    let Some(rt) = table(root) else {
        kprintln!("acpi: bad root table at {root:#x}");
        return p;
    };
    let entry = if wide { 8 } else { 4 };
    let mut names = [[0u8; 4]; 32];
    let mut n = 0;
    for i in (36..rt.len()).step_by(entry) {
        let at = if wide {
            u64_at(rt, i)
        } else {
            u64::from(u32_at(rt, i))
        };
        let Some(t) = table(at) else { continue };
        if n < names.len() {
            names[n].copy_from_slice(&t[..4]);
            n += 1;
        }
        match &t[..4] {
            b"MCFG" => p.ecam = mcfg(t),
            b"DMAR" => dmar(t, &mut p),
            b"IORT" => p.iommu = p.iommu.or(iort_smmu(t)),
            _ => {}
        }
    }
    let mut list = [0u8; 32 * 5];
    let mut len = 0;
    for name in &names[..n] {
        list[len..len + 4].copy_from_slice(name);
        list[len + 4] = b' ';
        len += 5;
    }
    kprintln!(
        "acpi: tables {}",
        core::str::from_utf8(&list[..len.saturating_sub(1)]).unwrap_or("?")
    );
    match p.ecam {
        Some(e) => kprintln!(
            "acpi: PCI configuration space at {:#x}, segment {}, buses {}-{}",
            e.base,
            e.segment,
            e.bus_start,
            e.bus_end
        ),
        None => kprintln!("acpi: no PCI configuration space (MCFG)"),
    }
    p
}

fn mcfg(t: &[u8]) -> Option<Ecam> {
    // Entries of 16 bytes after a 44-byte header; take segment 0.
    (44..t.len())
        .step_by(16)
        .filter(|&o| o + 16 <= t.len())
        .map(|o| Ecam {
            base: u64_at(t, o),
            segment: u16_at(t, o + 8),
            bus_start: t[o + 10],
            bus_end: t[o + 11],
        })
        .find(|e| e.segment == 0)
}

/// The first SMMUv3 node of an IORT table: its register base.
fn iort_smmu(t: &[u8]) -> Option<u64> {
    iort_nodes(t)
        .find(|n| n[0] == 4 && n.len() >= 24)
        .map(|n| u64_at(n, 16))
}

/// The nodes of an IORT table.
#[allow(dead_code)] // used by one processor family only
pub fn iort_nodes(t: &[u8]) -> impl Iterator<Item = &[u8]> {
    let count = if t.len() >= 48 { u32_at(t, 36) } else { 0 };
    let mut at = if t.len() >= 48 {
        u32_at(t, 40) as usize
    } else {
        0
    };
    (0..count).map_while(move |_| {
        let len = u16_at(t, at.checked_add(1)?.min(t.len().saturating_sub(2))) as usize;
        if len < 16 || at + len > t.len() {
            return None;
        }
        let node = &t[at..at + len];
        at += len;
        Some(node)
    })
}

fn dmar(t: &[u8], p: &mut Platform) {
    p.dma_width = t[36] + 1;
    let mut o = 48;
    while o + 4 <= t.len() {
        let kind = u16_at(t, o);
        let len = u16_at(t, o + 2) as usize;
        if len < 4 || o + len > t.len() {
            break;
        }
        // DRHD: a remapping unit. Flag bit 0: it covers every device of its
        // segment not listed under another unit; otherwise it lists the
        // devices it covers.
        if kind == 0 && len >= 16 && p.iommu.is_none() {
            let flags = t[o + 4];
            let segment = u16_at(t, o + 6);
            let base = u64_at(t, o + 8);
            if segment == 0 {
                p.iommu = Some(base);
                if flags & 1 == 0 {
                    let mut scope = Scope::default();
                    let mut d = o + 16;
                    while d + 6 <= o + len {
                        let (stype, slen, bus) = (t[d], t[d + 1] as usize, t[d + 5]);
                        if slen < 6 || d + slen > o + len {
                            break;
                        }
                        // A PCI endpoint at the end of a one-step path.
                        if stype == 1 && slen == 8 {
                            let (dev, f) = (t[d + 6], t[d + 7]);
                            scope.add(u16::from(bus) << 8 | u16::from(dev) << 3 | u16::from(f));
                        } else if stype == 2 {
                            // A bridge: everything behind it. Not tracked yet.
                            scope.all = true;
                        }
                        d += slen;
                    }
                    p.iommu_scope = Some(scope);
                }
            }
        }
        o += len;
    }
}

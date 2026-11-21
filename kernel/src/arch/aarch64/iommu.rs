// SPDX-License-Identifier: GPL-3.0-only
//! The device memory guard on ARM: the SMMU, version 3.
//!
//! It works like VT-d on a PC (see the PC's `iommu.rs`): every device starts
//! with no memory at all and reaches only the pages mapped into its I/O
//! space (`Kind::IoSpace`). An I/O space's tables have the same format as
//! the processor's own (stage 1, 4 KiB pages, 48-bit addresses), with the
//! space's number as their tag (ASID); the kernel keeps one *context
//! descriptor* per I/O space in the space's bookkeeping page, and points a
//! device's *stream table entry* at it when the device is attached.
//!
//! Devices are named by stream id, which the firmware's IORT table derives
//! from the PCI requester id (on QEMU they are the same). The stream table
//! has two levels and covers stream ids below 2^14 (PCI buses 0 to 63); its
//! pages, the command queue and the event queue (where the SMMU records
//! blocked accesses) come from the boot allocator.
//!
//! Message interrupts: a device raises one by writing to the interrupt
//! translator's doorbell (`its.rs`), and that write goes through the guard
//! too. The kernel maps the doorbell into every I/O space a device is
//! attached to, at [`MSI_IOVA`], where a PC's devices write theirs; drivers
//! cannot map anything there.

/// What to add to a virtual machine to get one, said when none is found.
pub const HINT: &str = "start QEMU's virt board with iommu=smmuv3";

use core::sync::atomic::{AtomicU64, Ordering};

use alaya_abi::{Error, Kind, iomap, rights};
use alaya_cap::{self as cap, INTERNAL, Memory, SlotRef};

use crate::caps::Phys;
use crate::mm::{self, frames};
use crate::obj::{self, SPACE_META, internal};
use crate::{kprintln, limine, sched};

/// Most I/O spaces at once (their numbers 1..=MAX_DOMAINS are their tags).
pub const MAX_DOMAINS: usize = 255;
/// Devices one I/O space can hold.
pub const MAX_BOUND: usize = 8;
/// Marks a page stamp as naming an I/O space (not an address space).
pub const IO_STAMP: u64 = 1 << 62;
/// Where devices see the interrupt doorbell.
pub const MSI_IOVA: u64 = 0xfee0_0000;
/// Bytes of SMMU registers (two 64 KiB pages).
pub const REGS_BYTES: u64 = 0x2_0000;
/// Physical address of the registers (0: none).
pub static PHYS: AtomicU64 = AtomicU64::new(0);

/// Stream ids covered: 2^SID_BITS.
const SID_BITS: u64 = 14;
/// Stream table entries per second-level page (2^SPLIT, 64 bytes each).
const SPLIT: u64 = 6;
const CMDQ_BITS: u64 = 8;
const EVTQ_BITS: u64 = 7;
/// The context descriptor's place in an I/O space's bookkeeping page.
const CD_OFFSET: u64 = SPACE_META + 0x800;
const _: () = assert!(core::mem::size_of::<obj::SpaceMeta>() <= 0x800);

const IDR0: u64 = 0x0;
const IDR1: u64 = 0x4;
const IDR5: u64 = 0x14;
const CR0: u64 = 0x20;
const CR0ACK: u64 = 0x24;
const CR1: u64 = 0x28;
const CR2: u64 = 0x2c;
const GERROR: u64 = 0x60;
const STRTAB_BASE: u64 = 0x80;
const STRTAB_BASE_CFG: u64 = 0x88;
const CMDQ_BASE: u64 = 0x90;
const CMDQ_PROD: u64 = 0x98;
const CMDQ_CONS: u64 = 0x9c;
const EVENTQ_BASE: u64 = 0xa0;
const EVENTQ_PROD: u64 = 0x1_00a8;
const EVENTQ_CONS: u64 = 0x1_00ac;

const CR0_SMMUEN: u32 = 1 << 0;
const CR0_EVENTQEN: u32 = 1 << 2;
const CR0_CMDQEN: u32 = 1 << 3;

// Stage-1 table entries.
const VALID: u64 = 1 << 0;
const TABLE: u64 = 1 << 1;
const ATTR_DEVICE: u64 = 1 << 2;
const AP_EL0: u64 = 1 << 6;
const AP_RO: u64 = 1 << 7;
const SH_INNER: u64 = 3 << 8;
const AF: u64 = 1 << 10;
const NG: u64 = 1 << 11;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const ADDRESS: u64 = 0x0000_ffff_ffff_f000;

/// The SMMU, once set up. Lives in `Sys`, under the big lock.
pub struct State {
    /// Virtual address of the registers (0: none).
    regs: u64,
    pub physical: u64,
    /// The command queue (virtual) and its next index (with the wrap bit).
    cmdq: u64,
    prod: u32,
    /// The event queue (virtual).
    evtq: u64,
    /// Second-level stream tables (physical), one per 2^SPLIT stream ids.
    l2: u64,
    /// I/O spaces by number - 1, and each number's generation.
    pub domains: [u64; MAX_DOMAINS],
    generation: [u32; MAX_DOMAINS],
    /// Blocked accesses recorded so far.
    pub faults: u64,
}

impl State {
    pub const EMPTY: State = State {
        regs: 0,
        physical: 0,
        cmdq: 0,
        prod: 0,
        evtq: 0,
        l2: 0,
        domains: [0; MAX_DOMAINS],
        generation: [0; MAX_DOMAINS],
        faults: 0,
    };

    pub fn present(&self) -> bool {
        self.regs != 0
    }

    fn r32(&self, off: u64) -> u32 {
        // SAFETY: the registers are mapped as device memory.
        unsafe { core::ptr::read_volatile((self.regs + off) as *const u32) }
    }

    fn w32(&self, off: u64, v: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.regs + off) as *mut u32, v) }
    }

    fn w64(&self, off: u64, v: u64) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.regs + off) as *mut u64, v) }
    }

    fn set_cr0(&self, v: u32) {
        self.w32(CR0, v);
        wait(|| self.r32(CR0ACK) == v, "enable");
    }

    /// Queue one command (no wait).
    fn command(&mut self, w0: u64, w1: u64) {
        let size = 1u32 << CMDQ_BITS;
        // Wait for room: the queue is full when only the wrap bits differ.
        wait(
            || {
                let cons = self.r32(CMDQ_CONS) & (2 * size - 1);
                cons != self.prod ^ size
            },
            "command queue",
        );
        let at = self.cmdq + u64::from(self.prod & (size - 1)) * 16;
        // SAFETY: the command queue page, through the direct map.
        unsafe {
            core::ptr::write_volatile(at as *mut u64, w0);
            core::ptr::write_volatile((at + 8) as *mut u64, w1);
        }
        self.prod = (self.prod + 1) & (2 * size - 1);
        super::cpu::dsb_sy();
        self.w32(CMDQ_PROD, self.prod);
    }

    /// Queue a sync and wait until the SMMU has done everything before it.
    fn sync(&mut self) {
        self.command(0x46, 0);
        let mask = (2u32 << CMDQ_BITS) - 1;
        wait(
            || {
                let cons = self.r32(CMDQ_CONS);
                assert!(
                    (cons >> 24) & 0x7f == 0,
                    "iommu: the SMMU refused a command (error {:#x})",
                    (cons >> 24) & 0x7f
                );
                cons & mask == self.prod
            },
            "command",
        );
    }

    fn forget_ste(&mut self, sid: u32) {
        self.command(0x03 | u64::from(sid) << 32, 1); // CFGI_STE, leaf
    }

    fn forget_tlb(&mut self, asid: u64) {
        self.command(0x11 | asid << 48, 0); // TLBI_NH_ASID
    }

    /// The stream table entry of stream `sid` (virtual).
    fn ste(&self, sid: u32) -> &'static mut [u64; 8] {
        let hhdm = limine::hhdm_offset().unwrap();
        let at = self.l2 + u64::from(sid) * 64 + hhdm;
        // SAFETY: the stream table, through the direct map; edited under
        // the big lock only.
        unsafe { &mut *(at as *mut [u64; 8]) }
    }
}

fn wait(done: impl Fn() -> bool, what: &str) {
    let end = crate::time::now() + crate::time::ns_to_ticks(100_000_000);
    while !done() {
        assert!(crate::time::now() < end, "iommu: {what} did not finish");
        core::hint::spin_loop();
    }
}

fn table(phys: u64) -> &'static mut [u64; 512] {
    obj::get(phys)
}

/// Set up the SMMU at `base` and turn it on: from now on no device can
/// reach any memory until it is given some. Boot only.
pub fn init(k: &mut sched::Sys, base: u64, _buses: (u8, u8), _scope: Option<crate::acpi::Scope>) {
    let hhdm = limine::hhdm_offset().unwrap();
    let s = &mut k.iommu;
    s.regs = mm::map_physical(base, REGS_BYTES, true);
    let (idr0, idr1, idr5) = (s.r32(IDR0), s.r32(IDR1), s.r32(IDR5));
    let two_level = (idr0 >> 27) & 3 != 0;
    let stage1 = idr0 & 1 << 1 != 0;
    let aarch64 = (idr0 >> 2) & 2 != 0;
    let gran4k = idr5 & 1 << 4 != 0;
    if !(two_level && stage1 && aarch64 && gran4k)
        || u64::from(idr1 & 0x3f) < SID_BITS
        || u64::from((idr1 >> 21) & 0x1f) < CMDQ_BITS
        || u64::from((idr1 >> 16) & 0x1f) < EVTQ_BITS
    {
        kprintln!("iommu: the SMMU at {base:#x} lacks what the kernel needs; not used");
        s.regs = 0;
        return;
    }
    s.physical = base;
    s.set_cr0(0);

    // Stream table: the first level, then every second-level page (all
    // entries invalid: devices are blocked).
    let entries = 1u64 << (SID_BITS - SPLIT);
    let l1 = frames::alloc_contiguous(entries * 8, 64);
    s.l2 = frames::alloc_contiguous(64 << SID_BITS, 4096);
    for i in 0..entries {
        let at = (l1 + hhdm + i * 8) as *mut u64;
        let l2 = s.l2 + (i << (SPLIT + 6));
        // SAFETY: the first-level table just allocated.
        unsafe { core::ptr::write_volatile(at, l2 | (SPLIT + 1)) };
    }
    let cmdq = frames::alloc_zeroed();
    let evtq = frames::alloc_zeroed();
    s.cmdq = cmdq + hhdm;
    s.evtq = evtq + hhdm;
    s.prod = 0;
    super::cpu::dsb_sy();

    // Tables and queues are cacheable, inner shareable.
    s.w32(CR1, 0xd75);
    // Record faults of unknown streams; private TLB maintenance.
    s.w32(CR2, 0x6);
    s.w64(STRTAB_BASE, 1 << 62 | l1);
    s.w32(STRTAB_BASE_CFG, (1 << 16 | SPLIT << 6 | SID_BITS) as u32);
    s.w64(CMDQ_BASE, 1 << 62 | cmdq | CMDQ_BITS);
    s.w32(CMDQ_PROD, 0);
    s.w32(CMDQ_CONS, 0);
    s.w64(EVENTQ_BASE, 1 << 62 | evtq | EVTQ_BITS);
    s.w32(EVENTQ_PROD, 0);
    s.w32(EVENTQ_CONS, 0);
    s.set_cr0(CR0_CMDQEN);
    s.command(0x04, 31); // CFGI_ALL
    s.command(0x30, 0); // TLBI_NSNH_ALL
    s.sync();
    s.set_cr0(CR0_CMDQEN | CR0_EVENTQEN);
    s.set_cr0(CR0_CMDQEN | CR0_EVENTQEN | CR0_SMMUEN);
    PHYS.store(base, Ordering::Relaxed);
    let iort = crate::acpi::find(b"IORT");
    IORT.store(iort.map_or(0, |t| t.as_ptr() as u64), Ordering::Relaxed);
    IORT_LEN.store(iort.map_or(0, |t| t.len() as u64), Ordering::Relaxed);
    kprintln!(
        "iommu: SMMUv3 at {base:#x}, stream ids below {}, {}-bit tags, translation on: devices reach no memory until given some",
        1 << SID_BITS,
        if idr0 & 1 << 12 != 0 { 16 } else { 8 }
    );
    if s.r32(GERROR) != 0 {
        kprintln!("iommu: the SMMU reports error {:#x}", s.r32(GERROR));
    }
}

static IORT: AtomicU64 = AtomicU64::new(0);
static IORT_LEN: AtomicU64 = AtomicU64::new(0);

/// Follow the firmware's IORT map for PCI requester id `rid`: the stream
/// id the SMMU sees (if the path goes through one) and the device id the
/// interrupt translator sees. The same numbers when there is no table.
fn iort_map(rid: u16) -> (u32, u32) {
    let (at, len) = (
        IORT.load(Ordering::Relaxed),
        IORT_LEN.load(Ordering::Relaxed),
    );
    let mut id = u32::from(rid);
    if at == 0 {
        return (id, id);
    }
    // SAFETY: the firmware table, mapped read-only at boot and never freed.
    let t = unsafe { core::slice::from_raw_parts(at as *const u8, len as usize) };
    use crate::acpi::{iort_nodes, u32_at};
    let offset_of = |n: &[u8]| n.as_ptr() as usize - t.as_ptr() as usize;
    // Start at the PCI root complex.
    let Some(mut node) = iort_nodes(t).find(|n| n[0] == 2) else {
        return (id, id);
    };
    let mut sid = id;
    for _ in 0..4 {
        let (count, first) = (u32_at(node, 8) as usize, u32_at(node, 12) as usize);
        let next = (0..count)
            .map(|i| first + i * 20)
            .filter(|&o| o + 20 <= node.len())
            .find_map(|o| {
                let (input, n, output, to, flags) = (
                    u32_at(node, o),
                    u32_at(node, o + 4),
                    u32_at(node, o + 8),
                    u32_at(node, o + 12) as usize,
                    u32_at(node, o + 16),
                );
                (flags & 1 == 0 && id >= input && id - input <= n)
                    .then(|| (output + (id - input), to))
            });
        let Some((out, to)) = next else { break };
        id = out;
        let Some(n) = iort_nodes(t).find(|n| offset_of(n) == to) else {
            break;
        };
        if n[0] == 4 {
            sid = id; // the SMMU sees this one
        }
        if n[0] == 0 {
            break; // the interrupt translator: done
        }
        node = n;
    }
    (sid, id)
}

/// The device id the interrupt translator sees for PCI function `rid`.
pub fn device_id(rid: u16) -> u32 {
    iort_map(rid).1
}

/// The address a device writes its interrupt messages to.
pub fn doorbell() -> u64 {
    let its = super::its::PHYS.load(Ordering::Relaxed);
    if PHYS.load(Ordering::Relaxed) != 0 {
        MSI_IOVA + (super::its::TRANSLATER & 0xfff)
    } else {
        its + super::its::TRANSLATER
    }
}

/// The I/O space a page capability's stamp names, if it still exists.
pub fn resolve(k: &sched::Sys, stamp: u64) -> Option<u64> {
    let id = (stamp & 0xffff_ffff) as usize;
    let i = id.checked_sub(1)?;
    let space = *k.iommu.domains.get(i)?;
    (space != 0 && u64::from(k.iommu.generation[i]) == (stamp >> 32) & 0x3fff_ffff).then_some(space)
}

/// `IOSPACE_ASSIGN`: move the untyped in `pool` into the I/O space and give
/// it a number (its tag).
pub fn assign(m: &mut Phys, space: u64, pool: SlotRef) -> Result<(), Error> {
    if !m.sys.iommu.present() {
        return Err(Error::NoSuchCall);
    }
    let meta = obj::space_meta(space);
    if !meta.pool.cap.is_empty() {
        return Err(Error::SlotInUse);
    }
    let s = m.read(pool);
    if !s.cap.is(Kind::Untyped) {
        return Err(Error::WrongKind);
    }
    if s.cap.rights & rights::WRITE == 0 {
        return Err(Error::NoRights);
    }
    if s.first != 0 || s.cap.a != 0 || s.cap.revoking() {
        return Err(Error::Busy);
    }
    if meta.id == 0 {
        let id = m
            .sys
            .iommu
            .domains
            .iter()
            .position(|&d| d == 0)
            .ok_or(Error::NoMemory)?;
        m.sys.iommu.domains[id] = space;
        meta.id = id as u64 + 1;
        // A number used before may still have translations cached.
        m.sys.iommu.forget_tlb(meta.id);
        m.sys.iommu.sync();
    }
    let dst = space + SPACE_META;
    cap::move_cap(m, pool, dst)?;
    let mut d = m.read(dst);
    d.cap.flags |= INTERNAL;
    d.cap._reserved = internal::IOSPACE_POOL;
    m.write(dst, &d);
    Ok(())
}

fn take_table(m: &mut Phys, space: u64) -> Option<u64> {
    let dst = space + SPACE_META;
    let mut s = m.read(dst);
    if !s.cap.is(Kind::Untyped) {
        return None;
    }
    if s.cap.a + 4096 > 1u64 << s.cap.size_bits {
        return None;
    }
    let phys = s.cap.object + s.cap.a;
    m.zero(phys, 4096);
    s.cap.a += 4096;
    s.cap.b = s.cap.b.max(s.cap.a);
    m.write(dst, &s);
    obj::space_meta(space).tables += 1;
    Some(phys)
}

/// Put `entry` at `iova` in `space`'s tables at level `leaf` (1: 4 KiB),
/// taking tables from the space's pool as needed.
fn put(m: &mut Phys, space: u64, iova: u64, leaf: u32, entry: u64) -> Result<(), Error> {
    let mut t = space;
    let mut level = 4;
    while level > leaf {
        let i = (iova >> (12 + 9 * (level - 1))) as usize & 511;
        let e = table(t)[i];
        if e & VALID == 0 {
            let new = take_table(m, space).ok_or(Error::NoMemory)?;
            table(t)[i] = new | VALID | TABLE;
            t = new;
        } else if e & TABLE == 0 {
            return Err(Error::SlotInUse);
        } else {
            t = e & ADDRESS;
        }
        level -= 1;
    }
    let i = (iova >> (12 + 9 * (leaf - 1))) as usize & 511;
    if table(t)[i] & VALID != 0 {
        return Err(Error::SlotInUse);
    }
    table(t)[i] = entry;
    super::cpu::dsb_ish();
    Ok(())
}

/// The leaf entry for `iova` in `space` and where it is, if mapped.
fn find(space: u64, iova: u64, leaf: u32) -> Option<(u64, usize)> {
    let mut t = space;
    let mut level = 4;
    while level > leaf {
        let e = table(t)[(iova >> (12 + 9 * (level - 1))) as usize & 511];
        if e & VALID == 0 || e & TABLE == 0 {
            return None;
        }
        t = e & ADDRESS;
        level -= 1;
    }
    let i = (iova >> (12 + 9 * (leaf - 1))) as usize & 511;
    (table(t)[i] & VALID != 0).then_some((t, i))
}

/// `DEVICE_BIND`: the PCI function `bdf` (bus << 8 | device << 3 |
/// function) now reaches exactly what is mapped in `space`.
pub fn bind(k: &mut sched::Sys, space: u64, bdf: u16) -> Result<(), Error> {
    let meta = obj::space_meta(space);
    if !k.iommu.present() || meta.id == 0 {
        return Err(Error::InvalidArgument);
    }
    let sid = iort_map(bdf).0;
    if u64::from(sid) >= 1 << SID_BITS {
        return Err(Error::InvalidArgument);
    }
    if k.iommu.ste(sid)[0] & VALID != 0 {
        return Err(Error::SlotInUse);
    }
    let slot = meta
        .bound
        .iter()
        .position(|&b| b == 0)
        .ok_or(Error::NoMemory)?;
    // The interrupt doorbell, so the device can raise its interrupts.
    let its = super::its::PHYS.load(Ordering::Relaxed);
    if its != 0 && find(space, MSI_IOVA, 1).is_none() {
        let mut m = Phys::new(k);
        let page = its + (super::its::TRANSLATER & !0xfff);
        let e = page | VALID | TABLE | ATTR_DEVICE | AP_EL0 | SH_INNER | AF | NG | PXN | UXN;
        put(&mut m, space, MSI_IOVA, 1, e)?;
    }
    write_cd(space, meta.id);
    let cd = space + CD_OFFSET; // objects are named by their physical address
    let ste = k.iommu.ste(sid);
    // Stage 1 translation with the space's descriptor; tables cacheable,
    // inner shareable; incoming shareability.
    ste[1] = 1 << 2 | 1 << 4 | 3 << 6 | 1 << 44;
    super::cpu::dsb_sy();
    ste[0] = VALID | 0b101 << 1 | cd;
    super::cpu::dsb_sy();
    meta.bound[slot] = u32::from(bdf) + 1;
    k.iommu.forget_ste(sid);
    k.iommu.sync();
    Ok(())
}

/// Fill in the context descriptor of `space`: its tables, tag `asid`.
fn write_cd(space: u64, asid: u64) {
    let cd: &mut [u64; 8] = obj::get(space + CD_OFFSET);
    // 48-bit addresses (T0SZ 16), 4 KiB pages, cacheable inner-shareable
    // walks, no upper half, valid, 48-bit output, faults on missing access
    // flags off, AArch64 format, record and abort faults, private tag.
    cd[0] = 16
        | 1 << 8
        | 1 << 10
        | 3 << 12
        | 1 << 30
        | 1 << 31
        | 5 << 32
        | 1 << 35
        | 1 << 41
        | 1 << 45
        | 1 << 46
        | 1 << 47
        | asid << 48;
    cd[1] = space;
    cd[2] = 0;
    // Memory types: 0 normal write-back, 1 device.
    cd[3] = 0x04ff;
    super::cpu::dsb_sy();
}

fn unbind_all(k: &mut sched::Sys, space: u64) {
    let meta = obj::space_meta(space);
    let mut any = false;
    for b in meta.bound.iter_mut().filter(|b| **b != 0) {
        let sid = iort_map((*b - 1) as u16).0;
        let ste = k.iommu.ste(sid);
        ste[0] = 0;
        super::cpu::dsb_sy();
        ste[1] = 0;
        k.iommu.forget_ste(sid);
        *b = 0;
        any = true;
    }
    if any {
        k.iommu.forget_tlb(meta.id);
        k.iommu.sync();
    }
}

fn level_of(bits: u8) -> u32 {
    match bits {
        12 => 1,
        21 => 2,
        _ => 3,
    }
}

/// `PAGE_IOMAP`: let the devices of `space` reach the page in `at` at
/// device address `iova`.
pub fn map_page(m: &mut Phys, at: SlotRef, space: u64, iova: u64, flags: u64) -> Result<(), Error> {
    let mut s = m.read(at);
    let c = s.cap;
    if c.b != 0 {
        return Err(Error::SlotInUse);
    }
    let meta = obj::space_meta(space);
    if meta.id == 0 {
        return Err(Error::InvalidArgument);
    }
    let size = 1u64 << c.size_bits;
    if flags & !iomap::WRITE != 0
        || !iova.is_multiple_of(size)
        || iova.checked_add(size).is_none_or(|e| e > 1 << 48)
        || (iova < MSI_IOVA + 4096 && MSI_IOVA < iova + size)
    {
        return Err(Error::InvalidArgument);
    }
    if flags & iomap::WRITE != 0 && c.rights & rights::WRITE == 0 {
        return Err(Error::NoRights);
    }
    // Devices only reach RAM through the guard, never other devices.
    if !mm::is_ram(c.object, size) {
        return Err(Error::InvalidArgument);
    }
    let leaf = level_of(c.size_bits);
    let mut e = c.object | VALID | AP_EL0 | SH_INNER | AF | NG | PXN | UXN;
    if leaf == 1 {
        e |= TABLE;
    }
    if flags & iomap::WRITE == 0 {
        e |= AP_RO;
    }
    put(m, space, iova, leaf, e)?;
    s.cap.a = iova | flags;
    let generation = u64::from(m.sys.iommu.generation[meta.id as usize - 1]);
    s.cap.b = IO_STAMP | (generation & 0x3fff_ffff) << 32 | meta.id;
    m.write(at, &s);
    Ok(())
}

/// Undo `map_page` for a page mapped in I/O space stamp `stamp`.
pub fn unmap(k: &mut sched::Sys, stamp: u64, iova: u64, size_bits: u8, object: u64) {
    let Some(space) = resolve(k, stamp) else {
        return;
    };
    let Some((t, i)) = find(space, iova, level_of(size_bits)) else {
        return;
    };
    if table(t)[i] & ADDRESS & !((1u64 << size_bits) - 1) == object {
        table(t)[i] = 0;
        super::cpu::dsb_sy();
        k.iommu.forget_tlb(obj::space_meta(space).id);
        k.iommu.sync();
    }
}

/// The pool was taken away: drop every mapping.
pub fn clear(k: &mut sched::Sys, space: u64) {
    table(space).fill(0);
    obj::space_meta(space).tables = 0;
    super::cpu::dsb_sy();
    k.iommu.forget_tlb(obj::space_meta(space).id);
    k.iommu.sync();
}

/// The last capability to the I/O space goes.
pub fn finalise(m: &mut Phys, g: &mut cap::Globals, space: u64) -> bool {
    unbind_all(m.sys, space);
    let pool = space + SPACE_META;
    if m.read(pool).cap.is_live() && !cap::delete_any(m, g, pool) {
        return false;
    }
    let meta = obj::space_meta(space);
    if meta.id != 0 {
        let i = meta.id as usize - 1;
        m.sys.iommu.domains[i] = 0;
        m.sys.iommu.generation[i] = m.sys.iommu.generation[i].wrapping_add(1);
        meta.id = 0;
    }
    true
}

/// Read and clear the SMMU's event records; returns how many blocked
/// accesses it has reported since boot.
pub fn poll_faults(k: &mut sched::Sys) -> u64 {
    let s = &mut k.iommu;
    if !s.present() {
        return 0;
    }
    let size = 1u32 << EVTQ_BITS;
    let mask = 2 * size - 1;
    let prod = s.r32(EVENTQ_PROD) & mask;
    let mut cons = s.r32(EVENTQ_CONS) & mask;
    while cons != prod {
        let at = s.evtq + u64::from(cons & (size - 1)) * 32;
        // SAFETY: the event queue page, through the direct map.
        let (w0, w2) = unsafe {
            (
                core::ptr::read_volatile(at as *const u64),
                core::ptr::read_volatile((at + 16) as *const u64),
            )
        };
        s.faults += 1;
        if s.faults <= 10 {
            let sid = w0 >> 32;
            kprintln!(
                "iommu: blocked device {:02x}:{:02x}.{} at {:#x} (event {:#x})",
                (sid >> 8) & 0xff,
                (sid >> 3) & 0x1f,
                sid & 7,
                w2 & !0xfff,
                w0 & 0xff
            );
        }
        cons = (cons + 1) & mask;
    }
    s.w32(EVENTQ_CONS, cons);
    s.faults
}

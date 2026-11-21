// SPDX-License-Identifier: GPL-3.0-only
//! The device memory guard: Intel VT-d (the IOMMU).
//!
//! A device reads and writes memory by itself (DMA). Without a guard, a
//! buggy or hostile device, or a driver that hands it a wrong address, can
//! overwrite anything, the kernel included. With the guard on, every
//! device starts with **no** memory at all; it can only reach pages that
//! were mapped into its *I/O space* (`Kind::IoSpace`), the way a program
//! can only reach pages mapped into its address space.
//!
//! An I/O space is made from untyped memory like an address space (8 KiB:
//! its top table and its bookkeeping) and is given an untyped of its own
//! for lower tables (decision 56's rule, so the kernel still allocates
//! nothing). The device manager attaches devices to it (`DEVICE_BIND`) and
//! a driver maps pages into it (`PAGE_IOMAP`) at addresses it chooses: the
//! addresses the device then uses.
//!
//! The kernel takes the root and context tables from the boot allocator,
//! turns translation on at boot, and uses register-based invalidation, which
//! every VT-d unit has. Only the unit that covers all PCI devices of segment
//! 0 is used for now; interrupt remapping is not on yet.

/// What to add to a virtual machine to get one, said when none is found.
pub const HINT: &str = "add QEMU's intel-iommu";

use alaya_abi::{Error, Kind, iomap, rights};
use alaya_cap::{self as cap, INTERNAL, Memory, SlotRef};

use crate::caps::Phys;
use crate::mm::{self, frames};
use crate::obj::{self, SPACE_META, internal};
use crate::{kprintln, sched};

/// Most I/O spaces at once (VT-d domain ids 1..=MAX_DOMAINS).
pub const MAX_DOMAINS: usize = 255;
/// Devices one I/O space can hold.
pub const MAX_BOUND: usize = 8;

// Register offsets.
const VER: u64 = 0x0;
const CAP: u64 = 0x8;
const ECAP: u64 = 0x10;
const GCMD: u64 = 0x18;
const GSTS: u64 = 0x1c;
const RTADDR: u64 = 0x20;
const CCMD: u64 = 0x28;
const FSTS: u64 = 0x34;

const GCMD_TE: u32 = 1 << 31;
const GCMD_SRTP: u32 = 1 << 30;
/// GSTS bits that are status of one-shot commands, not settings.
const GSTS_ONE_SHOT: u32 = 0x96ff_ffff;

// Second-level page table entries.
const SL_READ: u64 = 1 << 0;
const SL_WRITE: u64 = 1 << 1;
const SL_HUGE: u64 = 1 << 7;
const ADDRESS: u64 = 0x000f_ffff_ffff_f000;

/// The IOMMU unit, once set up. Lives in `Sys`, under the big lock.
pub struct State {
    /// Virtual address of the registers (0: no IOMMU).
    regs: u64,
    pub physical: u64,
    cap: u64,
    ecap: u64,
    /// 3 or 4 levels of tables.
    levels: u32,
    coherent: bool,
    /// Context tables, one per bus (physical; 0: none).
    context: [u64; 256],
    /// I/O spaces by domain id - 1, and each id's generation (moves on
    /// when its space goes, so old page stamps stop matching).
    pub domains: [u64; MAX_DOMAINS],
    generation: [u32; MAX_DOMAINS],
    /// The devices it covers when it lists them (None: all of segment 0).
    pub scope: Option<crate::acpi::Scope>,
    /// Faults the unit reported so far (devices reaching memory they were
    /// not given).
    pub faults: u64,
}

impl State {
    pub const EMPTY: State = State {
        regs: 0,
        physical: 0,
        cap: 0,
        ecap: 0,
        levels: 0,
        coherent: true,
        context: [0; 256],
        domains: [0; MAX_DOMAINS],
        generation: [0; MAX_DOMAINS],
        scope: None,
        faults: 0,
    };

    pub fn present(&self) -> bool {
        self.regs != 0
    }

    fn r32(&self, off: u64) -> u32 {
        // SAFETY: the register page is mapped uncached; reads have no side
        // effects other than those VT-d defines.
        unsafe { core::ptr::read_volatile((self.regs + off) as *const u32) }
    }

    fn r64(&self, off: u64) -> u64 {
        // SAFETY: as above.
        unsafe { core::ptr::read_volatile((self.regs + off) as *const u64) }
    }

    fn w32(&self, off: u64, v: u32) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.regs + off) as *mut u32, v) }
    }

    fn w64(&self, off: u64, v: u64) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile((self.regs + off) as *mut u64, v) }
    }

    fn command(&self, bit: u32) {
        let status = self.r32(GSTS) & GSTS_ONE_SHOT;
        self.w32(GCMD, status | bit);
        wait(|| self.r32(GSTS) & bit != 0, "command");
    }

    /// Drop every cached context entry.
    fn flush_context(&self) {
        // ICC, global invalidation.
        self.w64(CCMD, 1 << 63 | 1 << 61);
        wait(|| self.r64(CCMD) & 1 << 63 == 0, "context flush");
    }

    /// Drop cached translations: of one domain, or all (`did` 0).
    fn flush_iotlb(&self, did: u64) {
        let reg = ((self.ecap >> 8) & 0x3ff) * 16 + 8;
        let what = if did == 0 {
            1 << 60
        } else {
            2 << 60 | did << 32
        };
        // IVT, drain reads and writes.
        self.w64(reg, 1 << 63 | what | 1 << 49 | 1 << 48);
        wait(|| self.r64(reg) & 1 << 63 == 0, "IOTLB flush");
    }

    /// Make a table update visible to a unit that does not snoop caches.
    fn sync(&self, phys: u64) {
        if !self.coherent {
            let hhdm = crate::limine::hhdm_offset().unwrap();
            super::paging::clean_for_device(phys + hhdm);
        }
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

/// Set up the unit at `base` and turn translation on: from now on no device
/// can reach any memory until it is given some. Boot only.
pub fn init(k: &mut sched::Sys, base: u64, buses: (u8, u8), scope: Option<crate::acpi::Scope>) {
    let s = &mut k.iommu;
    s.scope = scope;
    s.regs = mm::map_physical(base, 4096, true);
    s.physical = base;
    s.cap = s.r64(CAP);
    s.ecap = s.r64(ECAP);
    let sagaw = (s.cap >> 8) & 0x1f;
    s.levels = if sagaw & 0b100 != 0 {
        4
    } else if sagaw & 0b10 != 0 {
        3
    } else {
        kprintln!("iommu: unit at {base:#x} has no 3- or 4-level tables; not used");
        s.regs = 0;
        return;
    };
    s.coherent = s.ecap & 1 != 0;
    let root = frames::alloc_zeroed();
    for bus in buses.0..=buses.1 {
        let ctx = frames::alloc_zeroed();
        s.context[bus as usize] = ctx;
        // A root entry: present, pointing at the bus's context table.
        table(root)[bus as usize * 2] = ctx | 1;
    }
    s.sync(root);
    s.w64(RTADDR, root);
    s.command(GCMD_SRTP);
    s.flush_context();
    s.flush_iotlb(0);
    s.command(GCMD_TE);
    let ver = s.r32(VER);
    let domains = 1u64 << (4 + 2 * (s.cap & 7));
    kprintln!(
        "iommu: VT-d {}.{} at {base:#x}, {}-level tables, {} domains, translation on: devices reach no memory until given some",
        (ver >> 4) & 0xf,
        ver & 0xf,
        s.levels,
        domains
    );
    if let Some(sc) = s.scope {
        kprintln!(
            "iommu: the unit guards {} listed device(s){}",
            sc.count,
            if sc.all { " and bridges" } else { "" }
        );
    }
}

/// The I/O space a page capability's stamp names, if it still exists.
pub fn resolve(k: &sched::Sys, stamp: u64) -> Option<u64> {
    let id = (stamp & 0xffff_ffff) as usize;
    let i = id.checked_sub(1)?;
    let space = *k.iommu.domains.get(i)?;
    (space != 0 && u64::from(k.iommu.generation[i]) == (stamp >> 32) & 0x3fff_ffff).then_some(space)
}

/// The top table the unit walks for `space` (with 3 levels, the table
/// under entry 0 of the 4-level top).
fn walk_root(k: &sched::Sys, space: u64) -> Option<u64> {
    if k.iommu.levels == 4 {
        return Some(space);
    }
    let e = table(space)[0];
    (e & SL_READ != 0).then_some(e & ADDRESS)
}

/// `IOSPACE_ASSIGN`: move the untyped in `pool` into the I/O space and give
/// it a domain id.
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
    }
    let dst = space + SPACE_META;
    cap::move_cap(m, pool, dst)?;
    let mut d = m.read(dst);
    d.cap.flags |= INTERNAL;
    d.cap._reserved = internal::IOSPACE_POOL;
    m.write(dst, &d);
    if m.sys.iommu.levels == 3 && table(space)[0] == 0 {
        let t = take_table(m, space).ok_or(Error::NoMemory)?;
        table(space)[0] = t | SL_READ | SL_WRITE;
    }
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

/// `DEVICE_BIND`: the PCI function `bdf` (bus << 8 | device << 3 |
/// function) now reaches exactly what is mapped in `space`.
pub fn bind(k: &mut sched::Sys, space: u64, bdf: u16) -> Result<(), Error> {
    let s = &k.iommu;
    let meta = obj::space_meta(space);
    if !s.present() || meta.id == 0 {
        return Err(Error::InvalidArgument);
    }
    if s.scope.is_some_and(|sc| !sc.covers(bdf)) {
        // Another unit (not driven yet) or none guards this device.
        return Err(Error::InvalidArgument);
    }
    let ctx = s.context[(bdf >> 8) as usize];
    if ctx == 0 {
        return Err(Error::InvalidArgument);
    }
    let i = (bdf & 0xff) as usize * 2;
    let t = table(ctx);
    if t[i] & 1 != 0 {
        return Err(Error::SlotInUse);
    }
    let slot = meta
        .bound
        .iter()
        .position(|&b| b == 0)
        .ok_or(Error::NoMemory)?;
    let root = walk_root(k, space).ok_or(Error::InvalidArgument)?;
    let aw = if k.iommu.levels == 4 { 2 } else { 1 };
    t[i + 1] = aw | meta.id << 8;
    t[i] = root | 1;
    meta.bound[slot] = u32::from(bdf) + 1;
    let s = &k.iommu;
    s.sync(ctx + i as u64 * 8);
    s.flush_context();
    s.flush_iotlb(0);
    Ok(())
}

fn unbind_all(k: &mut sched::Sys, space: u64) {
    let meta = obj::space_meta(space);
    let mut any = false;
    for b in meta.bound.iter_mut().filter(|b| **b != 0) {
        let bdf = (*b - 1) as u16;
        let ctx = k.iommu.context[(bdf >> 8) as usize];
        let i = (bdf & 0xff) as usize * 2;
        table(ctx)[i] = 0;
        table(ctx)[i + 1] = 0;
        k.iommu.sync(ctx + i as u64 * 8);
        *b = 0;
        any = true;
    }
    if any {
        k.iommu.flush_context();
        k.iommu.flush_iotlb(0);
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
    let limit = if m.sys.iommu.levels == 4 {
        1 << 48
    } else {
        1 << 39
    };
    if flags & !iomap::WRITE != 0
        || !iova.is_multiple_of(size)
        || iova.checked_add(size).is_none_or(|e| e > limit)
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
    let mut t = walk_root(m.sys, space).ok_or(Error::InvalidArgument)?;
    let mut level = m.sys.iommu.levels;
    while level > leaf {
        let i = (iova >> (12 + 9 * (level - 1))) as usize & 511;
        let e = table(t)[i];
        if e & (SL_READ | SL_WRITE) == 0 {
            let new = take_table(m, space).ok_or(Error::NoMemory)?;
            table(t)[i] = new | SL_READ | SL_WRITE;
            m.sys.iommu.sync(t + i as u64 * 8);
            t = new;
        } else if e & SL_HUGE != 0 {
            return Err(Error::SlotInUse);
        } else {
            t = e & ADDRESS;
        }
        level -= 1;
    }
    let i = (iova >> (12 + 9 * (leaf - 1))) as usize & 511;
    if table(t)[i] & (SL_READ | SL_WRITE) != 0 {
        return Err(Error::SlotInUse);
    }
    let mut e = c.object | SL_READ;
    if flags & iomap::WRITE != 0 {
        e |= SL_WRITE;
    }
    if leaf > 1 {
        e |= SL_HUGE;
    }
    table(t)[i] = e;
    m.sys.iommu.sync(t + i as u64 * 8);
    s.cap.a = iova | flags;
    let generation = u64::from(m.sys.iommu.generation[meta.id as usize - 1]);
    s.cap.b = IO_STAMP | (generation & 0x3fff_ffff) << 32 | meta.id;
    m.write(at, &s);
    Ok(())
}

/// Marks a page stamp as naming an I/O space (not an address space).
pub const IO_STAMP: u64 = 1 << 62;

/// Undo `map_page` for a page mapped in I/O space stamp `stamp`.
pub fn unmap(k: &mut sched::Sys, stamp: u64, iova: u64, size_bits: u8, object: u64) {
    let Some(space) = resolve(k, stamp) else {
        return;
    };
    let leaf = level_of(size_bits);
    let Some(mut t) = walk_root(k, space) else {
        return;
    };
    let mut level = k.iommu.levels;
    while level > leaf {
        let e = table(t)[(iova >> (12 + 9 * (level - 1))) as usize & 511];
        if e & (SL_READ | SL_WRITE) == 0 || e & SL_HUGE != 0 {
            return;
        }
        t = e & ADDRESS;
        level -= 1;
    }
    let i = (iova >> (12 + 9 * (leaf - 1))) as usize & 511;
    let e = table(t)[i];
    if e & (SL_READ | SL_WRITE) != 0 && e & ADDRESS & !((1u64 << size_bits) - 1) == object {
        table(t)[i] = 0;
        k.iommu.sync(t + i as u64 * 8);
        k.iommu.flush_iotlb(obj::space_meta(space).id);
    }
}

/// The pool was taken away: drop every mapping.
pub fn clear(k: &mut sched::Sys, space: u64) {
    table(space).fill(0);
    obj::space_meta(space).tables = 0;
    if k.iommu.levels == 3 {
        // The 3-level top table came from the pool: devices lose
        // everything at once.
        unbind_all(k, space);
    } else {
        k.iommu.sync(space);
        k.iommu.flush_iotlb(obj::space_meta(space).id);
    }
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

/// Read and clear the unit's fault records; returns how many faults it has
/// reported since boot.
pub fn poll_faults(k: &mut sched::Sys) -> u64 {
    let s = &mut k.iommu;
    if !s.present() {
        return 0;
    }
    let fro = ((s.cap >> 24) & 0x3ff) * 16;
    let nfr = ((s.cap >> 40) & 0xff) + 1;
    for i in 0..nfr {
        let hi_off = fro + i * 16 + 8;
        let hi = s.r64(hi_off);
        if hi & 1 << 63 != 0 {
            let lo = s.r64(fro + i * 16);
            let sid = hi & 0xffff;
            let reason = (hi >> 32) & 0xff;
            s.faults += 1;
            if s.faults <= 10 {
                kprintln!(
                    "iommu: blocked device {:02x}:{:02x}.{} at {:#x} (reason {:#x})",
                    sid >> 8,
                    (sid >> 3) & 0x1f,
                    sid & 7,
                    lo & !0xfff,
                    reason
                );
            }
            // Clear the record (write 1 to F).
            s.w64(hi_off, 1 << 63);
        }
    }
    // Clear overflow and pending bits.
    s.w32(FSTS, s.r32(FSTS));
    s.faults
}

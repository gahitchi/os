// SPDX-License-Identifier: GPL-3.0-only
//! The parts of the Limine boot protocol the kernel uses.
//!
//! Written here rather than taken from a crate so that the kernel builds on
//! stable Rust and depends on nothing outside this repository. Reference:
//! the Limine protocol specification (`PROTOCOL.md` in the Limine sources).
//!
//! Limine scans the kernel image for the request structures below, between
//! the start and end markers, and fills in their response pointers before
//! jumping to the kernel.

use core::cell::UnsafeCell;
use core::ffi::{CStr, c_char};
use core::ptr;

/// The base revision of the protocol this kernel is written against.
pub const REVISION: u64 = 3;

const COMMON_MAGIC: [u64; 2] = [0xc7b1dd30df4c8b88, 0x0a82e883a194f07b];

/// A value the bootloader may write to before the kernel starts.
#[repr(transparent)]
struct BootCell<T>(UnsafeCell<T>);

// SAFETY: written only by the bootloader, before any kernel code runs;
// the kernel only reads it, with volatile reads.
unsafe impl<T> Sync for BootCell<T> {}

impl<T: Copy> BootCell<T> {
    const fn new(value: T) -> Self {
        Self(UnsafeCell::new(value))
    }

    fn read(&self) -> T {
        // SAFETY: the pointer is valid and aligned; see the `Sync` note.
        unsafe { ptr::read_volatile(self.0.get()) }
    }
}

/// Tells Limine which base revision the kernel expects.
#[repr(C)]
pub struct BaseRevision {
    magic: [u64; 2],
    revision: BootCell<u64>,
}

impl BaseRevision {
    /// Limine sets the revision word to zero when it supports the revision.
    pub fn is_supported(&self) -> bool {
        self.revision.read() == 0
    }
}

#[repr(C)]
struct Request<Resp, Extra = ()> {
    id: [u64; 4],
    revision: u64,
    response: BootCell<*const Resp>,
    extra: Extra,
}

impl<Resp, Extra> Request<Resp, Extra> {
    const fn new(id: [u64; 2], extra: Extra) -> Self {
        Self {
            id: [COMMON_MAGIC[0], COMMON_MAGIC[1], id[0], id[1]],
            revision: 0,
            response: BootCell::new(ptr::null()),
            extra,
        }
    }

    fn response(&self) -> Option<&'static Resp> {
        // SAFETY: a non-null response points to bootloader-reclaimable memory
        // that Limine keeps mapped and that the kernel never frees.
        unsafe { self.response.read().as_ref() }
    }
}

#[repr(C)]
pub struct BootloaderInfoResponse {
    revision: u64,
    name: *const c_char,
    version: *const c_char,
}

#[repr(C)]
pub struct HhdmResponse {
    revision: u64,
    offset: u64,
}

#[repr(C)]
pub struct MpResponse {
    revision: u64,
    pub flags: u32,
    pub bsp_lapic_id: u32,
    pub cpu_count: u64,
    cpus: *const *const u8,
}

#[repr(C)]
pub struct MemmapResponse {
    revision: u64,
    entry_count: u64,
    entries: *const *const MemmapEntry,
}

#[repr(C)]
struct MemmapEntry {
    base: u64,
    length: u64,
    kind: u64,
}

const MEMMAP_USABLE: u64 = 0;

#[used]
#[unsafe(link_section = ".limine_requests_start")]
static REQUESTS_START: [u64; 4] = [
    0xf6b8f4b39de7d1ae,
    0xfab91a6940fcb9cf,
    0x785c6ed015d3e316,
    0x181e920a7852b9d9,
];

#[used]
#[unsafe(link_section = ".limine_requests")]
pub static BASE_REVISION: BaseRevision = BaseRevision {
    magic: [0xf9562b2d5c95a6c8, 0x6a7b384944536bdc],
    revision: BootCell::new(REVISION),
};

#[used]
#[unsafe(link_section = ".limine_requests")]
static BOOTLOADER_INFO: Request<BootloaderInfoResponse> =
    Request::new([0xf55038d8e2a1202f, 0x279426fcf5f59740], ());

#[used]
#[unsafe(link_section = ".limine_requests")]
static HHDM: Request<HhdmResponse> = Request::new([0x48dcf1cb8ad2b852, 0x63984e959a98244b], ());

/// Ask Limine to start every core and park it until the kernel wants it.
/// The extra word is the request flags; 0 means "do not enable x2APIC".
#[used]
#[unsafe(link_section = ".limine_requests")]
static MP: Request<MpResponse, u64> = Request::new([0x95a67b819a1b857e, 0xa0b61b723b6a73e0], 0);

#[used]
#[unsafe(link_section = ".limine_requests")]
static MEMMAP: Request<MemmapResponse> = Request::new([0x67cf3d9d378a806f, 0xe304acdfc50c3c62], ());

#[used]
#[unsafe(link_section = ".limine_requests_end")]
static REQUESTS_END: [u64; 2] = [0xadc0e0531bb10d03, 0x9572709f31764c62];

/// The bootloader's name and version.
pub fn bootloader_info() -> Option<(&'static str, &'static str)> {
    let resp = BOOTLOADER_INFO.response()?;
    // SAFETY: Limine provides NUL-terminated strings that stay mapped.
    let (name, version) = unsafe { (CStr::from_ptr(resp.name), CStr::from_ptr(resp.version)) };
    Some((
        name.to_str().unwrap_or("?"),
        version.to_str().unwrap_or("?"),
    ))
}

/// Where Limine mapped all of physical memory in the higher half.
pub fn hhdm_offset() -> Option<u64> {
    HHDM.response().map(|r| r.offset)
}

/// The cores Limine found. They wait parked until the kernel starts them.
pub fn mp() -> Option<&'static MpResponse> {
    MP.response()
}

pub struct MemorySummary {
    pub entries: u64,
    pub usable_regions: u64,
    pub usable_bytes: u64,
}

/// Totals from the physical memory map.
pub fn memory_summary() -> Option<MemorySummary> {
    let resp = MEMMAP.response()?;
    let mut summary = MemorySummary {
        entries: resp.entry_count,
        usable_regions: 0,
        usable_bytes: 0,
    };
    for i in 0..resp.entry_count as usize {
        // SAFETY: Limine provides `entry_count` valid entry pointers.
        let entry = unsafe { &**resp.entries.add(i) };
        if entry.kind == MEMMAP_USABLE {
            summary.usable_regions += 1;
            summary.usable_bytes += entry.length;
        }
    }
    Some(summary)
}

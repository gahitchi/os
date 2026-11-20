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
    cpus: *const *mut MpInfo,
}

/// Set in [`MpResponse::flags`] when Limine switched every core to x2APIC.
pub const MP_X2APIC: u32 = 1;

impl MpResponse {
    /// One entry per core, the bootstrap core included.
    pub fn cpus(&self) -> impl Iterator<Item = &'static MpInfo> {
        // SAFETY: Limine provides `cpu_count` valid pointers that stay mapped.
        (0..self.cpu_count as usize).map(|i| unsafe { &**self.cpus.add(i) })
    }
}

/// A core as Limine reports it. Each core except the bootstrap one spins on
/// `goto_address` until the kernel writes an entry point there.
#[repr(C)]
pub struct MpInfo {
    pub processor_id: u32,
    pub lapic_id: u32,
    reserved: u64,
    goto_address: core::sync::atomic::AtomicU64,
    extra_argument: core::sync::atomic::AtomicU64,
}

impl MpInfo {
    /// Start this core at `entry`, which receives a pointer to this
    /// structure; `argument` can be read back with [`MpInfo::argument`].
    pub fn start(&self, entry: extern "C" fn(&'static MpInfo) -> !, argument: u64) {
        use core::sync::atomic::Ordering;
        self.extra_argument.store(argument, Ordering::Relaxed);
        // The parked core polls this word; the release store publishes the
        // argument (and everything the kernel set up) before it starts.
        self.goto_address
            .store(entry as usize as u64, Ordering::Release);
    }

    pub fn argument(&self) -> u64 {
        self.extra_argument
            .load(core::sync::atomic::Ordering::Acquire)
    }
}

#[repr(C)]
pub struct MemmapResponse {
    revision: u64,
    entry_count: u64,
    entries: *const *const MemmapEntry,
}

#[repr(C)]
pub struct MemmapEntry {
    pub base: u64,
    pub length: u64,
    pub kind: u64,
}

pub const MEMMAP_USABLE: u64 = 0;
pub const MEMMAP_RESERVED: u64 = 1;
pub const MEMMAP_BAD_MEMORY: u64 = 4;
pub const MEMMAP_FRAMEBUFFER: u64 = 7;

/// A readable name for a memory map entry type.
pub fn memmap_kind_name(kind: u64) -> &'static str {
    match kind {
        0 => "usable",
        1 => "reserved",
        2 => "ACPI reclaimable",
        3 => "ACPI NVS",
        4 => "bad memory",
        5 => "bootloader reclaimable",
        6 => "kernel and modules",
        7 => "framebuffer",
        _ => "unknown",
    }
}

#[repr(C)]
pub struct ExecutableAddressResponse {
    revision: u64,
    pub physical_base: u64,
    pub virtual_base: u64,
}

#[repr(C)]
pub struct CmdlineResponse {
    revision: u64,
    cmdline: *const c_char,
}

#[repr(C)]
pub struct ModuleResponse {
    revision: u64,
    module_count: u64,
    modules: *const *const File,
}

/// A file Limine loaded (a module from `limine.conf`).
#[repr(C)]
pub struct File {
    revision: u64,
    address: *const u8,
    size: u64,
    path: *const c_char,
    string: *const c_char,
}

impl File {
    /// The file's contents, mapped in the direct map.
    pub fn bytes(&self) -> &'static [u8] {
        // SAFETY: Limine loaded `size` bytes there, in memory it marks as
        // kernel and modules, which the kernel never reuses.
        unsafe { core::slice::from_raw_parts(self.address, self.size as usize) }
    }

    pub fn path(&self) -> &'static str {
        // SAFETY: Limine provides a NUL-terminated string that stays mapped.
        unsafe { CStr::from_ptr(self.path) }.to_str().unwrap_or("?")
    }
}

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
/// The extra word is the request flags: switch to x2APIC where the CPU has
/// it, which machines with many cores need.
#[used]
#[unsafe(link_section = ".limine_requests")]
static MP: Request<MpResponse, u64> =
    Request::new([0x95a67b819a1b857e, 0xa0b61b723b6a73e0], MP_X2APIC as u64);

#[used]
#[unsafe(link_section = ".limine_requests")]
static EXECUTABLE_ADDRESS: Request<ExecutableAddressResponse> =
    Request::new([0x71ba76863cc55f63, 0xb2644a48c516a487], ());

#[used]
#[unsafe(link_section = ".limine_requests")]
static CMDLINE: Request<CmdlineResponse> =
    Request::new([0x4b161536e598651e, 0xb390ad4a2f1f303a], ());

#[used]
#[unsafe(link_section = ".limine_requests")]
static MEMMAP: Request<MemmapResponse> = Request::new([0x67cf3d9d378a806f, 0xe304acdfc50c3c62], ());

#[used]
#[unsafe(link_section = ".limine_requests")]
static MODULES: Request<ModuleResponse> =
    Request::new([0x3e7e279702be32af, 0xca1c4f3bd1280cee], ());

#[repr(C)]
pub struct RsdpResponse {
    revision: u64,
    /// Physical address of the ACPI RSDP (base revision 3).
    address: u64,
}

#[used]
#[unsafe(link_section = ".limine_requests")]
static RSDP: Request<RsdpResponse> = Request::new([0xc5e77b6b397e7b43, 0x27637845accdcf3c], ());

/// Physical address of the ACPI root pointer, if the firmware has one.
pub fn rsdp() -> Option<u64> {
    let r = RSDP.response()?;
    let hhdm = hhdm_offset().unwrap_or(0);
    // Older base revisions gave a direct-map address.
    Some(if r.address >= hhdm && hhdm != 0 {
        r.address - hhdm
    } else {
        r.address
    })
}

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

/// The physical memory map, sorted by address.
pub fn memmap() -> Option<impl Iterator<Item = &'static MemmapEntry>> {
    let resp = MEMMAP.response()?;
    // SAFETY: Limine provides `entry_count` valid entry pointers.
    Some((0..resp.entry_count as usize).map(move |i| unsafe { &**resp.entries.add(i) }))
}

/// Where Limine loaded the kernel, physically and virtually.
pub fn executable_address() -> Option<&'static ExecutableAddressResponse> {
    EXECUTABLE_ADDRESS.response()
}

/// The kernel command line from `limine.conf` (`cmdline:`), if any.
pub fn cmdline() -> &'static str {
    let Some(resp) = CMDLINE.response() else {
        return "";
    };
    if resp.cmdline.is_null() {
        return "";
    }
    // SAFETY: Limine provides a NUL-terminated string that stays mapped.
    unsafe { CStr::from_ptr(resp.cmdline) }
        .to_str()
        .unwrap_or("")
}

/// The modules Limine loaded, in `limine.conf` order.
pub fn modules() -> impl Iterator<Item = &'static File> {
    let (count, list) = match MODULES.response() {
        Some(r) => (r.module_count as usize, r.modules),
        None => (0, ptr::null()),
    };
    // SAFETY: Limine provides `module_count` valid file pointers.
    (0..count).map(move |i| unsafe { &**list.add(i) })
}

// SPDX-License-Identifier: GPL-3.0-only
//! The kernel's interface, as numbers and layouts shared by the kernel and
//! every program (`docs/design/kernel-interface.md`).
//!
//! # Calls
//!
//! A program enters the kernel with the `syscall` instruction:
//!
//! | Register | In | Out |
//! | --- | --- | --- |
//! | `rax` | call number ([`call`]) | error code ([`Error`], 0 is success) |
//! | `rdi` | handle the call acts on | |
//! | `rsi` | operation ([`op`]) | |
//! | `rdx`, `r10`, `r8`, `r9`, `r12`, `r13` | arguments 0 to 5 | results 0 to 3 in `rdx`, `r10`, `r8`, `r9` |
//!
//! `rcx` and `r11` are overwritten by the CPU; every other register is kept.
//!
//! # Handles
//!
//! Each program has one flat table of capabilities (decision 51). A handle
//! is an index into it. Handle 0 is never valid. Slots of *other* tables are
//! named by a handle to that table plus an index.
//!
//! # Messages
//!
//! `call`, `send` and `try_send` on an endpoint, `recv` and `reply_recv`
//! use more registers:
//!
//! | Register | In | Out |
//! | --- | --- | --- |
//! | `rdi` | endpoint, notification or reply handle | badge of the sender |
//! | `rsi` | [`Info`] word: label, length, capability | [`Info`] word received |
//! | `rdx`, `r10`, `r8`, `r9`, `r12`, `r13` | data words 0 to 5 | data words received |
//! | `r14` | bits 0-31: handle of a capability to send; bits 32-63: slot of the own table to receive one in (0: none) | |
//! | `r15` | reply object (`recv`, `reply_recv`) | |
//!
//! `call` on any other kind of object is an operation on it (`rsi` is the
//! operation, [`op`]); `send` on a reply object answers the caller.
//!
//! # On ARM64
//!
//! The same roles, entered with `svc #0`: the call number and error code
//! in `x8`, the handle (badge) in `x0`, the operation or info word in `x1`,
//! the six arguments or data words in `x2` to `x7` (results in `x2` to
//! `x5`), the capability slots in `x9` and the reply object in `x10`. Every
//! other register is kept. The table above names the PC's registers.

#![no_std]

/// Call numbers (`rax`).
pub mod call {
    pub const CALL: u64 = 0;
    pub const SEND: u64 = 1;
    pub const TRY_SEND: u64 = 2;
    pub const RECV: u64 = 3;
    pub const REPLY_RECV: u64 = 4;
    pub const SIGNAL: u64 = 5;
    pub const WAIT: u64 = 6;
    pub const POLL: u64 = 7;
    pub const YIELD: u64 = 8;
    pub const DEBUG: u64 = 9;
}

/// Operations of `debug` (`rdi`).
pub mod debug {
    /// Print `rsi` bytes at address `rdx` (at most [`MAX_PRINT`]).
    pub const PRINT: u64 = 0;
    /// Report the result of the boot tests: `rsi` = 0 for success.
    pub const REPORT: u64 = 1;
    /// Read kernel counters: returns restarts of interrupted calls in
    /// result 0, capabilities deleted by revokes in result 1, threads moved
    /// between cores in result 2, thread switches in result 3.
    pub const COUNTERS: u64 = 2;
    /// The core the caller runs on, in result 0.
    pub const CORE: u64 = 3;
    /// Big kernel lock use, summed over all cores since boot: times taken,
    /// times a core had to wait, clock ticks spent waiting, and held.
    pub const LOCK_STATS: u64 = 4;
    /// What switches cost, summed over all cores since boot: timer writes,
    /// branch-predictor wipes (IBPB), address-space loads, wake-ups sent
    /// to other cores.
    pub const SWITCH_STATS: u64 = 5;
    pub const MAX_PRINT: usize = 256;
}

/// Operations of `call` (`rsi`), by the kind of object the handle names.
pub mod op {
    // On a capability table. Slots are (table handle, index) pairs; the
    // invoked table holds the destination.
    /// Copy: args = dst index, src table, src index, rights.
    pub const COPY: u64 = 1;
    /// Mint: as copy, plus arg 4 = badge (non-zero).
    pub const MINT: u64 = 2;
    /// Move: args = dst index, src table, src index.
    pub const MOVE: u64 = 3;
    /// Delete: arg 0 = index.
    pub const DELETE: u64 = 4;
    /// Revoke: arg 0 = index. Deletes everything derived from the
    /// capability; for an untyped, also takes its memory back.
    pub const REVOKE: u64 = 5;
    /// Inspect: arg 0 = index. Results: kind | rights << 8 | size bits << 16
    /// | flags << 24 | the object's format label ([`super::format`]) << 32,
    /// object address, badge or untyped free offset, untyped
    /// clean offset.
    pub const INSPECT: u64 = 6;

    // On untyped memory.
    /// Retype: args = kind, size bits, count, dst table, dst index.
    /// Makes at most [`super::RETYPE_BATCH`] objects; result 0 is how many.
    pub const RETYPE: u64 = 16;

    // On a thread.
    /// Configure: args = table handle, address space handle, fault endpoint
    /// handle (0: none). Copies them into the thread; 0 for the table or
    /// space leaves that one as it is.
    pub const THREAD_CONFIGURE: u64 = 32;
    /// Set priority: args = priority, highest priority it may give others.
    /// Neither may exceed the caller's own limit.
    pub const THREAD_PRIORITY: u64 = 33;
    /// Set registers of a stopped thread: args = rip, rsp, rdi, rsi, fs base.
    pub const THREAD_WRITE: u64 = 34;
    /// Read registers: results = rip, rsp, rax, state ([`super::state`]).
    pub const THREAD_READ: u64 = 35;
    /// Start a stopped thread.
    pub const THREAD_START: u64 = 36;
    /// Stop a thread, cancelling any message it waits for.
    pub const THREAD_STOP: u64 = 37;
    /// Bind a notification: arg 0 = notification handle (0: unbind). A
    /// thread waiting in `recv` then also wakes for its signals.
    pub const THREAD_BIND: u64 = 38;
    /// Timeout messages: arg 0 = 1 to send one to the thread's fault
    /// endpoint whenever time lent to it by a caller runs out (label
    /// [`super::TIMEOUT_LABEL`]); 0 to just wait for the caller's next
    /// refill, as before (decision 55).
    pub const THREAD_TIMEOUTS: u64 = 39;
    /// Linux requests (decision 163): arg 0 = [`super::linux::ON`] makes a
    /// stopped thread speak Linux (every system call it makes becomes a
    /// message to its fault endpoint, labelled [`super::LINUX_LABEL`] plus
    /// the Linux call number, words = its six arguments; answering with
    /// word 0 sets its result); [`super::linux::OFF`] turns it back;
    /// [`super::linux::TLS`] sets its thread pointer to arg 1, and
    /// [`super::linux::JUMP`] its pc and stack pointer to args 1 and 2,
    /// while it is stopped or waits for an answer to a Linux request.
    pub const THREAD_LINUX: u64 = 40;

    // On a scheduling context.
    /// Bind to a thread: arg 0 = thread handle.
    pub const SC_BIND: u64 = 48;
    pub const SC_UNBIND: u64 = 49;
    /// Results = budget ns, period ns, time used in total ns, core it last
    /// ran on.
    pub const SC_INFO: u64 = 50;
    /// Set the deadline of the work this context pays for: arg 0 = the
    /// time-stamp counter value it is due at (0: none). Threads of equal
    /// priority run soonest deadline first; priority always wins
    /// (decision 81). Results = deadlines missed in total, the thread
    /// object that held the context when the last one passed, and 1 if the
    /// deadline this call replaces was missed.
    pub const SC_DEADLINE: u64 = 51;

    // On the scheduling control capability.
    /// Configure a scheduling context: args = its handle, budget µs, period
    /// µs, core type ([`super::core_type`]), core + 1 to keep it on one
    /// core (0: any core of that type).
    pub const SC_CONFIGURE: u64 = 56;

    // On an address space.
    /// Give it page-table memory: arg 0 = handle of an untyped nothing was
    /// made from. The untyped moves into the address space.
    pub const SPACE_ASSIGN: u64 = 64;
    /// Guard it: arg 0 = 1 on, 0 off. A guarded program holds secrets
    /// (keys, saved files, passwords): the core drops what the branch
    /// predictors learned before the program runs after another one, and
    /// after it ran, so no other program can steer its guesses or read
    /// them back. Other switches skip that (it costs microseconds).
    pub const SPACE_GUARD: u64 = 65;

    // On a page.
    /// Map: args = address space handle, address, [`super::map`] flags.
    pub const PAGE_MAP: u64 = 72;
    pub const PAGE_UNMAP: u64 = 73;
    /// Physical address, in result 0 (for drivers).
    pub const PAGE_ADDRESS: u64 = 74;
    /// Let the devices of an I/O space reach the page: args = I/O space
    /// handle, device address, [`super::iomap`] flags. A page capability
    /// is mapped in one place at a time; map a copy for a second place.
    pub const PAGE_IOMAP: u64 = 75;

    // On an I/O space.
    /// Give it table memory: arg 0 = handle of an untyped nothing was made
    /// from (as `SPACE_ASSIGN`).
    pub const IOSPACE_ASSIGN: u64 = 80;

    // On the device control capability.
    /// Make a page capability for device registers: args = physical
    /// address, size bits (12 or 21), destination table, index. Refused
    /// for RAM and for the kernel's own devices (APIC, IOMMU).
    pub const DEVICE_PAGE: u64 = 96;
    /// Attach the PCI function `bdf` (bus << 8 | device << 3 | function)
    /// to an I/O space: args = I/O space handle, bdf.
    pub const DEVICE_BIND: u64 = 97;
    /// How many blocked device accesses the IOMMU reported (result 0).
    pub const DEVICE_FAULTS: u64 = 98;

    // On the interrupt control capability.
    /// Take a free vector: args = core it is delivered to, destination
    /// table, index. Result 0: the vector.
    pub const IRQ_GET: u64 = 104;

    // On an interrupt.
    /// Deliver it as `bits` on a notification: args = notification handle
    /// (0: stop delivering), bits.
    pub const IRQ_SET: u64 = 112;
    /// What to program into a device's MSI or MSI-X entry: arg = the PCI
    /// function that raises it (bus << 8 | device << 3 | function; ARM's
    /// interrupt translator tells devices apart by it); results = address,
    /// data.
    pub const IRQ_MSI: u64 = 113;
}

/// Flags of [`op::PAGE_IOMAP`].
pub mod iomap {
    pub const WRITE: u64 = 1 << 0;
}

/// Flags of [`op::PAGE_MAP`].
pub mod map {
    pub const WRITE: u64 = 1 << 0;
    pub const EXECUTE: u64 = 1 << 1;
}

/// Thread states, as [`op::THREAD_READ`] reports them.
pub mod state {
    pub const STOPPED: u64 = 0;
    pub const READY: u64 = 1;
    pub const SENDING: u64 = 2;
    pub const RECEIVING: u64 = 3;
    pub const WAITING_REPLY: u64 = 4;
    pub const WAITING_SIGNAL: u64 = 5;
}

/// Core types (decision 54).
pub mod core_type {
    pub const ANY: u64 = 0;
    pub const PERFORMANCE: u64 = 1;
    pub const EFFICIENCY: u64 = 2;
}

/// The message word in `rsi`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Info(pub u64);

impl Info {
    /// Most data words in one message.
    pub const MAX_WORDS: u64 = 6;
    /// With [`Info::CAP`]: the capability moves to the receiver instead of
    /// being copied. Untyped memory can only be handed over this way (it
    /// has one owner at a time).
    pub const MOVE: u64 = 1 << 3;
    /// The message carries a capability (handle in `r14`).
    pub const CAP: u64 = 1 << 4;
    /// Received: this is a notification's signal bits (in `rdi`), not a
    /// message.
    pub const SIGNAL: u64 = 1 << 5;
    /// Received: a capability was sent but could not be stored.
    pub const CAP_LOST: u64 = 1 << 6;
    /// Received: the message was a call, so the reply object holds the
    /// caller.
    pub const CALLED: u64 = 1 << 7;

    pub const fn new(label: u64, words: u64) -> Info {
        Info(label << 8 | words)
    }

    pub const fn label(self) -> u64 {
        self.0 >> 8
    }

    pub const fn words(self) -> u64 {
        self.0 & 7
    }

    pub const fn with_cap(self) -> Info {
        Info(self.0 | Self::CAP)
    }

    /// Carries a capability that moves to the receiver.
    pub const fn moving_cap(self) -> Info {
        Info(self.0 | Self::CAP | Self::MOVE)
    }

    pub const fn has(self, flag: u64) -> bool {
        self.0 & flag != 0
    }
}

/// Label of a fault message, sent for a thread that faulted to its fault
/// endpoint. Words: vector, error code, faulting address, rip, rsp.
pub const FAULT_LABEL: u64 = 0xfa_17;

/// Label of a timeout message: a thread serving a call ran out of the time
/// its caller lent it (see [`op::THREAD_TIMEOUTS`]). Words: 0, time it
/// used of the caller's context (ns), how long until that context's next
/// refill (ns), rip, rsp. Answering it lets the thread wait for the refill.
pub const TIMEOUT_LABEL: u64 = 0x71_3e;

/// Label base of a Linux request: the label is this plus the Linux call
/// number; the words are the call's six arguments (decision 163).
pub const LINUX_LABEL: u64 = 0x4c_0000;

/// Arguments of [`op::THREAD_LINUX`].
pub mod linux {
    pub const OFF: u64 = 0;
    pub const ON: u64 = 1;
    pub const TLS: u64 = 2;
    pub const JUMP: u64 = 3;
    /// Linux call numbers are below this.
    pub const CALLS: u64 = 0x1_0000;
}

/// Highest priority.
pub const MAX_PRIORITY: u64 = 255;

/// Most objects one retype makes. Programs loop for more.
pub const RETYPE_BATCH: u64 = 16;

/// Inspect flags.
pub mod flags {
    /// A revoke of this capability has started and not finished.
    pub const REVOKING: u8 = 1 << 0;
    /// Something was derived from this capability.
    pub const HAS_CHILDREN: u8 = 1 << 1;
}

/// Error codes (`rax` on return).
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Ok = 0,
    /// The call number or operation does not exist.
    NoSuchCall = 1,
    /// The handle or index names no slot, or an empty one.
    InvalidHandle = 2,
    /// The capability is the wrong kind for this operation.
    WrongKind = 3,
    /// The capability lacks a right the operation needs.
    NoRights = 4,
    /// The destination slot is not empty.
    SlotInUse = 5,
    /// The capability is being revoked.
    Busy = 6,
    /// The untyped memory has no room for the objects.
    NoMemory = 7,
    /// An argument is out of range.
    InvalidArgument = 8,
    /// The wait was cancelled: the object went away or the thread was
    /// stopped.
    Cancelled = 9,
    /// `try_send` found nobody waiting.
    WouldBlock = 10,
}

impl Error {
    pub fn from_u64(v: u64) -> Error {
        match v {
            0 => Error::Ok,
            1 => Error::NoSuchCall,
            2 => Error::InvalidHandle,
            3 => Error::WrongKind,
            4 => Error::NoRights,
            5 => Error::SlotInUse,
            6 => Error::Busy,
            7 => Error::NoMemory,
            9 => Error::Cancelled,
            10 => Error::WouldBlock,
            _ => Error::InvalidArgument,
        }
    }
}

/// Rights a capability carries.
pub mod rights {
    pub const READ: u8 = 1 << 0;
    pub const WRITE: u8 = 1 << 1;
    /// May be copied or minted onward.
    pub const GRANT: u8 = 1 << 2;
    pub const ALL: u8 = READ | WRITE | GRANT;
}

/// Kinds of capability.
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Empty = 0,
    Untyped = 1,
    Table = 2,
    Endpoint = 3,
    Notification = 4,
    Page = 5,
    Thread = 6,
    SchedContext = 7,
    Reply = 8,
    AddressSpace = 9,
    /// The right to give out time (configure scheduling contexts).
    SchedControl = 10,
    /// One device interrupt (a vector), delivered as notification signals.
    Irq = 11,
    /// The right to take interrupt vectors.
    IrqControl = 12,
    /// The right to reach device registers and attach devices to I/O
    /// spaces: held by the device manager only.
    DeviceControl = 13,
    /// What a set of devices may reach through the IOMMU.
    IoSpace = 14,
}

impl Kind {
    pub fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            0 => Kind::Empty,
            1 => Kind::Untyped,
            2 => Kind::Table,
            3 => Kind::Endpoint,
            4 => Kind::Notification,
            5 => Kind::Page,
            6 => Kind::Thread,
            7 => Kind::SchedContext,
            8 => Kind::Reply,
            9 => Kind::AddressSpace,
            10 => Kind::SchedControl,
            11 => Kind::Irq,
            12 => Kind::IrqControl,
            13 => Kind::DeviceControl,
            14 => Kind::IoSpace,
            _ => return None,
        })
    }
}

/// Sizes, as powers of two. A table of `bits` has `2^bits` slots of
/// [`SLOT_BYTES`] bytes; slot 0 of every table is reserved.
pub const SLOT_BITS: u8 = 6;
pub const SLOT_BYTES: u64 = 1 << SLOT_BITS;
pub const TABLE_MIN_BITS: u8 = 2;
pub const TABLE_MAX_BITS: u8 = 16;
/// Endpoints, notifications and replies take 64 bytes each.
pub const SMALL_OBJECT_BITS: u8 = 6;
/// A thread takes 4 KiB: registers, links and the saved FPU/vector state.
pub const THREAD_BITS: u8 = 12;
/// A scheduling context takes 256 bytes.
pub const SCHED_BITS: u8 = 8;
/// An address space takes 8 KiB: its top page table and its bookkeeping.
pub const SPACE_BITS: u8 = 13;
/// Pages: 4 KiB, 2 MiB, 1 GiB.
pub const PAGE_BITS: [u8; 3] = [12, 21, 30];
/// Untyped memory made by retype: 4 KiB up to the parent's size.
pub const UNTYPED_MIN_BITS: u8 = 12;
pub const UNTYPED_MAX_BITS: u8 = 47;

/// Where the first program finds its boot information (read only).
pub const BOOT_INFO_ADDRESS: u64 = 0x7fff_f000_0000;
/// Bytes of boot information mapped there.
pub const BOOT_INFO_BYTES: u64 = 16 * 1024;
/// Most cores the boot information describes.
pub const MAX_CORES: usize = 256;
/// Top of the first program's stack.
pub const INIT_STACK_TOP: u64 = 0x7fff_e000_0000;
pub const INIT_STACK_BYTES: u64 = 64 * 1024;

pub const BOOT_INFO_MAGIC: u64 = 0x616c_6179_615f_6269; // "alaya_bi"
pub const MAX_BOOT_UNTYPED: usize = 192;

/// One block of untyped memory handed to the first program.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BootUntyped {
    pub handle: u64,
    pub base: u64,
    pub size_bits: u64,
}

/// What the kernel tells the first program at start.
#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    /// Handle of the program's own table.
    pub table: u64,
    /// The table has `2^table_bits` slots.
    pub table_bits: u64,
    /// First empty slot after the boot capabilities.
    pub first_free: u64,
    /// Online cores.
    pub cores: u64,
    /// Handles of the program's own thread, scheduling context, address
    /// space, and the scheduling control capability.
    pub thread: u64,
    pub sched: u64,
    pub space: u64,
    pub sched_control: u64,
    /// Clock ticks per second (the clock programs read with
    /// `alaya_sys::clock()`: the TSC on a PC, the generic timer on ARM).
    pub clock_hz: u64,
    /// Where the program's image lies, for programs that copy it.
    pub image_start: u64,
    pub image_end: u64,
    /// Kernel command-line flags ([`boot_flags`]).
    pub flags: u64,
    /// `fuzz=<seconds>`: how long to run the kernel fuzzer.
    pub fuzz_seconds: u64,
    /// Each core's type ([`core_type`]).
    pub core_types: [u8; MAX_CORES],
    pub untyped_count: u64,
    pub untyped: [BootUntyped; MAX_BOOT_UNTYPED],
    /// The processor protections in use ([`protect`] bits).
    pub protections: u64,
    /// Handles of the interrupt control and device control capabilities.
    pub irq_control: u64,
    pub device_control: u64,
    /// PCI configuration space (ECAM) of segment 0: physical base, first
    /// and last bus (base 0: none).
    pub ecam_base: u64,
    pub ecam_bus_start: u64,
    pub ecam_bus_end: u64,
    /// The IOMMU is on (devices reach only what is mapped for them).
    pub iommu: u64,
    /// Programs the boot image carries, mapped read only into the first
    /// program.
    pub module_count: u64,
    pub modules: [BootModule; MAX_BOOT_MODULES],
}

pub const MAX_BOOT_MODULES: usize = 24;
/// Where the boot modules are mapped in the first program.
pub const BOOT_MODULES_ADDRESS: u64 = 0x7ff0_0000_0000;

/// A program in the boot image.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct BootModule {
    /// Its file name, NUL-padded.
    pub name: [u8; 32],
    pub address: u64,
    pub size: u64,
}

impl BootModule {
    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(32);
        core::str::from_utf8(&self.name[..end]).unwrap_or("?")
    }
}

const _: () = assert!(core::mem::size_of::<BootInfo>() as u64 <= BOOT_INFO_BYTES);

/// Processor protections the kernel turned on (decision 77), as reported
/// in [`BootInfo::protections`].
pub mod protect {
    /// The kernel cannot run code in program memory.
    pub const SMEP: u64 = 1 << 0;
    /// The kernel cannot read or write program memory by accident.
    pub const SMAP: u64 = 1 << 1;
    /// Programs cannot read the kernel's descriptor tables (`sgdt`, `sidt`,
    /// `sldt`, `smsw`, `str` fault on a PC; on ARM every kernel system
    /// register is always out of a program's reach).
    pub const UMIP: u64 = 1 << 2;
    /// Tagged memory maps: switching programs keeps both TLBs' entries.
    pub const PCID: u64 = 1 << 3;
    /// Enhanced IBRS: programs cannot steer the kernel's branch predictions.
    pub const EIBRS: u64 = 1 << 4;
    /// Branch predictions are dropped when a core changes program.
    pub const IBPB: u64 = 1 << 5;
    /// The CPU's buffers are cleared on the way back to user mode (MDS).
    pub const VERW: u64 = 1 << 6;
    /// The CPU is not affected by Meltdown, so the kernel's half of the
    /// memory map can stay mapped while programs run.
    pub const NO_MELTDOWN: u64 = 1 << 7;
}

/// Format labels (decision 77): every kernel object carries a small header
/// naming its kind and the format of its layout, so a later kernel can
/// recognise the objects of an older one and take them over without a
/// restart. Bump an object's number whenever its layout changes.
pub mod format {
    /// The header's first two bytes.
    pub const MAGIC: u16 = 0xa1a7;
    pub const TABLE: u8 = 1;
    pub const ENDPOINT: u8 = 1;
    pub const NOTIFICATION: u8 = 1;
    pub const THREAD: u8 = 1;
    pub const SCHED_CONTEXT: u8 = 1;
    pub const REPLY: u8 = 1;
    pub const ADDRESS_SPACE: u8 = 1;
    pub const IO_SPACE: u8 = 1;
    /// The format of the kernel-wide state as a whole (`kernel/src/kstate.rs`).
    pub const KERNEL_STATE: u32 = 1;

    /// An object's format label, by kind (0: the kind has no header).
    pub const fn of(kind: super::Kind) -> u8 {
        use super::Kind as K;
        match kind {
            K::Table => TABLE,
            K::Endpoint => ENDPOINT,
            K::Notification => NOTIFICATION,
            K::Thread => THREAD,
            K::SchedContext => SCHED_CONTEXT,
            K::Reply => REPLY,
            K::AddressSpace => ADDRESS_SPACE,
            K::IoSpace => IO_SPACE,
            _ => 0,
        }
    }
}

/// How a program started by another one finds its way (`alaya_sys::program`).
///
/// The parent maps a read-only [`StartInfo`] page at [`START_INFO_ADDRESS`]
/// and passes its address in `rdi`. The program's table has these slots:
pub mod start {
    /// Its own table, thread and address space.
    pub const TABLE: u64 = 1;
    pub const THREAD: u64 = 2;
    pub const SPACE: u64 = 3;
    /// Untyped memory of its own, for stacks and objects.
    pub const UNTYPED: u64 = 4;
    /// A scheduling context the parent configured for a helper thread
    /// (its clock), or empty.
    pub const CLOCK_SC: u64 = 5;
    /// What the parent hands over starts here; the [`StartInfo`] says what
    /// each slot holds.
    pub const FIRST_GIVEN: u64 = 8;

    pub const MAGIC: u64 = 0x616c_6179_615f_7369; // "alaya_si"
    pub const MAX_ARGS: usize = 16;
}

pub const START_INFO_ADDRESS: u64 = 0x7fff_f100_0000;
/// The top of a started program's stack (64 KiB).
pub const PROGRAM_STACK_TOP: u64 = 0x7fff_e000_0000;
pub const PROGRAM_STACK_BYTES: u64 = 64 * 1024;

/// What a started program learns from its parent.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct StartInfo {
    pub magic: u64,
    pub name: [u8; 32],
    pub clock_hz: u64,
    pub cores: u64,
    /// The size of its untyped (`start::UNTYPED`), as a power of two.
    pub untyped_bits: u64,
    /// First empty slot of its table, and how many it has.
    pub first_free: u64,
    pub table_slots: u64,
    /// Its priority (it may give its own threads up to this).
    pub priority: u64,
    /// Words the parent passes: their meaning depends on the program.
    pub args: [u64; start::MAX_ARGS],
}

/// Flags in [`BootInfo::flags`], from the kernel command line.
pub mod boot_flags {
    /// `fuzz=<seconds>`: run the kernel fuzzer.
    pub const FUZZ: u64 = 1 << 0;
    /// `long`: run longer stress tests.
    pub const LONG: u64 = 1 << 1;
    /// `store-soak`: skip the store checks; open the store on the disk,
    /// show what survived, and keep saving until the machine is killed.
    pub const STORE_SOAK: u64 = 1 << 2;
    /// `desk`: a person drives the desktop (`cargo xtask run`): the
    /// desktop checks do not type for themselves; the person sets up and
    /// logs in.
    pub const DESK: u64 = 1 << 3;
}

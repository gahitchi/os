// SPDX-License-Identifier: GPL-3.0-only
//! The layouts of kernel objects that live in retyped untyped memory:
//! threads, scheduling contexts, endpoints, notifications, replies and
//! address spaces.
//!
//! Every object is made from memory that retype zeroed, so all-zero must be
//! a valid, empty object. Objects are reached through the direct map by
//! their physical address and only touched under the big kernel lock.
//! Links between objects are physical addresses, 0 for none.

use alaya_abi::{SCHED_BITS, SMALL_OBJECT_BITS, SPACE_BITS, THREAD_BITS};
use alaya_cap::Slot;
use alaya_sched::Budget;

/// The object at physical address `phys`.
///
/// # Safety (for callers)
/// The caller holds the big kernel lock and `phys` is an object of type
/// `T` (checked through the capability that named it). No two live
/// references to one object may be held at once; the kernel code takes
/// one, uses it and drops it.
#[allow(clippy::mut_from_ref)]
pub fn get<T>(phys: u64) -> &'static mut T {
    debug_assert!(phys != 0, "null object");
    // SAFETY: see above; the direct map covers all RAM.
    unsafe { &mut *((phys + crate::limine::hhdm_offset().unwrap()) as *mut T) }
}

/// A thread's registers as user mode sees them.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Regs {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8: u64,
    pub r9: u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub rip: u64,
    pub rflags: u64,
    pub rsp: u64,
    pub fs_base: u64,
    pub gs_base: u64,
}

impl Regs {
    /// Data words of a message, in register order.
    pub fn words(&self) -> [u64; 6] {
        [self.rdx, self.r10, self.r8, self.r9, self.r12, self.r13]
    }

    pub fn set_words(&mut self, w: [u64; 6]) {
        [self.rdx, self.r10, self.r8, self.r9, self.r12, self.r13] = w;
    }
}

/// Thread states (the numbers are `alaya_abi::state`).
pub mod state {
    pub const STOPPED: u8 = 0;
    pub const READY: u8 = 1;
    pub const SENDING: u8 = 2;
    pub const RECEIVING: u8 = 3;
    pub const WAITING_REPLY: u8 = 4;
    pub const WAITING_SIGNAL: u8 = 5;
}

/// Thread flags.
pub mod tflag {
    /// Entered the kernel last with `syscall`: `rcx` and `r11` are free, so
    /// it may return with `sysret`.
    pub const SYSCALL: u8 = 1 << 0;
    /// Sending a call (waits for a reply after delivery).
    pub const CALLING: u8 = 1 << 1;
    /// Sending a fault message (in `Tcb::fault`), not its registers.
    pub const FAULTING: u8 = 1 << 2;
    /// In the run queue of core `Tcb::core`.
    pub const QUEUED: u8 = 1 << 3;
    /// The FPU save area holds a valid state.
    pub const FPU_READY: u8 = 1 << 4;
    /// A queued sender may pass a capability (its endpoint capability
    /// has the grant right).
    pub const SEND_GRANT: u8 = 1 << 5;
    /// Running on a core now (`Tcb::core`): in user mode there, or about
    /// to be.
    pub const CURRENT: u8 = 1 << 6;
}

/// Internal slots of a thread.
pub const SLOT_TABLE: usize = 0;
pub const SLOT_SPACE: usize = 1;
pub const SLOT_FAULT: usize = 2;

/// Where the FPU/vector save area starts in a thread (64-byte aligned).
pub const FPU_OFFSET: u64 = 1024;
pub const FPU_BYTES: u64 = (1 << THREAD_BITS) - FPU_OFFSET;

/// A thread (4 KiB).
#[repr(C)]
pub struct Tcb {
    /// Its table, address space and fault endpoint (copies of capabilities,
    /// so revoking those takes them away from the thread too).
    pub slots: [Slot; 3],
    pub regs: Regs,
    pub state: u8,
    /// Its own priority, and the highest it may give to others.
    pub prio: u8,
    pub mcp: u8,
    pub flags: u8,
    /// The core it last ran on (placement prefers it), or whose run queue
    /// holds it.
    pub core: u16,
    /// The priority it was queued at.
    pub qprio: u8,
    pub _pad: u8,
    /// The scheduling context it runs on now: its own, or one lent by a
    /// caller.
    pub sc: u64,
    /// Its own scheduling context (bound with `SC_BIND`).
    pub home_sc: u64,
    /// Links in a run queue or an endpoint or notification queue.
    pub next: u64,
    pub prev: u64,
    /// What it waits on: endpoint, notification or reply object.
    pub waiting: u64,
    /// Receiving: the reply object it will bind a caller to.
    pub reply: u64,
    /// Its bound notification.
    pub ntfn: u64,
    /// A pending fault message: vector, error, address, rip, rsp.
    pub fault: [u64; 5],
    /// Badge of the endpoint capability it is sending through.
    pub send_badge: u64,
    /// When its time slice among equal priorities ends (wall time, so a
    /// thread preempted often still yields to its equals in time).
    pub slice_end: u64,
}

const _: () = assert!(core::mem::size_of::<Tcb>() as u64 <= FPU_OFFSET);

/// Scheduling-context flags.
pub mod scflag {
    /// Out of budget, on its core's release list.
    pub const RELEASING: u8 = 1 << 0;
}

/// A scheduling context (256 bytes).
#[repr(C)]
pub struct Sc {
    pub budget: Budget,
    /// The thread running on it now.
    pub thread: u64,
    /// The thread it is bound to.
    pub home: u64,
    /// Replies that lent this context onward, newest first.
    pub reply_top: u64,
    /// Release list link.
    pub next_release: u64,
    /// Time used, in total (TSC ticks).
    pub used: u64,
    pub core_type: u8,
    pub flags: u8,
    pub _pad: u16,
    /// The core it last ran on, or whose release list holds it.
    pub core: u32,
    /// Keep it on core `pin - 1` (0: any core of `core_type`).
    pub pin: u32,
    pub _pad2: u32,
}

const _: () = assert!(core::mem::size_of::<Sc>() <= 1 << SCHED_BITS);

/// A queue of threads (endpoint or notification).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Queue {
    pub head: u64,
    pub tail: u64,
}

/// What an endpoint's queue holds.
pub mod epstate {
    pub const IDLE: u64 = 0;
    pub const SENDERS: u64 = 1;
    pub const RECEIVERS: u64 = 2;
}

/// An endpoint (64 bytes).
#[repr(C)]
pub struct Endpoint {
    pub queue: Queue,
    pub state: u64,
}

/// A notification (64 bytes).
#[repr(C)]
pub struct Notification {
    pub word: u64,
    pub queue: Queue,
    /// The thread it is bound to.
    pub bound: u64,
}

/// A reply object (64 bytes): the right to answer one caller.
#[repr(C)]
pub struct Reply {
    /// The caller waiting for the answer.
    pub caller: u64,
    /// The thread waiting in `recv` that will take the next caller.
    pub receiver: u64,
    /// The scheduling context the caller lent (0: none).
    pub sc: u64,
    /// The next older reply that lent the same context.
    pub below: u64,
}

const _: () = assert!(core::mem::size_of::<Endpoint>() <= 1 << SMALL_OBJECT_BITS);
const _: () = assert!(core::mem::size_of::<Notification>() <= 1 << SMALL_OBJECT_BITS);
const _: () = assert!(core::mem::size_of::<Reply>() <= 1 << SMALL_OBJECT_BITS);

/// An address space's bookkeeping, in its second page. Its first page is
/// the top-level page table.
#[repr(C)]
pub struct SpaceMeta {
    /// The untyped page tables are taken from (an internal capability).
    pub pool: Slot,
    /// Its id in the kernel's address-space table, plus one (0: not set up).
    pub id: u64,
    /// Page tables taken so far.
    pub tables: u64,
}

pub const SPACE_META: u64 = 4096;
const _: () = assert!(SPACE_META + core::mem::size_of::<SpaceMeta>() as u64 <= 1 << SPACE_BITS);

pub fn space_meta(space: u64) -> &'static mut SpaceMeta {
    get(space + SPACE_META)
}

/// What kind of internal slot a capability with `alaya_cap::INTERNAL`
/// sits in (its `_reserved`).
pub mod internal {
    pub const THREAD_TABLE: u32 = 1;
    pub const THREAD_SPACE: u32 = 2;
    pub const THREAD_FAULT: u32 = 3;
    pub const SPACE_POOL: u32 = 4;
}

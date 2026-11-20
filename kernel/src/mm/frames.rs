// SPDX-License-Identifier: GPL-3.0-only
//! Physical frames for the kernel's own boot-time structures.
//!
//! The kernel allocates nothing once it runs (decision 9): every kernel
//! object will be made by user space from untyped memory. At boot, though,
//! the kernel needs a few frames for its page tables and per-core stacks.
//! They are taken from the start of usable memory by a simple bump
//! allocator; from milestone 2 everything after the bump pointer becomes
//! the untyped memory handed to the first user program.

use core::sync::atomic::{AtomicU64, Ordering};

use super::layout::PAGE_SIZE;
use crate::limine;
use crate::sync::SpinLock;

/// Low memory is left alone (future use: real-mode trampolines, firmware
/// quirks).
const LOWEST: u64 = 1 << 20;

pub(crate) static NEXT: SpinLock<u64> = SpinLock::new(LOWEST);
pub(crate) static TAKEN: AtomicU64 = AtomicU64::new(0);
pub(crate) static HHDM: AtomicU64 = AtomicU64::new(0);

pub fn init(hhdm: u64) {
    HHDM.store(hhdm, Ordering::Relaxed);
}

/// Take one zeroed 4 KiB frame. Panics when memory runs out.
pub fn alloc_zeroed() -> u64 {
    alloc_contiguous(PAGE_SIZE, PAGE_SIZE)
}

/// Take `len` zeroed bytes of physically contiguous memory, aligned to
/// `align`. Panics when memory runs out, or once the rest of memory has
/// been handed to the first program as untyped memory.
pub fn alloc_contiguous(len: u64, align: u64) -> u64 {
    let mut next = NEXT.lock();
    assert!(
        !FROZEN.load(Ordering::Relaxed),
        "boot allocation after memory was handed out"
    );
    let len = len.next_multiple_of(PAGE_SIZE);
    let usable = limine::memmap()
        .expect("no memory map")
        .filter(|e| e.kind == limine::MEMMAP_USABLE);
    for entry in usable {
        let start = (*next)
            .max(entry.base)
            .next_multiple_of(align.max(PAGE_SIZE));
        if start + len <= entry.base + entry.length {
            *next = start + len;
            TAKEN.fetch_add(len, Ordering::Relaxed);
            let virt = (start + HHDM.load(Ordering::Relaxed)) as *mut u8;
            // SAFETY: the memory is usable RAM, reached through the direct
            // map, and nobody else owns it.
            unsafe { core::ptr::write_bytes(virt, 0, len as usize) };
            return start;
        }
    }
    panic!("out of memory for kernel boot structures");
}

pub(crate) static FROZEN: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Stop boot allocations and return the usable memory nobody took, as
/// page-aligned ranges, through `each(base, end)`. From now on all of it
/// belongs to the first program.
pub fn freeze(mut each: impl FnMut(u64, u64)) {
    let next = NEXT.lock();
    FROZEN.store(true, Ordering::Relaxed);
    let usable = limine::memmap()
        .expect("no memory map")
        .filter(|e| e.kind == limine::MEMMAP_USABLE);
    for entry in usable {
        let start = (*next).max(entry.base).next_multiple_of(PAGE_SIZE);
        let end = (entry.base + entry.length) & !(PAGE_SIZE - 1);
        if start < end {
            each(start, end);
        }
    }
}

/// Bytes the kernel took for itself.
pub fn taken() -> u64 {
    TAKEN.load(Ordering::Relaxed)
}

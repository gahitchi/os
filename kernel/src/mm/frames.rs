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

static NEXT: SpinLock<u64> = SpinLock::new(LOWEST);
static TAKEN: AtomicU64 = AtomicU64::new(0);
static HHDM: AtomicU64 = AtomicU64::new(0);

pub fn init(hhdm: u64) {
    HHDM.store(hhdm, Ordering::Relaxed);
}

/// Take one zeroed 4 KiB frame. Panics when memory runs out.
pub fn alloc_zeroed() -> u64 {
    let mut next = NEXT.lock();
    let usable = limine::memmap()
        .expect("no memory map")
        .filter(|e| e.kind == limine::MEMMAP_USABLE);
    for entry in usable {
        let start = (*next).max(entry.base).next_multiple_of(PAGE_SIZE);
        if start + PAGE_SIZE <= entry.base + entry.length {
            *next = start + PAGE_SIZE;
            TAKEN.fetch_add(PAGE_SIZE, Ordering::Relaxed);
            let virt = (start + HHDM.load(Ordering::Relaxed)) as *mut u8;
            // SAFETY: the frame is usable RAM, reached through the direct map,
            // and nobody else owns it.
            unsafe { core::ptr::write_bytes(virt, 0, PAGE_SIZE as usize) };
            return start;
        }
    }
    panic!("out of memory for kernel boot structures");
}

/// Bytes the kernel took for itself.
pub fn taken() -> u64 {
    TAKEN.load(Ordering::Relaxed)
}

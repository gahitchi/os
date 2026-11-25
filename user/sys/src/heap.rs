// SPDX-License-Identifier: GPL-3.0-only WITH AdditionRef-Alaya-system-library-exception
//! A program's heap, for programs that want `alloc` (`Vec`, `Box`,
//! `BTreeMap`): a first-fit list of free ranges over pages the program
//! maps for it, with neighbours joined again when freed. One lock, so
//! every thread of the program may use it.
//!
//! A program opts in with [`alaya_sys::heap!`](crate::heap) and calls
//! [`init`] with its own pages before the first allocation.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

/// A free range: its size, and the next free range (by address).
struct Free {
    size: usize,
    next: *mut Free,
}

const MIN: usize = core::mem::size_of::<Free>();
const ALIGN: usize = 16;

pub struct Heap {
    lock: AtomicBool,
    head: core::cell::UnsafeCell<*mut Free>,
    used: core::cell::UnsafeCell<usize>,
}

// SAFETY: every access to the list is under the lock.
unsafe impl Sync for Heap {}

impl Heap {
    pub const fn new() -> Heap {
        Heap {
            lock: AtomicBool::new(false),
            head: core::cell::UnsafeCell::new(null_mut()),
            used: core::cell::UnsafeCell::new(0),
        }
    }

    fn lock(&self) {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    /// Give the heap `size` bytes at `base` (mapped and writable).
    ///
    /// # Safety
    /// The range must be mapped, writable, unused by anything else and
    /// stay so for the rest of the program.
    pub unsafe fn add(&self, base: usize, size: usize) {
        let start = base.next_multiple_of(ALIGN);
        let size = (base + size - start) & !(ALIGN - 1);
        if size < MIN {
            return;
        }
        self.lock();
        // SAFETY: under the lock; the range is ours.
        unsafe { self.insert(start as *mut Free, size) };
        self.unlock();
    }

    /// Bytes handed out now.
    pub fn used(&self) -> usize {
        self.lock();
        // SAFETY: under the lock.
        let u = unsafe { *self.used.get() };
        self.unlock();
        u
    }

    /// Put a range back into the address-ordered list, joining it with
    /// the ranges right before and after it.
    unsafe fn insert(&self, at: *mut Free, size: usize) {
        // SAFETY: the caller holds the lock and owns the range.
        unsafe {
            let head = &mut *self.head.get();
            let mut prev: *mut Free = null_mut();
            let mut cur = *head;
            while !cur.is_null() && (cur as usize) < at as usize {
                prev = cur;
                cur = (*cur).next;
            }
            (*at).size = size;
            (*at).next = cur;
            if !cur.is_null() && at as usize + size == cur as usize {
                (*at).size += (*cur).size;
                (*at).next = (*cur).next;
            }
            if prev.is_null() {
                *head = at;
            } else if prev as usize + (*prev).size == at as usize {
                (*prev).size += (*at).size;
                (*prev).next = (*at).next;
            } else {
                (*prev).next = at;
            }
        }
    }
}

impl Default for Heap {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(MIN).next_multiple_of(ALIGN);
        let align = layout.align().max(ALIGN);
        self.lock();
        // SAFETY: under the lock; the list holds only free ranges.
        let out = unsafe {
            let head = &mut *self.head.get();
            let mut prev: *mut Free = null_mut();
            let mut cur = *head;
            let mut out = null_mut();
            while !cur.is_null() {
                let start = cur as usize;
                let end = start + (*cur).size;
                let mut a = start.next_multiple_of(align);
                // A gap before the block must hold a free range itself.
                if a != start && a - start < MIN {
                    a = (start + MIN).next_multiple_of(align);
                }
                if a + size <= end {
                    let next = (*cur).next;
                    // Unlink, then give back what is left on either side.
                    if prev.is_null() {
                        *head = next;
                    } else {
                        (*prev).next = next;
                    }
                    if a > start {
                        self.insert(start as *mut Free, a - start);
                    }
                    // Sizes and places are multiples of 16, so what is
                    // left is either nothing or room for a free range.
                    let tail = end - (a + size);
                    if tail > 0 {
                        self.insert((a + size) as *mut Free, tail);
                    }
                    *self.used.get() += size;
                    out = a as *mut u8;
                    break;
                }
                prev = cur;
                cur = (*cur).next;
            }
            out
        };
        self.unlock();
        out
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let size = layout.size().max(MIN).next_multiple_of(ALIGN);
        self.lock();
        // SAFETY: the block came from `alloc` with this layout.
        unsafe {
            *self.used.get() -= size;
            self.insert(ptr as *mut Free, size);
        }
        self.unlock();
    }
}

/// The heap of this program, once it opted in with [`crate::heap!`].
pub static HEAP: Heap = Heap::new();

/// Give the heap `mib` MiB of fresh memory, in 2 MiB pages (each takes
/// one handle, so a big heap needs few).
pub fn init(mem: &mut crate::rt::Mem, mib: u64) -> Result<(), crate::Error> {
    const HUGE: u64 = 2 << 20;
    let pages = mib.div_ceil(2);
    let base = mem.heap.next_multiple_of(HUGE);
    for i in 0..pages {
        let p = mem.make(crate::Kind::Page, 21)?;
        crate::page_map(
            p,
            crate::abi::start::SPACE,
            base + i * HUGE,
            crate::abi::map::WRITE,
        )?;
    }
    mem.heap = base + (pages + 1) * HUGE;
    // SAFETY: just mapped, writable and ours.
    unsafe { HEAP.add(base as usize, (pages * HUGE) as usize) };
    Ok(())
}

/// What a program names as its allocator: forwards to [`HEAP`].
pub struct Global;

unsafe impl GlobalAlloc for Global {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded as is.
        unsafe { HEAP.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded as is.
        unsafe { HEAP.dealloc(ptr, layout) }
    }
}

/// Use [`HEAP`] as this program's allocator.
#[macro_export]
macro_rules! heap {
    () => {
        #[global_allocator]
        static __ALAYA_HEAP: $crate::heap::Global = $crate::heap::Global;
    };
}

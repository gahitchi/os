// SPDX-License-Identifier: GPL-3.0-only
//! Locks. The kernel is designed for many cores from day one (decision 10):
//! one big kernel lock serialises all kernel work, as in seL4. Kernel paths
//! in a microkernel are short, so the lock is rarely contended, and a single
//! lock is far easier to reason about (and to model-check) than many.
//!
//! Rules:
//! - Kernel code runs with interrupts off. A core takes the big lock when it
//!   enters the kernel (an interrupt, later a system call) and drops it when
//!   it leaves, so it never waits for the lock with the lock already held.
//! - Taking the big lock with interrupts on, or twice on one core, is a bug
//!   and panics with a clear message instead of deadlocking.
//! - The serial console has its own small lock so that fault reports can be
//!   printed even while the big lock is held.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::arch;

/// A simple spin lock.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: UnsafeCell<T>,
}

// SAFETY: access to `value` is serialised by `locked`.
unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while !self.try_acquire() {
            while self.locked.load(Ordering::Relaxed) {
                arch::spin_hint();
            }
        }
        SpinLockGuard { lock: self }
    }

    /// Try for roughly `spins` iterations, then give up.
    pub fn lock_bounded(&self, spins: u64) -> Option<SpinLockGuard<'_, T>> {
        for _ in 0..spins {
            if self.try_acquire() {
                return Some(SpinLockGuard { lock: self });
            }
            arch::spin_hint();
        }
        None
    }

    fn try_acquire(&self) -> bool {
        self.locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Get the value without taking the lock.
    ///
    /// # Safety
    /// Only for paths that can never return, such as the panic handler,
    /// where waiting for a lock that may never be released would hang.
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn force_get(&self) -> &mut T {
        unsafe { &mut *self.value.get() }
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves the lock is held.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves the lock is held.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

const NO_OWNER: u32 = u32::MAX;

/// The big kernel lock: a ticket lock, so cores get it in the order they
/// asked and none can starve, which matters with 20 cores or more.
pub struct BigKernelLock<T> {
    next: AtomicU32,
    serving: AtomicU32,
    owner: AtomicU32,
    value: UnsafeCell<T>,
}

// SAFETY: access to `value` is serialised by the tickets.
unsafe impl<T: Send> Sync for BigKernelLock<T> {}

impl<T> BigKernelLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            next: AtomicU32::new(0),
            serving: AtomicU32::new(0),
            owner: AtomicU32::new(NO_OWNER),
            value: UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> BklGuard<'_, T> {
        let me = crate::percpu::index();
        if arch::interrupts_enabled() {
            panic!("big kernel lock taken with interrupts on (cpu {me})");
        }
        if self.owner.load(Ordering::Relaxed) == me {
            panic!("big kernel lock taken twice on cpu {me}");
        }
        let ticket = self.next.fetch_add(1, Ordering::Relaxed);
        let stats = crate::percpu::get(me);
        let mut now = crate::time::now();
        if self.serving.load(Ordering::Acquire) != ticket {
            let start = now;
            while self.serving.load(Ordering::Acquire) != ticket {
                // The holder may be waiting for this core to flush its TLB.
                crate::vspace::poll_flush();
                arch::spin_hint();
            }
            now = crate::time::now();
            stats.lock_waits.fetch_add(1, Ordering::Relaxed);
            stats
                .lock_wait_ticks
                .fetch_add(now.wrapping_sub(start), Ordering::Relaxed);
        }
        stats.lock_takes.fetch_add(1, Ordering::Relaxed);
        stats.lock_since.store(now, Ordering::Relaxed);
        self.owner.store(me, Ordering::Relaxed);
        BklGuard { lock: self }
    }

    /// The core holding the lock right now, for fault reports.
    pub fn owner(&self) -> Option<u32> {
        match self.owner.load(Ordering::Relaxed) {
            NO_OWNER => None,
            cpu => Some(cpu),
        }
    }
}

pub struct BklGuard<'a, T> {
    lock: &'a BigKernelLock<T>,
}

impl<T> Deref for BklGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the guard proves the lock is held.
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> DerefMut for BklGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the guard proves the lock is held.
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for BklGuard<'_, T> {
    fn drop(&mut self) {
        let stats = crate::percpu::this();
        let held = crate::time::now().wrapping_sub(stats.lock_since.load(Ordering::Relaxed));
        stats.lock_held_ticks.fetch_add(held, Ordering::Relaxed);
        self.lock.owner.store(NO_OWNER, Ordering::Relaxed);
        self.lock.serving.fetch_add(1, Ordering::Release);
    }
}

/// State that only kernel code under the big lock may touch.
pub struct KernelState {
    /// Incremented by every core in the boot self-test; checks that the
    /// lock really excludes (the increment is a plain, non-atomic one).
    pub selftest_counter: u64,
    /// The purge list and counters of the capability code.
    pub caps: alaya_cap::Globals,
    /// Threads, cores and address spaces.
    pub sys: Sys,
}

/// The kernel's scheduling and address-space state. Kept apart from the
/// capability globals so that both can be borrowed at once.
pub struct Sys {
    pub cores: [crate::sched::CoreState; crate::percpu::MAX_CPUS],
    /// The CPU has performance and efficiency cores.
    pub hybrid: bool,
    /// The scheduler runs (the boot self-tests are over).
    pub started: bool,
    /// Address spaces by id.
    pub spaces: [crate::vspace::SpaceEntry; crate::vspace::MAX_SPACES],
    /// TLB tags each core must flush before loading them again.
    pub stale: crate::vspace::Stale,
    /// The IOMMU (device memory guard).
    pub iommu: crate::iommu::State,
    /// Device interrupts by vector.
    pub irqs: [crate::device::IrqEntry; 256],
    /// System calls restarted because an interruptible operation stopped.
    pub restarts: u64,
    /// Threads that started running on another core than before.
    pub migrations: u64,
    /// Switches between threads.
    pub switches: u64,
    /// Threads stopped by a fault with no fault endpoint.
    pub faults: u64,
    /// The first program's thread: the only one whose boot report counts,
    /// so a stray call from a fuzzed thread cannot end the boot tests.
    pub first_thread: u64,
}

pub static BKL: BigKernelLock<KernelState> = BigKernelLock::new(KernelState {
    selftest_counter: 0,
    caps: alaya_cap::Globals::new(),
    sys: Sys {
        cores: [crate::sched::CoreState::EMPTY; crate::percpu::MAX_CPUS],
        hybrid: false,
        started: false,
        first_thread: 0,
        spaces: [crate::vspace::SpaceEntry::EMPTY; crate::vspace::MAX_SPACES],
        stale: crate::vspace::Stale::EMPTY,
        iommu: crate::iommu::State::EMPTY,
        irqs: [crate::device::IrqEntry::EMPTY; 256],
        restarts: 0,
        migrations: 0,
        switches: 0,
        faults: 0,
    },
});

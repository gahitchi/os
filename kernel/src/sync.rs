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
        if arch::cpu::interrupts_enabled() {
            panic!("big kernel lock taken with interrupts on (cpu {me})");
        }
        if self.owner.load(Ordering::Relaxed) == me {
            panic!("big kernel lock taken twice on cpu {me}");
        }
        let ticket = self.next.fetch_add(1, Ordering::Relaxed);
        while self.serving.load(Ordering::Acquire) != ticket {
            arch::spin_hint();
        }
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
    /// The first program, once it runs.
    pub init: Option<crate::user::Init>,
    /// System calls restarted because an interruptible operation stopped.
    pub restarts: u64,
}

pub static BKL: BigKernelLock<KernelState> = BigKernelLock::new(KernelState {
    selftest_counter: 0,
    caps: alaya_cap::Globals::new(),
    init: None,
    restarts: 0,
});

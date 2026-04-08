//! SMP synchronization primitives for multi-hart kernel.
//!
//! Provides interrupt-safe spinlocks, per-hart state, and boot synchronization.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// Number of harts (hardware threads) supported.
#[allow(dead_code)]
pub const NUM_HARTS: usize = 4;

/// Boot synchronization flag. Hart 0 sets this after initialization.
pub static BOOT_HART_DONE: AtomicBool = AtomicBool::new(false);

/// Read the hart ID from the tp register.
#[cfg(target_arch = "riscv64")]
#[inline(always)]
pub fn hart_id() -> usize {
    let id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) id) };
    id
}

/// Stub hart_id for non-riscv64 targets (always returns 0).
#[cfg(not(target_arch = "riscv64"))]
#[inline(always)]
pub fn hart_id() -> usize {
    0
}

// ========== Raw SpinLock with Interrupt Disable ==========

/// A raw spinlock that disables S-mode interrupts before acquiring.
///
/// This prevents deadlock when a timer interrupt fires on the same hart
/// that holds the lock (the #1 SMP kernel bug).
pub struct RawSpinLockIrq {
    locked: AtomicBool,
}

unsafe impl Sync for RawSpinLockIrq {}
unsafe impl Send for RawSpinLockIrq {}

impl RawSpinLockIrq {
    /// Create a new unlocked spinlock.
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Acquire the lock, disabling S-mode interrupts first.
    pub fn lock(&self) -> RawSpinLockIrqGuard<'_> {
        // Save and disable S-mode interrupts (sstatus.SIE)
        let sie_was_enabled = Self::disable_interrupts();

        // TTAS (test-and-test-and-set) spin pattern
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }

        RawSpinLockIrqGuard {
            lock: self,
            sie_was_enabled,
        }
    }

    /// Save SIE bit and disable interrupts. Returns whether SIE was enabled.
    #[cfg(target_arch = "riscv64")]
    fn disable_interrupts() -> bool {
        let sstatus: usize;
        unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) sstatus) };
        let sie_was_enabled = (sstatus & (1 << 1)) != 0;
        if sie_was_enabled {
            unsafe { core::arch::asm!("csrc sstatus, {}", in(reg) (1usize << 1)) };
        }
        sie_was_enabled
    }

    #[cfg(not(target_arch = "riscv64"))]
    fn disable_interrupts() -> bool {
        false
    }

    /// Restore SIE bit if it was previously enabled.
    #[cfg(target_arch = "riscv64")]
    fn restore_interrupts(sie_was_enabled: bool) {
        if sie_was_enabled {
            unsafe { core::arch::asm!("csrs sstatus, {}", in(reg) (1usize << 1)) };
        }
    }

    #[cfg(not(target_arch = "riscv64"))]
    fn restore_interrupts(_sie_was_enabled: bool) {}
}

/// Guard for RawSpinLockIrq. Releases the lock and restores interrupts on drop.
pub struct RawSpinLockIrqGuard<'a> {
    lock: &'a RawSpinLockIrq,
    sie_was_enabled: bool,
}

impl Drop for RawSpinLockIrqGuard<'_> {
    fn drop(&mut self) {
        // Release lock FIRST, then restore interrupts
        // (if we restored interrupts first, a timer interrupt could fire
        // while we still hold the lock, and if that handler tries to
        // acquire the same lock, deadlock)
        self.lock.locked.store(false, Ordering::Release);
        RawSpinLockIrq::restore_interrupts(self.sie_was_enabled);
    }
}

// ========== Generic SpinLockIrq<T> ==========

/// An interrupt-safe spinlock wrapping data of type T.
pub struct SpinLockIrq<T> {
    lock: RawSpinLockIrq,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLockIrq<T> {}
unsafe impl<T: Send> Send for SpinLockIrq<T> {}

impl<T> SpinLockIrq<T> {
    /// Create a new SpinLockIrq with the given data.
    pub const fn new(data: T) -> Self {
        Self {
            lock: RawSpinLockIrq::new(),
            data: UnsafeCell::new(data),
        }
    }

    /// Acquire the lock and return a guard providing access to the data.
    pub fn lock(&self) -> SpinLockIrqGuard<'_, T> {
        let raw_guard = self.lock.lock();
        SpinLockIrqGuard {
            data: unsafe { &mut *self.data.get() },
            _raw_guard: raw_guard,
        }
    }
}

/// Guard for SpinLockIrq<T>. Provides Deref/DerefMut access to the data.
pub struct SpinLockIrqGuard<'a, T> {
    data: &'a mut T,
    _raw_guard: RawSpinLockIrqGuard<'a>,
}

impl<T> Deref for SpinLockIrqGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.data
    }
}

impl<T> DerefMut for SpinLockIrqGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.data
    }
}

// ========== Print Lock ==========

/// Global lock for console output serialization.
/// Prevents interleaved multi-hart println! output.
pub static PRINT_LOCK: RawSpinLockIrq = RawSpinLockIrq::new();

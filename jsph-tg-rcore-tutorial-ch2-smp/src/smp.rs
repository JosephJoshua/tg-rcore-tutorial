//! SMP synchronization primitives for multi-hart boot.

use core::sync::atomic::{AtomicBool, Ordering};

/// Boot synchronization flag. Hart 0 sets this after initialization.
pub static BOOT_HART_DONE: AtomicBool = AtomicBool::new(false);

/// Global UART output lock.
pub static PRINT_LOCK: SpinLock = SpinLock::new();

/// Test-and-test-and-set spinlock.
pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    /// Create a new unlocked spinlock.
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }

    /// Acquire the lock, spinning until available.
    pub fn lock(&self) {
        while self.locked.compare_exchange_weak(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_err() {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    /// Release the lock.
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

/// Read the hart ID from the tp register.
#[inline(always)]
pub fn hart_id() -> usize {
    let id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) id) };
    id
}

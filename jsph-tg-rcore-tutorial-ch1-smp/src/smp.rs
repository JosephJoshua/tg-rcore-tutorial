//! SMP synchronization primitives for multi-hart bare-metal execution.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Maximum number of hardware threads (compile-time upper bound).
/// Used in `_start` for stack array sizing; actual count determined at runtime.
#[allow(dead_code)]
pub const NUM_HARTS: usize = 4;

/// How many harts actually booted (set dynamically at runtime).
/// Hart 0 increments this after init; each secondary hart increments on arrival.
pub static ACTIVE_HARTS: AtomicUsize = AtomicUsize::new(0);

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
        Self {
            locked: AtomicBool::new(false),
        }
    }

    /// Acquire the spinlock, spinning until it is available.
    pub fn lock(&self) {
        loop {
            // Test-and-test-and-set: first spin on a relaxed load (cache-friendly),
            // then attempt the atomic swap only when the lock appears free.
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    }

    /// Release the spinlock.
    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

/// Generation-based barrier for multi-hart synchronization.
///
/// The last hart to arrive resets the count and advances the generation,
/// releasing all waiting harts.
pub struct Barrier {
    count: AtomicUsize,
    generation: AtomicUsize,
}

impl Barrier {
    /// Create a new barrier.
    pub const fn new() -> Self {
        Self {
            count: AtomicUsize::new(0),
            generation: AtomicUsize::new(0),
        }
    }

    /// Wait until all `num_harts` harts have reached this barrier.
    pub fn wait(&self, num_harts: usize) {
        let cur_gen = self.generation.load(Ordering::Acquire);
        if self.count.fetch_add(1, Ordering::AcqRel) + 1 == num_harts {
            // Last hart to arrive: reset count and advance generation.
            self.count.store(0, Ordering::Relaxed);
            self.generation
                .store(cur_gen.wrapping_add(1), Ordering::Release);
        } else {
            // Wait for the generation to change.
            while self.generation.load(Ordering::Acquire) == cur_gen {
                core::hint::spin_loop();
            }
        }
    }
}

/// Read the hart ID from the `tp` register.
#[inline(always)]
pub fn hart_id() -> usize {
    let id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) id) };
    id
}

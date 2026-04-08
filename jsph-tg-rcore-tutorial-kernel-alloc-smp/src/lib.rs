//! SMP-safe kernel memory allocator.
//!
//! Wraps a buddy allocator with an interrupt-disabling spinlock so that
//! `alloc()` and `dealloc()` are safe to call from multiple harts
//! simultaneously. This replaces the need for manual `ALLOC_LOCK` at
//! every allocation call site.

#![no_std]
#![deny(missing_docs)]

extern crate alloc;

use alloc::alloc::handle_alloc_error;
use core::{
    alloc::{GlobalAlloc, Layout},
    cell::UnsafeCell,
    ptr::NonNull,
    sync::atomic::{AtomicBool, Ordering},
};
use customizable_buddy::{BuddyAllocator, LinkedListBuddy, UsizeBuddy};

// ========== Interrupt-safe spinlock (self-contained) ==========

/// Minimal spinlock that disables S-mode interrupts before acquiring.
/// Prevents deadlock when a timer interrupt fires while the allocator lock is held.
struct AllocSpinLock {
    locked: AtomicBool,
}

impl AllocSpinLock {
    const fn new() -> Self {
        Self {
            locked: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> AllocSpinLockGuard<'_> {
        let sie_was_enabled = Self::disable_interrupts();
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        AllocSpinLockGuard {
            lock: self,
            sie_was_enabled,
        }
    }

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

    #[cfg(target_arch = "riscv64")]
    fn restore_interrupts(sie_was_enabled: bool) {
        if sie_was_enabled {
            unsafe { core::arch::asm!("csrs sstatus, {}", in(reg) (1usize << 1)) };
        }
    }

    #[cfg(not(target_arch = "riscv64"))]
    fn restore_interrupts(_: bool) {}
}

struct AllocSpinLockGuard<'a> {
    lock: &'a AllocSpinLock,
    sie_was_enabled: bool,
}

impl Drop for AllocSpinLockGuard<'_> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        AllocSpinLock::restore_interrupts(self.sie_was_enabled);
    }
}

// ========== Heap storage ==========

/// Wrapper to make UnsafeCell<BuddyAllocator> Sync.
/// All access goes through LOCK.
struct HeapCell(UnsafeCell<BuddyAllocator<21, UsizeBuddy, LinkedListBuddy>>);
unsafe impl Sync for HeapCell {}

/// The buddy allocator instance. Max capacity: 2^(6+21+3) = 1 GiB.
static HEAP: HeapCell = HeapCell(UnsafeCell::new(BuddyAllocator::new()));

/// The lock protecting all allocator operations.
static LOCK: AllocSpinLock = AllocSpinLock::new();

#[inline]
fn heap_mut() -> &'static mut BuddyAllocator<21, UsizeBuddy, LinkedListBuddy> {
    unsafe { &mut *HEAP.0.get() }
}

// ========== Public API ==========

/// Initialize the allocator with a base address.
///
/// Must be called once before any heap allocation, while only hart 0 is running.
#[inline]
pub fn init(base_address: usize) {
    heap_mut().init(
        core::mem::size_of::<usize>().trailing_zeros() as _,
        NonNull::new(base_address as *mut u8).unwrap(),
    );
}

/// Transfer a memory region to the allocator.
///
/// # Safety
///
/// The region must not overlap with previously transferred regions,
/// must not be referenced by other code, and must lie after the base address.
#[inline]
pub unsafe fn transfer(region: &'static mut [u8]) {
    let ptr = NonNull::new(region.as_mut_ptr()).unwrap();
    unsafe { heap_mut().transfer(ptr, region.len()) };
}

// ========== Global allocator ==========

struct SmpGlobalAlloc;

unsafe impl Sync for SmpGlobalAlloc {}

#[global_allocator]
static GLOBAL: SmpGlobalAlloc = SmpGlobalAlloc;

unsafe impl GlobalAlloc for SmpGlobalAlloc {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _guard = LOCK.lock();
        if let Ok((ptr, _)) = heap_mut().allocate_layout::<u8>(layout) {
            ptr.as_ptr()
        } else {
            handle_alloc_error(layout)
        }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _guard = LOCK.lock();
        unsafe { heap_mut().deallocate_layout(NonNull::new(ptr).unwrap(), layout) }
    }
}

//! Static bump allocator for VirtIO DMA and `extern crate alloc`.
//!
//! Allocate-only (never frees). Backed by a 5 MiB static array in BSS.
//! Safe for single-core, one-shot init-then-display usage.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 5 MiB pool — enough for 1024x768 BGRA framebuffer (~3 MiB) + VirtQueue + headroom.
const POOL_SIZE: usize = 5 * 1024 * 1024;

/// Page size used by virtio-drivers for DMA allocations.
const PAGE_SIZE: usize = 4096;

#[repr(align(4096))]
struct AlignedPool {
    data: UnsafeCell<[u8; POOL_SIZE]>,
}

unsafe impl Sync for AlignedPool {}

static POOL: AlignedPool = AlignedPool {
    data: UnsafeCell::new([0u8; POOL_SIZE]),
};

/// Atomic offset into POOL.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Bump-allocate `size` bytes with the given alignment from the static pool.
/// Returns a pointer into the pool, or null if out of space.
fn bump_alloc(size: usize, align: usize) -> *mut u8 {
    loop {
        let current = NEXT.load(Ordering::Relaxed);
        let base = POOL.data.get() as usize;
        let addr = base + current;
        let aligned = (addr + align - 1) & !(align - 1);
        let offset = aligned - base;
        let new_offset = offset + size;
        if new_offset > POOL_SIZE {
            return core::ptr::null_mut();
        }
        if NEXT
            .compare_exchange(current, new_offset, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            // Zero the allocated region (DMA buffers must be zeroed)
            unsafe { core::ptr::write_bytes(aligned as *mut u8, 0, size) };
            return aligned as *mut u8;
        }
    }
}

struct BumpAllocator;

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump_alloc(layout.size(), layout.align())
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        // Never frees — one-shot program.
    }
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

// ---- Hal trait implementation for virtio-drivers ----

use virtio_drivers::{Hal, PhysAddr, VirtAddr};

/// VirtIO HAL for ch1: identity-mapped, no paging, DMA from static pool.
pub struct HalImpl;

impl Hal for HalImpl {
    fn dma_alloc(pages: usize) -> PhysAddr {
        let size = pages * PAGE_SIZE;
        let ptr = bump_alloc(size, PAGE_SIZE);
        if ptr.is_null() {
            0
        } else {
            ptr as PhysAddr
        }
    }

    fn dma_dealloc(_paddr: PhysAddr, _pages: usize) -> i32 {
        0 // Never frees.
    }

    fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr // Identity mapping — no page tables in ch1.
    }

    fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        vaddr // Identity mapping.
    }
}

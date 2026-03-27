//! Bump allocator for VirtIO DMA and `extern crate alloc`.
//!
//! Allocate-only (never frees). Uses a fixed region of RAM at 0x8100_0000
//! to avoid overlapping with user programs loaded at 0x8040_0000.
//! QEMU virt has 128 MiB RAM (0x8000_0000–0x87FF_FFFF), so this is safe.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Fixed base address for the DMA/heap pool, placed at 16 MiB into RAM.
/// This is well past the user program region (0x8040_0000) and the kernel
/// BSS, avoiding the overlap that caused GPU hangs in the batch processing loop.
const POOL_BASE: usize = 0x8100_0000;

/// 5 MiB pool — enough for 1280x800 BGRA framebuffer (~4 MiB) + VirtQueue + headroom.
const POOL_SIZE: usize = 5 * 1024 * 1024;

/// Page size used by virtio-drivers for DMA allocations.
const PAGE_SIZE: usize = 4096;

/// Atomic offset into the pool.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Bump-allocate `size` bytes with the given alignment from the pool.
/// Returns a pointer into the pool, or null if out of space.
fn bump_alloc(size: usize, align: usize) -> *mut u8 {
    loop {
        let current = NEXT.load(Ordering::Relaxed);
        let addr = POOL_BASE + current;
        let aligned = (addr + align - 1) & !(align - 1);
        let offset = aligned - POOL_BASE;
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

/// VirtIO HAL for ch2-moving-tangram: identity-mapped, no paging, DMA from fixed pool.
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
        paddr // Identity mapping — no page tables in ch2.
    }

    fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        vaddr // Identity mapping.
    }
}

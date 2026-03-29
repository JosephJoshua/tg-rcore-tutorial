//! Bump allocator for VirtIO DMA and `extern crate alloc`.
//!
//! Allocate-only (never frees). Uses a fixed region of RAM at 0x8200_0000
//! to avoid overlapping with user programs loaded at 0x8040_0000 with step
//! 0x0020_0000 (13 programs end at 0x81E0_0000).
//! QEMU virt has 128 MiB RAM (0x8000_0000–0x87FF_FFFF), so this is safe.

use core::alloc::{GlobalAlloc, Layout};
use core::sync::atomic::{AtomicUsize, Ordering};

/// Fixed base address for the DMA/heap pool.
const POOL_BASE: usize = 0x8200_0000;

/// 5 MiB pool.
const POOL_SIZE: usize = 5 * 1024 * 1024;

/// Page size used by virtio-drivers for DMA allocations.
const PAGE_SIZE: usize = 4096;

/// Atomic offset into the pool.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Bump-allocate `size` bytes with the given alignment from the pool.
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

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static ALLOCATOR: BumpAllocator = BumpAllocator;

use virtio_drivers::{Hal, PhysAddr, VirtAddr};

/// VirtIO HAL: identity-mapped, no paging, DMA from fixed pool.
pub struct HalImpl;

impl Hal for HalImpl {
    fn dma_alloc(pages: usize) -> PhysAddr {
        let size = pages * PAGE_SIZE;
        let ptr = bump_alloc(size, PAGE_SIZE);
        if ptr.is_null() { 0 } else { ptr as PhysAddr }
    }

    fn dma_dealloc(_paddr: PhysAddr, _pages: usize) -> i32 {
        0
    }

    fn phys_to_virt(paddr: PhysAddr) -> VirtAddr {
        paddr
    }

    fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr {
        vaddr
    }
}

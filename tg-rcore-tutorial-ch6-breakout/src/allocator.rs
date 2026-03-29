//! DMA bump allocator and VirtIO Hal implementation.

use core::sync::atomic::{AtomicUsize, Ordering};
use virtio_drivers::{Hal, PhysAddr, VirtAddr};

const POOL_BASE: usize = 0x8500_0000;
const POOL_SIZE: usize = 5 * 1024 * 1024;
const PAGE_SIZE: usize = 4096;

static NEXT: AtomicUsize = AtomicUsize::new(0);

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

/// VirtIO HAL: identity-mapped, DMA from fixed pool.
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

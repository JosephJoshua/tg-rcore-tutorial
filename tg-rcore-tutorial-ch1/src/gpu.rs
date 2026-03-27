//! VirtIO-GPU initialization and framebuffer management.

use crate::allocator::HalImpl;
use core::ptr::NonNull;
use virtio_drivers::{MmioTransport, VirtIOGpu, VirtIOHeader};

/// VirtIO MMIO base address for bus slot 0 on QEMU virt platform.
const VIRTIO0: usize = 0x10001000;

/// Screen width and height (populated after GPU init).
pub struct Framebuffer {
    /// Raw pointer to the pixel buffer (BGRA format, 4 bytes per pixel).
    pub ptr: *mut u8,
    /// Length of the pixel buffer in bytes.
    pub len: usize,
    /// Screen width in pixels.
    pub width: u32,
    /// Screen height in pixels.
    pub height: u32,
}

/// Initialize the VirtIO-GPU device, set up the framebuffer, and return
/// a raw pointer to the pixel buffer (BGRA format, 4 bytes per pixel).
///
/// Also returns a mutable reference to the GPU driver for flushing.
pub fn init() -> (VirtIOGpu<'static, HalImpl, MmioTransport>, Framebuffer) {
    let header = NonNull::new(VIRTIO0 as *mut VirtIOHeader).expect("null VirtIOHeader");
    let transport =
        unsafe { MmioTransport::new(header) }.expect("failed to create MmioTransport");
    let mut gpu = VirtIOGpu::new(transport).expect("failed to create VirtIOGpu");

    let (width, height) = gpu.resolution().expect("failed to get resolution");

    // setup_framebuffer returns &mut [u8] tied to &mut gpu by lifetime elision.
    // Extract raw pointer to break the borrow so we can call flush() later.
    let (ptr, len) = {
        let buf = gpu.setup_framebuffer().expect("failed to setup framebuffer");
        (buf.as_mut_ptr(), buf.len())
    };

    (
        gpu,
        Framebuffer {
            ptr,
            len,
            width,
            height,
        },
    )
}

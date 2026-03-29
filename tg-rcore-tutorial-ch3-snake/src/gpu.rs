//! VirtIO-GPU initialization and framebuffer management.

use crate::allocator::HalImpl;
use core::ptr::NonNull;
use virtio_drivers::{MmioTransport, VirtIOGpu, VirtIOHeader};

const VIRTIO_MMIO_BASE: usize = 0x10001000;
const VIRTIO_MMIO_END: usize = 0x10008000;
const VIRTIO_MMIO_STRIDE: usize = 0x1000;

/// Framebuffer descriptor returned by GPU initialization.
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

/// Initialize VirtIO-GPU, set up framebuffer, return driver + framebuffer info.
pub fn init() -> (VirtIOGpu<'static, HalImpl, MmioTransport>, Framebuffer) {
    let mut transport = None;
    let mut addr = VIRTIO_MMIO_BASE;
    while addr <= VIRTIO_MMIO_END {
        let header = NonNull::new(addr as *mut VirtIOHeader).unwrap();
        if let Ok(t) = unsafe { MmioTransport::new(header) } {
            transport = Some(t);
            break;
        }
        addr += VIRTIO_MMIO_STRIDE;
    }
    let transport = transport.expect("no VirtIO device found on any MMIO slot");
    let mut gpu = VirtIOGpu::new(transport).expect("failed to create VirtIOGpu");

    let (width, height) = gpu.resolution().expect("failed to get resolution");

    let (ptr, len) = {
        let buf = gpu.setup_framebuffer().expect("failed to setup framebuffer");
        (buf.as_mut_ptr(), buf.len())
    };

    (
        gpu,
        Framebuffer { ptr, len, width, height },
    )
}

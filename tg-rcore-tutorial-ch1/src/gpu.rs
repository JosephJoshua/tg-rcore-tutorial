//! VirtIO-GPU initialization and framebuffer management.

use crate::allocator::HalImpl;
use core::ptr::NonNull;
use virtio_drivers::{MmioTransport, VirtIOGpu, VirtIOHeader};

/// QEMU virt platform has 8 VirtIO MMIO slots at 0x10001000..0x10008000.
/// Devices are assigned to the highest slot first, so we probe all slots.
const VIRTIO_MMIO_BASE: usize = 0x10001000;
/// Last MMIO slot address.
const VIRTIO_MMIO_END: usize = 0x10008000;
/// Stride between MMIO slots.
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

/// Initialize the VirtIO-GPU device, set up the framebuffer, and return
/// a raw pointer to the pixel buffer (BGRA format, 4 bytes per pixel).
///
/// Also returns the GPU driver for flushing.
pub fn init() -> (VirtIOGpu<'static, HalImpl, MmioTransport>, Framebuffer) {
    // Probe all MMIO slots to find the GPU device.
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

//! VirtIO device initialization: GPU framebuffer and keyboard input.

use crate::allocator::HalImpl;
use core::ptr::NonNull;
use virtio_drivers::{
    device::{gpu::VirtIOGpu, input::VirtIOInput},
    transport::{
        mmio::{MmioTransport, VirtIOHeader},
        DeviceType, Transport,
    },
};

const VIRTIO_MMIO_BASE: usize = 0x10001000;
const VIRTIO_MMIO_END: usize = 0x10008000;
const VIRTIO_MMIO_STRIDE: usize = 0x1000;

/// Framebuffer descriptor.
pub struct Framebuffer {
    /// Pointer to framebuffer memory.
    pub ptr: *mut u8,
    /// Length of framebuffer in bytes.
    pub len: usize,
    /// Display width in pixels.
    pub width: u32,
    /// Display height in pixels.
    pub height: u32,
}

/// Result of scanning all VirtIO MMIO slots.
pub struct VirtIODevices {
    /// GPU device and its framebuffer.
    pub gpu: (VirtIOGpu<'static, HalImpl, MmioTransport>, Framebuffer),
    /// Keyboard input device (optional).
    pub keyboard: Option<VirtIOInput<HalImpl, MmioTransport>>,
}

/// Scan all VirtIO MMIO slots, initialize GPU and keyboard devices.
pub fn init() -> VirtIODevices {
    let mut gpu_transport: Option<MmioTransport> = None;
    let mut kbd_transport: Option<MmioTransport> = None;

    let mut addr = VIRTIO_MMIO_BASE;
    while addr <= VIRTIO_MMIO_END {
        let header = NonNull::new(addr as *mut VirtIOHeader).unwrap();
        if let Ok(transport) = unsafe { MmioTransport::new(header) } {
            match transport.device_type() {
                DeviceType::GPU => {
                    tg_console::log::info!("found VirtIO GPU at {addr:#x}");
                    gpu_transport = Some(transport);
                }
                DeviceType::Input => {
                    tg_console::log::info!("found VirtIO Input at {addr:#x}");
                    kbd_transport = Some(transport);
                }
                other => {
                    tg_console::log::debug!("skipping VirtIO device type {other:?} at {addr:#x}");
                }
            }
        }
        addr += VIRTIO_MMIO_STRIDE;
    }

    let gpu_transport = gpu_transport.expect("no VirtIO GPU device found");
    let mut gpu = VirtIOGpu::new(gpu_transport).expect("failed to create VirtIOGpu");
    let (width, height) = gpu.resolution().expect("failed to get resolution");
    let (ptr, len) = {
        let buf = gpu.setup_framebuffer().expect("failed to setup framebuffer");
        (buf.as_mut_ptr(), buf.len())
    };

    let fb = Framebuffer { ptr, len, width, height };
    let keyboard = kbd_transport.map(|t| VirtIOInput::new(t).expect("failed to create VirtIOInput"));
    if keyboard.is_none() {
        tg_console::log::warn!("no VirtIO keyboard device found");
    }

    VirtIODevices { gpu: (gpu, fb), keyboard }
}

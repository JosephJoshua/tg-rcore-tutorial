//! VirtIO device initialization: GPU framebuffer and keyboard input.

use crate::allocator::HalImpl;
use core::ptr::NonNull;
use virtio_drivers::{DeviceType, MmioTransport, Transport, VirtIOGpu, VirtIOHeader, VirtIOInput};

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
    /// GPU driver and framebuffer info.
    pub gpu: (VirtIOGpu<'static, HalImpl, MmioTransport>, Framebuffer),
    /// Optional keyboard input driver.
    pub keyboard: Option<VirtIOInput<HalImpl, MmioTransport>>,
    /// Keyboard MMIO base address (for queue notify workaround).
    pub kbd_mmio_addr: usize,
}

/// Notify the VirtIO keyboard device that new buffers are available on queue 0.
/// Workaround for virtio-drivers 0.1.0 bug: pop_pending_event() re-adds buffers
/// to the available ring but never notifies the device.
/// queue_notify register is at MMIO offset 0x50.
pub fn kbd_notify(mmio_addr: usize) {
    if mmio_addr != 0 {
        unsafe {
            core::ptr::write_volatile((mmio_addr + 0x50) as *mut u32, 0);
        }
    }
}

/// Scan all VirtIO MMIO slots, initialize GPU and keyboard devices.
pub fn init() -> VirtIODevices {
    let mut gpu_transport: Option<MmioTransport> = None;
    let mut kbd_transport: Option<MmioTransport> = None;
    let mut kbd_mmio: usize = 0;

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
                    kbd_mmio = addr;
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

    VirtIODevices { gpu: (gpu, fb), keyboard, kbd_mmio_addr: kbd_mmio }
}

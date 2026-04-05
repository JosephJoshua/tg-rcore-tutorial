//! VirtIO 块设备驱动（使用 virtio-drivers 0.3.0 + 共享 HalImpl）

use crate::allocator::HalImpl;
use alloc::sync::Arc;
use core::ptr::NonNull;
use spin::{Lazy, Mutex};
use tg_easy_fs::BlockDevice;
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::mmio::{MmioTransport, VirtIOHeader},
};

/// VirtIO 块设备 MMIO 基地址
const VIRTIO0: usize = 0x10001000;

/// 全局块设备实例
pub static BLOCK_DEVICE: Lazy<Arc<dyn BlockDevice>> = Lazy::new(|| {
    Arc::new(unsafe {
        VirtIOBlock(Mutex::new({
            let header = NonNull::new(VIRTIO0 as *mut VirtIOHeader).unwrap();
            let transport = MmioTransport::new(header).expect("MmioTransport creation failed");
            VirtIOBlk::<HalImpl, _>::new(transport).expect("VirtIOBlk creation failed")
        }))
    })
});

struct VirtIOBlock(Mutex<VirtIOBlk<HalImpl, MmioTransport>>);

// Safety: 内部使用 Mutex 保护，确保线程安全
unsafe impl Send for VirtIOBlock {}
unsafe impl Sync for VirtIOBlock {}

impl BlockDevice for VirtIOBlock {
    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        self.0
            .lock()
            .read_block(block_id, buf)
            .expect("Error when reading VirtIOBlk");
    }

    fn write_block(&self, block_id: usize, buf: &[u8]) {
        self.0
            .lock()
            .write_block(block_id, buf)
            .expect("Error when writing VirtIOBlk");
    }
}

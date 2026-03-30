//! VirtIO 块设备驱动模块
//!
//! 使用 allocator::HalImpl 提供的 v0.3.0 Hal 实现。

use crate::allocator::HalImpl;
use alloc::sync::Arc;
use core::ptr::NonNull;
use spin::{Lazy, Mutex};
use tg_easy_fs::BlockDevice;
use virtio_drivers::{
    device::blk::VirtIOBlk,
    transport::mmio::{MmioTransport, VirtIOHeader},
};

/// VirtIO MMIO 基地址
const VIRTIO0: usize = 0x10001000;

/// 全局块设备实例
pub static BLOCK_DEVICE: Lazy<Arc<dyn BlockDevice>> = Lazy::new(|| {
    Arc::new(unsafe {
        VirtIOBlock(Mutex::new(
            VirtIOBlk::new(
                MmioTransport::new(NonNull::new(VIRTIO0 as *mut VirtIOHeader).unwrap())
                    .expect("Error when creating MmioTransport"),
            )
            .expect("Error when creating VirtIOBlk"),
        ))
    })
});

/// VirtIO 块设备封装
struct VirtIOBlock(Mutex<VirtIOBlk<HalImpl, MmioTransport>>);

// Safety: 内部使用 Mutex 保护
unsafe impl Send for VirtIOBlock {}
unsafe impl Sync for VirtIOBlock {}

/// 实现 BlockDevice trait
impl BlockDevice for VirtIOBlock {
    /// 读取磁盘块
    fn read_block(&self, block_id: usize, buf: &mut [u8]) {
        self.0
            .lock()
            .read_block(block_id, buf)
            .expect("Error when reading VirtIOBlk");
    }
    /// 写入磁盘块
    fn write_block(&self, block_id: usize, buf: &[u8]) {
        self.0
            .lock()
            .write_block(block_id, buf)
            .expect("Error when writing VirtIOBlk");
    }
}

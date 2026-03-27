# Ch2 Moving Tangram Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Animate tangram "OS" pieces appearing one-by-one in ch2's batch OS via VirtIO-GPU framebuffer syscalls.

**Architecture:** Kernel initializes VirtIO-GPU and exposes FB_INFO/FB_WRITE custom syscalls. 14 user programs each rasterize one tangram piece into a heap buffer and push it to the kernel framebuffer. Animation timing via rdtime spin-wait in user-space.

**Tech Stack:** Rust (edition 2024, riscv64gc-unknown-none-elf), virtio-drivers 0.1.0, QEMU virt platform

---

### Task 1: Project Scaffolding

**Files:**
- Create: `jsph-tg-rcore-tutorial-ch2-moving-tangram/` (copy of `tg-rcore-tutorial-ch2/`)
- Modify: `jsph-tg-rcore-tutorial-ch2-moving-tangram/Cargo.toml`
- Modify: `jsph-tg-rcore-tutorial-ch2-moving-tangram/.cargo/config.toml`

- [ ] **Step 1: Copy ch2 directory and clean build artifacts**

```bash
cp -r tg-rcore-tutorial-ch2 jsph-tg-rcore-tutorial-ch2-moving-tangram
rm -rf jsph-tg-rcore-tutorial-ch2-moving-tangram/{target,Cargo.lock,.gitrepo}
```

- [ ] **Step 2: Update Cargo.toml**

Replace the entire `jsph-tg-rcore-tutorial-ch2-moving-tangram/Cargo.toml` with:

```toml
[package]
name = "jsph-tg-rcore-tutorial-ch2-moving-tangram"
description = "Chapter 2 moving tangram extension: VirtIO-GPU animated 'OS' tangram display via batch processing"
version = "0.4.8"
edition = "2024"
authors = ["Joseph Joshua Anggita <jj.anggita@gmail.com>"]
repository = "https://github.com/JosephJoshua/tg-rcore-tutorial"
license = "GPL-3.0"
readme = "README.md"
keywords = ["rcore", "tutorial", "no-std", "riscv", "tangram"]
categories = ["no-std", "embedded"]
exclude = [
    "tg-user/**",
]

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"

[dependencies]
riscv = "0.10.1"

tg-sbi = { package = "tg-rcore-tutorial-sbi", version = "0.4.8", features = ["nobios"] }
tg-linker = { package = "tg-rcore-tutorial-linker", version = "0.4.8" }
tg-console = { package = "tg-rcore-tutorial-console", version = "0.4.8" }
tg-kernel-context = { package = "tg-rcore-tutorial-kernel-context", version = "0.4.8" }
tg-syscall = { package = "tg-rcore-tutorial-syscall", version = "0.4.8", features = ["kernel"] }

[target.'cfg(target_arch = "riscv64")'.dependencies]
virtio-drivers = "0.1.0"

[build-dependencies]
tg-linker = { package = "tg-rcore-tutorial-linker", version = "0.4.8" }
serde = { version = "1", features = ["derive"] }
toml = "0.8"
```

- [ ] **Step 3: Update .cargo/config.toml**

Replace `jsph-tg-rcore-tutorial-ch2-moving-tangram/.cargo/config.toml` with:

```toml
[build]
target = "riscv64gc-unknown-none-elf"

[target.riscv64gc-unknown-none-elf]
runner = [
    "qemu-system-riscv64",
    "-machine", "virt",
    "-serial", "stdio",
    "-bios", "none",
    "-device", "virtio-gpu-device",
    "-kernel",
]

[env]
TG_USER_CRATE     = "tg-rcore-tutorial-user"
TG_USER_LOCAL_DIR = "../tg-rcore-tutorial-user"
TG_USER_VERSION   = "0.4.8"
```

Key changes from ch2: `-nographic` replaced with `-serial stdio` + `-device virtio-gpu-device`, and `TG_USER_LOCAL_DIR` points to `../tg-rcore-tutorial-user` (relative to new crate's manifest dir).

- [ ] **Step 4: Commit**

```bash
git add jsph-tg-rcore-tutorial-ch2-moving-tangram/
git commit -m "chore: scaffold ch2-moving-tangram from ch2"
```

---

### Task 2: Kernel GPU Infrastructure

**Files:**
- Create: `jsph-tg-rcore-tutorial-ch2-moving-tangram/src/allocator.rs`
- Create: `jsph-tg-rcore-tutorial-ch2-moving-tangram/src/gpu.rs`
- Modify: `jsph-tg-rcore-tutorial-ch2-moving-tangram/src/main.rs`

- [ ] **Step 1: Create allocator.rs**

Create `jsph-tg-rcore-tutorial-ch2-moving-tangram/src/allocator.rs` — copied from `tg-rcore-tutorial-ch1-tangram/src/allocator.rs` verbatim:

```rust
//! Static bump allocator for VirtIO DMA and `extern crate alloc`.
//!
//! Allocate-only (never frees). Backed by a 5 MiB static array in BSS.
//! Safe for single-core, one-shot init-then-display usage.

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

/// 5 MiB pool — enough for 1024x768 BGRA framebuffer (~3 MiB) + VirtQueue + headroom.
const POOL_SIZE: usize = 5 * 1024 * 1024;

/// Page size used by virtio-drivers for DMA allocations.
const PAGE_SIZE: usize = 4096;

#[repr(align(4096))]
struct AlignedPool {
    data: UnsafeCell<[u8; POOL_SIZE]>,
}

unsafe impl Sync for AlignedPool {}

static POOL: AlignedPool = AlignedPool {
    data: UnsafeCell::new([0u8; POOL_SIZE]),
};

/// Atomic offset into POOL.
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// Bump-allocate `size` bytes with the given alignment from the static pool.
/// Returns a pointer into the pool, or null if out of space.
fn bump_alloc(size: usize, align: usize) -> *mut u8 {
    loop {
        let current = NEXT.load(Ordering::Relaxed);
        let base = POOL.data.get() as usize;
        let addr = base + current;
        let aligned = (addr + align - 1) & !(align - 1);
        let offset = aligned - base;
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

/// VirtIO HAL for ch2-moving-tangram: identity-mapped, no paging, DMA from static pool.
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
```

- [ ] **Step 2: Create gpu.rs**

Create `jsph-tg-rcore-tutorial-ch2-moving-tangram/src/gpu.rs` — copied from `tg-rcore-tutorial-ch1-tangram/src/gpu.rs` verbatim:

```rust
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
```

- [ ] **Step 3: Modify main.rs — add module declarations, GPU globals, increase stack**

In `jsph-tg-rcore-tutorial-ch2-moving-tangram/src/main.rs`, make these changes:

**3a.** After line 23 (`#![no_main]`), add:

```rust
// Allocator needed for VirtIO DMA pool
#[cfg(target_arch = "riscv64")]
extern crate alloc;
```

**3b.** After line 31 (`extern crate tg_console;`), add module declarations:

```rust
#[cfg(target_arch = "riscv64")]
mod allocator;

#[cfg(target_arch = "riscv64")]
mod gpu;
```

**3c.** Change the kernel stack size from 8 pages (32 KiB) to 16 pages (64 KiB) — VirtIO-GPU init needs more stack space. On line 62, change:

```rust
    const STACK_SIZE: usize = 16 * 4096; // 64 KiB — VirtIO-GPU init needs extra stack
```

**3d.** After the imports block (after `use tg_syscall::{Caller, SyscallId};`), add GPU globals:

```rust
#[cfg(target_arch = "riscv64")]
use virtio_drivers::{MmioTransport, VirtIOGpu};

/// Global GPU driver, initialized once in rust_main before the batch loop.
#[cfg(target_arch = "riscv64")]
static mut GPU: Option<VirtIOGpu<'static, allocator::HalImpl, MmioTransport>> = None;

/// Global framebuffer info, initialized once in rust_main.
#[cfg(target_arch = "riscv64")]
static mut FRAMEBUFFER: Option<gpu::Framebuffer> = None;
```

**3e.** In `rust_main()`, after the syscall init block (after `tg_syscall::init_process(&SyscallContext);` on line 89), add GPU initialization:

```rust
    // Initialize VirtIO-GPU and fill white background
    #[cfg(target_arch = "riscv64")]
    {
        let (gpu_driver, fb_info) = gpu::init();
        let buf = unsafe { core::slice::from_raw_parts_mut(fb_info.ptr, fb_info.len) };

        // Fill white background (BGRA: all 0xFF)
        for byte in buf.iter_mut() {
            *byte = 0xFF;
        }

        // Initial flush to show white screen
        let mut gpu_driver = gpu_driver;
        gpu_driver.flush().expect("GPU flush failed");

        // Store globally for syscall handlers
        unsafe {
            GPU = Some(gpu_driver);
            FRAMEBUFFER = Some(fb_info);
        }

        // Enable rdtime in U-mode for user-space spin-wait timing
        unsafe { core::arch::asm!("csrs scounteren, {}", in(reg) (1 << 1)) };
    }
```

- [ ] **Step 4: Verify compilation**

```bash
cd jsph-tg-rcore-tutorial-ch2-moving-tangram && cargo check
```

Expected: compiles successfully (user apps still build without tangram feature — that's fine for now).

- [ ] **Step 5: Commit**

```bash
git add jsph-tg-rcore-tutorial-ch2-moving-tangram/src/allocator.rs \
        jsph-tg-rcore-tutorial-ch2-moving-tangram/src/gpu.rs \
        jsph-tg-rcore-tutorial-ch2-moving-tangram/src/main.rs
git commit -m "feat(ch2-tangram): add VirtIO-GPU driver and framebuffer init"
```

---

### Task 3: Kernel FB Syscall Handlers

**Files:**
- Modify: `jsph-tg-rcore-tutorial-ch2-moving-tangram/src/main.rs`

- [ ] **Step 1: Add custom syscall ID constants**

After the GPU globals (added in Task 2), add:

```rust
/// Custom syscall IDs for framebuffer operations.
/// These are handled directly in handle_syscall, bypassing tg_syscall::handle.
const SYSCALL_FB_INFO: usize = 2000;
const SYSCALL_FB_WRITE: usize = 2001;
```

- [ ] **Step 2: Modify handle_syscall to intercept custom syscalls**

Replace the entire `handle_syscall` function (lines 172-193) with:

```rust
fn handle_syscall(ctx: &mut LocalContext) -> SyscallResult {
    use tg_syscall::{SyscallId as Id, SyscallResult as Ret};

    let id_raw: usize = ctx.a(7);
    let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];

    // Intercept custom framebuffer syscalls before standard dispatch
    #[cfg(target_arch = "riscv64")]
    match id_raw {
        SYSCALL_FB_INFO => {
            let ret = handle_fb_info();
            *ctx.a_mut(0) = ret as usize;
            ctx.move_next();
            return SyscallResult::Done;
        }
        SYSCALL_FB_WRITE => {
            let ret = handle_fb_write(args[0], args[1], args[2], args[3], args[4]);
            *ctx.a_mut(0) = ret as usize;
            ctx.move_next();
            return SyscallResult::Done;
        }
        _ => {}
    }

    let id: Id = id_raw.into();
    match tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args) {
        Ret::Done(ret) => match id {
            Id::EXIT => SyscallResult::Exit(ctx.a(0)),
            _ => {
                *ctx.a_mut(0) = ret as _;
                ctx.move_next();
                SyscallResult::Done
            }
        },
        Ret::Unsupported(id) => SyscallResult::Error(id),
    }
}
```

- [ ] **Step 3: Add FB_INFO handler**

After `handle_syscall`, add:

```rust
/// FB_INFO syscall: returns (width << 32) | height packed in a0.
#[cfg(target_arch = "riscv64")]
fn handle_fb_info() -> usize {
    let fb = unsafe { FRAMEBUFFER.as_ref().expect("GPU not initialized") };
    ((fb.width as usize) << 32) | (fb.height as usize)
}
```

- [ ] **Step 4: Add FB_WRITE handler**

After `handle_fb_info`, add:

```rust
/// FB_WRITE syscall: copy user pixel data into framebuffer and flush.
/// Args: a0=x, a1=y, a2=w, a3=h, a4=data_ptr
#[cfg(target_arch = "riscv64")]
fn handle_fb_write(x: usize, y: usize, w: usize, h: usize, data_ptr: usize) -> usize {
    let fb = unsafe { FRAMEBUFFER.as_ref().expect("GPU not initialized") };
    let fb_w = fb.width as usize;
    let fb_h = fb.height as usize;

    // Bounds check
    if x + w > fb_w || y + h > fb_h || w == 0 || h == 0 {
        return usize::MAX; // -1 as usize
    }

    let fb_buf = unsafe { core::slice::from_raw_parts_mut(fb.ptr, fb.len) };
    let user_data = unsafe { core::slice::from_raw_parts(data_ptr as *const u8, w * h * 4) };

    // Copy row-by-row into framebuffer
    for row in 0..h {
        let fb_offset = ((y + row) * fb_w + x) * 4;
        let src_offset = row * w * 4;
        fb_buf[fb_offset..fb_offset + w * 4]
            .copy_from_slice(&user_data[src_offset..src_offset + w * 4]);
    }

    // Flush display
    let gpu = unsafe { GPU.as_mut().expect("GPU not initialized") };
    gpu.flush().expect("GPU flush failed");

    0
}
```

- [ ] **Step 5: Verify compilation**

```bash
cd jsph-tg-rcore-tutorial-ch2-moving-tangram && cargo check
```

- [ ] **Step 6: Commit**

```bash
git add jsph-tg-rcore-tutorial-ch2-moving-tangram/src/main.rs
git commit -m "feat(ch2-tangram): add FB_INFO and FB_WRITE syscall handlers"
```

---

### Task 4: User Tangram Module

**Files:**
- Create: `tg-rcore-tutorial-user/src/tangram.rs`
- Modify: `tg-rcore-tutorial-user/src/lib.rs`
- Modify: `tg-rcore-tutorial-user/Cargo.toml`

- [ ] **Step 1: Add tangram feature to user Cargo.toml**

At the end of `tg-rcore-tutorial-user/Cargo.toml`, add:

```toml

[features]
tangram = []
```

- [ ] **Step 2: Add fb syscall wrappers and tangram module to lib.rs**

In `tg-rcore-tutorial-user/src/lib.rs`, after the `pub use tg_syscall::*;` line (line 15), add:

```rust

#[cfg(feature = "tangram")]
pub mod tangram;

/// Query framebuffer dimensions from kernel.
/// Returns (width, height).
pub fn fb_info() -> (u32, u32) {
    let packed = unsafe { tg_syscall::native::syscall0(2000.into()) } as usize;
    let width = (packed >> 32) as u32;
    let height = packed as u32;
    (width, height)
}

/// Write a rectangular BGRA pixel region to the kernel framebuffer.
/// The kernel copies the data and flushes the display.
pub fn fb_write(x: u32, y: u32, w: u32, h: u32, data: *const u8) -> isize {
    unsafe {
        tg_syscall::native::syscall5(
            2001.into(),
            x as usize,
            y as usize,
            w as usize,
            h as usize,
            data as usize,
        )
    }
}
```

**Important:** Check whether `tg_syscall::native::syscall0` / `syscall5` are `pub`. If not, use inline assembly directly:

```rust
pub fn fb_info() -> (u32, u32) {
    let packed: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") 2000usize,
            lateout("a0") packed,
        );
    }
    let width = (packed >> 32) as u32;
    let height = packed as u32;
    (width, height)
}

pub fn fb_write(x: u32, y: u32, w: u32, h: u32, data: *const u8) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") 2001usize,
            inlateout("a0") x as usize => ret,
            in("a1") y as usize,
            in("a2") w as usize,
            in("a3") h as usize,
            in("a4") data as usize,
        );
    }
    ret
}
```

Verify which approach compiles. The inline assembly version is guaranteed to work regardless of tg_syscall's API visibility.

- [ ] **Step 3: Create tangram.rs**

Create `tg-rcore-tutorial-user/src/tangram.rs`:

```rust
//! Tangram piece geometry, colors, polygon rasterization, and rendering helpers.
//!
//! Gated by the `tangram` feature. Each piece is rendered into a heap-allocated
//! buffer and pushed to the kernel framebuffer via the FB_WRITE syscall.

use alloc::vec;

// ---- Color and palette ----

/// A color in BGRA format (matching VirtIO-GPU's B8G8R8A8UNORM).
#[derive(Clone, Copy)]
pub struct Color {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r, a: 0xFF }
    }
}

/// 7 distinct colors for tangram pieces.
pub const COLORS: [Color; 7] = [
    Color::new(0xE0, 0x40, 0x40), // 0: Red
    Color::new(0xFF, 0xA0, 0x20), // 1: Orange
    Color::new(0xFF, 0xE0, 0x20), // 2: Yellow
    Color::new(0x40, 0xC0, 0x40), // 3: Green
    Color::new(0x20, 0xC0, 0xE0), // 4: Cyan
    Color::new(0x40, 0x60, 0xE0), // 5: Blue
    Color::new(0xC0, 0x40, 0xC0), // 6: Magenta
];

// ---- Piece geometry ----

pub struct Piece {
    pub vertices: &'static [(i32, i32)],
    pub color_idx: usize,
}

// "O" pieces — rectangular frame with hole
const O1: [(i32, i32); 3] = [(140, 184), (440, 184), (140, 274)];
const O2: [(i32, i32); 3] = [(440, 184), (440, 274), (140, 274)];
const O3: [(i32, i32); 4] = [(140, 274), (220, 274), (220, 494), (140, 494)];
const O4: [(i32, i32); 4] = [(360, 274), (440, 274), (440, 494), (360, 494)];
const O5: [(i32, i32); 3] = [(140, 494), (440, 494), (290, 584)];
const O6: [(i32, i32); 3] = [(140, 494), (290, 584), (140, 584)];
const O7: [(i32, i32); 3] = [(440, 494), (440, 584), (290, 584)];

// "S" pieces — two offset blocks forming Z/S shape
const S1: [(i32, i32); 3] = [(580, 184), (840, 184), (710, 384)];
const S2: [(i32, i32); 3] = [(580, 184), (710, 384), (580, 384)];
const S3: [(i32, i32); 3] = [(840, 184), (840, 384), (710, 384)];
const S4: [(i32, i32); 3] = [(540, 384), (800, 384), (670, 484)];
const S5: [(i32, i32); 3] = [(540, 384), (670, 484), (540, 584)];
const S6: [(i32, i32); 3] = [(800, 384), (800, 584), (670, 484)];
const S7: [(i32, i32); 3] = [(540, 584), (670, 484), (800, 584)];

/// All 14 tangram pieces for the "OS" display.
pub static PIECES: [Piece; 14] = [
    Piece { vertices: &O1, color_idx: 0 }, // Red
    Piece { vertices: &O2, color_idx: 1 }, // Orange
    Piece { vertices: &O3, color_idx: 2 }, // Yellow
    Piece { vertices: &O4, color_idx: 3 }, // Green
    Piece { vertices: &O5, color_idx: 4 }, // Cyan
    Piece { vertices: &O6, color_idx: 5 }, // Blue
    Piece { vertices: &O7, color_idx: 6 }, // Magenta
    Piece { vertices: &S1, color_idx: 4 }, // Cyan
    Piece { vertices: &S2, color_idx: 5 }, // Blue
    Piece { vertices: &S3, color_idx: 0 }, // Red
    Piece { vertices: &S4, color_idx: 6 }, // Magenta
    Piece { vertices: &S5, color_idx: 1 }, // Orange
    Piece { vertices: &S6, color_idx: 3 }, // Green
    Piece { vertices: &S7, color_idx: 2 }, // Yellow
];

/// Piece names for serial output.
pub const PIECE_NAMES: [&str; 14] = [
    "O1 (top-left tri)",
    "O2 (top-right tri)",
    "O3 (left bar)",
    "O4 (right bar)",
    "O5 (bottom-center tri)",
    "O6 (bottom-left tri)",
    "O7 (bottom-right tri)",
    "S1 (top-center tri)",
    "S2 (top-left tri)",
    "S3 (top-right tri)",
    "S4 (mid-center tri)",
    "S5 (bottom-left tri)",
    "S6 (bottom-right tri)",
    "S7 (bottom-center tri)",
];

// ---- Polygon rasterization ----

/// Fill a convex polygon into a BGRA pixel buffer.
/// Uses scanline rasterization with integer-only math.
pub fn fill_polygon(fb: &mut [u8], width: u32, height: u32, vertices: &[(i32, i32)], color: Color) {
    if vertices.len() < 3 {
        return;
    }

    let mut min_y = vertices[0].1;
    let mut max_y = vertices[0].1;
    for &(_, y) in vertices {
        if y < min_y { min_y = y; }
        if y > max_y { max_y = y; }
    }

    let min_y = if min_y < 0 { 0 } else { min_y };
    let max_y = if max_y >= height as i32 { height as i32 - 1 } else { max_y };
    let n = vertices.len();

    for y in min_y..=max_y {
        let mut x_intersections = [0i32; 16];
        let mut count = 0;

        for i in 0..n {
            let (x0, y0) = vertices[i];
            let (x1, y1) = vertices[(i + 1) % n];

            if y0 == y1 { continue; }

            let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
            if y < lo || y >= hi { continue; }

            let x = x0 + ((y - y0) as i64 * (x1 - x0) as i64 / (y1 - y0) as i64) as i32;
            if count < 16 {
                x_intersections[count] = x;
                count += 1;
            }
        }

        for i in 1..count {
            let key = x_intersections[i];
            let mut j = i;
            while j > 0 && x_intersections[j - 1] > key {
                x_intersections[j] = x_intersections[j - 1];
                j -= 1;
            }
            x_intersections[j] = key;
        }

        let mut i = 0;
        while i + 1 < count {
            let x_start = if x_intersections[i] < 0 { 0 } else { x_intersections[i] };
            let x_end = if x_intersections[i + 1] >= width as i32 {
                width as i32 - 1
            } else {
                x_intersections[i + 1]
            };

            for x in x_start..=x_end {
                let offset = ((y as u32 * width + x as u32) * 4) as usize;
                if offset + 3 < fb.len() {
                    fb[offset] = color.b;
                    fb[offset + 1] = color.g;
                    fb[offset + 2] = color.r;
                    fb[offset + 3] = color.a;
                }
            }
            i += 2;
        }
    }
}

// ---- Rendering helpers ----

/// Render a single tangram piece to the kernel framebuffer via FB_WRITE.
///
/// 1. Queries framebuffer dimensions
/// 2. Computes bounding box of the piece
/// 3. Allocates a local pixel buffer (heap)
/// 4. Rasterizes the polygon into the local buffer
/// 5. Pushes the buffer to the kernel via fb_write()
pub fn render_piece(piece_idx: usize) {
    let piece = &PIECES[piece_idx];
    let color = COLORS[piece.color_idx];

    // Compute bounding box
    let mut min_x = piece.vertices[0].0;
    let mut min_y = piece.vertices[0].1;
    let mut max_x = min_x;
    let mut max_y = min_y;
    for &(x, y) in piece.vertices {
        if x < min_x { min_x = x; }
        if x > max_x { max_x = x; }
        if y < min_y { min_y = y; }
        if y > max_y { max_y = y; }
    }

    let bbox_w = (max_x - min_x + 1) as usize;
    let bbox_h = (max_y - min_y + 1) as usize;

    // Allocate local buffer, filled with white (BGRA 0xFF)
    let buf_size = bbox_w * bbox_h * 4;
    let mut buf = vec![0xFFu8; buf_size];

    // Offset vertices to local buffer coordinates
    let mut local_verts = [(0i32, 0i32); 8];
    for (i, &(x, y)) in piece.vertices.iter().enumerate() {
        local_verts[i] = (x - min_x, y - min_y);
    }
    let local_verts = &local_verts[..piece.vertices.len()];

    // Rasterize into local buffer
    fill_polygon(&mut buf, bbox_w as u32, bbox_h as u32, local_verts, color);

    // Push to kernel framebuffer
    crate::fb_write(min_x as u32, min_y as u32, bbox_w as u32, bbox_h as u32, buf.as_ptr());
}

/// Spin-wait for `ms` milliseconds using rdtime (QEMU virt: 10 MHz timer).
pub fn spin_wait_ms(ms: usize) {
    let ticks = ms * 10_000;
    let start: usize;
    unsafe { core::arch::asm!("rdtime {}", out(reg) start) };
    loop {
        let now: usize;
        unsafe { core::arch::asm!("rdtime {}", out(reg) now) };
        if now.wrapping_sub(start) >= ticks {
            break;
        }
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add tg-rcore-tutorial-user/src/tangram.rs \
        tg-rcore-tutorial-user/src/lib.rs \
        tg-rcore-tutorial-user/Cargo.toml
git commit -m "feat(user): add tangram module and fb syscall wrappers (feature-gated)"
```

---

### Task 5: User Programs and Cases

**Files:**
- Create: `tg-rcore-tutorial-user/src/bin/tangram_00.rs` through `tangram_13.rs`
- Modify: `tg-rcore-tutorial-user/cases.toml`

- [ ] **Step 1: Create 14 tangram user programs**

Each program follows this template (only `PIECE_INDEX` differs):

```rust
#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    let idx: usize = PIECE_INDEX;
    println!("[tangram] rendering piece {}: {}", idx, user_lib::tangram::PIECE_NAMES[idx]);
    user_lib::tangram::render_piece(idx);
    user_lib::tangram::spin_wait_ms(300);
    0
}
```

Create 14 files — `tangram_00.rs` (PIECE_INDEX=0) through `tangram_13.rs` (PIECE_INDEX=13).

Fastest approach — generate them with a shell loop:

```bash
for i in $(seq 0 13); do
    printf -v padded "%02d" "$i"
    cat > tg-rcore-tutorial-user/src/bin/tangram_${padded}.rs << 'RUSTEOF'
#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    let idx: usize = PIECE_INDEX;
    println!("[tangram] rendering piece {}: {}", idx, user_lib::tangram::PIECE_NAMES[idx]);
    user_lib::tangram::render_piece(idx);
    user_lib::tangram::spin_wait_ms(300);
    0
}
RUSTEOF
    sed -i "s/PIECE_INDEX/$i/" "tg-rcore-tutorial-user/src/bin/tangram_${padded}.rs"
done
```

- [ ] **Step 2: Add tangram programs to cases.toml**

In `tg-rcore-tutorial-user/cases.toml`, append the tangram entries to the end of the `[ch2]` cases list. The `[ch2]` section becomes:

```toml
[ch2]
base = 0x8040_0000
step = 0
cases = [
    "00hello_world",
    "01store_fault",
    "02power",
    "03priv_inst",
    "04priv_csr",
    "08power_3",
    "09power_5",
    "10power_7",
    "tangram_00",
    "tangram_01",
    "tangram_02",
    "tangram_03",
    "tangram_04",
    "tangram_05",
    "tangram_06",
    "tangram_07",
    "tangram_08",
    "tangram_09",
    "tangram_10",
    "tangram_11",
    "tangram_12",
    "tangram_13",
]
```

- [ ] **Step 3: Commit**

```bash
git add tg-rcore-tutorial-user/src/bin/tangram_*.rs \
        tg-rcore-tutorial-user/cases.toml
git commit -m "feat(user): add 14 tangram user programs and register in cases.toml"
```

---

### Task 6: Build Integration

**Files:**
- Modify: `jsph-tg-rcore-tutorial-ch2-moving-tangram/build.rs`

- [ ] **Step 1: Add --features tangram to user app builds**

In `jsph-tg-rcore-tutorial-ch2-moving-tangram/build.rs`, modify the `build_user_app` function to pass `--features tangram`. Find the `cmd.args` block in `build_user_app` (around line 103) and add the features flag:

```rust
fn build_user_app(tg_user_root: &PathBuf, name: &str, base_address: u64) {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "build",
        "--manifest-path",
        tg_user_root.join("Cargo.toml").to_string_lossy().as_ref(),
        "--bin",
        name,
        "--target",
        TARGET_ARCH,
        "--features",
        "tangram",
    ]);

    if base_address != 0 {
        cmd.env("BASE_ADDRESS", base_address.to_string());
    }

    let status = cmd.status().expect("failed to execute cargo build for user app");
    if !status.success() {
        panic!("failed to build user app {name}");
    }
}
```

- [ ] **Step 2: Verify full build**

```bash
cd jsph-tg-rcore-tutorial-ch2-moving-tangram && cargo build
```

Expected: builds all user programs (including tangram_00–tangram_13) and the kernel successfully.

- [ ] **Step 3: Commit**

```bash
git add jsph-tg-rcore-tutorial-ch2-moving-tangram/build.rs
git commit -m "feat(ch2-tangram): pass --features tangram when building user apps"
```

---

### Task 7: Test Script

**Files:**
- Modify: `jsph-tg-rcore-tutorial-ch2-moving-tangram/test.sh`

- [ ] **Step 1: Update test.sh for headless GPU testing**

Replace `jsph-tg-rcore-tutorial-ch2-moving-tangram/test.sh` with:

```bash
#!/bin/bash
# ch2-moving-tangram test script
# Runs kernel with VirtIO-GPU in headless mode (no display) with timeout.

set -e

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[0;33m'
NC='\033[0m'

echo "Running ch2-moving-tangram test..."
echo -e "${YELLOW}────────── cargo run output ──────────${NC}"

# Run with timeout (60s) — GPU init + 14 pieces × 300ms + overhead
# The -display none flag makes QEMU headless for CI
set -o pipefail
if timeout 60 cargo run 2>&1 | tee /dev/stderr | grep -q "\[tangram\] rendering piece 13"; then
    echo ""
    echo -e "${YELLOW}────────── test result ──────────${NC}"
    echo -e "${GREEN}✓ ch2-moving-tangram: all 14 pieces rendered${NC}"
    exit 0
else
    echo ""
    echo -e "${YELLOW}────────── test result ──────────${NC}"
    echo -e "${RED}✗ ch2-moving-tangram: failed to render all pieces${NC}"
    exit 1
fi
```

- [ ] **Step 2: Commit**

```bash
git add jsph-tg-rcore-tutorial-ch2-moving-tangram/test.sh
git commit -m "feat(ch2-tangram): add headless GPU test script"
```

---

### Task 8: Verification

- [ ] **Step 1: Run cargo run — visual verification**

```bash
cd jsph-tg-rcore-tutorial-ch2-moving-tangram && cargo run
```

Expected:
- QEMU window opens with white background
- Serial output shows kernel boot, existing ch2 test programs run
- After tests: tangram pieces appear one by one (~300ms apart)
- "OS" pattern fully assembled after all 14 pieces
- System shuts down cleanly

- [ ] **Step 2: Verify existing ch2 tests still work**

Check serial output for the standard ch2 programs:
- `Hello, world from user mode program!` (00hello_world)
- Store fault / privilege violation messages (01-04)
- Power calculations (08, 09, 10)

- [ ] **Step 3: Run cargo check on the new kernel crate**

```bash
cd jsph-tg-rcore-tutorial-ch2-moving-tangram && cargo check
```

Expected: no errors or warnings.

- [ ] **Step 4: Run cargo publish --dry-run on the new kernel crate**

```bash
cd jsph-tg-rcore-tutorial-ch2-moving-tangram && cargo publish --dry-run
```

Expected: passes (dummy app.asm generated, no user apps built in package context).

- [ ] **Step 5: Run cargo check on the user crate (without tangram feature)**

```bash
cd tg-rcore-tutorial-user && cargo check
```

Expected: passes — tangram module not compiled without the feature.

- [ ] **Step 6: Run cargo publish --dry-run on the user crate**

```bash
cd tg-rcore-tutorial-user && cargo publish --dry-run
```

Expected: passes.

- [ ] **Step 7: Final commit (if any fixups needed)**

```bash
git add -A && git commit -m "fix(ch2-tangram): address verification issues"
```

Only if fixups were needed.

# Ch5-Pingpong Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add VirtIO GPU/keyboard support and shared memory IPC to the ch5 process-management kernel, then build a two-player Pong game using two cooperating processes.

**Architecture:** Copy ch5 to ch5-pingpong, add VirtIO drivers (DMA allocator + device init), framebuffer/keyboard syscalls, and a shared memory mechanism that maps the same physical page into parent and child after fork(). User-side Pong game uses fork() to create two processes that communicate via shared memory.

**Tech Stack:** Rust no_std, RISC-V 64, QEMU virt, virtio-drivers 0.1.0, Sv39 page tables, tg-rcore-tutorial ecosystem

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `tg-rcore-tutorial-ch5-pingpong/Cargo.toml` | Modify | Crate name, virtio-drivers dep |
| `tg-rcore-tutorial-ch5-pingpong/.cargo/config.toml` | Modify | QEMU GPU/keyboard flags, TG_USER_DIR |
| `tg-rcore-tutorial-ch5-pingpong/src/allocator.rs` | Create | DMA bump allocator + Hal trait impl |
| `tg-rcore-tutorial-ch5-pingpong/src/virtio.rs` | Create | MMIO scan, GPU + keyboard init |
| `tg-rcore-tutorial-ch5-pingpong/src/process.rs` | Modify | shared_page field, fork shared mem |
| `tg-rcore-tutorial-ch5-pingpong/src/main.rs` | Modify | MEMORY, MMIO map, VirtIO init, FB/kbd/shm syscalls |
| `tg-rcore-tutorial-ch5-pingpong/build.rs` | Modify | case_key, --features pingpong |
| `tg-rcore-tutorial-ch5-pingpong/test.sh` | Modify | Headless QEMU with GPU + timeout |
| `tg-rcore-tutorial-user/Cargo.toml` | Modify | pingpong feature |
| `tg-rcore-tutorial-user/src/lib.rs` | Modify | pingpong module, shm_create() |
| `tg-rcore-tutorial-user/src/pingpong.rs` | Create | Game logic, physics, rendering |
| `tg-rcore-tutorial-user/src/bin/pingpong.rs` | Create | Binary entry point |
| `tg-rcore-tutorial-user/cases.toml` | Modify | ch5_pingpong section |

## Task Dependency Graph

```
Task 1 (copy + config)
  ├── Task 2 (allocator.rs)
  │     └── Task 4 (virtio.rs)
  │           └── Task 5 (main.rs: VirtIO init + MMIO + FB/kbd syscalls)
  │                 └── Task 8 (build.rs + test.sh + integration)
  ├── Task 3 (process.rs: shared_page + fork)
  │     └── Task 6 (main.rs: SHM_CREATE syscall) ──┐
  └── Task 7A (user crate: feature + wrappers + cases.toml)
        └── Task 7B (user: pingpong.rs game module + binary) ──> Task 8
```

Tasks 2, 3, 7A are parallelizable after Task 1.
Tasks 4 and 6 are parallelizable after their dependencies.
Task 7B can run in parallel with kernel tasks 4-6 after 7A.

---

### Task 1: Copy ch5 and configure crate

**Files:**
- Create: `tg-rcore-tutorial-ch5-pingpong/` (copy from ch5)
- Modify: `tg-rcore-tutorial-ch5-pingpong/Cargo.toml`
- Modify: `tg-rcore-tutorial-ch5-pingpong/.cargo/config.toml`

- [ ] **Step 1: Copy ch5 directory**

```bash
cp -r tg-rcore-tutorial-ch5 tg-rcore-tutorial-ch5-pingpong
rm -rf tg-rcore-tutorial-ch5-pingpong/{target,Cargo.lock,.gitrepo}
```

- [ ] **Step 2: Update Cargo.toml**

In `tg-rcore-tutorial-ch5-pingpong/Cargo.toml`, make these changes:

Change `name`:
```toml
name = "jsph-tg-rcore-tutorial-ch5-pingpong"
```

Change `description`:
```toml
description = "Chapter 5 Pingpong: Two-player Pong game with VirtIO GPU, shared memory IPC, and fork-based multi-process architecture."
```

Add `virtio-drivers` to `[dependencies]`:
```toml
virtio-drivers = "0.1.0"
```

- [ ] **Step 3: Update .cargo/config.toml**

Replace the entire `[target.riscv64gc-unknown-none-elf]` runner section:

```toml
[target.riscv64gc-unknown-none-elf]
runner = [
    "qemu-system-riscv64",
    "-machine", "virt",
    "-serial", "stdio",
    "-bios", "none",
    "-device", "virtio-gpu-device",
    "-device", "virtio-keyboard-device",
    "-kernel",
]
```

Add `TG_USER_DIR` to the `[env]` section:
```toml
TG_USER_DIR       = { value = "../tg-rcore-tutorial-user", relative = true }
```

- [ ] **Step 4: Verify it compiles**

```bash
cd tg-rcore-tutorial-ch5-pingpong && TG_SKIP_USER_APPS=1 cargo check
```

Expected: compiles successfully (no VirtIO code yet, just config changes).

- [ ] **Step 5: Commit**

```bash
git add tg-rcore-tutorial-ch5-pingpong/
git commit -m "build(ch5-pingpong): copy ch5, configure crate name and QEMU devices"
```

---

### Task 2: Create DMA bump allocator

**Files:**
- Create: `tg-rcore-tutorial-ch5-pingpong/src/allocator.rs`

**Reference:** `tg-rcore-tutorial-ch4-tetris/src/allocator.rs` — copy this file verbatim.

- [ ] **Step 1: Create allocator.rs**

Create `tg-rcore-tutorial-ch5-pingpong/src/allocator.rs` with this exact content:

```rust
//! DMA bump allocator and VirtIO Hal implementation.
//!
//! Uses a fixed region at 0x8500_0000 for DMA buffers (above kernel heap).
//! The kernel identity-maps physical RAM, so phys_to_virt is identity.

use core::sync::atomic::{AtomicUsize, Ordering};
use virtio_drivers::{Hal, PhysAddr, VirtAddr};

/// Fixed base address for the DMA pool — above kernel heap, within MEMORY range.
const POOL_BASE: usize = 0x8500_0000;

/// 5 MiB pool for VirtIO GPU framebuffer + virtqueues.
const POOL_SIZE: usize = 5 * 1024 * 1024;

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
```

- [ ] **Step 2: Add module declaration to main.rs**

Add at the top of `tg-rcore-tutorial-ch5-pingpong/src/main.rs`, after the existing `mod processor;` line:

```rust
/// DMA bump allocator for VirtIO devices
mod allocator;
/// VirtIO GPU and keyboard device drivers
mod virtio;
```

Note: `virtio.rs` doesn't exist yet — this will fail to compile until Task 4. That's OK for now if building with `TG_SKIP_USER_APPS=1 cargo check` produces just a "file not found" error for virtio. Alternatively, create an empty placeholder:

Create `tg-rcore-tutorial-ch5-pingpong/src/virtio.rs`:
```rust
// Placeholder — populated in Task 4
```

- [ ] **Step 3: Verify it compiles**

```bash
cd tg-rcore-tutorial-ch5-pingpong && TG_SKIP_USER_APPS=1 cargo check
```

Expected: compiles (allocator.rs has no callers yet, virtio.rs is a placeholder).

- [ ] **Step 4: Commit**

```bash
git add tg-rcore-tutorial-ch5-pingpong/src/allocator.rs tg-rcore-tutorial-ch5-pingpong/src/virtio.rs tg-rcore-tutorial-ch5-pingpong/src/main.rs
git commit -m "feat(ch5-pingpong): add DMA bump allocator for VirtIO"
```

---

### Task 3: Add shared_page to Process and modify fork()

**Files:**
- Modify: `tg-rcore-tutorial-ch5-pingpong/src/process.rs`

- [ ] **Step 1: Add shared_page field to Process struct**

In `tg-rcore-tutorial-ch5-pingpong/src/process.rs`, add the field to the `Process` struct:

```rust
pub struct Process {
    /// 进程标识符（PID），创建后不可变
    pub pid: ProcId,
    /// 用户态上下文，包含 satp 和通用寄存器
    pub context: ForeignContext,
    /// 进程的独立地址空间（Sv39 页表）
    pub address_space: AddressSpace<Sv39, Sv39Manager>,
    /// 堆底地址
    pub heap_bottom: usize,
    /// 当前程序 break 位置
    pub program_brk: usize,
    /// stride 调度算法：当前步长值
    pub stride: usize,
    /// stride 调度算法：进程优先级
    pub priority: usize,
    /// Shared memory page: physical page number shared between parent and child after fork
    pub shared_page: Option<PPN<Sv39>>,
}
```

- [ ] **Step 2: Update from_elf to initialize shared_page**

In the `from_elf` method's return value, add `shared_page: None`:

```rust
        Some(Self {
            pid: ProcId::new(),
            context: ForeignContext { context, satp },
            address_space,
            heap_bottom,
            program_brk: heap_bottom,
            stride: 0,
            priority: 16,
            shared_page: None,
        })
```

- [ ] **Step 3: Modify fork() to share the page instead of copying**

In the `fork()` method, after the existing code that creates the child and before the `Some(Self { ... })` return, add shared memory handling. The complete modified `fork()`:

```rust
    pub fn fork(&mut self) -> Option<Process> {
        let pid = ProcId::new();
        let parent_addr_space = &self.address_space;
        let mut address_space: AddressSpace<Sv39, Sv39Manager> = AddressSpace::new();
        parent_addr_space.cloneself(&mut address_space);
        map_portal(&address_space);

        // If parent has a shared memory page, remap it in child to the SAME physical page
        // (cloneself deep-copied it — we need to undo the copy and map the original)
        let shared_page = if let Some(ppn) = self.shared_page {
            const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;
            const SHARED_MEM_VA: usize = 0x3000_0000;
            // Unmap the deep-copied page at the shared VA
            address_space.unmap(
                VAddr::new(SHARED_MEM_VA).floor()..VAddr::new(SHARED_MEM_VA + PAGE_SIZE).ceil(),
            );
            // Map the SAME physical page from parent
            address_space.map_extern(
                VAddr::new(SHARED_MEM_VA).floor()..VAddr::new(SHARED_MEM_VA + PAGE_SIZE).ceil(),
                ppn,
                build_flags("U_WRV"),
            );
            Some(ppn)
        } else {
            None
        };

        let context = self.context.context.clone();
        let satp = (8 << 60) | address_space.root_ppn().val();
        let foreign_ctx = ForeignContext { context, satp };
        Some(Self {
            pid,
            context: foreign_ctx,
            address_space,
            heap_bottom: self.heap_bottom,
            program_brk: self.program_brk,
            stride: 0,
            priority: 16,
            shared_page,
        })
    }
```

- [ ] **Step 4: Verify it compiles**

```bash
cd tg-rcore-tutorial-ch5-pingpong && TG_SKIP_USER_APPS=1 cargo check
```

Expected: compiles successfully.

- [ ] **Step 5: Commit**

```bash
git add tg-rcore-tutorial-ch5-pingpong/src/process.rs
git commit -m "feat(ch5-pingpong): add shared memory page support to Process and fork"
```

---

### Task 4: Create VirtIO device initialization

**Files:**
- Modify: `tg-rcore-tutorial-ch5-pingpong/src/virtio.rs`

**Reference:** `tg-rcore-tutorial-ch4-tetris/src/virtio.rs` — copy this file verbatim.

- [ ] **Step 1: Write virtio.rs**

Replace the placeholder `tg-rcore-tutorial-ch5-pingpong/src/virtio.rs` with:

```rust
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
```

- [ ] **Step 2: Verify it compiles**

```bash
cd tg-rcore-tutorial-ch5-pingpong && TG_SKIP_USER_APPS=1 cargo check
```

Expected: compiles (virtio::init() not called yet, but types are valid).

- [ ] **Step 3: Commit**

```bash
git add tg-rcore-tutorial-ch5-pingpong/src/virtio.rs
git commit -m "feat(ch5-pingpong): add VirtIO GPU and keyboard initialization"
```

---

### Task 5: Modify main.rs — VirtIO init, MMIO mapping, FB/keyboard/SHM syscalls

This is the largest task. It modifies `tg-rcore-tutorial-ch5-pingpong/src/main.rs` with:
1. Memory constant change (48 -> 70 MiB)
2. MMIO region mapping in kernel_space()
3. VirtIO global state and initialization in rust_main()
4. Custom syscall interception (FB_INFO, FB_WRITE, SHM_CREATE) in the scheduling loop
5. Non-blocking keyboard IO::read

**Files:**
- Modify: `tg-rcore-tutorial-ch5-pingpong/src/main.rs`

- [ ] **Step 1: Change MEMORY constant**

Find and change:
```rust
const MEMORY: usize = 48 << 20;
```
To:
```rust
/// 物理内存容量 = 70 MiB (increased for VirtIO GPU DMA pool)
const MEMORY: usize = 70 << 20;
```

- [ ] **Step 2: Add VirtIO global state**

After the `static KERNEL_SPACE` declaration, add:

```rust
/// Custom syscall IDs for framebuffer operations.
const SYSCALL_FB_INFO: usize = 2000;
/// Write pixels to framebuffer.
const SYSCALL_FB_WRITE: usize = 2001;
/// Create shared memory page.
const SYSCALL_SHM_CREATE: usize = 2002;

/// Global GPU driver.
#[cfg(target_arch = "riscv64")]
static mut GPU: Option<virtio_drivers::VirtIOGpu<'static, crate::allocator::HalImpl, virtio_drivers::MmioTransport>> = None;

/// Global framebuffer info.
#[cfg(target_arch = "riscv64")]
static mut FRAMEBUFFER: Option<crate::virtio::Framebuffer> = None;

/// Global keyboard driver.
#[cfg(target_arch = "riscv64")]
static mut KEYBOARD: Option<virtio_drivers::VirtIOInput<crate::allocator::HalImpl, virtio_drivers::MmioTransport>> = None;
```

- [ ] **Step 3: Add MMIO mapping to kernel_space()**

In the `kernel_space()` function, after the heap mapping (the `space.map_extern(s.floor()..e.ceil(), ...)` for heap) and before the portal mapping, add:

```rust
    // Map VirtIO MMIO region — identity-mapped, R+W
    let mmio_start = VAddr::<Sv39>::new(0x1000_0000);
    let mmio_end = VAddr::<Sv39>::new(0x1000_9000);
    space.map_extern(
        mmio_start.floor()..mmio_end.ceil(),
        PPN::new(mmio_start.floor().val()),
        build_flags("_WRV"),
    );
```

- [ ] **Step 4: Add VirtIO initialization to rust_main()**

In `rust_main()`, after the syscall init calls (`tg_syscall::init_memory(&SyscallContext);`) and before the initproc loading, add:

```rust
    // Initialize VirtIO GPU and keyboard
    #[cfg(target_arch = "riscv64")]
    {
        let devices = virtio::init();
        let (gpu_driver, fb_info) = devices.gpu;
        unsafe {
            GPU = Some(gpu_driver);
            FRAMEBUFFER = Some(fb_info);
            KEYBOARD = devices.keyboard;
        }
    }
```

- [ ] **Step 5: Add FB_INFO and FB_WRITE handler functions**

Add these functions before the `mod impls` block (or after `kernel_space()`, before `map_portal()`):

```rust
/// FB_INFO: returns (width << 32) | height.
#[cfg(target_arch = "riscv64")]
fn handle_fb_info() -> usize {
    let (width, height) = unsafe {
        let p = &raw const FRAMEBUFFER;
        let fb = (*p).as_ref().expect("GPU not initialized");
        (fb.width, fb.height)
    };
    ((width as usize) << 32) | (height as usize)
}

/// FB_WRITE: copy BGRA pixels from user buffer into framebuffer and flush.
#[cfg(target_arch = "riscv64")]
fn handle_fb_write(x: usize, y: usize, w: usize, h: usize, data_ptr: usize) -> usize {
    let (fb_ptr, fb_len, fb_w, fb_h) = unsafe {
        let p = &raw const FRAMEBUFFER;
        let fb = (*p).as_ref().expect("GPU not initialized");
        (fb.ptr, fb.len, fb.width as usize, fb.height as usize)
    };

    if x + w > fb_w || y + h > fb_h || w == 0 || h == 0 {
        return usize::MAX;
    }

    let fb_buf = unsafe { core::slice::from_raw_parts_mut(fb_ptr, fb_len) };

    const READABLE: VmFlags<Sv39> = build_flags("RV");
    let process = PROCESSOR.get_mut().current().unwrap();

    for row in 0..h {
        let row_va = data_ptr + row * w * 4;
        if let Some(ptr) = process
            .address_space
            .translate::<u8>(VAddr::new(row_va), READABLE)
        {
            let user_row = unsafe { core::slice::from_raw_parts(ptr.as_ptr(), w * 4) };
            for col in 0..w {
                let src_off = col * 4;
                let alpha = user_row[src_off + 3];
                if alpha == 0 {
                    continue;
                }
                let fb_off = ((y + row) * fb_w + (x + col)) * 4;
                fb_buf[fb_off] = user_row[src_off];
                fb_buf[fb_off + 1] = user_row[src_off + 1];
                fb_buf[fb_off + 2] = user_row[src_off + 2];
                fb_buf[fb_off + 3] = user_row[src_off + 3];
            }
        } else {
            return usize::MAX;
        }
    }

    let gpu = unsafe {
        let p = &raw mut GPU;
        (*p).as_mut().expect("GPU not initialized")
    };
    gpu.flush().expect("GPU flush failed");

    0
}

/// SHM_CREATE: allocate a physical page and map it at SHARED_MEM_VA in the current process.
#[cfg(target_arch = "riscv64")]
fn handle_shm_create() -> usize {
    const SHARED_MEM_VA: usize = 0x3000_0000;
    const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;

    let process = PROCESSOR.get_mut().current().unwrap();

    // Already has shared memory?
    if process.shared_page.is_some() {
        return SHARED_MEM_VA;
    }

    // Allocate one zeroed physical page
    let page_ptr = unsafe {
        alloc::alloc::alloc_zeroed(core::alloc::Layout::from_size_align_unchecked(
            PAGE_SIZE,
            PAGE_SIZE,
        ))
    };
    if page_ptr.is_null() {
        return usize::MAX;
    }

    let ppn = PPN::new(page_ptr as usize >> Sv39::PAGE_BITS);

    // Map into current process at the fixed virtual address
    process.address_space.map_extern(
        VAddr::new(SHARED_MEM_VA).floor()..VAddr::new(SHARED_MEM_VA + PAGE_SIZE).ceil(),
        ppn,
        build_flags("U_WRV"),
    );
    process.shared_page = Some(ppn);

    SHARED_MEM_VA
}
```

- [ ] **Step 6: Intercept custom syscalls in the scheduling loop**

In `rust_main()`, find the `UserEnvCall` match arm. The current code does:

```rust
scause::Trap::Exception(scause::Exception::UserEnvCall) => {
    use tg_syscall::{SyscallId as Id, SyscallResult as Ret};
    let ctx = &mut task.context.context;
    ctx.move_next();
    let id: Id = ctx.a(7).into();
    let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];
    match tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args) {
```

Replace the entire `UserEnvCall` arm with:

```rust
                scause::Trap::Exception(scause::Exception::UserEnvCall) => {
                    use tg_syscall::{SyscallId as Id, SyscallResult as Ret};
                    let ctx = &mut task.context.context;
                    ctx.move_next();
                    let id: Id = ctx.a(7).into();
                    let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];

                    // Handle custom syscalls before standard dispatch
                    let id_num = ctx.a(7);
                    #[cfg(target_arch = "riscv64")]
                    match id_num {
                        SYSCALL_FB_INFO => {
                            let ret = handle_fb_info();
                            *ctx.a_mut(0) = ret;
                            unsafe { (*processor).make_current_suspend() };
                            continue;
                        }
                        SYSCALL_FB_WRITE => {
                            let ret = handle_fb_write(args[0], args[1], args[2], args[3], args[4]);
                            *ctx.a_mut(0) = ret;
                            unsafe { (*processor).make_current_suspend() };
                            continue;
                        }
                        SYSCALL_SHM_CREATE => {
                            let ret = handle_shm_create();
                            *ctx.a_mut(0) = ret;
                            unsafe { (*processor).make_current_suspend() };
                            continue;
                        }
                        _ => {}
                    }

                    match tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args) {
                        Ret::Done(ret) => match id {
                            Id::EXIT => unsafe { (*processor).make_current_exited(ret) },
                            _ => {
                                let ctx = &mut task.context.context;
                                *ctx.a_mut(0) = ret as _;
                                unsafe { (*processor).make_current_suspend() };
                            }
                        },
                        Ret::Unsupported(_) => {
                            log::info!("id = {id:?}");
                            unsafe { (*processor).make_current_exited(-2) };
                        }
                    }
                }
```

- [ ] **Step 7: Replace blocking IO::read with non-blocking VirtIO keyboard**

In the `impl IO for SyscallContext` block inside `mod impls`, replace the `read` method:

```rust
        #[inline]
        fn read(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            if fd == STDIN {
                const WRITEABLE: VmFlags<Sv39> = build_flags("W_V");
                if let Some(mut ptr) = PROCESSOR
                    .get_mut()
                    .current()
                    .unwrap()
                    .address_space
                    .translate::<u8>(VAddr::new(buf), WRITEABLE)
                {
                    // Non-blocking VirtIO keyboard poll
                    #[cfg(target_arch = "riscv64")]
                    {
                        let keyboard = unsafe {
                            let p = &raw mut crate::KEYBOARD;
                            (*p).as_mut()
                        };
                        if let Some(kbd) = keyboard {
                            if let Some(event) = kbd.pop_pending_event() {
                                // Only handle key press events (type=1, value=1)
                                if event.event_type == 1 && event.value == 1 {
                                    let keycode = event.code as u8;
                                    unsafe { *ptr.as_mut() = keycode };
                                    return 1;
                                }
                            }
                        }
                        return 0;
                    }
                    #[cfg(not(target_arch = "riscv64"))]
                    {
                        let _ = (ptr, count);
                        return 0;
                    }
                } else {
                    log::error!("ptr not writeable");
                    -1
                }
            } else {
                log::error!("unsupported fd: {fd}");
                -1
            }
        }
```

- [ ] **Step 8: Verify it compiles**

```bash
cd tg-rcore-tutorial-ch5-pingpong && TG_SKIP_USER_APPS=1 cargo check
```

Expected: compiles successfully.

- [ ] **Step 9: Commit**

```bash
git add tg-rcore-tutorial-ch5-pingpong/src/main.rs
git commit -m "feat(ch5-pingpong): add VirtIO init, MMIO mapping, FB/keyboard/SHM syscalls"
```

---

### Task 6: User crate — feature, syscall wrappers, cases.toml

**Files:**
- Modify: `tg-rcore-tutorial-user/Cargo.toml`
- Modify: `tg-rcore-tutorial-user/src/lib.rs`
- Modify: `tg-rcore-tutorial-user/cases.toml`

- [ ] **Step 1: Add pingpong feature to Cargo.toml**

In `tg-rcore-tutorial-user/Cargo.toml`, in the `[features]` section, add:

```toml
pingpong = []
```

- [ ] **Step 2: Add shm_create() wrapper and pingpong module to lib.rs**

In `tg-rcore-tutorial-user/src/lib.rs`, after the existing `tetris` module declaration, add:

```rust
#[cfg(feature = "pingpong")]
pub mod pingpong;
```

Also add the `shm_create()` syscall wrapper function (after `fb_write`):

```rust
/// Request a shared memory page from the kernel.
/// Returns the virtual address of the shared page, or usize::MAX on failure.
pub fn shm_create() -> usize {
    let ret: usize;
    unsafe {
        core::arch::asm!(
            "ecall",
            in("a7") 2002usize,
            lateout("a0") ret,
        );
    }
    ret
}
```

- [ ] **Step 3: Add ch5_pingpong section to cases.toml**

Append to `tg-rcore-tutorial-user/cases.toml`:

```toml
[ch5_pingpong]
cases = [
    "00hello_world",
    "01store_fault",
    "02power",
    "03priv_inst",
    "04priv_csr",
    "05write_a",
    "06write_b",
    "07write_c",
    "08power_3",
    "09power_5",
    "10power_7",
    "12forktest",
    "13forktree",
    "14forktest2",
    "15matrix",
    "fork_exit",
    "forktest_simple",
    "sbrk",
    "ch5b_usertest",
    "user_shell",
    "initproc",
    "pingpong",
]
```

- [ ] **Step 4: Commit**

```bash
git add tg-rcore-tutorial-user/Cargo.toml tg-rcore-tutorial-user/src/lib.rs tg-rcore-tutorial-user/cases.toml
git commit -m "feat(user): add pingpong feature, shm_create wrapper, cases.toml entry"
```

---

### Task 7: Create Pong game module and binary

**Files:**
- Create: `tg-rcore-tutorial-user/src/pingpong.rs`
- Create: `tg-rcore-tutorial-user/src/bin/pingpong.rs`

- [ ] **Step 1: Create the pingpong binary**

Create `tg-rcore-tutorial-user/src/bin/pingpong.rs`:

```rust
#![no_std]
#![no_main]

#[unsafe(no_mangle)]
pub extern "C" fn main() -> i32 {
    #[cfg(feature = "pingpong")]
    {
        tg_rcore_tutorial_user::pingpong::run();
    }
    0
}
```

- [ ] **Step 2: Create the pingpong game module**

Create `tg-rcore-tutorial-user/src/pingpong.rs` with the full game implementation. This is the largest single file.

```rust
//! Two-player Pong game using shared memory IPC between two processes.
//!
//! Parent process: player 1 (W/S keys), runs ball physics and rendering.
//! Child process: player 2 (Up/Down keys), writes input to shared memory.

use crate::{fb_info, fb_write, shm_create};

// ─── Constants ───

const PLAY_LEFT: i32 = 90;
const PLAY_RIGHT: i32 = 1190;
const PLAY_TOP: i32 = 50;
const PLAY_BOTTOM: i32 = 750;
const PLAY_WIDTH: i32 = PLAY_RIGHT - PLAY_LEFT;
const PLAY_HEIGHT: i32 = PLAY_BOTTOM - PLAY_TOP;

const PADDLE_WIDTH: i32 = 12;
const PADDLE_HEIGHT: i32 = 100;
const PADDLE1_X: i32 = 100;
const PADDLE2_X: i32 = 1168;
const PADDLE_SPEED: i32 = 6;

const BALL_SIZE: i32 = 12;
const BALL_INIT_SPEED: i32 = 4 * 256; // fixed-point x256
const BALL_SPEED_CAP: i32 = 10 * 256;

const FP_SHIFT: i32 = 8; // multiply by 256
const FP_ONE: i32 = 1 << FP_SHIFT;

const WIN_SCORE: u32 = 5;

// Colors (BGRA)
const BG_COLOR: [u8; 4] = [0x1A, 0x0D, 0x0D, 0xFF]; // #0D0D1A
const WALL_COLOR: [u8; 4] = [0x4E, 0x3E, 0x2E, 0xFF]; // dark brown-gray
const PADDLE_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white
const BALL_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white
const CENTER_LINE_COLOR: [u8; 4] = [0x40, 0x40, 0x40, 0xFF]; // dark gray
const SCORE_COLOR_P1: [u8; 4] = [0x44, 0xCC, 0x44, 0xFF]; // green
const SCORE_COLOR_P2: [u8; 4] = [0x44, 0x44, 0xCC, 0xFF]; // blue (red in BGRA=blue channel)
const MSG_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF]; // white

// Keycodes (VirtIO keyboard scancodes)
const KEY_W: u8 = 17;
const KEY_S: u8 = 31;
const KEY_UP: u8 = 103;
const KEY_DOWN: u8 = 108;
const KEY_SPACE: u8 = 57;
const KEY_ENTER: u8 = 28;

// Game states
const STATE_WAITING: u32 = 0;
const STATE_PLAYING: u32 = 1;
const STATE_POINT_SCORED: u32 = 2;
const STATE_GAME_OVER: u32 = 3;

// ─── Shared memory layout ───

#[repr(C)]
struct SharedState {
    ball_x: i32,
    ball_y: i32,
    ball_vx: i32,
    ball_vy: i32,
    paddle1_y: i32,
    paddle2_y: i32,
    score1: u32,
    score2: u32,
    tick: u32,
    game_state: u32,
    p2_up: u8,
    p2_down: u8,
}

// ─── Drawing helpers ───

/// Fill a rectangle with a solid color via fb_write.
/// Uses a stack buffer — max 32x32 pixels per call, tiles larger rects.
fn fill_rect(x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    if w <= 0 || h <= 0 {
        return;
    }
    const TILE: i32 = 32;
    // Buffer for one tile row: 32 * 32 * 4 = 4096 bytes (fits in stack)
    let mut buf = [0u8; (TILE * TILE * 4) as usize];

    let mut ty = 0;
    while ty < h {
        let th = if h - ty > TILE { TILE } else { h - ty };
        let mut tx = 0;
        while tx < w {
            let tw = if w - tx > TILE { TILE } else { w - tx };
            // Fill the tile buffer
            let pixels = (tw * th) as usize;
            for i in 0..pixels {
                let off = i * 4;
                buf[off] = color[0];
                buf[off + 1] = color[1];
                buf[off + 2] = color[2];
                buf[off + 3] = color[3];
            }
            fb_write(
                (x + tx) as u32,
                (y + ty) as u32,
                tw as u32,
                th as u32,
                buf.as_ptr(),
            );
            tx += TILE;
        }
        ty += TILE;
    }
}

// ─── 7-segment digit rendering ───

// Each digit is 20x30 pixels, segments are 4px thick
// Segments: top, top-right, bottom-right, bottom, bottom-left, top-left, middle
const DIGIT_W: i32 = 20;
const DIGIT_H: i32 = 30;
const SEG_T: i32 = 4;

// 7 segments encoded as bits: 0=top,1=top-right,2=bot-right,3=bottom,4=bot-left,5=top-left,6=middle
const DIGIT_SEGS: [u8; 10] = [
    0b0111111, // 0
    0b0000110, // 1
    0b1011011, // 2
    0b1001111, // 3
    0b1100110, // 4
    0b1101101, // 5
    0b1111101, // 6
    0b0000111, // 7
    0b1111111, // 8
    0b1101111, // 9
];

fn draw_digit(x: i32, y: i32, digit: u32, color: [u8; 4]) {
    if digit > 9 {
        return;
    }
    let segs = DIGIT_SEGS[digit as usize];
    let w = DIGIT_W;
    let h = DIGIT_H;
    let half = h / 2;
    // top
    if segs & (1 << 0) != 0 {
        fill_rect(x, y, w, SEG_T, color);
    }
    // top-right
    if segs & (1 << 1) != 0 {
        fill_rect(x + w - SEG_T, y, SEG_T, half, color);
    }
    // bottom-right
    if segs & (1 << 2) != 0 {
        fill_rect(x + w - SEG_T, y + half, SEG_T, half, color);
    }
    // bottom
    if segs & (1 << 3) != 0 {
        fill_rect(x, y + h - SEG_T, w, SEG_T, color);
    }
    // bottom-left
    if segs & (1 << 4) != 0 {
        fill_rect(x, y + half, SEG_T, half, color);
    }
    // top-left
    if segs & (1 << 5) != 0 {
        fill_rect(x, y, SEG_T, half, color);
    }
    // middle
    if segs & (1 << 6) != 0 {
        fill_rect(x, y + half - SEG_T / 2, w, SEG_T, color);
    }
}

fn draw_score(score1: u32, score2: u32) {
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let sy = PLAY_TOP + 10;

    // Erase old score area
    fill_rect(cx - 60, sy, 120, DIGIT_H + 4, BG_COLOR);

    // P1 score (left of center)
    draw_digit(cx - 50, sy, score1, SCORE_COLOR_P1);
    // Colon
    fill_rect(cx - 4, sy + 8, 8, 4, MSG_COLOR);
    fill_rect(cx - 4, sy + 18, 8, 4, MSG_COLOR);
    // P2 score (right of center)
    draw_digit(cx + 30, sy, score2, SCORE_COLOR_P2);
}

// ─── Game initialization and rendering ───

fn draw_background() {
    // Fill entire play area background
    fill_rect(PLAY_LEFT, PLAY_TOP, PLAY_WIDTH, PLAY_HEIGHT, BG_COLOR);

    // Top and bottom walls
    fill_rect(PLAY_LEFT, PLAY_TOP, PLAY_WIDTH, 4, WALL_COLOR);
    fill_rect(PLAY_LEFT, PLAY_BOTTOM - 4, PLAY_WIDTH, 4, WALL_COLOR);

    // Left and right boundaries (thin)
    fill_rect(PLAY_LEFT, PLAY_TOP, 2, PLAY_HEIGHT, WALL_COLOR);
    fill_rect(PLAY_RIGHT - 2, PLAY_TOP, 2, PLAY_HEIGHT, WALL_COLOR);

    // Center dashed line
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2 - 1;
    let mut y = PLAY_TOP + 10;
    while y < PLAY_BOTTOM - 10 {
        fill_rect(cx, y, 2, 10, CENTER_LINE_COLOR);
        y += 20;
    }
}

fn draw_paddle(x: i32, y: i32) {
    fill_rect(x, y, PADDLE_WIDTH, PADDLE_HEIGHT, PADDLE_COLOR);
}

fn erase_paddle(x: i32, y: i32) {
    fill_rect(x, y, PADDLE_WIDTH, PADDLE_HEIGHT, BG_COLOR);
}

fn draw_ball(x: i32, y: i32) {
    fill_rect(x, y, BALL_SIZE, BALL_SIZE, BALL_COLOR);
}

fn erase_ball(x: i32, y: i32) {
    fill_rect(x, y, BALL_SIZE, BALL_SIZE, BG_COLOR);
}

fn draw_message(msg: &[u8]) {
    // Simple text rendering: each char as a 6x8 filled block at center
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let cy = (PLAY_TOP + PLAY_BOTTOM) / 2;
    let total_w = msg.len() as i32 * 8;
    let sx = cx - total_w / 2;

    // Erase message area
    fill_rect(sx - 4, cy - 4, total_w + 8, 16, BG_COLOR);

    // Draw each character as a small filled block (very simple)
    for (i, &_ch) in msg.iter().enumerate() {
        fill_rect(sx + i as i32 * 8, cy, 6, 8, MSG_COLOR);
    }
}

fn erase_message_area() {
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let cy = (PLAY_TOP + PLAY_BOTTOM) / 2;
    fill_rect(cx - 120, cy - 4, 240, 16, BG_COLOR);
}

// ─── PRNG ───

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn range(&mut self, min: i32, max: i32) -> i32 {
        let r = self.next() as i32;
        let range = max - min;
        if range <= 0 {
            return min;
        }
        min + ((r as u32) % (range as u32)) as i32
    }
}

// ─── Ball reset ───

fn reset_ball(state: &mut SharedState, rng: &mut Rng) {
    let center_x = ((PLAY_LEFT + PLAY_RIGHT) / 2) * FP_ONE;
    let center_y = ((PLAY_TOP + PLAY_BOTTOM) / 2) * FP_ONE;
    state.ball_x = center_x;
    state.ball_y = center_y;

    // Random direction
    let dir_x = if rng.next() & 1 == 0 { 1 } else { -1 };
    let vy = rng.range(-2 * FP_ONE, 2 * FP_ONE);
    state.ball_vx = BALL_INIT_SPEED * dir_x;
    state.ball_vy = vy;
}

fn init_game(state: &mut SharedState, rng: &mut Rng) {
    let center_y = (PLAY_TOP + PLAY_BOTTOM) / 2 - PADDLE_HEIGHT / 2;
    state.paddle1_y = center_y;
    state.paddle2_y = center_y;
    state.score1 = 0;
    state.score2 = 0;
    state.tick = 0;
    state.game_state = STATE_WAITING;
    state.p2_up = 0;
    state.p2_down = 0;
    reset_ball(state, rng);
}

// ─── Input reading ───

fn poll_key() -> u8 {
    let mut buf = [0u8; 1];
    let ret = crate::read(crate::STDIN, &mut buf);
    if ret > 0 {
        buf[0]
    } else {
        0
    }
}

// ─── Physics ───

fn clamp(val: i32, min: i32, max: i32) -> i32 {
    if val < min {
        min
    } else if val > max {
        max
    } else {
        val
    }
}

fn update_physics(state: &mut SharedState, rng: &mut Rng) {
    // Move ball
    state.ball_x += state.ball_vx;
    state.ball_y += state.ball_vy;

    let bx = state.ball_x / FP_ONE;
    let by = state.ball_y / FP_ONE;

    // Wall bounce (top/bottom)
    if by <= PLAY_TOP + 4 {
        state.ball_y = (PLAY_TOP + 5) * FP_ONE;
        state.ball_vy = state.ball_vy.abs();
    }
    if by + BALL_SIZE >= PLAY_BOTTOM - 4 {
        state.ball_y = (PLAY_BOTTOM - 5 - BALL_SIZE) * FP_ONE;
        state.ball_vy = -(state.ball_vy.abs());
    }

    // Paddle 1 collision (left paddle)
    if state.ball_vx < 0
        && bx <= PADDLE1_X + PADDLE_WIDTH
        && bx + BALL_SIZE >= PADDLE1_X
        && by + BALL_SIZE >= state.paddle1_y
        && by <= state.paddle1_y + PADDLE_HEIGHT
    {
        state.ball_x = (PADDLE1_X + PADDLE_WIDTH + 1) * FP_ONE;
        state.ball_vx = -(state.ball_vx);
        // Adjust vy based on where ball hit paddle
        let hit_pos = (by + BALL_SIZE / 2) - state.paddle1_y;
        let offset = hit_pos - PADDLE_HEIGHT / 2;
        state.ball_vy = offset * 4; // scale to reasonable vy
        // Speed up slightly
        if state.ball_vx.abs() < BALL_SPEED_CAP {
            state.ball_vx = state.ball_vx + state.ball_vx.signum() * 32;
        }
    }

    // Paddle 2 collision (right paddle)
    if state.ball_vx > 0
        && bx + BALL_SIZE >= PADDLE2_X
        && bx <= PADDLE2_X + PADDLE_WIDTH
        && by + BALL_SIZE >= state.paddle2_y
        && by <= state.paddle2_y + PADDLE_HEIGHT
    {
        state.ball_x = (PADDLE2_X - BALL_SIZE - 1) * FP_ONE;
        state.ball_vx = -(state.ball_vx);
        let hit_pos = (by + BALL_SIZE / 2) - state.paddle2_y;
        let offset = hit_pos - PADDLE_HEIGHT / 2;
        state.ball_vy = offset * 4;
        if state.ball_vx.abs() < BALL_SPEED_CAP {
            state.ball_vx = state.ball_vx - state.ball_vx.signum() * 32;
        }
    }

    // Scoring — ball passed left or right edge
    let bx = state.ball_x / FP_ONE;
    if bx < PLAY_LEFT {
        state.score2 += 1;
        if state.score2 >= WIN_SCORE {
            state.game_state = STATE_GAME_OVER;
        } else {
            state.game_state = STATE_POINT_SCORED;
        }
        reset_ball(state, rng);
    } else if bx + BALL_SIZE > PLAY_RIGHT {
        state.score1 += 1;
        if state.score1 >= WIN_SCORE {
            state.game_state = STATE_GAME_OVER;
        } else {
            state.game_state = STATE_POINT_SCORED;
        }
        reset_ball(state, rng);
    }
}

// ─── Main entry points ───

/// Run the Pong game. Called from the pingpong binary.
pub fn run() {
    let (width, height) = fb_info();
    crate::println!(
        "[pingpong] PID={}, framebuffer {}x{}",
        crate::getpid(),
        width,
        height
    );

    // Create shared memory
    let shm_addr = shm_create();
    if shm_addr == usize::MAX {
        crate::println!("[pingpong] ERROR: shm_create failed");
        return;
    }
    crate::println!("[pingpong] shared memory at {:#x}", shm_addr);

    let state = shm_addr as *mut SharedState;
    let state = unsafe { &mut *state };

    // Seed PRNG from system time
    let seed = crate::get_time() as u64;
    let mut rng = Rng::new(seed);

    // Initialize game state
    init_game(state, &mut rng);

    // Fork: parent = player 1 + renderer, child = player 2
    let pid = crate::fork();
    if pid < 0 {
        crate::println!("[pingpong] ERROR: fork failed");
        return;
    }

    if pid == 0 {
        // ─── Child process: Player 2 ───
        child_loop(state);
    } else {
        // ─── Parent process: Player 1 + physics + rendering ───
        crate::println!("[pingpong] parent PID={}, child PID={}", crate::getpid(), pid);
        parent_loop(state, &mut rng);

        // Wait for child to exit
        let mut exit_code: i32 = 0;
        crate::waitpid(pid as usize, &mut exit_code);
    }
}

fn child_loop(state: &mut SharedState) -> ! {
    crate::println!("[pingpong] child PID={} (player 2)", crate::getpid());
    loop {
        let key = poll_key();
        match key {
            KEY_UP => {
                state.p2_up = 1;
            }
            KEY_DOWN => {
                state.p2_down = 1;
            }
            _ => {}
        }

        // Exit if game is over and parent signals (tick wraps to special value)
        // For simplicity, child runs until parent kills it via exit
        if state.game_state == STATE_GAME_OVER && state.tick == u32::MAX {
            crate::exit(0);
            unreachable!();
        }

        crate::sched_yield();
    }
}

fn parent_loop(state: &mut SharedState, rng: &mut Rng) {
    // Draw initial background
    draw_background();
    draw_score(state.score1, state.score2);
    draw_paddle(PADDLE1_X, state.paddle1_y);
    draw_paddle(PADDLE2_X, state.paddle2_y);

    // Draw "PRESS SPACE" message
    draw_message(b"PRESS SPACE TO START");

    // Track previous positions for incremental rendering
    let mut prev_ball_x = state.ball_x / FP_ONE;
    let mut prev_ball_y = state.ball_y / FP_ONE;
    let mut prev_paddle1_y = state.paddle1_y;
    let mut prev_paddle2_y = state.paddle2_y;
    let mut prev_score1 = state.score1;
    let mut prev_score2 = state.score2;

    loop {
        // Read player 1 input
        let key = poll_key();

        match state.game_state {
            STATE_WAITING => {
                if key == KEY_SPACE {
                    state.game_state = STATE_PLAYING;
                    erase_message_area();
                }
            }
            STATE_PLAYING => {
                // Player 1 paddle movement
                match key {
                    KEY_W => {
                        state.paddle1_y -= PADDLE_SPEED;
                    }
                    KEY_S => {
                        state.paddle1_y += PADDLE_SPEED;
                    }
                    _ => {}
                }
                state.paddle1_y = clamp(
                    state.paddle1_y,
                    PLAY_TOP + 4,
                    PLAY_BOTTOM - 4 - PADDLE_HEIGHT,
                );

                // Read player 2 input from shared memory
                if state.p2_up != 0 {
                    state.paddle2_y -= PADDLE_SPEED;
                    state.p2_up = 0;
                }
                if state.p2_down != 0 {
                    state.paddle2_y += PADDLE_SPEED;
                    state.p2_down = 0;
                }
                state.paddle2_y = clamp(
                    state.paddle2_y,
                    PLAY_TOP + 4,
                    PLAY_BOTTOM - 4 - PADDLE_HEIGHT,
                );

                // Physics
                update_physics(state, rng);
            }
            STATE_POINT_SCORED => {
                if key == KEY_SPACE {
                    state.game_state = STATE_PLAYING;
                    erase_message_area();
                } else {
                    draw_message(b"POINT! PRESS SPACE");
                }
            }
            STATE_GAME_OVER => {
                if state.score1 >= WIN_SCORE {
                    draw_message(b"PLAYER 1 WINS! ENTER");
                } else {
                    draw_message(b"PLAYER 2 WINS! ENTER");
                }
                if key == KEY_ENTER {
                    erase_message_area();
                    init_game(state, rng);
                    draw_background();
                    prev_score1 = 0;
                    prev_score2 = 0;
                }
            }
            _ => {}
        }

        // ─── Rendering (incremental) ───

        // Ball
        let ball_x = state.ball_x / FP_ONE;
        let ball_y = state.ball_y / FP_ONE;
        if ball_x != prev_ball_x || ball_y != prev_ball_y {
            erase_ball(prev_ball_x, prev_ball_y);
            draw_ball(ball_x, ball_y);
            prev_ball_x = ball_x;
            prev_ball_y = ball_y;
        }

        // Paddle 1
        if state.paddle1_y != prev_paddle1_y {
            erase_paddle(PADDLE1_X, prev_paddle1_y);
            draw_paddle(PADDLE1_X, state.paddle1_y);
            prev_paddle1_y = state.paddle1_y;
        }

        // Paddle 2
        if state.paddle2_y != prev_paddle2_y {
            erase_paddle(PADDLE2_X, prev_paddle2_y);
            draw_paddle(PADDLE2_X, state.paddle2_y);
            prev_paddle2_y = state.paddle2_y;
        }

        // Score
        if state.score1 != prev_score1 || state.score2 != prev_score2 {
            draw_score(state.score1, state.score2);
            prev_score1 = state.score1;
            prev_score2 = state.score2;
        }

        state.tick = state.tick.wrapping_add(1);
        crate::sched_yield();
    }
}
```

- [ ] **Step 3: Verify user crate compiles**

```bash
cd tg-rcore-tutorial-user && cargo check --features pingpong --target riscv64gc-unknown-none-elf
```

Expected: compiles (no kernel to link against, but types check out).

- [ ] **Step 4: Commit**

```bash
git add tg-rcore-tutorial-user/src/pingpong.rs tg-rcore-tutorial-user/src/bin/pingpong.rs
git commit -m "feat(user): add two-player Pong game with shared memory IPC"
```

---

### Task 8: Update build.rs and test.sh

**Files:**
- Modify: `tg-rcore-tutorial-ch5-pingpong/build.rs`
- Modify: `tg-rcore-tutorial-ch5-pingpong/test.sh`

- [ ] **Step 1: Update build.rs**

In `tg-rcore-tutorial-ch5-pingpong/build.rs`, change the `case_key` selection to use `ch5_pingpong`:

Find the section that selects the case_key (around the build_apps function). In ch5's build.rs, look for where it sets the case key. The ch5 build.rs has this pattern:

```rust
    let case_key = if env::var("CARGO_FEATURE_EXERCISE").is_ok() {
        "ch5_exercise"
    } else {
        "ch5"
    };
```

Change it to:

```rust
    let case_key = if env::var("CARGO_FEATURE_EXERCISE").is_ok() {
        "ch5_exercise"
    } else {
        "ch5_pingpong"
    };
```

Also find the `build_user_app` function and add `--features pingpong` to the cargo build command. Find the line where features are passed and add:

```rust
    cmd.args(["--features", "pingpong"]);
```

(Same pattern as ch4-tetris's `cmd.args(["--features", "tetris"]);`)

Also add `cargo:rerun-if-env-changed=TG_SKIP_USER_APPS` if not already present.

- [ ] **Step 2: Update test.sh**

Replace `tg-rcore-tutorial-ch5-pingpong/test.sh` with a headless test script (same pattern as ch4-tetris test.sh):

```bash
#!/usr/bin/env bash
set -euo pipefail

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

if ! command -v tg-rcore-tutorial-checker &> /dev/null; then
    echo -e "${YELLOW}Installing tg-rcore-tutorial-checker...${NC}"
    cargo install tg-rcore-tutorial-checker
fi

echo -e "${YELLOW}Building ch5-pingpong kernel...${NC}"
cargo build

KERNEL="target/riscv64gc-unknown-none-elf/debug/jsph-tg-rcore-tutorial-ch5-pingpong"

echo -e "${YELLOW}Running ch5-pingpong tests (90s timeout)...${NC}"
if timeout 90 qemu-system-riscv64 \
    -machine virt \
    -serial stdio \
    -bios none \
    -device virtio-gpu-device \
    -device virtio-keyboard-device \
    -display none \
    -kernel "$KERNEL" \
    2>&1 | tee /dev/stderr | tg-rcore-tutorial-checker --ch 5; then
    echo -e "${GREEN}PASS${NC}"
else
    echo -e "${RED}FAIL${NC}"
    exit 1
fi
```

- [ ] **Step 3: Verify full kernel build**

```bash
cd tg-rcore-tutorial-ch5-pingpong && cargo build
```

Expected: full build including user programs compiles successfully.

- [ ] **Step 4: Commit**

```bash
git add tg-rcore-tutorial-ch5-pingpong/build.rs tg-rcore-tutorial-ch5-pingpong/test.sh
git commit -m "build(ch5-pingpong): update build.rs and test.sh for pingpong"
```

---

### Task 9: Integration testing and fixes

- [ ] **Step 1: Run the kernel**

```bash
cd tg-rcore-tutorial-ch5-pingpong && cargo run
```

Open VNC viewer to `localhost:5900` to see the game.

Expected: QEMU starts, existing ch5 test programs print output, shell runs, and when `pingpong` is exec'd by initproc/shell, it shows the Pong game on VNC.

- [ ] **Step 2: Test the checker**

```bash
cd tg-rcore-tutorial-ch5-pingpong && bash test.sh
```

Expected: ch5 checker passes (existing test programs produce correct output).

- [ ] **Step 3: Verify user crate publishability**

```bash
cd tg-rcore-tutorial-user && cargo publish --dry-run
```

Expected: passes (pingpong module gated behind feature, binary body gated).

- [ ] **Step 4: Fix any issues discovered**

Address compilation errors, runtime panics, or rendering glitches.

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "feat(ch5-pingpong): complete VirtIO GPU Pong game with shared memory IPC"
```

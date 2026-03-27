# Ch1 Tangram VirtIO-GPU Display — Design Spec

## Goal

Extend `tg-rcore-tutorial-ch1` to display a tangram "OS" pattern on screen using a VirtIO-GPU framebuffer. Serial "Hello, world!" output must still work. Clean shutdown after displaying.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch1-tangram.png

## Constraints

- Only modify files under `tg-rcore-tutorial-ch1/`
- Ch1 has no page tables (identity mapping), no heap allocator, and a 4KiB stack
- Do not modify `tg-rcore-tutorial-sbi` or other shared crates
- `cargo check` and `cargo publish --dry-run` must still pass

## Architecture

All new code lives in `tg-rcore-tutorial-ch1/src/main.rs` (plus a `src/tangram.rs` for piece geometry). The crate gains one new dependency: `virtio-drivers = "0.1.0"`, target-gated to riscv64 only.

### Execution Flow

```
rust_main()
  1. Print "Hello, world!\n" via SBI console_putchar (existing)
  2. Initialize VirtIOGpu via MmioTransport at 0x10001000
  3. Call setup_framebuffer() → extract raw pointer + length, release borrow
  4. Reconstruct framebuffer slice from raw pointer (unsafe, see §Borrow Workaround)
  5. Fill entire buffer white (background)
  6. For each of 14 tangram pieces: rasterize polygon into buffer
  7. Call gpu.flush() to push framebuffer to display
  8. Spin-wait ~3 seconds via SBI set_timer / rdtime so the image is visible
  9. Call shutdown(false)
```

### Components

#### 1. Static Bump Allocator

A minimal `#[global_allocator]` backed by a static `[u8; N]` array in BSS. Gated behind `#[cfg(target_arch = "riscv64")]`.

- **Pool size**: 5 MiB — enough for 1024x768x4 framebuffer (~3 MiB) + VirtQueue pages + command buffers + headroom
- **Implementation**: Atomic pointer bump with alignment support
- **Never frees**: `dealloc` is a no-op; `dma_dealloc` returns 0
- **Thread safety**: Single-core, but uses `AtomicUsize` for the bump pointer to satisfy Rust's `GlobalAlloc` trait requirements
- **Shared by both paths**: `GlobalAlloc::alloc` and `Hal::dma_alloc` both bump from the same pool. `dma_alloc` enforces page alignment (4KiB); `GlobalAlloc::alloc` respects the requested `Layout` alignment.

```rust
struct BumpAllocator {
    pool: UnsafeCell<[u8; POOL_SIZE]>,
    next: AtomicUsize,
}

unsafe impl GlobalAlloc for BumpAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Bump pointer, align up to layout.align(), bounds check
    }
    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}
```

**Correctness invariant — identity mapping**: The pool is a static in BSS. Since ch1 runs without page tables (M-mode configures PMP for full access, S-mode uses physical addresses directly), the virtual address of any byte in the pool IS its physical address. This means pointers returned by `dma_alloc` are valid DMA addresses that the VirtIO device can access.

#### 2. Hal Trait Implementation

Adapts ch6's `VirtioHal` for ch1's identity-mapped, no-paging environment.

```rust
struct HalImpl;

impl Hal for HalImpl {
    fn dma_alloc(pages: usize) -> PhysAddr {
        // Allocate pages * 4096 bytes from bump allocator, page-aligned
        // Returns the physical address (= virtual address under identity map)
    }
    fn dma_dealloc(_paddr: PhysAddr, _pages: usize) -> i32 { 0 }
    fn phys_to_virt(paddr: PhysAddr) -> VirtAddr { paddr }
    fn virt_to_phys(vaddr: VirtAddr) -> PhysAddr { vaddr }
}
```

#### 3. VirtIO-GPU Initialization

```rust
fn init_gpu() -> VirtIOGpu<'static, HalImpl, MmioTransport> {
    let header = NonNull::new(VIRTIO0 as *mut VirtIOHeader).unwrap();
    let transport = MmioTransport::new(header).expect("mmio transport");
    VirtIOGpu::new(transport).expect("gpu init")
}
```

- MMIO address: `0x10001000` (QEMU virt `virtio-mmio-bus.0`)
- The GPU device is attached here via QEMU's `-device virtio-gpu-device`

#### 4. Framebuffer Borrow Workaround

`setup_framebuffer(&mut self) -> Result<&mut [u8]>` returns a slice whose lifetime is tied to `&mut self` by Rust's elision rules, even though the underlying DMA buffer is `'static`. This prevents calling `flush(&mut self)` while holding the framebuffer.

**Solution**: Extract the raw pointer immediately, release the mutable borrow, then reconstruct:

```rust
let (fb_ptr, fb_len) = {
    let buf = gpu.setup_framebuffer().expect("framebuffer");
    (buf.as_mut_ptr(), buf.len())
};
// Borrow on gpu is now released
let fb = unsafe { core::slice::from_raw_parts_mut(fb_ptr, fb_len) };
// Draw into fb...
gpu.flush().expect("flush");
```

This is safe because: (a) the DMA buffer is owned by the GPU struct and lives as long as the struct does, (b) we maintain exclusive access (single-threaded), (c) the GPU struct is not dropped until after shutdown.

#### 5. Tangram Renderer

14 tangram pieces (7 for "O", 7 for "S") defined as convex polygons with integer vertex coordinates.

**Color palette** (BGRA format, 7 distinct colors reused for both letters):
| Piece | Color | BGRA value |
|-------|-------|------------|
| 1 | Red | `(0x00, 0x00, 0xE0, 0xFF)` |
| 2 | Orange | `(0x00, 0x80, 0xFF, 0xFF)` |
| 3 | Yellow | `(0x00, 0xE0, 0xFF, 0xFF)` |
| 4 | Green | `(0x00, 0xC0, 0x00, 0xFF)` |
| 5 | Cyan | `(0xFF, 0xE0, 0x00, 0xFF)` |
| 6 | Blue | `(0xFF, 0x40, 0x00, 0xFF)` |
| 7 | Magenta | `(0xC0, 0x00, 0xC0, 0xFF)` |

**Rasterization**: Simple scanline polygon fill — for each row in the polygon's bounding box, compute the X intersections with each edge, sort, fill between pairs. All integer math (no floats). Each polygon is convex (triangles, square, parallelogram) so at most 2 intersections per scanline.

**Tangram piece geometry**: Define each piece as `&[(i32, i32)]` vertex arrays in `src/tangram.rs`. Position both letters centered on screen. Exact coordinates will be determined during implementation to match the demo screenshot.

### Memory Budget

| Region | Size | Location |
|--------|------|----------|
| Bump allocator pool | 5 MiB | `.bss` (static) |
| Stack | 64 KiB | `.bss.uninit` (static) |
| M-mode (tg-sbi) | ~20 KiB | Below 0x80200000 |
| **Total** | ~5.1 MiB | Within QEMU's 128 MiB RAM |

### QEMU Configuration Changes

**`.cargo/config.toml`** — replace the runner:

```toml
runner = [
    "qemu-system-riscv64",
    "-machine", "virt",
    "-serial", "stdio",
    "-bios", "none",
    "-device", "virtio-gpu-device",
    "-kernel",
]
```

Changes from current config:
- Remove `-nographic` (need graphical window for GPU output)
- Add `-serial stdio` (keep serial console for "Hello, world!")
- Add `-device virtio-gpu-device` (attach VirtIO GPU on MMIO bus)

**Headless / CI compatibility**: Removing `-nographic` means QEMU requires a display. `test.sh` must be updated to invoke QEMU directly with `-display none -serial stdio` (no `-device virtio-gpu-device` needed) so serial-only tests still pass on headless CI. Alternatively, `test.sh` can set `QT_QPA_PLATFORM=offscreen` or use `xvfb-run`.

### Cargo.toml Changes

Target-gate the dependency so it is only compiled for riscv64:

```toml
[target.'cfg(target_arch = "riscv64")'.dependencies]
virtio-drivers = "0.1.0"
```

This prevents `extern crate alloc` from being pulled in on host builds, keeping `cargo check` and `cargo publish --dry-run` clean without needing a global allocator on x86_64.

### Stack Size Change

In `_start`, change `STACK_SIZE` from `4096` to `65536` (64 KiB). The VirtIOGpu struct and its internal VirtQueue buffers live on the stack during init.

### cargo check / publish --dry-run Compatibility

- `virtio-drivers` is target-gated to riscv64 only (not compiled on host)
- `#[global_allocator]` and all GPU code gated behind `#[cfg(target_arch = "riscv64")]`
- Existing `#[cfg(not(target_arch = "riscv64"))]` stub module unchanged
- `tangram.rs` contains only data (vertex arrays, colors) — no target-specific code, compiles on all targets

## Acceptance Criteria

1. `cargo run` opens a QEMU window showing tangram "OS" pattern (7 pieces per letter, distinct colors)
2. Serial terminal prints "Hello, world!"
3. Image remains visible for ~3 seconds, then QEMU shuts down cleanly
4. `cargo check` passes
5. `cargo publish --dry-run` passes
6. `test.sh` still passes on headless environments (serial-only check)

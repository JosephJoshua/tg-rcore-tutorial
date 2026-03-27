# Ch2 Moving Tangram — Design Spec

**Date:** 2026-03-27
**Status:** Draft

## Overview

Extend ch2 batch OS to animate tangram "OS" pieces appearing one-by-one via VirtIO-GPU. Each of the 14 tangram pieces is rendered by a separate user program. The kernel initializes the GPU and exposes framebuffer access through two custom syscalls. User programs rasterize their assigned piece into a heap-allocated buffer and push it to the kernel framebuffer.

## Architecture

### Kernel-Side (`jsph-tg-rcore-tutorial-ch2-moving-tangram/`)

**GPU initialization:** Reuse ch1-tangram's approach — static 5 MiB bump allocator for VirtIO DMA, `HalImpl` with identity mapping, MMIO slot probing from `0x10001000` to `0x10008000`. Initialize GPU and framebuffer in `rust_main()` before the batch loop. Fill background white.

**Storage:** GPU driver and framebuffer metadata stored as `static mut` globals (no kernel heap allocator).

**Syscall interception:** In `handle_syscall()`, check for custom IDs `2000`/`2001` before dispatching to `tg_syscall::handle()`. This avoids modifying the shared `tg-rcore-tutorial-syscall` crate.

**Files to create/modify:**

| File | Action | Source |
|------|--------|--------|
| `src/allocator.rs` | Create | Copy from `jsph-tg-rcore-tutorial-ch1-tangram/src/allocator.rs` |
| `src/gpu.rs` | Create | Copy from `jsph-tg-rcore-tutorial-ch1-tangram/src/gpu.rs` |
| `src/main.rs` | Modify | Add GPU init, FB_INFO/FB_WRITE handlers |
| `Cargo.toml` | Modify | New name/author, add `virtio-drivers` dep |
| `.cargo/config.toml` | Modify | Add `-device virtio-gpu-device`, replace `-nographic` with `-serial stdio` |
| `test.sh` | Modify | Add GPU device flag, headless CI with timeout |

### Syscall Interface

#### `FB_INFO` (ID 2000)

Returns framebuffer dimensions.

- **Parameters:** none
- **Returns:** `(width << 32) | height` packed in `a0` (RV64)
- **User wrapper:** `fb_info() -> (u32, u32)` — unpacks width and height

#### `FB_WRITE` (ID 2001)

Writes a rectangular BGRA pixel region to the framebuffer and flushes.

- **Parameters:**
  - `a0` = x offset (pixels)
  - `a1` = y offset (pixels)
  - `a2` = width (pixels)
  - `a3` = height (pixels)
  - `a4` = pointer to BGRA pixel data (`w * h * 4` bytes)
- **Returns:** 0 on success, -1 on error (out of bounds)
- **Kernel behavior:** Copy `w * h * 4` bytes from user pointer into framebuffer at `(x, y)`, then `gpu.flush()`

### User-Side (`tg-rcore-tutorial-user/`)

**Feature gate:** All tangram-specific code behind `tangram` Cargo feature.

**Tangram module** (`src/tangram.rs`, gated by `#[cfg(feature = "tangram")]`):
- `Color` struct (BGRA)
- `COLORS` array (7 colors from ch1-tangram)
- `PIECES` array (14 pieces with vertex coordinates and color indices)
- `Piece` struct with `vertices: &'static [(i32, i32)]` and `color_idx: usize`
- `fill_polygon(buf, buf_w, buf_h, vertices, color)` — scanline fill rasterizer, same algorithm as ch1-tangram but writes to a provided buffer instead of the kernel framebuffer
- `render_piece(piece_idx) -> Result<(), &'static str>` — convenience function that:
  1. Calls `fb_info()` to get screen dimensions
  2. Computes piece bounding box
  3. Heap-allocates a `Vec<u8>` for the bounding box region (w × h × 4 bytes)
  4. Fills transparent/white background
  5. Calls `fill_polygon()` with coordinates offset to local buffer origin
  6. Calls `fb_write(x, y, w, h, buf.as_ptr())` to push to kernel

**Syscall wrappers** (in `src/lib.rs`, NOT gated — general framebuffer syscalls):
- `pub fn fb_info() -> (u32, u32)` — calls syscall ID 2000, unpacks result
- `pub fn fb_write(x: u32, y: u32, w: u32, h: u32, data: *const u8) -> isize` — calls syscall ID 2001

**14 user programs** (`src/bin/tangram_00.rs` through `tangram_13.rs`):
- Each calls `render_piece(N)` where N is its piece index
- Sleeps 300ms after rendering for animation effect
- Prints piece name to serial console
- Returns 0

**cases.toml:** Append tangram programs after existing ch2 test cases.

**Cargo.toml:** Add `[features] tangram = []`.

### Kernel Build Integration

The kernel's `build.rs` passes `--features tangram` when building user programs so the tangram module and binaries compile. This is done by modifying the cargo build invocation in `build.rs`.

### QEMU Configuration

```
qemu-system-riscv64 \
  -machine virt \
  -serial stdio \
  -bios none \
  -device virtio-gpu-device \
  -kernel <elf>
```

Replaces `-nographic` with `-serial stdio` + `-device virtio-gpu-device`.

### Timing

Each tangram user program calls `sleep(300)` (300ms spin-wait via rdtime) after rendering. Total animation: ~4.2 seconds for 14 pieces.

## Constraints

- Do NOT modify `tg-rcore-tutorial-ch2/`, `tg-rcore-tutorial-sbi/`, or `tg-rcore-tutorial-syscall/`
- Existing ch2 test programs (`00hello_world`, `01store_fault`, etc.) must still work
- `cargo check` and `cargo publish --dry-run` must pass for both the kernel crate and user crate
- User programs load at `0x80400000` (step=0, sequential execution)
- No page tables — identity mapping for allocator

## Acceptance Criteria

1. `cargo run` opens QEMU window showing tangram pieces appearing one-by-one
2. Serial terminal shows kernel boot messages and each user program's output
3. Existing ch2 test programs still function correctly
4. Animation has visible timing (~300ms between pieces)
5. Clean shutdown after the last piece
6. `cargo check` and `cargo publish --dry-run` pass

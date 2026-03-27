Extend `tg-rcore-tutorial-ch2` to animate tangram "OS" pieces appearing one-by-one via VirtIO-GPU, using ch2's batch processing model where each user program renders one piece.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch2-moving-tangram.gif

## Setup

Before starting, copy the ch2 directory to create the working crate:

```bash
cp -r tg-rcore-tutorial-ch2 jsph-tg-rcore-tutorial-ch2-moving-tangram
rm -rf jsph-tg-rcore-tutorial-ch2-moving-tangram/{target,Cargo.lock,.gitrepo}
```

Then update `jsph-tg-rcore-tutorial-ch2-moving-tangram/Cargo.toml`:
- Change `name` to `"jsph-tg-rcore-tutorial-ch2-moving-tangram"`
- Change `authors` to `["Joseph Joshua Anggita <jj.anggita@gmail.com>"]`
- Change `description` to describe the moving tangram extension
- Change path dependencies to version-only dependencies (for crates.io publishing)

**All modifications go into `jsph-tg-rcore-tutorial-ch2-moving-tangram/`** — do NOT modify the original `tg-rcore-tutorial-ch2/`.

## Goal

Add VirtIO-GPU framebuffer support to the ch2 batch OS kernel. Create 14 user programs (one per tangram piece). The kernel initializes the GPU and sets up a shared framebuffer, then runs each user program in sequence via the existing batch loader. Each program rasterizes its assigned tangram piece in user-space and pushes pixel data to the kernel framebuffer via a new syscall, producing a progressive assembly animation — pieces appear one at a time until the full "OS" pattern is complete.

## Architecture Overview

Ch2 is a batch OS: the kernel loads embedded user binaries sequentially, running each to completion before starting the next. There is no virtual memory (identity mapping), no heap allocator in the kernel, and user programs run at a fixed base address (`0x80400000`).

The key design challenge: the VirtIO-GPU driver lives in the kernel (S-mode, with MMIO access), but the rendering requests come from user programs (U-mode). This requires a new syscall to bridge user-space rendering to the kernel framebuffer.

### Approach: Low-Level Framebuffer Syscalls

Expose two syscalls from the kernel:

1. **`FB_INFO`** — returns framebuffer dimensions (width, height) so user programs know the canvas size.

2. **`FB_WRITE`** — writes a rectangular region of BGRA pixel data from a user-space buffer into the kernel's framebuffer and flushes the display. Parameters: `x, y, width, height, pixel_data_ptr`. The kernel copies the pixel data from the user buffer into the framebuffer at the specified offset, then calls `gpu.flush()`.

Each of the 14 user programs contains its own piece geometry and color, rasterizes the polygon into a local pixel buffer using the scanline fill algorithm (in the user library), then calls `FB_WRITE` to push the rendered region to the screen.

This approach is more general than a per-piece kernel syscall — the kernel knows nothing about tangram pieces, only about writing rectangles to the framebuffer. The polygon rasterizer lives in user-space (in `tg-rcore-tutorial-user`).

### Kernel-Side

1. **GPU driver**: Reuse the ch1-tangram approach (static bump allocator, Hal trait with identity mapping, MMIO slot probing). Initialize the GPU and framebuffer in `rust_main` before the batch loop starts. Fill the background white.

2. **Syscall handler**: Add `FB_INFO` and `FB_WRITE` to the syscall dispatch. `FB_WRITE` copies user pixel data into the framebuffer slice and calls `gpu.flush()`.

### User-Side

1. **Tangram library**: Add polygon rasterizer and piece geometry to `tg-rcore-tutorial-user/src/` (as a module, not in each binary). Each user program calls a shared function like `render_piece(piece_index)` which rasterizes into a local buffer and invokes `FB_WRITE`.

2. **14 user programs** (`tangram_00.rs` through `tangram_13.rs`): Each prints its piece name, calls the render function with its piece index, sleeps briefly for animation timing, and exits.

3. **Timing**: Each user program adds a brief delay (200–500ms via `sleep()` or `rdtime` spin-wait) after rendering, so pieces appear progressively.

## Key Constraints

- All modifications go in `jsph-tg-rcore-tutorial-ch2-moving-tangram/` and `tg-rcore-tutorial-user/`
- Do not modify `tg-rcore-tutorial-sbi` or other shared crates
- The existing ch2 test programs (`00hello_world`, `01store_fault`, `02power`, etc.) must still work
- QEMU runner needs `-device virtio-gpu-device` and `-serial stdio` (replace `-nographic`)
- Ch2 has no page tables — identity mapping means the bump allocator pool's virtual address equals its physical address (same as ch1-tangram)
- User programs are all loaded to the same base address (`0x80400000`, `step=0` in `cases.toml`) and run one at a time
- `cargo check` and `cargo publish --dry-run` must still pass
- Reference `jsph-tg-rcore-tutorial-ch1-tangram/src/` for the allocator, GPU init, and polygon rasterizer code

## Files to Create/Modify

All paths relative to `jsph-tg-rcore-tutorial-ch2-moving-tangram/` unless noted.

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` | Modify | New name/author, add `virtio-drivers` dep |
| `.cargo/config.toml` | Modify | Add GPU device, replace `-nographic` |
| `src/allocator.rs` | Create | Static bump allocator + Hal trait (from ch1-tangram) |
| `src/gpu.rs` | Create | VirtIO-GPU init + framebuffer (from ch1-tangram) |
| `src/main.rs` | Modify | Init GPU before batch loop, add FB_INFO/FB_WRITE syscall handlers |
| `test.sh` | Modify | Headless CI with timeout + GPU device |
| `tg-rcore-tutorial-user/src/tangram.rs` | Create | Piece geometry, colors, polygon rasterizer (user-space) |
| `tg-rcore-tutorial-user/src/lib.rs` | Modify | Add tangram module, FB syscall wrappers |
| `tg-rcore-tutorial-user/src/bin/tangram_00.rs` ... `tangram_13.rs` | Create | 14 user programs, one per piece |
| `tg-rcore-tutorial-user/cases.toml` | Modify | Add tangram programs to ch2 section |

## Acceptance Criteria

1. `cargo run` opens a QEMU window showing tangram pieces appearing one-by-one, assembling into the "OS" pattern
2. Serial terminal shows kernel boot messages and each user program's output
3. Existing ch2 test programs still function correctly
4. Animation has visible timing (pieces don't all appear instantly)
5. Clean shutdown after the last piece (timeout or keypress)
6. `cargo check` and `cargo publish --dry-run` pass

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine the syscall interface, user program structure, and timing approach. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — create step-by-step implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task with review checkpoints
4. `superpowers:requesting-code-review` — final review

Do NOT publish the crate — the user will review and publish manually.

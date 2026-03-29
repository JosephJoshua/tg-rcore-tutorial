Extend `tg-rcore-tutorial-ch6` with a user-space Breakout (brick-breaker) game rendered via VirtIO-GPU, with VirtIO keyboard input from VNC. The game uses ch6's **filesystem** to save and restore game progress via hotkeys, demonstrating file I/O syscalls alongside VirtIO device drivers.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch6-breakout.gif

## Setup

Copy the ch6 directory to create the working crate:

```bash
cp -r tg-rcore-tutorial-ch6 tg-rcore-tutorial-ch6-breakout
rm -rf tg-rcore-tutorial-ch6-breakout/{target,Cargo.lock,.gitrepo}
```

Then update `tg-rcore-tutorial-ch6-breakout/Cargo.toml`:
- Change `name` to a unique crate name for publishing
- Update `authors` and `description`

**All kernel modifications go into `tg-rcore-tutorial-ch6-breakout/`** — do NOT modify the original `tg-rcore-tutorial-ch6/` or shared crates. User-side changes go in `tg-rcore-tutorial-user/`.

## Goal

Add VirtIO-GPU framebuffer rendering and VirtIO keyboard input to the ch6 filesystem OS kernel. Create a Breakout game that saves/loads game state to the easy-fs filesystem via `open`/`write`/`read`/`close` syscalls, triggered by hotkeys during gameplay.

## Architecture Overview — How ch6 differs from ch5

Ch6 adds a **disk-based filesystem** (easy-fs on VirtIO-blk). Programs are loaded from `fs.img` instead of being embedded in the kernel. Per-process file descriptor tables enable file I/O.

| Feature | Ch5 | Ch6 |
|---------|-----|-----|
| Program storage | Embedded in kernel (APP_ASM) | Disk image (fs.img) |
| Program loading | Name lookup in memory table | File open + read from disk |
| I/O model | SBI console only | FD table + file handles |
| Block device | None | VirtIO-blk driver |
| Per-process FD table | No | Yes (stdin/stdout/stderr + files) |
| New syscalls | None | open, close, read/write (with fd), link, unlink, fstat |

### Key ch6 mechanisms

- **VirtIO block device**: MMIO at 0x10001000, provides disk access for easy-fs. Ch6 already has a VirtIO-blk driver and Hal implementation in `virtio_block.rs`.
- **easy-fs**: Simple inode-based filesystem. Single-level directory (root only). 512-byte blocks. Files up to ~16 MB.
- **File descriptor table**: Per-process `Vec<Option<Mutex<FileHandle>>>`. FD 0=stdin, 1=stdout, 2=stderr. FD 3+ for opened files.
- **`open(path, flags)`**: Opens/creates a file, returns fd. Flags: `RDONLY`, `WRONLY`, `RDWR`, `CREATE`, `TRUNC`.
- **`read(fd, buf)`/`write(fd, buf)`**: Read/write file at current offset, offset advances automatically.
- **`close(fd)`**: Closes file handle.
- **MMIO mapping**: Ch6 already maps VirtIO MMIO ranges in `kernel_space()` for the block device. The GPU and keyboard devices share the same MMIO region.

### What needs to be added

**Kernel-side:**

1. **VirtIO-GPU driver** — Ch6 already has VirtIO-blk at MMIO slot 0x10001000. The GPU and keyboard are at different MMIO slots (0x10002000+). The existing MMIO scanning in ch6 finds the block device by type. Extend it (or add a separate scan) to also detect `DeviceType::GPU` and `DeviceType::Input`.

   **IMPORTANT**: Ch6 already has a `virtio_block.rs` with a `VirtioHal` implementation that uses `alloc::alloc::alloc_zeroed` (kernel heap, not a bump pool). Reuse this Hal for the GPU/keyboard. The GPU framebuffer (~4 MiB) will come from the kernel heap — ensure `MEMORY` is large enough.

   Ch6's MMIO const only covers one slot: `MMIO = &[(0x1000_1000, 0x1000)]`. The GPU and keyboard are at different MMIO addresses (e.g., 0x10007000, 0x10008000). Expand the MMIO mapping to cover the full VirtIO range `(0x1000_0000, 0x9000)` or add the GPU/keyboard addresses individually.

2. **FB_INFO and FB_WRITE syscalls** (IDs 2000/2001) — Same as ch4-tetris. FB_WRITE must translate user data pointer through the process's page table. Use ch6's `PROCESSOR.get_mut().current().unwrap()` to access the current process.

3. **Non-blocking IO::read for keyboard** — Ch6 already has `IO::read` for fd 0 (stdin via SBI). Extend it: when fd=0, first try VirtIO keyboard `pop_pending_event()`. If no VirtIO keyboard event, return 0 (non-blocking). Do NOT call `tg_sbi::console_getchar()` — it blocks.

   **Critical**: Ch6's existing `IO::read` implementation for STDIN uses `sbi::console_getchar()` which BLOCKS. This must be replaced with VirtIO keyboard polling for the game to work.

4. **QEMU config** — Add `-device virtio-gpu-device` and `-device virtio-keyboard-device` to the existing QEMU flags. Ch6 already has `-drive file=fs.img,...` and `-device virtio-blk-device,...`. Keep those and add the GPU/keyboard. Replace `-nographic` with `-serial stdio`.

5. **MEMORY increase** — Ch6 with filesystem overhead + GPU framebuffer needs more heap. Check the current `MEMORY` value and increase if needed (70 MiB worked for ch4-tetris).

**User-side:**

1. **Breakout game module** (`tg-rcore-tutorial-user/src/breakout.rs`, feature-gated with `breakout`) — Game logic, rendering, physics, save/load.

2. **One binary** (`breakout.rs`) — Gate the body with `#[cfg(feature = "breakout")]`.

3. **Save/Load via filesystem**:
   ```rust
   // Save: serialize game state to bytes, write to file
   let fd = open("breakout_save\0", OpenFlags::CREATE | OpenFlags::WRONLY | OpenFlags::TRUNC);
   write(fd as usize, &game_state_bytes);
   close(fd as usize);

   // Load: read bytes from file, deserialize
   let fd = open("breakout_save\0", OpenFlags::RDONLY);
   read(fd as usize, &mut game_state_bytes);
   close(fd as usize);
   ```

## VirtIO Keyboard — Keycode Translation

```
EV_KEY (event_type == 1), value == 1 (key press):
  Left arrow = 105, A = 30  -> move paddle left
  Right arrow = 106, D = 32 -> move paddle right
  Space = 57                -> launch ball / pause
  F5 = 63                   -> save game
  F9 = 67                   -> load game
  Enter = 28                -> restart after game over
```

## Breakout Game Design

### Field
- 1280x800 framebuffer
- Play area: ~1000x700 pixels, centered
- Top: brick grid
- Bottom: paddle + ball
- Walls on left/right/top (ball bounces). Bottom edge: ball falls through (lose life).

### Bricks
- Grid: 10 columns x 5 rows
- Brick size: ~90x25 pixels (with 2px gap)
- Each row a different color (classic rainbow: red, orange, yellow, green, cyan)
- Hit once to destroy. When hit: remove brick, score points, ball bounces.
- Row scoring: top rows worth more (50, 40, 30, 20, 10 per brick)

### Paddle
- Width: 120px, Height: 14px
- Positioned near bottom of play area
- Movement speed: 8px per frame
- Clamped to play area bounds

### Ball
- Size: 10x10 pixels
- **Fixed-point physics** (x256): all positions and velocities multiplied by 256 for sub-pixel precision. Integer-only math.
- Initial speed: ~3px/frame, launched upward at slight angle
- Ball angle changes based on where it hits the paddle (left edge = steep left, center = straight up, right edge = steep right)
- Speed increases slightly every N bricks destroyed

### Collision detection
- **Wall bounce**: ball hits left/right/top wall -> negate appropriate velocity component
- **Paddle bounce**: ball bottom edge touches paddle top AND ball_x within paddle range -> negate ball_vy, adjust ball_vx based on hit offset from paddle center
- **Brick collision**: check ball against each remaining brick. On hit: remove brick, bounce ball (negate the velocity component based on which face was hit), add score.
- **Ball lost**: ball_y passes bottom edge -> lose a life, reset ball on paddle

### Lives & Scoring
- 3 lives
- Score: per-brick based on row
- All bricks cleared: next level (reset bricks, increase ball speed, keep score)
- All lives lost: game over

### Save/Load
- **F5 saves**: serialize entire game state (brick grid, ball position/velocity, paddle position, score, lives, level) as a flat byte array. Write to `"breakout_save\0"` via open/write/close.
- **F9 loads**: read from `"breakout_save\0"` via open/read/close. Deserialize and restore game state. Redraw entire screen.
- **On-screen feedback**: brief "SAVED" or "LOADED" text flash after save/load.

### Save File Format
Simple fixed-size binary format (no serialization library needed):

```rust
#[repr(C)]
struct SaveData {
    magic: u32,           // 0x42524B4F ("BRKO") — validate on load
    ball_x: i32,          // fixed-point x256
    ball_y: i32,
    ball_vx: i32,
    ball_vy: i32,
    paddle_x: i32,
    score: u32,
    lives: u32,
    level: u32,
    bricks: [u8; 50],     // 10x5 grid, 0=destroyed, 1=alive
    ball_attached: u8,     // 1 if ball is on paddle (pre-launch)
}
```

Total: ~90 bytes. Well within easy-fs limits.

Serialize: cast `&SaveData` to `&[u8]` via `core::slice::from_raw_parts`. Deserialize: cast `&[u8]` back.

### Rendering
- **Incremental**: only redraw changed elements (destroyed brick -> background, ball erase/draw, paddle erase/draw)
- **Full redraw on load**: after loading a save, redraw the entire screen
- **Color scheme**: dark background, colorful bricks (row-based rainbow), white ball, bright paddle, neon score/lives display

## Lessons Learned (apply these)

1. **SBI console_getchar BLOCKS** — Replace ch6's STDIN read with VirtIO keyboard polling. This is the most critical change.

2. **customizable-buddy 0.0.2 user heap bug** — Keep 16 KiB heap when no game feature active. Breakout should avoid heap allocation.

3. **Ch6 already has VirtIO-blk and MMIO mapping** — Don't duplicate. Check `virtio_block.rs` and the existing MMIO mapping in `kernel_space()`. The GPU/keyboard are additional MMIO devices at different addresses.

4. **Ch6 Hal uses kernel heap for DMA** — Check `virtio_block.rs` for the Hal implementation. If it uses `alloc::alloc::alloc_zeroed`, the GPU framebuffer (~4 MiB) also comes from the kernel heap. Ensure MEMORY is large enough.

5. **FB_WRITE row-by-row translation** — Same as ch4-tetris. User virtual pages may not be physically contiguous.

6. **File paths must be null-terminated** — The `open()` syscall in ch6 reads the path byte-by-byte until null. Pass `"breakout_save\0"` from user space.

7. **No seek syscall** — easy-fs reads/writes sequentially from the current offset. For saving, open with `CREATE | WRONLY | TRUNC` flags (TRUNC clears existing content, resets offset to 0). For loading, open with `RDONLY` (offset starts at 0).

8. **Feature-gate binary bodies** — `#[cfg(feature = "breakout")]` for `cargo publish --dry-run`.

9. **Integer-only physics** — Fixed-point (x256) for ball position/velocity.

10. **User stack 8 KiB** — Keep stack allocations under ~5 KiB.

11. **build.rs caching** — `cargo:rerun-if-env-changed=TG_SKIP_USER_APPS`.

12. **Kernel stack 64 KiB** — For VirtIO GPU init.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch6-breakout/Cargo.toml` | Modify | New name, ensure virtio-drivers dep |
| `ch6-breakout/.cargo/config.toml` | Modify | Add GPU + keyboard devices, `-serial stdio`, `TG_USER_DIR` |
| `ch6-breakout/src/main.rs` | Modify | GPU/keyboard init, FB/read syscalls, replace STDIN blocking read |
| `ch6-breakout/src/virtio_block.rs` | Modify (maybe) | Extend MMIO scan for GPU/keyboard, or add separate module |
| `ch6-breakout/build.rs` | Modify | ch6_breakout case_key, `--features breakout` |
| `ch6-breakout/test.sh` | Modify | Headless CI with GPU device |
| `user/src/breakout.rs` | Create | Game logic, physics, rendering, save/load |
| `user/src/lib.rs` | Modify | Add breakout module |
| `user/Cargo.toml` | Modify | Add `breakout` feature |
| `user/src/bin/breakout.rs` | Create | Binary entry (feature-gated body) |
| `user/cases.toml` | Modify | Add ch6_breakout section |

## Acceptance Criteria

1. `cargo run` opens QEMU with a playable Breakout game on VNC
2. Paddle moves with A/D or arrow keys, ball bounces off walls/paddle/bricks
3. Bricks are destroyed on hit, score increases
4. F5 saves game to filesystem, F9 loads — verified by save, quit, restart, load
5. 3 lives, game over when all lost, Enter restarts
6. Level progression when all bricks cleared
7. Existing ch6 test programs still pass (serial output correct)
8. `cargo check` and `cargo publish --dry-run` pass
9. Score, lives, and level displayed on screen

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine game design, ch6-specific filesystem integration. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task
4. `superpowers:requesting-code-review` — final review

Also load `frontend-design:frontend-design` for the visual design phase.

Do NOT publish the crate — the user will review and publish manually.

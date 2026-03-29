# Ch6 Breakout Game — Design Spec

## Overview

Extend `tg-rcore-tutorial-ch6` with a user-space Breakout (brick-breaker) game rendered via VirtIO-GPU with VirtIO keyboard input from VNC. The game uses ch6's filesystem to save/load game progress via hotkeys, demonstrating file I/O syscalls alongside VirtIO device drivers.

## Architecture

### Kernel Modifications (ch6-breakout/)

The ch6 kernel already has VirtIO-blk at MMIO 0x10001000 and easy-fs. We add GPU + keyboard support by following the ch4-tetris pattern, adapted for ch6's process manager architecture.

**Key difference from ch4-tetris**: Ch6 uses `PROCESSOR` (PManager with BTreeMap) not `PROCESSES` (Vec). Current process is accessed via `PROCESSOR.get_mut().current().unwrap()`.

#### 1. VirtIO GPU + Keyboard Module (`virtio.rs`)

Copy ch4-tetris `virtio.rs` verbatim. It scans MMIO 0x10001000..0x10008000 for GPU and Input devices. Uses `HalImpl` from the allocator module.

#### 2. DMA Allocator (`allocator.rs`)

Copy ch4-tetris `allocator.rs`. Bump allocator at fixed address 0x8500_0000, 5 MiB pool. Identity-mapped.

#### 3. MMIO Mapping

Expand ch6's `MMIO` const from `(0x1000_1000, 0x1000)` to cover the full VirtIO range `(0x1000_0000, 0x9000)`. Also add DMA pool mapping `(0x8500_0000, 0x50_0000)`.

#### 4. Memory

Increase `MEMORY` from 48 MiB to 70 MiB to accommodate GPU framebuffer (~4 MiB) via DMA pool.

#### 5. Kernel Stack

Already 128 KiB (32 pages) — sufficient for VirtIO GPU init.

#### 6. FB_INFO (syscall 2000) and FB_WRITE (syscall 2001)

Same as ch4-tetris. Intercepted in the scheduling loop before standard `tg_syscall::handle`. FB_WRITE translates user pointers row-by-row via `PROCESSOR.get_mut().current().unwrap().address_space.translate()`.

#### 7. Non-blocking Keyboard Input

Replace ch6's blocking `tg_sbi::console_getchar()` in `IO::read` for STDIN with VirtIO keyboard polling via `keyboard_trygetchar()`. Returns 0 when no key available.

**Keycode translation** — extends ch4-tetris mapping:
- Left/A=30/105 -> `b'a'`, Right/D=32/106 -> `b'd'`
- Space=57 -> `b' '`, Enter=28 -> `b'\r'`
- F5=63 -> `b'\x05'` (save), F9=67 -> `b'\x09'` (load)
- W=17/Up=103 -> `b'w'`, S=31/Down=108 -> `b's'` (kept for compatibility)

#### 8. QEMU Config

Add `-device virtio-gpu-device` and `-device virtio-keyboard-device`. Replace `-nographic` with `-serial stdio`. Keep existing `-drive` and `-device virtio-blk-device`.

### User-Space (tg-rcore-tutorial-user/)

#### 1. Feature Gate

Add `breakout` feature in Cargo.toml. Add `#[cfg(feature = "breakout")] pub mod breakout;` in lib.rs.

#### 2. Binary (`src/bin/breakout.rs`)

Feature-gated body calling `user_lib::breakout::run_game()`.

#### 3. Game Module (`src/breakout.rs`)

Single-file game module (~600 lines). No heap allocation. All state on stack.

#### 4. Cases

Add `[ch6_breakout]` section to cases.toml with all ch6 base cases plus `breakout`.

## Game Design

### Layout (1280x800 framebuffer)

- Play area: 920x600 pixels, origin at (180, 100)
- Brick grid at top: 10 columns x 5 rows
- Paddle near bottom
- Ball bounces within play area
- HUD: score, lives, level displayed to the right

### Bricks

- Size: 90x25 pixels, 2px gap between bricks
- 10 columns x 5 rows = 50 bricks total
- Row colors (BGRA, top to bottom): red, orange, yellow, green, cyan
- Scoring by row (top to bottom): 50, 40, 30, 20, 10

### Paddle

- Width: 120px, Height: 14px
- Speed: 8px/frame
- Clamped to play area bounds

### Ball

- Size: 10x10 pixels
- Fixed-point physics (x256) for sub-pixel precision
- Initial speed: ~3px/frame, launched upward at angle
- Angle varies based on paddle hit position
- Speed increases every 5 bricks destroyed

### Collision

- Wall bounce: negate appropriate velocity component
- Paddle bounce: negate vy, adjust vx based on hit offset from paddle center
- Brick collision: check ball against each brick, negate velocity based on hit face
- Ball lost: passes bottom -> lose life, reset ball on paddle

### Lives & Scoring

- 3 lives
- Score per brick based on row
- All bricks cleared -> next level (reset bricks, increase speed, keep score)
- All lives lost -> game over, Enter restarts

### Save/Load

- F5 -> save: serialize `SaveData` struct as raw bytes, write to `"breakout_save\0"` via `open(CREATE|WRONLY|TRUNC)`, `write()`, `close()`
- F9 -> load: read from `"breakout_save\0"` via `open(RDONLY)`, `read()`, `close()`, restore state, full redraw
- On-screen "SAVED"/"LOADED" text flash (drawn for ~30 frames then erased)

### SaveData Format (~90 bytes)

```rust
#[repr(C)]
struct SaveData {
    magic: u32,           // 0x42524B4F ("BRKO")
    ball_x: i32,          // fixed-point x256
    ball_y: i32,
    ball_vx: i32,
    ball_vy: i32,
    paddle_x: i32,
    score: u32,
    lives: u32,
    level: u32,
    bricks: [u8; 50],     // 10x5 grid, 0=destroyed, 1=alive
    ball_attached: u8,    // 1 if ball on paddle
    bricks_destroyed: u32, // track for speed increase
}
```

### Rendering

- BGRA pixel format, same as ch4-tetris
- Reuse ch4-tetris rendering primitives (draw_rect, draw_char, draw_string, draw_number)
- Incremental: only redraw changed elements
- Full redraw on load or level change
- Dark background with colorful bricks, white ball, bright paddle

### Controls

| Key | Action |
|-----|--------|
| A / Left | Move paddle left |
| D / Right | Move paddle right |
| Space | Launch ball / pause |
| F5 | Save game |
| F9 | Load game |
| Enter | Restart after game over |

## Files to Create/Modify

| File | Action |
|------|--------|
| `tg-rcore-tutorial-ch6-breakout/` | Copy from ch6 |
| `ch6-breakout/Cargo.toml` | New name, add virtio-drivers dep (already present) |
| `ch6-breakout/.cargo/config.toml` | GPU+keyboard QEMU flags, TG_USER_DIR |
| `ch6-breakout/src/main.rs` | GPU/keyboard init, FB syscalls, keyboard read |
| `ch6-breakout/src/allocator.rs` | New: DMA bump allocator |
| `ch6-breakout/src/virtio.rs` | New: GPU+keyboard init |
| `ch6-breakout/build.rs` | ch6_breakout case_key, --features breakout |
| `ch6-breakout/test.sh` | Headless CI |
| `user/src/breakout.rs` | New: game module |
| `user/src/lib.rs` | Add breakout module |
| `user/Cargo.toml` | Add breakout feature |
| `user/src/bin/breakout.rs` | New: binary entry |
| `user/cases.toml` | Add ch6_breakout section |

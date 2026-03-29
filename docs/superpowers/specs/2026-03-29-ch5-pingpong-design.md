# Ch5-Pingpong Design Spec

## Overview

Add VirtIO-GPU framebuffer rendering, VirtIO keyboard input, and a kernel-managed shared memory mechanism to the ch5 process-management OS kernel. Create a two-player Pong game where each player runs as a separate process, communicating paddle positions and game state through shared memory mapped into both processes at the same physical page. The game runs alongside existing ch5 test programs under stride scheduling with per-process Sv39 virtual address spaces.

## Architecture

### Ch5 vs Ch4 — Key Differences Affecting This Work

| Concern | Ch4-Tetris | Ch5-Pingpong |
|---------|------------|--------------|
| Process access | `PROCESSES.get_mut().get_mut(entity)` | `PROCESSOR.get_mut().current().unwrap()` |
| Process manager | `Vec<Process>` | `PManager<Process, ProcManager>` with BTreeMap + stride scheduler |
| Process creation | Kernel loads ELF at boot | `fork()` deep-copies address space; `exec()` replaces |
| Process identity | Implicit index | Unique `ProcId` per process |
| Scheduling | Round-robin in loop | Stride scheduling (`BIG_STRIDE / priority`) |
| IPC | None needed (single process game) | **Shared memory page** — same physical page in parent + child |
| User stack | 8 KiB (2 pages) | 8 KiB (2 pages) — same |
| Kernel stack | 128 KiB (32 pages) | 128 KiB — already sufficient for VirtIO GPU init |

### Kernel-Side Components

**1. VirtIO Device Drivers**

Same pattern as ch4-tetris: `virtio-drivers = "0.1.0"` provides `VirtIOGpu`, `VirtIOInput`, and the `Hal` trait.

- **Hal trait**: Ch5 kernel identity-maps physical RAM (same as ch4). Use a fixed-address bump allocator for DMA, placed above the kernel heap to avoid overlap.
- **MMIO mapping**: Add VirtIO MMIO range `0x1000_0000..0x1000_9000` to `kernel_space()` as identity-mapped R+W.
- **DMA pool**: Fixed address at `0x8500_0000`, 5 MiB pool. With ch5's process table and stride scheduling overhead, use `MEMORY = 70 MiB`.

Files: `allocator.rs` (new), `virtio.rs` (new) — identical pattern to ch4-tetris.

**2. Framebuffer Syscalls (IDs 2000/2001)**

- **FB_INFO (2000)**: Return `(width << 32) | height`. No address translation needed.
- **FB_WRITE (2001)**: args = `[x, y, w, h, data_ptr]`. Translate `data_ptr` through the calling process's page table row-by-row (virtual pages not necessarily contiguous). Access current process via `PROCESSOR.get_mut().current().unwrap()` (not ch4's `PROCESSES` pattern).

Handle in the main loop's `UserEnvCall` arm, before standard syscall dispatch (same interception point as ch4-tetris).

**3. Non-blocking IO::read for Keyboard**

Replace ch5's blocking `tg_sbi::console_getchar` with VirtIO keyboard polling:
- Poll `VirtIOInput::pop_pending_event()`, filter `event_type == 1 && value == 1` (key press)
- Translate keycode to byte, write to user buffer (translated via address_space)
- Return 1 if key available, 0 otherwise

**4. Shared Memory Syscall (ID 2002)**

`SHM_CREATE`: Allocates one physical page (4 KiB), maps it into the current process's address space at `0x3000_0000` with U+R+W flags. Returns the virtual address on success, -1 on failure.

**Process struct addition:**
```rust
pub struct Process {
    // ... existing fields ...
    pub shared_page: Option<PPN<Sv39>>,
}
```

**Fork modification:** When a process with a shared page calls `fork()`:
1. `cloneself()` deep-copies the entire address space (including the shared page)
2. After clone, unmap the copy at `0x3000_0000` in the child
3. `map_extern()` the **same physical PPN** from parent into the child at `0x3000_0000`
4. Set `child.shared_page = Some(ppn)`

This gives parent and child a shared memory region. No synchronization primitives needed — single writer per field.

**5. QEMU Configuration**

Add `-device virtio-gpu-device`, `-device virtio-keyboard-device`. Replace `-nographic` with `-serial stdio`.

**6. Memory Layout**

```
0x8000_0000  Kernel start
...          Kernel image (text, rodata, data, boot)
...          Kernel heap (managed by customizable-buddy)
0x8500_0000  DMA pool (5 MiB for VirtIO GPU/virtqueues)
0x8460_0000  End of 70 MiB physical RAM (0x8000_0000 + 70 MiB)
```

`MEMORY = 70 << 20` (70 MiB).

### User-Side Components

**1. Pong Game Module** (`user/src/pingpong.rs`, feature-gated with `pingpong`)

All game logic, rendering, physics, input handling in one module.

**2. Binary** (`user/src/bin/pingpong.rs`)

Body gated with `#[cfg(feature = "pingpong")]`.

**3. Multi-Process Architecture**

```
pingpong process starts
  |
  +- calls shm_create() -> gets shared memory page at 0x3000_0000
  +- initializes SharedState (ball, paddles, scores, tick=0)
  +- fork()
  |    |
  |    +- Parent (player 1):
  |    |    loop { read W/S keys, update paddle1_y in shared mem,
  |    |           read p2_up/p2_down flags and update paddle2_y,
  |    |           run ball physics, render everything,
  |    |           increment tick, yield }
  |    |
  |    +- Child (player 2):
  |         loop { read Up/Down keys, set p2_up/p2_down in shared mem,
  |                yield }
  |
  +- Parent waits for child on game exit
```

## Shared Memory Layout

The shared memory page (4 KiB at virtual address `0x3000_0000`) contains:

```rust
#[repr(C)]
struct SharedState {
    // Ball (fixed-point: value * 256 for sub-pixel precision)
    ball_x: i32,      // horizontal position x 256
    ball_y: i32,      // vertical position x 256
    ball_vx: i32,     // horizontal velocity x 256
    ball_vy: i32,     // vertical velocity x 256

    // Paddles (pixel coordinates)
    paddle1_y: i32,   // left paddle Y
    paddle2_y: i32,   // right paddle Y

    // Score
    score1: u32,
    score2: u32,

    // Synchronization
    tick: u32,        // incremented by parent each frame
    game_state: u32,  // 0=waiting_start, 1=playing, 2=point_scored, 3=game_over

    // Input flags (child writes, parent reads and clears)
    p2_up: u8,
    p2_down: u8,
}
```

Parent reads `p2_up`/`p2_down` each frame, clears after reading. Child sets them based on keyboard input. No locks needed — single writer per field.

## VirtIO Keyboard Keycodes

```
EV_KEY (event_type == 1), value == 1 (key press):
  W = 17          -> player 1 paddle up
  S = 31          -> player 1 paddle down
  Up arrow = 103  -> player 2 paddle up
  Down arrow = 108 -> player 2 paddle down
  Space = 57      -> start / restart after point
  Enter = 28      -> restart after game over
```

## Pong Game Design

### Field
- 1280x800 framebuffer
- Play area: ~1100x700 pixels, centered (x: 90..1190, y: 50..750)
- Top/bottom walls: solid borders (ball bounces off)
- Left/right edges: scoring zones (ball passes through = point for opponent)
- Center line: dashed vertical line

### Paddles
- Width: 12px, Height: 100px
- Left paddle: x=100, Right paddle: x=1168 (near edges of play area)
- Movement speed: 6px per frame
- Clamped to play area bounds (50..750 - paddle_height)

### Ball
- Size: 12x12 pixels
- Initial speed: ~4px/frame, angled toward random player
- Speed increases slightly after each paddle hit (1.05x multiplier on vx, capped)
- **Fixed-point physics**: all positions/velocities multiplied by 256. Divide by 256 only when rendering.

### Collision Detection
- **Wall bounce**: ball_y touches top/bottom wall -> negate ball_vy
- **Paddle bounce**: ball_x overlaps paddle AND ball_y within paddle range -> negate ball_vx, adjust ball_vy based on hit position (top = upward, center = straight, bottom = downward)
- **Scoring**: ball_x passes left/right edge -> point for opponent, reset ball to center

### Scoring
- First to 5 points wins
- After each point: brief pause (game_state=2), ball resets to center on Space
- After game over: display winner (game_state=3), Enter restarts

### Rendering
- **Incremental**: erase old ball/paddles (draw background color), draw new positions. Avoid full redraws.
- **Only parent renders**: child writes p2_up/p2_down to shared mem, parent draws everything.
- **Color scheme**: dark background (#0D0D1A), bright white paddles and ball, green/red score digits, dashed gray center line.

### Score Display
- Large pixel-art digits (7-segment style) rendered at top-center
- Format: "P1_SCORE : P2_SCORE"

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch5-pingpong/` (whole dir) | Copy from ch5 | Working crate |
| `ch5-pingpong/Cargo.toml` | Modify | New name, add `virtio-drivers = "0.1.0"` |
| `ch5-pingpong/.cargo/config.toml` | Modify | GPU + keyboard devices, `-serial stdio`, `TG_USER_DIR` |
| `ch5-pingpong/src/main.rs` | Modify | VirtIO init, MMIO/DMA mapping, FB/read/shm syscalls, keyboard |
| `ch5-pingpong/src/process.rs` | Modify | `shared_page` field, fork shared memory handling |
| `ch5-pingpong/src/processor.rs` | No change | Already has stride scheduling |
| `ch5-pingpong/src/allocator.rs` | Create | DMA bump allocator + Hal impl |
| `ch5-pingpong/src/virtio.rs` | Create | MMIO scanning, GPU + keyboard init |
| `ch5-pingpong/build.rs` | Modify | `ch5_pingpong` case_key, `--features pingpong` |
| `ch5-pingpong/test.sh` | Modify | Headless CI with timeout + GPU device |
| `user/src/pingpong.rs` | Create | Game logic, physics, rendering, multi-process |
| `user/src/lib.rs` | Modify | Add `pingpong` module, `shm_create()` wrapper |
| `user/Cargo.toml` | Modify | Add `pingpong` feature |
| `user/src/bin/pingpong.rs` | Create | Binary entry (feature-gated body) |
| `user/cases.toml` | Modify | Add `ch5_pingpong` section |

## Acceptance Criteria

1. `cargo run` opens QEMU with a playable two-player Pong game on VNC
2. Two separate processes (verified by different PIDs printed at startup)
3. Player 1 (W/S) controls left paddle, Player 2 (Up/Down) controls right paddle
4. Ball bounces off walls and paddles with angle variation
5. Scoring works, first to 5 wins, Enter restarts
6. Existing ch5 test programs still pass (serial output correct)
7. `cargo check` and `cargo publish --dry-run` pass for user crate
8. Score displayed on screen

# Ch3 Snake Game — Design Spec

**Date:** 2026-03-28
**Status:** Approved

## Overview

Extend ch3 multiprogramming OS with a VirtIO-GPU snake game. Two variants demonstrate different input strategies: polling-based and interrupt-buffered. The game renders as a proper pixel-art GUI to the QEMU VirtIO-GPU framebuffer, running alongside all 12 existing ch3 test programs under round-robin scheduling.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Input keys | WASD + arrow keys | Arrow keys need escape sequence parser (3-byte: ESC [ dir) |
| Snake variants | Separate cases.toml sections | One snake binary per config, not both simultaneously |
| Visual style | Clean pixel-art retro | Solid colors, 1px grid lines, bitmap font. Fits bare-metal context |
| Game over | Show score, wait for keypress, restart | Keeps game as persistent task, allows replaying |
| Grid size | 20x20 cells, 32px each | 640x640 play area on 1280x800 framebuffer |
| Input architecture | Approach A — minimal kernel | Non-blocking read + optional ring buffer in timer handler |
| DMA pool address | 0x82000000 | 13 programs end at 0x81E00000, 2 MiB gap |

## Kernel Architecture

All kernel modifications go in `tg-rcore-tutorial-ch3-snake/`. Do NOT modify shared crates.

### VirtIO-GPU (reused from ch2-moving-tangram)

- **`src/allocator.rs`**: Static bump allocator with `AtomicUsize` CAS loop. DMA pool at `0x82000000`, 5 MiB. Identity-mapped `HalImpl` implementing `virtio_drivers::Hal` trait. Never frees.
- **`src/gpu.rs`**: Probe MMIO slots `0x10001000`–`0x10008000` stride `0x1000`. Find VirtIO-GPU device, create `VirtIOGpu`, call `setup_framebuffer()`. Return `GPU` driver + `Framebuffer { ptr, len, width, height }`.
- **Initialization in `rust_main()`**: After BSS zeroing and console init, before scheduling loop. Fill framebuffer white, flush. Store GPU + framebuffer in `static mut` globals. Enable rdtime in U-mode via `csrs scounteren, (1 << 1)`.
- **Stack size**: Increase to `32 * 4096` = 128 KiB (flat constant, sufficient for VirtIO init).
- **Overlap guard**: `assert!(kernel_end < 0x8200_0000)` at startup.

### Syscalls

| ID | Name | Args | Return | Behavior |
|----|------|------|--------|----------|
| 2000 | `FB_INFO` | none | `(width << 32) \| height` | Return framebuffer dimensions |
| 2001 | `FB_WRITE` | a0=x, a1=y, a2=w, a3=h, a4=data_ptr | 0 or usize::MAX | Write BGRA rect, skip alpha=0 pixels, flush GPU |
| `read` | fd=0 (STDIN) | a0=0, a1=buf_ptr, a2=len | 0 or 1 | Non-blocking: call `console_getchar()`, return 0 if nothing available, else write byte and return 1 |
| `read` | fd=3 (STDIN_BUFFERED) | a0=3, a1=buf_ptr, a2=len | 0 or 1 | Non-blocking: drain one byte from timer-polled ring buffer, return 0 if empty |

FB_INFO/FB_WRITE are intercepted in `handle_syscall()` before routing to `tg_syscall::handle()`, same pattern as ch2-moving-tangram.

`read(STDIN)` and `read(STDIN_BUFFERED)` are handled in the IO trait implementation.

### Input Ring Buffer (interrupt mode)

- 64-byte `static mut` ring buffer + `head: usize` + `tail: usize` in `main.rs`
- In the timer interrupt handler (after `set_timer`): call `console_getchar()` in a loop, push each available byte into ring buffer until `console_getchar()` returns `usize::MAX`
- `read(fd=3)` pops one byte from this buffer (non-blocking, returns 0 if empty)
- Buffer overflow: if full, drop oldest byte (advance head)

## User-Side Architecture

All user changes go in `tg-rcore-tutorial-user/`. Feature-gated with `snake`.

### Module: `src/snake.rs`

Contains all game logic, rendering, and input handling. No heap allocation — all structures are fixed-size.

#### Data Structures

```rust
enum Direction { Up, Down, Left, Right }
enum GameState { Playing, GameOver }

struct Snake {
    body: [(u8, u8); 400],  // ring buffer of (x,y) coords, max 20*20
    head_idx: usize,
    tail_idx: usize,
    length: usize,
    direction: Direction,
    next_direction: Direction,  // buffered, applied on tick to prevent 180 reversal
}

struct Game {
    snake: Snake,
    food: (u8, u8),
    score: u32,
    state: GameState,
    rng: u64,  // xorshift64 state, seeded from rdtime
}
```

#### Constants

```
GRID_W = 20, GRID_H = 20
CELL_SIZE = 32 px
BOARD_X = 160, BOARD_Y = 80  (top-left of play area on 1280x800 screen)
BOARD_PX_W = 640, BOARD_PX_H = 640
TICK_MS = 150
```

#### Input Parsing — Escape Sequence State Machine

```
State: Normal | EscSeen | BracketSeen

Normal:
  'w'/'W' -> Up,  'a'/'A' -> Left,  's'/'S' -> Down,  'd'/'D' -> Right
  0x1B -> EscSeen
EscSeen:
  '[' -> BracketSeen
  else -> Normal (discard)
BracketSeen:
  'A' -> Up, 'B' -> Down, 'C' -> Right, 'D' -> Left
  -> Normal
```

Drains all available bytes each tick, keeps only last valid direction. Prevents input lag.

#### Game Loop (per tick)

1. **Read input**: Drain bytes via `read(fd, buf, 1)` in a loop until 0 returned. Parse through escape state machine. Update `next_direction` from last valid keypress.
2. **Update state** (if Playing):
   - Apply `next_direction` to `direction` (reject 180 reversal)
   - Compute new head = current head + direction offset
   - Wall collision (x/y out of 0..20) -> GameOver
   - Self collision (new head in body) -> GameOver
   - Food check: if head == food, grow (don't remove tail), spawn new food, score++
   - Else: advance head, remove tail
3. **Render changes**: Only redraw cells that changed:
   - New head (head color)
   - Previous head (now body color)
   - Removed tail (board background)
   - New food (if respawned)
   - Score digits (if changed)
4. **Sleep**: `sleep(150)` — user lib's poll+yield loop

#### Rendering

**Screen layout** (1280x800):

```
+---------------------------------------------+
|                                             |
|   +------------------+  +-----------+       |
|   |                  |  |  SNAKE    |       |
|   |   20x20 grid     |  |           |       |
|   |   640x640 px     |  |  Score: 7 |       |
|   |                  |  |           |       |
|   |                  |  | WASD/Arrows|       |
|   |                  |  |  to move  |       |
|   +------------------+  +-----------+       |
|                                             |
+---------------------------------------------+
```

**Color palette (BGRA byte order):**

| Element | RGB Hex | BGRA bytes |
|---------|---------|------------|
| Background (outside) | `#1A1A2E` | `[0x2E, 0x1A, 0x1A, 0xFF]` |
| Board cells | `#16213E` | `[0x3E, 0x21, 0x16, 0xFF]` |
| Grid lines | `#1A2744` | `[0x44, 0x27, 0x1A, 0xFF]` |
| Snake head | `#00FF41` | `[0x41, 0xFF, 0x00, 0xFF]` |
| Snake body | `#00B830` | `[0x30, 0xB8, 0x00, 0xFF]` |
| Food | `#FF4444` | `[0x44, 0x44, 0xFF, 0xFF]` |
| Border | `#0F9B8E` | `[0x8E, 0x9B, 0x0F, 0xFF]` |
| Text (score, labels) | `#FFFFFF` | `[0xFF, 0xFF, 0xFF, 0xFF]` |
| Game Over text | `#FF4444` | `[0x44, 0x44, 0xFF, 0xFF]` |

**Bitmap font**: Hardcoded 5x7 pixel glyphs for digits 0-9 and letters needed for "SNAKE", "SCORE", "GAME", "OVER". Each glyph is `[u8; 7]` (5 bits per row). Rendered at 4x scale (20x28 px per character).

**Initial render**: Full board draw on start/restart:
1. Fill entire screen with background color via fb_write
2. Draw 2px border around play area (border color)
3. Draw all 400 grid cells (board color with 1px grid line gaps)
4. Draw score panel: "SNAKE" title, "Score: 0", controls hint
5. Draw initial snake (3 cells centered, facing right)
6. Draw initial food

**Per-tick render**: Only changed cells + score if changed. Each cell = one `fb_write(x, y, 32, 32, buf)`.

**Game Over**: Overlay "GAME OVER" and final score centered on board. Serial print too. Wait for any keypress via `getchar()`, then reinitialize and full redraw.

### PRNG

Xorshift64 seeded from `rdtime` at game init:
```
fn next(&mut self) -> u64 {
    self.rng ^= self.rng << 13;
    self.rng ^= self.rng >> 7;
    self.rng ^= self.rng << 17;
    self.rng
}
```
Food placement: `next() % GRID_W`, retry if overlaps snake body.

### Binaries

**`src/bin/snake_poll.rs`**: Calls game loop with `STDIN` (fd=0) for input. Non-blocking direct `console_getchar()`.

**`src/bin/snake_interrupt.rs`**: Calls game loop with `STDIN_BUFFERED` (fd=3) for input. Timer-polled ring buffer.

Both binaries are thin wrappers — pass the fd to `snake::run_game(input_fd)`.

## Build & Config

### New crate: `tg-rcore-tutorial-ch3-snake/`

Copied from `tg-rcore-tutorial-ch3/` with:
- **Cargo.toml**: name = `"jsph-tg-rcore-tutorial-ch3-snake"`, add `virtio-drivers = "0.1.0"` (cfg riscv64), add `interrupt` feature
- **Features**: `interrupt` — selects `ch3_snake_interrupt` cases.toml section (default: `ch3_snake_poll`)
- **.cargo/config.toml**: Add `-device virtio-gpu-device`, replace `-nographic` with `-serial stdio`

### build.rs modifications (in ch3-snake copy)

The existing build.rs selects `case_key = "ch3"` or `"ch3_exercise"`. Modify to:
- Default case_key: `"ch3_snake_poll"`
- With `--features interrupt`: `"ch3_snake_interrupt"`
- Add `--features snake` to the `cargo build` command for user apps (in `build_user_app()`) so the snake module compiles
- Add `println!("cargo:rerun-if-env-changed=CARGO_FEATURE_INTERRUPT");`

### cases.toml additions

```toml
[ch3_snake_poll]
base = 0x8040_0000
step = 0x0020_0000
cases = [00hello_world, 01store_fault, 02power, 03priv_inst, 04priv_csr,
         05write_a, 06write_b, 07write_c, 08power_3, 09power_5,
         10power_7, 11sleep, snake_poll]

[ch3_snake_interrupt]
base = 0x8040_0000
step = 0x0020_0000
cases = [00hello_world, 01store_fault, 02power, 03priv_inst, 04priv_csr,
         05write_a, 06write_b, 07write_c, 08power_3, 09power_5,
         10power_7, 11sleep, snake_interrupt]
```

### User crate Cargo.toml

Add `snake = []` feature. The snake module is `#[cfg(feature = "snake")]`.

### Memory Layout

```
0x80000000  Kernel image
0x80400000  Program 0   (00hello_world)
0x80600000  Program 1   (01store_fault)
  ...
0x81A00000  Program 11  (11sleep)
0x81C00000  Program 12  (snake_poll or snake_interrupt)
0x81E00000  End of program 12 slot
0x82000000  DMA pool start (5 MiB)
0x82500000  DMA pool end
0x88000000  End of QEMU RAM (128 MiB)
```

### test.sh

- Add `-display none` for headless CI
- Timeout of 30s
- Pipe through `tg-rcore-tutorial-checker --ch 3`
- Snake game prints to serial but checker ignores it

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `tg-rcore-tutorial-ch3-snake/Cargo.toml` | Modify | Name, author, virtio-drivers dep, interrupt feature |
| `tg-rcore-tutorial-ch3-snake/.cargo/config.toml` | Modify | GPU device, serial stdio, user crate env |
| `tg-rcore-tutorial-ch3-snake/src/allocator.rs` | Create | Bump allocator at 0x82000000, HalImpl |
| `tg-rcore-tutorial-ch3-snake/src/gpu.rs` | Create | VirtIO-GPU init, framebuffer struct |
| `tg-rcore-tutorial-ch3-snake/src/main.rs` | Modify | GPU init, FB/read syscalls, ring buffer, stack size |
| `tg-rcore-tutorial-ch3-snake/test.sh` | Modify | Headless CI with GPU device |
| `tg-rcore-tutorial-user/src/snake.rs` | Create | Game logic, rendering, input, bitmap font |
| `tg-rcore-tutorial-user/src/lib.rs` | Modify | Add snake module (feature-gated), add non-blocking read wrapper |
| `tg-rcore-tutorial-user/Cargo.toml` | Modify | Add snake feature |
| `tg-rcore-tutorial-user/src/bin/snake_poll.rs` | Create | Polling input snake binary |
| `tg-rcore-tutorial-user/src/bin/snake_interrupt.rs` | Create | Buffered input snake binary |
| `tg-rcore-tutorial-user/cases.toml` | Modify | Add ch3_snake_poll and ch3_snake_interrupt sections |

## Acceptance Criteria

1. `cargo run` in ch3-snake opens QEMU window showing playable snake game with pixel-art GUI
2. WASD and arrow keys both control direction
3. Snake grows on food, game over on wall/self collision
4. Game over screen shows score, any key restarts
5. `cargo run --features interrupt` uses buffered input mode
6. All 12 existing ch3 test programs still produce correct serial output
7. Score displayed on-screen in bitmap font
8. ~7 FPS game tick (150ms), smooth animation
9. `cargo check` and `cargo publish --dry-run` pass

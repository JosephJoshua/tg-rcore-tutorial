# Ch4-Tetris Design Spec

## Overview

Add VirtIO-GPU framebuffer rendering and VirtIO keyboard input to the ch4 virtual-memory OS kernel. Create a Tetris user program that renders to the GPU framebuffer and receives keyboard input from VNC. The game runs alongside existing ch4 test programs under round-robin scheduling with per-process Sv39 virtual address spaces.

## Architecture

### Ch4 vs Ch3 — Key Differences

| Concern | Ch3-Snake | Ch4-Tetris |
|---------|-----------|------------|
| Address space | Identity-mapped physical | Per-process Sv39 page tables |
| User pointers | Direct access (`data_ptr as *const u8`) | Must `address_space.translate::<u8>(VAddr::new(ptr), READABLE)` |
| Context switch | `LocalContext::execute()` | `ForeignContext::execute(portal, ())` via MultislotPortal |
| VirtIO Hal | `phys_to_virt(p) = p` (identity) | Same — kernel identity-maps physical RAM |
| DMA allocation | Bump allocator at fixed address | Bump allocator at fixed address (kernel heap also viable) |
| MMIO access | Direct (no paging) | Must ensure MMIO range mapped in `kernel_space()` |
| Syscall dispatch | `task.rs` intercepts before standard dispatch | Inline in scheduling loop's UserEnvCall match arm |

### Kernel-Side Components

**1. VirtIO Device Drivers**

Reuse the ch3-snake pattern with `virtio-drivers = "0.1.0"`:
- Scan MMIO slots `0x10001000..0x10008000` (stride 0x1000)
- `MmioTransport::new()` per slot, check `device_type()`
- `VirtIOGpu::new(transport)` → `setup_framebuffer()` → get `(ptr, len, width, height)`
- `VirtIOInput::new(transport)` → `pop_pending_event()` for keyboard

**Hal Implementation**: Bump allocator at `0x8200_0000`, 5 MiB pool. Identity mapping (`phys_to_virt(p) = p`) works because the kernel identity-maps all physical RAM in `kernel_space()`.

**MMIO Mapping**: The ch4 `kernel_space()` maps physical RAM from kernel start to `MEMORY` (24 MiB at `0x80000000..0x81800000`). The VirtIO MMIO range `0x10001000..0x10008000` is NOT in this range. We must add an identity-mapped region for MMIO in `kernel_space()` with Read+Write flags.

**2. Framebuffer Syscalls (IDs 2000/2001)**

- **FB_INFO (2000)**: Return `(width << 32) | height`. No address translation needed.
- **FB_WRITE (2001)**: args = `[x, y, w, h, data_ptr]`. Must translate `data_ptr` through the calling process's page table before reading pixel data. For cell-sized writes (32x32x4 = 4096 bytes = 1 page), single translate suffices. For larger writes (background strips), handle page boundaries by copying row-by-row with per-row translation if needed.

**3. Non-blocking IO::read for Keyboard**

Poll `VirtIOInput::pop_pending_event()`. Filter `event_type == 1 && value == 1` (key press). Translate keycode to byte. Write to user buffer (translated via address_space). Return 1 if key available, 0 otherwise.

**4. QEMU Configuration**

Replace `-nographic` with `-serial stdio -device virtio-gpu-device -device virtio-keyboard-device`.

### User-Side Components

**1. Tetris Game Module** (`user/src/tetris.rs`, feature-gated with `tetris`)

All game logic, rendering, and input in one module. Called from `user/src/bin/tetris.rs`.

**2. Binary** (`user/src/bin/tetris.rs`)

Body gated with `#[cfg(feature = "tetris")]` so `cargo publish --dry-run` passes.

## Tetris Game Design

### Board & Layout
- 10 columns x 20 rows (standard Tetris)
- Cell size: 30px (fits 300x600 board in 1280x800 framebuffer)
- Board position: left-center area
- Right panel: next piece preview, score, level, lines cleared

### Pieces (7 standard tetrominoes)
I, O, T, S, Z, J, L — each stored as 4 rotation states of 4 (row, col) offsets.

### Game Mechanics
- **Gravity**: Piece drops one row per tick. Start ~800ms, minimum ~100ms.
- **Soft drop** (Down/S): ~50ms per row.
- **Hard drop** (Space): Instant placement at lowest valid position.
- **Rotation**: Clockwise. Simple rotation (no wall kicks).
- **Line clearing**: Full row removed, above rows shift down.
- **Scoring**: 1 line=100*level, 2=300*level, 3=500*level, 4=800*level.
- **Level up**: Every 10 lines.
- **Game over**: New piece cannot spawn.
- **Restart**: Game over screen, any key restarts.

### Rendering
- **Incremental**: Only redraw changed cells. Each `fb_write` triggers GPU flush.
- **Ghost piece**: Dim outline showing landing position.
- **Static render buffer**: 30x30x4 = 3600 bytes for cell drawing (stack-safe).
- **Background**: Drawn via fb_write strips in game init, not by kernel.

### Data Structures
```
Board: [u8; 10 * 20]  — 0=empty, 1-7=piece color index
Piece: { shape: u8, rotation: u8, row: i8, col: i8 }
Game: { board, current_piece, next_piece, score, level, lines, state, rng, tick_ms }
```

### PRNG
Xorshift64 seeded from `rdtime` via `get_time()`.

### Input
Non-blocking `read(STDIN, &mut buf, 1)` returns 0 if no key. Keycode mapping:
- Left/A=105/30 -> move left
- Right/D=106/32 -> move right
- Down/S=108/31 -> soft drop
- Up/W=103/17 -> rotate
- Space=57 -> hard drop
- Enter=28 -> restart

### Colors (BGRA)
- Background: dark (#0D0D1A)
- Board: dark grid (#1A1A2E, #16213E)
- I: cyan, O: yellow, T: purple, S: green, Z: red, J: blue, L: orange
- Ghost: dimmed version of piece color
- Text: white/light gray

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch4-tetris/` (whole dir) | Copy from ch4 | Working crate |
| `ch4-tetris/Cargo.toml` | Modify | New name, add `virtio-drivers = "0.1.0"` |
| `ch4-tetris/.cargo/config.toml` | Modify | Add GPU + keyboard devices, `-serial stdio` |
| `ch4-tetris/src/main.rs` | Modify | VirtIO init, MMIO mapping, FB/read syscalls with address translation, keyboard driver |
| `ch4-tetris/build.rs` | Modify | ch4_tetris case_key, `--features tetris`, rerun triggers |
| `ch4-tetris/test.sh` | Modify | Headless CI with timeout + GPU device |
| `user/src/tetris.rs` | Create | Game logic, rendering, input (feature-gated) |
| `user/src/lib.rs` | Modify | Add tetris module |
| `user/Cargo.toml` | Modify | Add `tetris` feature |
| `user/src/bin/tetris.rs` | Create | Tetris binary (feature-gated body) |
| `user/cases.toml` | Modify | Add ch4_tetris section |

## Acceptance Criteria

1. `cargo run` in ch4-tetris opens QEMU with playable Tetris on VNC
2. Arrow keys / WASD control movement and rotation, Space for hard drop
3. Pieces fall with gravity, lines clear, score/level increase
4. Speed increases with level
5. Game over when pieces reach top, any key restarts
6. Existing ch4 test programs still run (serial output correct)
7. `cargo check` and `cargo publish --dry-run` pass for user crate
8. Score, level, and lines displayed on screen

# Ch7 Pac-Man Game Design Spec

**Date:** 2026-03-30
**Source:** `docs/prompts/ch7-pacman.md`
**Status:** Approved

## Overview

Extend `tg-rcore-tutorial-ch7` with a user-space Pac-Man game rendered via VirtIO-GPU with VirtIO keyboard input. The game uses ch7's **pipes** for inter-process communication between a game coordinator and 4 ghost AI child processes, and **signals** for async game events (power pellet broadcast, ghost-eaten notification).

## Scope

### Kernel-side (tg-rcore-tutorial-ch7-pacman/)

1. **VirtIO GPU + keyboard drivers** — Scan MMIO slots 0x10001000-0x10008000 for GPU and Input devices. Use `virtio-drivers = "0.3.0"` with updated Hal trait (BufferDirection, share/unshare/mmio_phys_to_virt). DMA pool at 0x8500_0000 (5 MiB bump allocator).

2. **Framebuffer syscalls** — FB_INFO (2000), FB_WRITE (2001), FB_FLUSH (2003). FB_WRITE translates user data pointer through process page table row-by-row with alpha blending (skip alpha==0 pixels). FB_FLUSH triggers GPU display update.

3. **Keyboard via Fd::Empty stdin** — When `Fd::Empty { read: true }` is read (fd 0), poll VirtIO keyboard. Return keycode for press, keycode|0x80 for release. Drain EV_SYN events. Non-blocking (return 0 if no events). Does NOT touch pipe reads.

4. **QEMU config** — Add `-device virtio-gpu-device -device virtio-keyboard-device -vnc :0 -serial stdio`.

5. **Memory** — Increase to 70 MiB. Map DMA pool 0x8500_0000..0x8550_0000 and MMIO 0x1000_0000..0x1000_9000 in kernel_space().

### User-side (tg-rcore-tutorial-user/)

1. **Game module** `src/pacman.rs` — Feature-gated with `pacman`. Contains all game logic, rendering, ghost AI, pipe IPC, signal handling.

2. **Binary** `src/bin/pacman.rs` — Feature-gated entry calling `user_lib::pacman::run_game()`.

3. **Multi-process architecture:**
   - Parent creates 2 pipes per ghost (8 total), forks 4 children
   - Each ghost child: close unused pipe ends, register SIGUSR1/SIGUSR2 handlers, loop reading state from pipe and writing direction back
   - Parent: close unused pipe ends, read keyboard, move pacman, write state to ghost pipes, read ghost directions, check collisions, render, flush

## Pipe Protocol

Parent → Ghost (7 bytes per tick):
```
[pac_x: u8, pac_y: u8, pac_dir: u8, ghost_x: u8, ghost_y: u8, blinky_x: u8, blinky_y: u8]
```

Ghost → Parent (1 byte per tick):
```
[direction: u8]  // 0=up, 1=right, 2=down, 3=left
```

Lockstep: parent writes to all ghosts, then reads from all ghosts (blocking reads = natural sync).

## Signal Usage

| Signal | Direction | Purpose |
|--------|-----------|---------|
| SIGUSR1 (10) | Parent → All ghosts | Power pellet eaten — enter frightened mode |
| SIGUSR2 (12) | Parent → Specific ghost | Ghost was eaten — return to ghost house |

Parent detects all collisions (it knows all positions). Signals flow parent→ghost only.

## Ghost AI

| Ghost | Color | Chase Target | Personality |
|-------|-------|-------------|-------------|
| Blinky | Red | Pac-Man's cell | Aggressive, faster when <20 dots remain |
| Pinky | Pink | 4 cells ahead of Pac-Man | Ambusher, cuts off escape routes |
| Inky | Cyan | Alternates chase/random dot | Unpredictable |
| Clyde | Orange | Pac-Man if >8 cells away, else corner | Shy push-pull |

Modes cycle: Scatter (7s) → Chase (20s) → repeat (scatter shortens each cycle).
Frightened mode: random movement, half speed, 6s duration (decreasing per level).

## Maze

21x21 grid, 28x28 pixel cells. ~180 dots, 4 power pellets, ghost house at center. Two tunnel rows (wrap left↔right). 1280x800 framebuffer, maze centered in left 2/3, HUD in right 1/3.

## Visual Design

"Warm Arcade Phosphor" palette — rich cobalt blue beveled walls, golden yellow Pac-Man, candy-bright ghost colors, warm peach dots. Walls rendered with 3D bevel effect (bright edges facing corridors). Full BGRA color table in source prompt.

## Game States

READY → PLAYING → DYING/LEVEL_COMPLETE/GAME_OVER/PAUSED. Lives: 3 (extra at 10k). Dots: 10pts, Pellets: 50pts, Ghosts: 200/400/800/1600. Fruit at 70/170 dots eaten.

## Key Constraints

- `virtio-drivers = "0.3.0"` (v0.1.0 keyboard bug)
- FB_WRITE must NOT flush GPU (separate FB_FLUSH)
- DMA pool needs explicit identity mapping
- Keyboard: press AND release events, drain sync events
- User stack 8 KiB — keep stack allocations under ~5 KiB
- File paths null-terminated for open()
- Signal handlers run in user space (set global flag, main loop checks)
- Integer-only grid math
- Feature-gate binary body with `#[cfg(feature = "pacman")]`

## Files

| File | Action |
|------|--------|
| `ch7-pacman/Cargo.toml` | Modify: new name, virtio-drivers 0.3.0 |
| `ch7-pacman/.cargo/config.toml` | Modify: GPU+keyboard+VNC QEMU args |
| `ch7-pacman/src/main.rs` | Modify: GPU/kbd init, FB syscalls, keyboard IO::read |
| `ch7-pacman/src/virtio.rs` | Create: MMIO scanning, GPU/keyboard drivers |
| `ch7-pacman/src/allocator.rs` | Create: DMA bump allocator with v0.3.0 Hal |
| `ch7-pacman/src/virtio_block.rs` | Modify: use shared Hal from allocator.rs |
| `ch7-pacman/build.rs` | Modify: ch7_pacman case_key, --features pacman |
| `ch7-pacman/test.sh` | Modify: headless CI |
| `user/src/pacman.rs` | Create: game logic, maze, rendering, ghost AI, IPC |
| `user/src/lib.rs` | Modify: add pacman module |
| `user/Cargo.toml` | Modify: add pacman feature |
| `user/src/bin/pacman.rs` | Create: feature-gated binary entry |
| `user/cases.toml` | Modify: add ch7_pacman section |

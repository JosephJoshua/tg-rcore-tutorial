Extend `tg-rcore-tutorial-ch3` with a user-space snake game rendered via VirtIO-GPU, supporting both **polling-based input** and **interrupt-based input** control modes. The game runs as a user program under ch3's multiprogramming scheduler alongside existing test programs.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch3-snake.gif

## Setup

Before starting, copy the ch3 directory to create the working crate:

```bash
cp -r tg-rcore-tutorial-ch3 tg-rcore-tutorial-ch3-snake
rm -rf tg-rcore-tutorial-ch3-snake/{target,Cargo.lock,.gitrepo}
```

Then update `tg-rcore-tutorial-ch3-snake/Cargo.toml`:
- Change `name` to `"jsph-tg-rcore-tutorial-ch3-snake"`
- Change `authors` to `["Joseph Joshua Anggita <jj.anggita@gmail.com>"]`
- Change `description` to describe the snake game extension
- Change path dependencies to version-only dependencies (for crates.io publishing)

**All kernel modifications go into `tg-rcore-tutorial-ch3-snake/`** — do NOT modify the original `tg-rcore-tutorial-ch3/`. User-side changes go in `tg-rcore-tutorial-user/`.

## Goal

Add VirtIO-GPU framebuffer and keyboard input support to the ch3 multiprogramming OS kernel. Create a snake game user program that renders to the GPU framebuffer and reads keyboard input for direction control. The snake game runs concurrently with other ch3 test programs under the round-robin scheduler. Support two input modes:

1. **Polling mode**: The snake game actively polls for keyboard input each game tick using a non-blocking read syscall. Simple, but burns CPU cycles.
2. **Interrupt mode**: The kernel buffers keyboard events via SBI console interrupt or polling in the timer handler, and the user program reads from a buffer via a blocking/non-blocking read syscall. More efficient.

## Architecture Overview

Ch3 is a multiprogramming OS with round-robin time-slice scheduling. Multiple user programs are loaded into memory simultaneously at different addresses (base=`0x80400000`, step=`0x200000` = 2 MiB per program). The kernel uses timer interrupts (~1ms quantum) for preemptive task switching. Each task has its own `TaskControlBlock` with an 8 KiB stack and saved register context.

### Key ch3 capabilities (already present)
- Round-robin scheduler with preemptive timer interrupts (12,500 cycles ≈ 1ms)
- `clock_gettime` syscall (nanosecond precision, 12.5 MHz clock)
- `sched_yield` syscall (cooperative yield)
- `write` syscall (stdout/stderr)
- `exit` syscall
- `sleep(ms)` in user lib (poll + yield loop)
- Up to 32 concurrent tasks (APP_CAPACITY)

### What needs to be added

**Kernel-side:**

1. **VirtIO-GPU driver** — Reuse from `tg-rcore-tutorial-ch2-moving-tangram/`: static bump allocator at fixed address `0x81000000`, Hal trait with identity mapping, MMIO slot probing. Initialize GPU in `rust_main` before the scheduling loop. The DMA pool at `0x81000000` is safe because ch3 programs load at `0x80400000`–`0x80C00000` (12 programs × 2 MiB step = max `0x80400000 + 12*0x200000 = 0x81C00000`) — actually this overlaps! Need to check: with the snake program added, how many total programs will there be? The DMA pool address may need to be pushed higher (e.g., `0x82000000`) to avoid overlap with the last user program slots. Calculate carefully.

2. **FB_INFO and FB_WRITE syscalls** (IDs 2000/2001) — Same as ch2-moving-tangram. Intercept in `handle_syscall` before `tg_syscall::handle()`. FB_WRITE skips alpha=0 pixels.

3. **Keyboard input syscall** — Implement `IO::read` for fd=STDIN. Two approaches for the kernel side:
   - **Non-blocking read**: Call `tg_sbi::console_getchar()`. If no char available (returns `usize::MAX`), return 0 or -1 immediately. The user program decides whether to retry.
   - **Buffered read with timer polling**: In the timer interrupt handler, poll `console_getchar()` and push any available characters into a kernel ring buffer. The read syscall drains from this buffer. This provides better responsiveness since characters are captured even while other tasks are running.

   The non-blocking approach is simpler and sufficient for the snake game's polling mode. The buffered approach enables the interrupt mode. Implement both: a custom syscall or a flag parameter to `read` that selects blocking vs non-blocking behavior.

4. **Kernel stack size**: ch3 currently uses `(APP_CAPACITY + 2) * 8192 = 272 KiB`. VirtIO-GPU init needs extra stack. Increase the multiplier or add padding.

**User-side:**

1. **Snake game module** (`tg-rcore-tutorial-user/src/snake.rs`, feature-gated with `snake` feature) — Game logic:
   - Grid-based game board (e.g., 20×20 cells)
   - Snake body as a linked list or ring buffer of cell coordinates
   - Food spawning (pseudo-random using rdtime seed)
   - Collision detection (walls, self)
   - Score tracking
   - Direction control via WASD or arrow keys
   - Game loop with configurable tick rate (~100-200ms per tick)
   - Rendering: draw grid, snake body, food, score to a pixel buffer, then FB_WRITE

2. **Two snake binaries**:
   - `snake_poll.rs` — Polling mode: each game tick, calls non-blocking read to check for keypresses. If no key, snake continues in current direction.
   - `snake_interrupt.rs` — Interrupt mode: uses blocking read with timeout, or reads from kernel's input buffer. More responsive to key input.

3. **Rendering approach**: Each game tick, redraw only changed cells (snake head, new food, cleared tail) rather than the full board. Or redraw the full board if simpler — the board is small so performance is fine.

4. **Pseudo-random number generation**: Use `rdtime` as seed for a simple LCG or xorshift PRNG for food placement. No need for cryptographic randomness.

## Key Constraints

- All kernel modifications go in `tg-rcore-tutorial-ch3-snake/`; user-side changes in `tg-rcore-tutorial-user/`
- Do not modify `tg-rcore-tutorial-sbi`, `tg-rcore-tutorial-syscall`, or other shared crates
- The existing ch3 test programs (`00hello_world`, `01store_fault`, ..., `11sleep`) must still work
- QEMU runner needs `-device virtio-gpu-device` and `-serial stdio` (replace `-nographic`)
- Ch3 has no page tables — identity mapping, same as ch1/ch2 tangram
- User programs load at different addresses (step=`0x200000`); calculate the DMA pool address to avoid overlap with the highest-addressed user program
- `cargo check` and `cargo publish --dry-run` must still pass
- The snake game user program should be feature-gated (`snake` feature in the user crate) so it's optional
- User programs share the CPU via the scheduler — the snake game must yield or sleep between ticks, not busy-loop
- Reference `tg-rcore-tutorial-ch2-moving-tangram/src/` for the allocator, GPU init, FB syscall handlers, and `tg-rcore-tutorial-user/src/tangram.rs` for rendering patterns and the `fb_write`/`fb_info` wrappers (already in lib.rs)

## Memory Layout Consideration

Ch3 loads programs at `base + i * step`:
- Program 0: `0x80400000`
- Program 1: `0x80600000`
- ...
- Program N: `0x80400000 + N * 0x200000`

With 12 existing programs + 2 snake programs = 14 total, the last program loads at `0x80400000 + 13 * 0x200000 = 0x81E00000`. The 5 MiB DMA pool must start AFTER this. Use `0x82000000` (32 MiB offset into RAM) as the pool base. QEMU virt default 128 MiB RAM ends at `0x88000000`, so `0x82000000 + 5 MiB = 0x82500000` is safe.

## Files to Create/Modify

All paths relative to `tg-rcore-tutorial-ch3-snake/` unless noted.

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` | Modify | New name/author, add `virtio-drivers` dep |
| `.cargo/config.toml` | Modify | Add GPU device, replace `-nographic`, update TG_USER_* |
| `src/allocator.rs` | Create | Static bump allocator at `0x82000000` + Hal trait |
| `src/gpu.rs` | Create | VirtIO-GPU init + framebuffer (from ch2-moving-tangram) |
| `src/main.rs` | Modify | Init GPU, add FB/read syscall handlers, increase stack |
| `test.sh` | Modify | Headless CI with timeout + GPU device |
| `tg-rcore-tutorial-user/src/snake.rs` | Create | Snake game logic, rendering, input handling |
| `tg-rcore-tutorial-user/src/lib.rs` | Modify | Add snake module (feature-gated), add non-blocking read wrapper |
| `tg-rcore-tutorial-user/Cargo.toml` | Modify | Add `snake` feature |
| `tg-rcore-tutorial-user/src/bin/snake_poll.rs` | Create | Snake game with polling input |
| `tg-rcore-tutorial-user/src/bin/snake_interrupt.rs` | Create | Snake game with interrupt/buffered input |
| `tg-rcore-tutorial-user/cases.toml` | Modify | Add snake programs to ch3 section |

## Acceptance Criteria

1. `cargo run` opens a QEMU window showing a playable snake game
2. Arrow keys or WASD control the snake direction
3. Snake grows when eating food, game ends on wall/self collision
4. Polling mode (`snake_poll`) and interrupt mode (`snake_interrupt`) both work
5. Serial terminal shows kernel boot messages, other ch3 test program output, and snake game status
6. Existing ch3 test programs still function correctly (they share CPU time with the snake game)
7. Animation is smooth at ~5-10 FPS (100-200ms per game tick)
8. `cargo check` and `cargo publish --dry-run` pass
9. Score is displayed (on-screen or via serial)

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine game design, input architecture, rendering strategy, memory layout. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — create step-by-step implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task with review checkpoints
4. `superpowers:requesting-code-review` — final review

Do NOT publish the crate — the user will review and publish manually.

# Ch6 Breakout — Implementation Plan

## Phase 1: Kernel Setup (ch6-breakout crate)

### Task 1.1: Copy ch6 and configure crate
- Copy `tg-rcore-tutorial-ch6/` to `tg-rcore-tutorial-ch6-breakout/`
- Remove `target/`, `Cargo.lock`, `.gitrepo`
- Update `Cargo.toml`: change name, authors, description
- Update `.cargo/config.toml`:
  - Add `-device virtio-gpu-device`, `-device virtio-keyboard-device`
  - Replace `-nographic` with `-serial stdio`
  - Add `TG_USER_DIR` pointing to `../tg-rcore-tutorial-user`
  - Keep existing `-drive` and `-device virtio-blk-device`

### Task 1.2: Add VirtIO GPU/keyboard modules
- Create `src/allocator.rs`: Copy ch4-tetris bump allocator (DMA pool at 0x8500_0000, 5 MiB)
- Create `src/virtio.rs`: Copy ch4-tetris MMIO scanner (GPU + keyboard init)

### Task 1.3: Modify main.rs for GPU/keyboard
- Add `mod allocator` and `mod virtio` (cfg-gated on riscv64)
- Import `VirtIOGpu`, `VirtIOInput`, `MmioTransport` from virtio_drivers
- Add global statics: `GPU`, `FRAMEBUFFER`, `KEYBOARD`
- Add `SYSCALL_FB_INFO` (2000) and `SYSCALL_FB_WRITE` (2001) constants
- Expand MMIO mapping: change from `(0x1000_1000, 0x1000)` to `(0x1000_0000, 0x9000)`, add DMA pool `(0x8500_0000, 0x50_0000)`
- Increase MEMORY from 48 MiB to 70 MiB
- Initialize VirtIO devices after kernel_space() call, before initproc load
- Add `translate_keycode()` with F5/F9 support
- Add `keyboard_trygetchar()` function
- Add `handle_fb_info()` and `handle_fb_write()` — adapted for ch6's PROCESSOR
- Intercept FB syscalls in scheduling loop before `tg_syscall::handle`
- Replace STDIN blocking read with VirtIO keyboard polling (non-blocking)

### Task 1.4: Modify build.rs
- Change case_key from `"ch6"` to `"ch6_breakout"`
- Add `--features breakout` to user app build command

### Task 1.5: Create test.sh
- Headless CI script with GPU device

## Phase 2: User-Space (tg-rcore-tutorial-user/)

### Task 2.1: Add breakout feature and module registration
- Add `breakout = []` to `[features]` in Cargo.toml
- Add `#[cfg(feature = "breakout")] pub mod breakout;` to lib.rs

### Task 2.2: Add cases.toml section
```toml
[ch6_breakout]
cases = [
    "00hello_world", "01store_fault", "02power", "03priv_inst", "04priv_csr",
    "05write_a", "06write_b", "07write_c", "08power_3", "09power_5", "10power_7",
    "12forktest", "13forktree", "14forktest2", "15matrix", "fork_exit", "forktest_simple",
    "sbrk", "filetest_simple", "cat_filea", "ch6b_usertest", "user_shell",
    "initproc", "breakout",
]
```

### Task 2.3: Create breakout binary
- Create `src/bin/breakout.rs` with feature-gated body

### Task 2.4: Create breakout game module
- Create `src/breakout.rs` with complete game logic (~600 lines)
- Layout constants, colors (BGRA), font glyphs (reuse from tetris)
- PRNG (xorshift64)
- Game state struct (no heap)
- Fixed-point ball physics (x256)
- Brick grid (10x5), collision detection
- Paddle movement, ball-paddle angle reflection
- Level progression, lives, scoring
- Save/Load via open/write/read/close syscalls
- Rendering: draw_rect, draw_char, draw_string, draw_number
- Brick rendering with row colors
- HUD panel (score, lives, level, controls)
- Main game loop: poll input, update physics, render, ~60fps timing

## Phase 3: Build and Test

### Task 3.1: Verify cargo check passes
- `cargo check` on ch6-breakout
- `cargo check` on user crate with breakout feature

### Task 3.2: Build and run
- `cargo run` in ch6-breakout
- Verify VNC shows game
- Test paddle movement, ball physics, brick destruction
- Test save (F5) and load (F9)
- Test game over and restart

## Parallelization Strategy

- **Task 1.1** can run independently (crate setup)
- **Task 2.1 + 2.2 + 2.3** can run in parallel with kernel tasks (user-side registration)
- **Task 2.4** (game module) can run in parallel with kernel tasks
- **Task 1.2 + 1.3 + 1.4** depend on 1.1 completing
- **Task 3.x** depends on all prior tasks

Optimal grouping for subagent-driven development:
1. **Agent A**: Task 1.1 + 1.2 + 1.3 + 1.4 + 1.5 (all kernel work)
2. **Agent B**: Task 2.1 + 2.2 + 2.3 + 2.4 (all user-space work)
3. **Agent C**: Task 3.1 + 3.2 (build verification) — after A and B complete

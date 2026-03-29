Extend `tg-rcore-tutorial-ch5` with a user-space two-player Pong game rendered via VirtIO-GPU, with VirtIO keyboard input from VNC. The game uses **two cooperating processes** (one per player) communicating through a kernel-managed shared memory page, demonstrating ch5's fork/wait/exit process management alongside virtual memory and VirtIO device drivers.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch5-pingpong.gif

## Setup

Copy the ch5 directory to create the working crate:

```bash
cp -r tg-rcore-tutorial-ch5 tg-rcore-tutorial-ch5-pingpong
rm -rf tg-rcore-tutorial-ch5-pingpong/{target,Cargo.lock,.gitrepo}
```

Then update `tg-rcore-tutorial-ch5-pingpong/Cargo.toml`:
- Change `name` to a unique crate name for publishing
- Update `authors` and `description`

**All kernel modifications go into `tg-rcore-tutorial-ch5-pingpong/`** — do NOT modify the original `tg-rcore-tutorial-ch5/` or shared crates. User-side changes go in `tg-rcore-tutorial-user/`.

## Goal

Add VirtIO-GPU framebuffer rendering, VirtIO keyboard input, and a shared memory mechanism to the ch5 process-management OS kernel. Create a two-player Pong game where **each player runs as a separate process**, communicating paddle positions and game state through kernel-managed shared memory. The game runs alongside existing ch5 test programs under stride scheduling.

## Architecture Overview — How ch5 differs from ch4

Ch5 builds on ch4's virtual memory with full **process management**: fork, exec, wait, exit, and per-process PIDs. Understanding these additions is critical.

| Feature | Ch4 | Ch5 |
|---------|-----|-----|
| Process creation | Kernel loads ELF at boot | `fork()` / `exec()` / `spawn()` at runtime |
| Process identity | None | Unique PID per process |
| Process relationship | Flat list | Parent-child tree |
| Resource cleanup | Kernel removes on exit | Parent must `wait()` to reclaim |
| Scheduling | FIFO (sequential) | **Stride scheduling** (priority-based) |
| Process manager | `Vec<Process>` | `BTreeMap<ProcId, Process>` + ready queue |
| User interaction | None | Shell + initproc bootstrap |

### Key ch5 mechanisms

- **`fork()`**: Deep copies parent's entire address space (page tables + data). Child gets independent memory. Returns child PID to parent, 0 to child.
- **`exec(path)`**: Replaces current address space with new ELF program. PID preserved.
- **`wait(pid)`/`waitpid(pid)`**: Blocks until child exits, retrieves exit code. Essential for synchronization.
- **`exit(code)`**: Terminates process, parent collects via wait.
- **`getpid()`**: Returns current process PID.
- **Stride scheduling**: Each process has `stride` and `priority`. Scheduler picks process with minimum stride, increments by `BIG_STRIDE / priority`. Higher priority = more CPU time.
- **Process tree**: `initproc` -> `user_shell` -> user programs. All via fork/exec.

### The IPC problem — why we need shared memory

Ch5 has **no inter-process communication**: no pipes (ch7), no signals (ch7), no shared memory primitives. After `fork()`, parent and child have completely independent address spaces. Changes in one are invisible to the other.

For a two-player Pong game, the two player processes need to share:
- Ball position and velocity
- Both paddle Y positions
- Score
- Game tick counter (synchronization)

**Solution: Add a kernel-managed shared memory page.** This is a minimal, educationally valuable kernel extension (~30 lines) that maps the same physical page into both parent's and child's address space at a known virtual address.

### What needs to be added

**Kernel-side:**

1. **VirtIO device drivers** — Same pattern as ch3-snake and ch4-tetris. `virtio-drivers = "0.1.0"` provides `VirtIOGpu`, `VirtIOInput`, and the `Hal` trait.

   **Hal trait**: Ch5's kernel identity-maps physical RAM (same as ch4). Use a fixed-address bump allocator for DMA, placed above the kernel heap to avoid overlap. See ch4-tetris `allocator.rs` for the exact pattern.

   **MMIO mapping**: Add VirtIO MMIO range (0x1000_0000..0x1000_9000) to `kernel_space()` as identity-mapped R+W. Same as ch4-tetris.

   **DMA pool**: Fixed address above kernel heap end. With ch5's larger process table and stride scheduling overhead, use `MEMORY = 70 MiB` (matching ch4-tetris). Place DMA at 0x8500_0000 (5 MiB).

2. **FB_INFO and FB_WRITE syscalls** (IDs 2000/2001) — Same as ch4-tetris. FB_WRITE must translate user data pointer through the process's page table. Use the ch5 process manager to get the current process's address space. The ch5 pattern for accessing the current process differs from ch4:
   - Ch4: `PROCESSES.get_mut()[0]`
   - Ch5: `PROCESSOR.current_process()` or similar (check the `processor.rs` API)

3. **Non-blocking IO::read** — Poll VirtIO keyboard `pop_pending_event()`, translate user buffer pointer. Same pattern as ch4-tetris but adapted to ch5's process manager.

4. **Shared memory syscall** (ID 2002) — `SHM_CREATE`:
   - Allocates one physical page (4 KiB) from kernel heap
   - Maps it into the **current process's address space** at a fixed virtual address (e.g., `0x3000_0000`) with U+R+W flags
   - Returns the virtual address on success, -1 on failure
   - The key feature: when the process later calls `fork()`, the kernel must map the **same physical page** (not a copy) into the child's address space at the same virtual address. Modify the `fork()` implementation to detect shared memory pages (use a custom PTE flag bit or track the shared page in the process struct) and `map_extern()` the same PPN instead of allocating a new page.

   This gives parent and child a shared memory region they can both read/write. No synchronization primitives needed — the game uses a simple tick counter and yield-based coordination.

5. **QEMU config** — Add `-device virtio-gpu-device` and `-device virtio-keyboard-device`. Replace `-nographic` with `-serial stdio`.

**User-side:**

1. **Pong game module** (`tg-rcore-tutorial-user/src/pingpong.rs`, feature-gated with `pingpong`) — Game logic, rendering, physics, input handling.

2. **One binary** (`pingpong.rs`) — Gate the body with `#[cfg(feature = "pingpong")]`.

3. **Multi-process architecture**:
   ```
   pingpong process starts
     |
     +- calls shm_create() -> gets shared memory page at 0x3000_0000
     +- initializes shared state (ball, paddles, scores, tick=0)
     +- fork()
     |    |
     |    +- Parent (player 1):
     |    |    loop { read W/S keys, update paddle1_y in shared mem,
     |    |           run ball physics, render everything,
     |    |           increment tick, yield }
     |    |
     |    +- Child (player 2):
     |         loop { read Up/Down keys, update paddle2_y in shared mem,
     |                yield }
     |
     +- Parent waits for child on game exit
   ```

4. **Rendering via `fb_write`** — Only the parent renders to avoid framebuffer conflicts. Child writes paddle position to shared memory; parent reads it and draws.

## VirtIO Keyboard — Keycode Translation

```
EV_KEY (event_type == 1), value == 1 (key press):
  W = 17          -> player 1 paddle up
  S = 31          -> player 1 paddle down
  Up arrow = 103  -> player 2 paddle up
  Down arrow = 108 -> player 2 paddle down
  Space = 57      -> start / restart after point
  Enter = 28      -> restart after game over
```

## Shared Memory Layout

The shared memory page (4 KiB at virtual address `0x3000_0000`) contains the game state:

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
    game_state: u32,  // 0=playing, 1=point_scored, 2=game_over

    // Input flags (child writes, parent reads and clears)
    p2_up: u8,
    p2_down: u8,
}
```

Parent reads `p2_up`/`p2_down` each frame, clears after reading. Child sets them based on keyboard input. No locks needed — single writer per field.

## Pong Game Design

### Field
- 1280x800 framebuffer
- Play area: ~1100x700 pixels, centered
- Top/bottom walls: solid borders (ball bounces off)
- Left/right edges: scoring zones (ball passes through = point for opponent)
- Center line: dashed vertical line

### Paddles
- Width: 12px, Height: 100px
- Left paddle: x=40, Right paddle: x=1228 (near edges)
- Movement speed: 6px per frame
- Clamped to play area bounds

### Ball
- Size: 12x12 pixels
- Initial speed: ~4px/frame, angled randomly
- Speed increases slightly after each paddle hit
- **Fixed-point physics**: multiply all positions/velocities by 256 for sub-pixel precision. Divide by 256 only when rendering. Smooth diagonal movement with integer-only math.

### Collision detection
- **Wall bounce**: ball_y touches top/bottom wall -> negate ball_vy
- **Paddle bounce**: ball_x overlaps paddle AND ball_y within paddle range -> negate ball_vx, adjust ball_vy based on hit position (top = upward, center = straight, bottom = downward)
- **Scoring**: ball_x passes left/right edge -> point for opponent, reset ball to center

### Scoring
- First to 5 points wins
- After each point: brief pause, ball resets to center
- After game over: display winner, wait for Enter to restart

### Rendering
- **Incremental**: erase old ball, draw new ball. Same for paddles. Avoid full redraws.
- **Only parent renders**: child writes paddle2_y to shared mem, parent draws everything.
- **Color scheme**: dark background, bright paddles and ball, neon score display

## Lessons Learned from ch3-snake and ch4-tetris (apply these)

1. **SBI console_getchar BLOCKS** — Never use it. Use VirtIO keyboard `pop_pending_event()`.

2. **customizable-buddy 0.0.2 crashes with large user heap in virtual memory** — Keep user heap at 16 KiB when `pingpong` feature is not active. The pong game should avoid heap allocation (use stack/static buffers only).

3. **DMA pool must not overlap kernel heap** — Place above heap end. MEMORY=70 MiB, DMA at 0x8500_0000.

4. **MMIO region needs explicit mapping in kernel_space()** — VirtIO MMIO at 0x1000_0000.

5. **FB_WRITE must translate user pointers row-by-row** — Virtual pages may not be physically contiguous.

6. **build.rs caching** — Add `cargo:rerun-if-env-changed=TG_SKIP_USER_APPS`.

7. **User stack is 8 KiB** — Keep stack allocations under ~5 KiB.

8. **Feature-gate binary bodies** — `#[cfg(feature = "pingpong")] { ... }` for `cargo publish --dry-run`.

9. **Integer-only rendering** — Fixed-point (x256) for ball physics. No floating point.

10. **Kernel stack needs 64 KiB** — VirtIO GPU init overhead.

## Shared Memory Implementation Guide

**1. Process struct** — Track the shared page:
```rust
pub struct Process {
    // ... existing fields ...
    pub shared_page: Option<PPN<Sv39>>,
}
```

**2. SHM_CREATE syscall** — Allocate and map:
```rust
const SHARED_MEM_VA: usize = 0x3000_0000;
const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;

// Allocate one zeroed physical page
let page_ptr = Sv39Manager::page_alloc::<u8>(1);
let ppn = PPN::new(page_ptr as usize >> Sv39::PAGE_BITS);

// Map into current process
process.address_space.map_extern(
    VAddr::new(SHARED_MEM_VA).floor()..VAddr::new(SHARED_MEM_VA + PAGE_SIZE).ceil(),
    ppn,
    build_flags("U_WRV"),
);
process.shared_page = Some(ppn);
```

**3. Fork modification** — Share instead of copy:
```rust
// In Process::fork(), after cloning the address space:
if let Some(ppn) = self.shared_page {
    // The clone deep-copied the shared page — unmap the copy, remap to original PPN
    child.address_space.unmap(
        VAddr::new(SHARED_MEM_VA).floor()..VAddr::new(SHARED_MEM_VA + PAGE_SIZE).ceil()
    );
    child.address_space.map_extern(
        VAddr::new(SHARED_MEM_VA).floor()..VAddr::new(SHARED_MEM_VA + PAGE_SIZE).ceil(),
        ppn,  // same physical page as parent
        build_flags("U_WRV"),
    );
    child.shared_page = Some(ppn);
}
```

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch5-pingpong/Cargo.toml` | Modify | New name, add `virtio-drivers = "0.1.0"` |
| `ch5-pingpong/.cargo/config.toml` | Modify | GPU + keyboard devices, `-serial stdio`, `TG_USER_DIR` |
| `ch5-pingpong/src/main.rs` | Modify | VirtIO init, MMIO/DMA mapping, FB/read/shm syscalls, keyboard |
| `ch5-pingpong/src/process.rs` | Modify | `shared_page` field, fork shared memory handling |
| `ch5-pingpong/src/processor.rs` | Modify (maybe) | Expose current process for FB_WRITE translation |
| `ch5-pingpong/src/allocator.rs` | Create | DMA bump allocator + Hal impl |
| `ch5-pingpong/src/virtio.rs` | Create | MMIO scanning, GPU + keyboard init |
| `ch5-pingpong/build.rs` | Modify | ch5_pingpong case_key, `--features pingpong` |
| `ch5-pingpong/test.sh` | Modify | Headless CI with timeout + GPU device |
| `user/src/pingpong.rs` | Create | Game logic, physics, rendering, multi-process |
| `user/src/lib.rs` | Modify | Add pingpong module, `shm_create()` wrapper |
| `user/Cargo.toml` | Modify | Add `pingpong` feature |
| `user/src/bin/pingpong.rs` | Create | Binary entry (feature-gated body) |
| `user/cases.toml` | Modify | Add ch5_pingpong section |

## Acceptance Criteria

1. `cargo run` opens QEMU with a playable two-player Pong game on VNC
2. Two separate processes (verified by different PIDs printed at startup)
3. Player 1 (W/S) controls left paddle, Player 2 (Up/Down) controls right paddle
4. Ball bounces off walls and paddles with angle variation
5. Scoring works, first to 5 wins, Enter restarts
6. Existing ch5 test programs still pass (serial output correct)
7. `cargo check` and `cargo publish --dry-run` pass
8. Score displayed on screen

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine multi-process architecture, shared memory design, game physics. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task
4. `superpowers:requesting-code-review` — final review

Also load `frontend-design:frontend-design` for the visual design phase.

Do NOT publish the crate — the user will review and publish manually.

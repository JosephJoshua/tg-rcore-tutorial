Extend `tg-rcore-tutorial-ch7` with a user-space simplified Pac-Man game rendered via VirtIO-GPU, with VirtIO keyboard input from VNC. The game uses ch7's **pipes** for inter-process communication between a game coordinator and ghost AI processes, and **signals** for game events (ghost collision, power pellet timeout), demonstrating ch7's IPC and signal handling alongside filesystem, virtual memory, and VirtIO device drivers.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch7-pacman.gif

## Setup

Copy the ch7 directory to create the working crate:

```bash
cp -r tg-rcore-tutorial-ch7 tg-rcore-tutorial-ch7-pacman
rm -rf tg-rcore-tutorial-ch7-pacman/{target,Cargo.lock,.gitrepo}
```

Then update `tg-rcore-tutorial-ch7-pacman/Cargo.toml`:
- Change `name` to a unique crate name for publishing
- Update `authors` and `description`

**All kernel modifications go into `tg-rcore-tutorial-ch7-pacman/`** — do NOT modify the original `tg-rcore-tutorial-ch7/` or shared crates. User-side changes go in `tg-rcore-tutorial-user/`.

## Goal

Add VirtIO-GPU framebuffer rendering and VirtIO keyboard input to the ch7 IPC/signal OS kernel. Create a simplified Pac-Man game where **ghost AI runs as separate child processes** communicating with the parent game process through **pipes**, and **signals** are used for asynchronous game events (e.g., ghost-kills-pacman notification, power pellet timer expiry). The game also uses ch6's **filesystem** to save/load high scores.

## Architecture Overview — How ch7 differs from ch6

Ch7 adds **pipes** (inter-process byte streams) and **signals** (asynchronous event notification) to the ch6 filesystem OS. These are the core IPC primitives that make multi-process game architectures possible.

| Feature | Ch6 | Ch7 |
|---------|-----|-----|
| IPC | None | Pipes (unidirectional byte streams) |
| Async events | None | Signals (kill/sigaction/sigprocmask/sigreturn) |
| fd types | FileHandle only | `Fd` enum: File, PipeRead, PipeWrite, Empty |
| Signal handling | None | Per-process signal actions + mask + handling state |
| Dependencies | No signal libs | tg-signal + tg-signal-impl |
| File I/O | open/read/write/close | Same, plus pipe() syscall |

### Key ch7 mechanisms

- **`pipe()`**: Creates a unidirectional byte stream. Returns two file descriptors: `pipe[0]` (read end) and `pipe[1]` (write end). Data written to `pipe[1]` can be read from `pipe[0]`. Used for parent-child communication.
- **`Fd` enum**: Unified file descriptor type — `File(FileHandle)`, `PipeRead(PipeReader)`, `PipeWrite(Arc<PipeWriter>)`, `Empty { read, write }`. Makes read/write syscalls polymorphic.
- **`kill(pid, signum)`**: Send a signal to a process. Used for async event notification.
- **`sigaction(signum, action, old_action)`**: Register a signal handler function for a given signal number.
- **`sigprocmask(mask)`**: Block/unblock signals.
- **`sigreturn()`**: Return from a signal handler.
- **Signal handling flow**: After each syscall return, kernel checks for pending unmasked signals. If found, saves user context, jumps to handler. Handler calls `sigreturn()` to restore.
- **Pipe semantics**: Read blocks if pipe is empty and write end is open. Returns 0 (EOF) if write end is closed. Write blocks if buffer is full. Returns error if read end is closed.
- **Fork inherits fd_table and signals**: Child gets copies of all open file descriptors (including pipe ends) and signal configuration.

### The IPC design — pipes for ghost AI, signals for events

Ch7 enables a sophisticated multi-process game architecture:

1. **Parent process** (game coordinator): owns the game state, renders the screen, handles player input, runs game logic.
2. **Ghost child processes** (2-4 ghosts): each runs an independent AI loop. Receives game state (Pac-Man position, maze) from parent via pipe. Sends movement decisions back via another pipe.
3. **Signals for events**:
   - Parent sends `SIGUSR1` to ghosts when power pellet is eaten (ghosts enter "frightened" mode).
   - Ghost sends `SIGUSR2` to parent when it catches Pac-Man (life lost).
   - These demonstrate the asynchronous nature of signals — the recipient handles them at the next syscall return.

**Pipe communication protocol**:
```
Parent -> Ghost pipe (per ghost):
  [pacman_x: u8, pacman_y: u8, ghost_x: u8, ghost_y: u8, game_state: u8]
  (5 bytes per tick)

Ghost -> Parent pipe (per ghost):
  [direction: u8]
  (1 byte per tick: 0=up, 1=right, 2=down, 3=left)
```

The parent writes updated positions to each ghost's input pipe each tick. Each ghost reads its pipe, runs its AI algorithm, and writes its chosen direction back. This creates a clean producer-consumer IPC pattern.

### What needs to be added

**Kernel-side:**

1. **VirtIO-GPU and keyboard drivers** — Same pattern as ch6-breakout. Ch7 already has VirtIO-blk at MMIO slot 0x10001000 with a `VirtioHal` in `virtio_block.rs`. Extend MMIO scanning for GPU and keyboard. Reuse the existing Hal (kernel heap DMA). Expand MMIO mapping from `(0x1000_1000, 0x1000)` to `(0x1000_0000, 0x9000)`.

   **IMPORTANT**: Use `virtio-drivers = "0.3.0"` (not 0.1.0) — v0.1.0 has a bug where `VirtIOInput::pop_pending_event()` never notifies the device after recycling buffers, causing keyboard input to stop after 32 events.

2. **FB_INFO (2000), FB_WRITE (2001), FB_FLUSH (2003) syscalls** — Same as ch5-pingpong and ch6-breakout. FB_WRITE must translate user data pointer through the process's page table row-by-row. FB_FLUSH triggers GPU display update (decoupled from pixel writes for VNC performance). Use `PROCESSOR.get_mut().current().unwrap()` for process access.

3. **Non-blocking IO::read for keyboard** — Ch7's `IO::read` handles multiple fd types via the `Fd` enum. For fd 0 (STDIN/Empty), replace blocking `sbi::console_getchar()` with VirtIO keyboard polling. Return both key press (keycode) and release (keycode|0x80) events, draining sync events. The game needs held-key state tracking for smooth movement. **Do NOT break pipe reads** — only change the STDIN/Empty path, not PipeRead.

4. **QEMU config** — Add `-device virtio-gpu-device -device virtio-keyboard-device -vnc :0 -serial stdio`. Keep the existing `-drive file=fs.img,... -device virtio-blk-device,...` for filesystem.

5. **MEMORY increase** — Increase to 70 MiB for GPU framebuffer + process overhead.

6. **DMA pool mapping** — Add identity mapping for DMA pool region (0x8500_0000..0x8550_0000) in `kernel_space()`. Also ensure VirtIO MMIO range covers GPU/keyboard slots.

**User-side:**

1. **Pac-Man game module** (`tg-rcore-tutorial-user/src/pacman.rs`, feature-gated with `pacman`) — Game logic, maze, rendering, ghost AI, pipe IPC, signal handling.

2. **One binary** (`pacman.rs`) — Gate the body with `#[cfg(feature = "pacman")]`.

3. **Multi-process architecture**:
   ```
   pacman process starts
     |
     +- creates pipes (2 per ghost: parent->ghost, ghost->parent)
     +- fork() for each ghost (2-4 ghosts)
     |    |
     |    +- Ghost child:
     |         - close unused pipe ends
     |         - register SIGUSR1 handler (power pellet -> frightened mode)
     |         - loop: read game state from pipe, compute AI move, write direction to pipe
     |
     +- Parent (game coordinator):
          - close unused pipe ends
          - register SIGUSR2 handler (ghost caught pacman -> lose life)
          - loop: read keyboard, move pacman, write state to ghost pipes,
                  read ghost directions from pipes, update ghost positions,
                  check collisions, render, fb_flush
   ```

4. **Ghost AI** — Simple chase/scatter behavior:
   - **Chase mode**: move toward Pac-Man (pick direction that minimizes Manhattan distance)
   - **Frightened mode** (after power pellet): move randomly for N ticks
   - **Scatter mode**: move toward assigned corner every M ticks
   - Each ghost has slightly different behavior (different target corners, different chase offsets)

5. **Save/load high scores** — Use open/write/read/close to persist the top score to a file.

## VirtIO Keyboard — Keycode Translation

```
EV_KEY (event_type == 1):
  value == 1 (press) -> keycode
  value == 0 (release) -> keycode | 0x80

Keycodes:
  W = 17, Up = 103     -> move up
  A = 30, Left = 105   -> move left
  S = 31, Down = 108   -> move down
  D = 32, Right = 106  -> move right
  Space = 57            -> start / pause
  Enter = 28            -> restart after game over
  F5 = 63               -> save high score
```

## Pac-Man Game Design

### Maze
- 1280x800 framebuffer
- Grid-based maze: 28 columns x 31 rows (classic Pac-Man proportions)
- Cell size: 24x24 pixels
- Maze area: 672x744 pixels, centered horizontally
- Maze defined as a static byte array: 0=wall, 1=dot, 2=power pellet, 3=empty path, 4=ghost house
- Walls rendered as dark blue, paths as black, dots as small white squares (4x4), power pellets as larger circles (8x8, flashing)

### Pac-Man
- Size: 20x20 pixels (fits in 24x24 cell)
- Yellow color
- Moves one cell per N ticks (grid-snapped movement)
- Direction queued from keyboard input, applied at next grid intersection
- Eats dots/pellets on contact

### Ghosts (2-4)
- Size: 20x20 pixels
- Different colors: red (Blinky), pink (Pinky), cyan (Inky), orange (Clyde)
- Each runs as a **separate child process**
- Movement via pipe IPC: parent sends state, ghost sends direction
- AI behavior cycles between Chase and Scatter modes

### Dots and Power Pellets
- Small dots: 4x4 white, 10 points each
- Power pellets: 8x8, placed at 4 corners, 50 points each
- Eating a power pellet: parent sends SIGUSR1 to all ghost children. Ghosts enter frightened mode (random movement, blue color) for ~5 seconds.
- Eating a frightened ghost: 200 points, ghost resets to ghost house

### Collision Detection
- Grid-based: check if Pac-Man and ghost occupy the same cell
- Normal ghost: Pac-Man loses a life (ghost sends SIGUSR2 to parent)
- Frightened ghost: ghost is eaten (200 points, ghost respawns)

### Lives & Scoring
- 3 lives
- Dots: 10 points, Power pellets: 50 points, Ghosts: 200 points
- All dots cleared: next level (reset dots, increase ghost speed, keep score)
- All lives lost: game over, save high score if new record
- High score saved to filesystem via open/write/close

### Signal Usage

| Signal | Sender | Receiver | Purpose |
|--------|--------|----------|---------|
| SIGUSR1 (10) | Parent | All ghosts | Power pellet eaten — enter frightened mode |
| SIGUSR2 (12) | Ghost | Parent | Ghost caught Pac-Man — lose a life |

Ghost signal handler for SIGUSR1:
```rust
// In ghost child process:
fn handle_sigusr1() {
    // Set a flag that the ghost AI loop checks
    unsafe { FRIGHTENED = true; }
    unsafe { FRIGHTENED_TIMER = 300; } // ~5 seconds worth of ticks
}
sigaction(10, &SignalAction { handler: handle_sigusr1 as usize, mask: 0 });
```

Parent signal handler for SIGUSR2:
```rust
fn handle_sigusr2() {
    unsafe { LIFE_LOST = true; }
}
sigaction(12, &SignalAction { handler: handle_sigusr2 as usize, mask: 0 });
```

### Pipe Communication Detail

Per ghost, two pipes:
```
parent_to_ghost: pipe() -> [read_fd, write_fd]
  - Parent keeps write_fd, closes read_fd
  - Ghost keeps read_fd, closes write_fd

ghost_to_parent: pipe() -> [read_fd, write_fd]
  - Ghost keeps write_fd, closes read_fd
  - Parent keeps read_fd, closes write_fd
```

After fork, each process closes the pipe ends it doesn't use. This ensures EOF detection works correctly (when parent closes write end, ghost's read returns 0).

Each tick:
1. Parent writes 5 bytes to each ghost's input pipe: `[pac_x, pac_y, ghost_x, ghost_y, mode]`
2. Parent reads 1 byte from each ghost's output pipe: `[direction]`
3. Ghost reads 5 bytes from its input pipe, runs AI, writes 1 byte to its output pipe

### Rendering
- **Incremental**: only redraw changed cells (eaten dots, moved characters)
- **Full redraw on level start**: draw entire maze, all dots, characters
- **Color scheme**: Neon arcade style (dark background, bright characters)
  - Walls: deep blue with glow (#0000AA with #000044 glow)
  - Dots: warm white
  - Power pellets: bright white, flashing
  - Pac-Man: neon yellow
  - Blinky: neon red, Pinky: neon pink, Inky: neon cyan, Clyde: neon orange
  - Frightened ghosts: blue with white
  - Score/lives: neon green HUD at top
- **Maze rendering**: each wall cell drawn as a filled rectangle. Path cells drawn as black.

### Simplified Maze Layout (28x31)

Use a simplified maze that captures the classic feel:
- Outer walls, internal corridors
- Ghost house in center (4x2 cells with a gate)
- 4 power pellets near corners
- Symmetrical left-right

Encode as a static `[u8; 28*31]` array where each byte is the cell type.

## Lessons Learned from ch1-ch6 (apply ALL of these)

1. **virtio-drivers 0.3.0, not 0.1.0** — v0.1.0 has a confirmed bug where `VirtIOInput::pop_pending_event()` doesn't notify the device after recycling buffers. Keyboard stops after 32 events. Fixed in v0.3.0 (commit b559de30). Update the Hal trait impl for the new API (`BufferDirection`, `share`/`unshare`/`mmio_phys_to_virt`).

2. **FB_WRITE must NOT flush GPU** — Decouple pixel writes from GPU flush. Add a separate FB_FLUSH syscall (ID 2003). Game calls `fb_flush()` once per frame after all draws. Without this, VNC gets thousands of partial updates per frame and displays garbage.

3. **DMA pool needs explicit mapping** — Map 0x8500_0000..0x8550_0000 in `kernel_space()`. Without this, GPU init hangs because the framebuffer is allocated in unmapped DMA memory.

4. **Keyboard: return press AND release, drain sync events** — Return keycode for press, keycode|0x80 for release. Loop to drain EV_SYN events. Game tracks held-key state via `KeyState` struct for smooth movement.

5. **Single keyboard reader** — Only one process should read the VirtIO keyboard (there's one event queue). Parent reads all input, dispatches to children via pipes/shared memory.

6. **IO::read fd=0 must stay blocking for shell** — Use fd 0 (STDIN) for blocking SBI console reads (shell needs this). Use fd 3 (STDIN_BUFFERED) for non-blocking VirtIO keyboard (game uses this). Don't break the fd dispatch for pipes (fd 3+ might be pipe fds — check the Fd enum type).

7. **MEMORY = 70 MiB** — Large kernel image (embedded user programs) + GPU framebuffer + process page tables.

8. **-vnc :0 in QEMU args** — Required for VNC keyboard input routing to VirtIO keyboard device.

9. **customizable-buddy 0.0.2 user heap crash** — Keep 16 KiB heap when no game feature active. Game should avoid heap allocation (use stack buffers, static arrays).

10. **Feature-gate binary bodies** — `#[cfg(feature = "pacman")]` for `cargo publish --dry-run`.

11. **Integer-only physics** — Grid-based movement (no sub-pixel needed for Pac-Man, unlike ball physics).

12. **User stack 8 KiB** — Keep stack allocations under ~5 KiB. Use 16x16 tile buffer (1024 bytes) for `fill_rect`.

13. **File paths null-terminated** — `open("pacman_hiscore\0", flags)`.

14. **build.rs caching** — Add `cargo:rerun-if-env-changed=TG_SKIP_USER_APPS`.

15. **Pipe reads can block** — Ghost processes call `read(pipe_fd, ...)` which blocks until parent writes data. This is correct and desirable for synchronization. Use `pipe_read`/`pipe_write` helpers from user lib that handle partial reads.

16. **Signal handlers run in user space** — The handler function address must be in the user's address space. Use a simple function that sets a global flag; the main loop checks the flag.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch7-pacman/Cargo.toml` | Modify | New name, `virtio-drivers = "0.3.0"` |
| `ch7-pacman/.cargo/config.toml` | Modify | Add GPU + keyboard + VNC, `-serial stdio`, `TG_USER_DIR` |
| `ch7-pacman/src/main.rs` | Modify | GPU/keyboard init, FB/FLUSH syscalls, keyboard IO::read (preserve pipe reads) |
| `ch7-pacman/src/virtio_block.rs` | Modify | Extend MMIO scan for GPU/keyboard, or add separate virtio module |
| `ch7-pacman/build.rs` | Modify | ch7_pacman case_key, `--features pacman` |
| `ch7-pacman/test.sh` | Modify | Headless CI with GPU + block device |
| `user/src/pacman.rs` | Create | Game logic, maze, rendering, ghost AI, pipe IPC, signals |
| `user/src/lib.rs` | Modify | Add pacman module |
| `user/Cargo.toml` | Modify | Add `pacman` feature |
| `user/src/bin/pacman.rs` | Create | Binary entry (feature-gated body) |
| `user/cases.toml` | Modify | Add ch7_pacman section |

## Acceptance Criteria

1. `cargo run` opens QEMU with a playable simplified Pac-Man game on VNC
2. Pac-Man moves with WASD/arrow keys, eats dots and power pellets
3. 2-4 ghost child processes communicate via pipes (verified by PID prints at startup)
4. Ghosts chase Pac-Man using AI, with different behaviors
5. Power pellet triggers SIGUSR1 to ghosts — they turn blue/frightened
6. Ghost-Pac-Man collision triggers SIGUSR2 to parent — life lost
7. Score, lives, level displayed on screen
8. High score saved to filesystem (persists across restarts)
9. Game over when all lives lost, Enter restarts
10. Level progression when all dots cleared
11. Existing ch7 test programs still pass (serial output correct)
12. `cargo check` and `cargo publish --dry-run` pass

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine game design, pipe protocol, signal usage, ghost AI. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task
4. `superpowers:requesting-code-review` — final review

Also load `frontend-design:frontend-design` for the visual design phase.

Do NOT publish the crate — the user will review and publish manually.

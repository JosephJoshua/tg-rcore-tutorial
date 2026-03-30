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
- Grid: 21 columns x 21 rows (simplified from classic 28x31 — still fun, less implementation)
- Cell size: 28x28 pixels
- Maze area: 588x588 pixels, centered in the left 2/3 of the screen
- Right 1/3: HUD panel (score, high score, lives, level, ghost status)
- Maze defined as `static MAZE: [u8; 21*21]` — cell types: `W`=wall, `D`=dot, `P`=power pellet, `E`=empty, `G`=ghost house, `T`=tunnel entrance

**Concrete maze layout** (21x21, symmetric, fun corridors):
```
WWWWWWWWWWWWWWWWWWWWW
WDDDDDDDDDWDDDDDDDDDW  <- note: W=wall wraps left/right edges
WDWWWDWWWDWDWWWDWWWDW
WPWWWDWWWDWDWWWDWWWPW  <- P = power pellets in corners
WDDDDDDDDDDDDDDDDDDDW
WDWWWDWDWWWWWDWDWWWDW
WDDDDDWDDDWDDDWDDDDDW
WWWWWDWWWEWDWWWDWWWWW
EEEEEWDEEEEEEDWEEEEEE  <- tunnel row (wraps left<->right)
WWWWWDWEWGGEWDWDWWWWW  <- G = ghost house (center)
TEEEEDEEGEGEDEEDEEEET  <- T = tunnel exits
WWWWWDWEWGGEWDWDWWWWW
EEEEEWDEEEEEEDWEEEEEE  <- second tunnel row
WWWWWDWDWWWWWDWDWWWWW
WDDDDDDDDDWDDDDDDDDDW
WDWWWDWWWDWDWWWDWWWDW
WPDDWDDDDDEDDDDWDDPW  <- E in center = Pac-Man start
WWDWWDWDWWWWWDWDWWDWW
WDDDDDWDDDWDDDWDDDDDW
WDWWWWWWWDWDWWWWWWWDW
WDDDDDDDDDDDDDDDDDDDW
WWWWWWWWWWWWWWWWWWWWW
```

This is a schematic — the actual byte array encodes cell types numerically. Key features:
- **Two tunnel passages** (rows 9 and 13): Pac-Man/ghosts exiting left reappear on right (and vice versa). Ghosts slow to half speed in tunnels.
- **Ghost house** (center, 4x2 cells): ghosts spawn here. A gate cell at the top center lets ghosts exit one at a time.
- **4 power pellets** in near-corner positions
- **Pac-Man starts** at the center below the ghost house
- **Symmetrical** left-right for fair gameplay

### Pac-Man Movement

- **Grid-snapped**: Pac-Man is always aligned to cell centers. Movement is one full cell per game tick.
- **Direction queuing**: player presses a direction key, it's stored as `queued_dir`. At each tick, if Pac-Man can move in `queued_dir` (no wall), adopt it as `current_dir`. Otherwise keep moving in `current_dir`. If `current_dir` is also blocked (wall), stop.
- **Cornering feel**: this input buffering is what makes classic Pac-Man feel responsive — you can pre-press a turn before reaching the intersection.
- **Speed**: 1 cell per tick. Tick rate ~8 ticks/second (using `get_time()` with 125ms interval). This gives a comfortable pace.
- **Tunnel wrap**: when Pac-Man's x goes below 0, wrap to column 20. Above 20, wrap to column 0. Only on tunnel rows.

### Ghosts — 4 with Distinct Personalities

Each ghost runs as a **child process** and has a unique AI that makes the game feel dynamic and unpredictable:

| Ghost | Color | Name | Chase Target | Personality |
|-------|-------|------|-------------|-------------|
| 0 | Red | Blinky | Pac-Man's current cell | **Aggressive chaser**. Always targets Pac-Man directly. Gets faster when few dots remain ("Cruise Elroy" behavior: when <20 dots left, moves every tick instead of every other tick). |
| 1 | Pink | Pinky | 4 cells ahead of Pac-Man | **Ambusher**. Targets the cell 4 positions ahead of Pac-Man in Pac-Man's current direction. Often cuts off escape routes. |
| 2 | Cyan | Inky | Complex offset from Blinky | **Unpredictable**. Target = take the cell 2 ahead of Pac-Man, draw a vector from Blinky to that cell, double it. This creates erratic movement that's hard to predict. In the simplified version: alternate between chasing Pac-Man and running to a random dot. |
| 3 | Orange | Clyde | Pac-Man or corner | **Shy**. Chases Pac-Man when far away (>8 cells Manhattan distance), but runs to his scatter corner when close. Creates a push-pull dynamic. |

**Ghost AI modes** (cycle during gameplay):

1. **Scatter** (7 seconds): each ghost targets its assigned corner. Gives the player breathing room.
   - Blinky: top-right, Pinky: top-left, Inky: bottom-right, Clyde: bottom-left
2. **Chase** (20 seconds): each ghost uses its unique targeting algorithm.
3. **Repeat**: scatter → chase → scatter → chase, with scatter periods getting shorter each cycle (7s, 7s, 5s, 5s, then permanent chase)

**Frightened mode** (triggered by power pellet via SIGUSR1):
- All ghosts reverse direction immediately
- Move randomly at intersections (pick a random valid direction)
- Move at half speed (skip every other tick)
- Duration: 6 seconds on level 1, decreasing by 1 second per level, minimum 1 second
- When time is almost up (last 2 seconds): ghosts flash white/blue alternately (warning)
- If eaten: ghost "eyes" return to ghost house at double speed, then respawn

**Ghost house release timing** (staggered):
- Blinky: starts outside, immediately active
- Pinky: released after 3 seconds
- Inky: released after 10 seconds (or when 30 dots eaten)
- Clyde: released after 20 seconds (or when 80 dots eaten)

**Ghost pathfinding**: at each intersection, ghost picks the direction (excluding reverse of current direction) that puts it closest to its target cell (Manhattan distance). If tied, preference order: up > left > down > right. This is the classic Pac-Man pathfinding and creates predictable-but-complex emergent behavior.

### Dots, Power Pellets, and Fruit

- **Dots**: 4x4 bright pixels at cell center. ~180 dots total. 10 points each. Pac-Man eats on contact (same cell).
- **Power pellets**: 10x10 pixels, pulsing (drawn every other half-second for a blink effect). 4 placed near corners. 50 points each. Triggers SIGUSR1 to all ghosts on eat.
- **Fruit bonus**: a bonus item appears at the center (Pac-Man's start position) twice per level — once after 70 dots eaten, again after 170 dots eaten. Stays for 10 seconds, then disappears.
  - Level 1: cherry (100 pts), Level 2: strawberry (300 pts), Level 3+: orange (500 pts)
  - Rendered as a colored square with a small stem (simple pixel art)
- **Ghost eat combo**: eating multiple ghosts during one power pellet gives escalating points: 200, 400, 800, 1600. Resets when power pellet expires.

### Collision Detection

- **Grid-based**: every tick, check if Pac-Man and any ghost occupy the same cell (or swapped cells — moving through each other)
- **Normal ghost → Pac-Man**: life lost. Freeze for 1 second (death animation: Pac-Man shrinks). All ghosts reset to ghost house. Pac-Man respawns at start. Dots are NOT reset.
- **Frightened ghost → Pac-Man**: ghost eaten. Show points at that location for 0.5 seconds. Ghost eyes return to ghost house. Ghost respawns in house after 3 seconds.
- **Collision is checked by parent process** (it knows all positions). Parent sends SIGUSR1 (power pellet) proactively. For ghost-catches-Pac-Man, parent detects it directly (no SIGUSR2 needed from ghost — simpler than having the ghost detect it, since the parent has all the state).

Note: the original prompt suggested SIGUSR2 from ghost→parent for collisions, but since the parent knows all positions, it's cleaner to detect collisions in the parent and use signals only for the power pellet broadcast. This still demonstrates both `kill()` (parent→ghosts) and signal handlers in the ghosts.

### Lives, Scoring, and Levels

- **3 lives** (can earn extra life at 10,000 points)
- **Score display**: top of HUD panel, large 7-segment digits
- **High score**: displayed below current score. Loaded from filesystem at game start, saved when beaten.
- **Level progression**:
  - All dots + power pellets eaten → level complete
  - Brief celebration flash (maze flashes white 3 times over 2 seconds)
  - Reset dots and power pellets, keep score and lives
  - Ghost speed increases: base tick interval decreases by 10ms per level (125ms → 115ms → 105ms, minimum 70ms)
  - Frightened duration decreases: 6s → 5s → 4s → 3s → 2s → 1s
  - Ghosts are released from house faster each level
- **Game over**: when all lives lost, display "GAME OVER" in center of maze for 3 seconds. If score > high score, save to filesystem. Show final score. Any key restarts.

### HUD Panel (right side)

Positioned to the right of the maze (x ≈ 700..1200):
- **"SCORE"** label + large score digits
- **"HIGH"** label + high score digits
- **Level number**
- **Lives remaining**: shown as small Pac-Man icons (yellow circles)
- **Current fruit icon** for the level
- **Ghost status indicators**: colored dots showing which ghosts are active/frightened/eaten

### Game States

| State | Description | Transitions |
|-------|-------------|-------------|
| READY | "READY!" text in maze center, 2-second countdown | → PLAYING (auto) |
| PLAYING | Normal gameplay, all systems active | → DYING, LEVEL_COMPLETE, GAME_OVER |
| DYING | Pac-Man death animation (1 second), freeze all | → PLAYING (if lives > 0), GAME_OVER |
| LEVEL_COMPLETE | Maze flashes (2 seconds) | → READY (next level) |
| GAME_OVER | "GAME OVER" display, save high score | → READY (any key) |
| PAUSED | Freeze all, "PAUSED" text | → PLAYING (Space) |

### Signal Usage

| Signal | Sender | Receiver | Purpose |
|--------|--------|----------|---------|
| SIGUSR1 (10) | Parent | All ghosts | Power pellet eaten — enter frightened mode |
| SIGUSR2 (12) | Parent | Specific ghost | Ghost was eaten — return to ghost house |

Collisions are detected by the parent (it knows all positions). Signals flow parent→ghost only, which is simpler and avoids race conditions.

Ghost signal handler for SIGUSR1 (power pellet):
```rust
// In ghost child process:
static mut FRIGHTENED_TICKS: u32 = 0;
fn handle_power_pellet() {
    unsafe { FRIGHTENED_TICKS = 48; } // ~6 seconds at 8 ticks/sec
}
sigaction(10, &SignalAction { handler: handle_power_pellet as usize, mask: 0 });
```

Ghost signal handler for SIGUSR2 (eaten — return to house):
```rust
static mut RESPAWNING: bool = false;
fn handle_eaten() {
    unsafe { RESPAWNING = true; }
}
sigaction(12, &SignalAction { handler: handle_eaten as usize, mask: 0 });
```

The ghost's main loop checks these flags:
- If `FRIGHTENED_TICKS > 0`: use random movement, decrement counter each tick
- If `RESPAWNING`: target ghost house, move at double speed, ignore walls on the path back. Once home, wait 3 seconds, then respawn.

### Pipe Communication Detail

Per ghost, two pipes (8 pipe fds per ghost × 4 ghosts = 32 fds, well within fd_table capacity):

```
parent_to_ghost: pipe() -> [read_fd, write_fd]
  - Parent keeps write_fd, closes read_fd
  - Ghost keeps read_fd, closes write_fd

ghost_to_parent: pipe() -> [read_fd, write_fd]
  - Ghost keeps write_fd, closes read_fd
  - Parent keeps read_fd, closes write_fd
```

After fork, each process closes the pipe ends it doesn't use. This ensures EOF detection works (when parent exits, ghost's read returns 0 → ghost exits cleanly).

**Per-tick protocol** (lockstep synchronization):

```rust
// Parent sends to each ghost (7 bytes):
struct GhostInput {
    pac_x: u8,        // Pac-Man grid column
    pac_y: u8,        // Pac-Man grid row
    pac_dir: u8,      // Pac-Man direction (for Pinky's look-ahead)
    ghost_x: u8,      // This ghost's grid column
    ghost_y: u8,      // This ghost's grid row
    blinky_x: u8,     // Blinky's position (for Inky's algorithm)
    blinky_y: u8,     // Blinky's position
}

// Ghost responds (1 byte):
//   0=up, 1=right, 2=down, 3=left
```

Tick flow:
1. Parent updates game state (move Pac-Man, check dots/collisions)
2. Parent writes 7 bytes to each ghost's input pipe
3. Parent reads 1 byte from each ghost's output pipe (**blocks until ghost responds** — natural synchronization)
4. Parent applies ghost movements, renders frame, flushes
5. Each ghost: reads 7 bytes (**blocks until parent writes**), runs AI, writes 1 byte direction

This creates a clean lockstep: parent and ghosts alternate in sync. No need for shared memory or explicit synchronization. The pipe's blocking read/write IS the synchronization mechanism.

### Visual Design — "Warm Arcade Phosphor"

A deliberate departure from the cold neon palette used in ch4-ch6. Pac-Man's visuals should evoke the warm, saturated glow of a real CRT arcade cabinet — rich blues, golden yellows, and candy-bright characters that feel like they're burning into phosphor.

**BGRA Color Palette** (all colors in BGRA byte order for `fill_rect`):

```
// ─── Environment ───
BG_VOID:      [0x00, 0x00, 0x00, 0xFF]  // #000000 true black outside maze
PATH_COLOR:   [0x08, 0x08, 0x08, 0xFF]  // #080808 near-black paths (not pure black — slight warmth)
WALL_FACE:    [0xDE, 0x21, 0x21, 0xFF]  // #2121DE rich cobalt blue (classic Pac-Man)
WALL_EDGE:    [0xFF, 0x52, 0x52, 0xFF]  // #5252FF brighter blue edge highlight
WALL_SHADOW:  [0x80, 0x10, 0x10, 0xFF]  // #1010C0 dark blue shadow
GATE_COLOR:   [0xB4, 0x69, 0xFF, 0xFF]  // #FF69B4 hot pink ghost house gate

// ─── Collectibles ───
DOT_COLOR:    [0x97, 0xB8, 0xFF, 0xFF]  // #FFB897 warm peach-salmon dots
PELLET_COLOR: [0xD0, 0xFF, 0xFF, 0xFF]  // #FFFFD0 warm white power pellets
PELLET_GLOW:  [0x60, 0x80, 0x80, 0xFF]  // #808060 subtle pellet aura
FRUIT_CHERRY: [0x33, 0x33, 0xFF, 0xFF]  // #FF3333 cherry red
FRUIT_STRAW:  [0x55, 0x88, 0xFF, 0xFF]  // #FF8855 strawberry
FRUIT_ORANGE: [0x22, 0xAA, 0xFF, 0xFF]  // #FFAA22 orange

// ─── Pac-Man ───
PAC_BODY:     [0x00, 0xD7, 0xFF, 0xFF]  // #FFD700 rich golden yellow
PAC_HIGHLIGHT:[0x44, 0xEE, 0xFF, 0xFF]  // #FFEE44 lighter center highlight
PAC_DEATH1:   [0x00, 0xAA, 0xFF, 0xFF]  // #FFAA00 orange (dying phase 1)
PAC_DEATH2:   [0x00, 0x44, 0xFF, 0xFF]  // #FF4400 red-orange (dying phase 2)

// ─── Ghosts ───
BLINKY_BODY:  [0x00, 0x00, 0xFF, 0xFF]  // #FF0000 true red
BLINKY_LIGHT: [0x44, 0x44, 0xFF, 0xFF]  // #FF4444 lighter red highlight
PINKY_BODY:   [0xC0, 0x80, 0xFF, 0xFF]  // #FF80C0 candy pink
PINKY_LIGHT:  [0xDD, 0xAA, 0xFF, 0xFF]  // #FFAADD lighter pink
INKY_BODY:    [0xDE, 0xFF, 0x00, 0xFF]  // #00FFDE electric cyan
INKY_LIGHT:   [0xEE, 0xFF, 0x66, 0xFF]  // #66FFEE lighter cyan
CLYDE_BODY:   [0x52, 0xB8, 0xFF, 0xFF]  // #FFB852 warm amber orange
CLYDE_LIGHT:  [0x88, 0xDD, 0xFF, 0xFF]  // #FFDD88 lighter orange
GHOST_EYE_W:  [0xFF, 0xFF, 0xFF, 0xFF]  // #FFFFFF white eye outer
GHOST_EYE_B:  [0x88, 0x22, 0x22, 0xFF]  // #222288 dark blue pupil (looks toward movement dir)
FRIGHT_BODY:  [0xB8, 0x18, 0x18, 0xFF]  // #1818B8 deep indigo
FRIGHT_FACE:  [0xFF, 0xFF, 0xFF, 0xFF]  // #FFFFFF white eyes+mouth in fright
FRIGHT_FLASH: [0xEE, 0xEE, 0xEE, 0xFF]  // #EEEEEE near-white (flash phase)

// ─── HUD ───
HUD_BG:       [0x0A, 0x06, 0x04, 0xFF]  // #04060A very deep blue-black panel
HUD_LABEL:    [0x88, 0x88, 0x88, 0xFF]  // #888888 muted gray labels
HUD_SCORE:    [0xD0, 0xFF, 0xFF, 0xFF]  // #FFFFD0 warm cream score digits
HUD_HISCORE:  [0x66, 0xFF, 0x00, 0xFF]  // #00FF66 green high score
HUD_LIVES:    [0x00, 0xD7, 0xFF, 0xFF]  // #FFD700 same gold as Pac-Man
```

**Wall rendering — beveled 3D channels**:

Each wall cell (28x28) is drawn with a beveled effect to create the classic "maze of light" look:
```
For each wall cell at (cx, cy):
  1. Fill entire cell with WALL_SHADOW (dark blue base)
  2. Check which edges face a path/dot cell:
     - If neighbor above is path: draw 2px WALL_EDGE strip along top
     - If neighbor below is path: draw 2px WALL_EDGE strip along bottom
     - If neighbor left is path: draw 2px WALL_EDGE strip along left
     - If neighbor right is path: draw 2px WALL_EDGE strip along right
  3. Fill interior (inset 2px from edges) with WALL_FACE
```
This creates glowing blue walls where only the edges facing open corridors are bright, making the maze look like luminous channels carved into darkness. Interior walls with no path neighbors render as solid dark blue.

**Character rendering**:

Each character occupies a 20x20 area centered in a 28x28 cell (4px padding each side).

*Pac-Man* (20x20):
```
Fill 20x20 with PAC_BODY (golden yellow)
Fill inner 12x12 centered with PAC_HIGHLIGHT (brighter yellow)
```
No mouth animation needed — the highlight creates visual interest. When moving, a 2px darker "shadow" trail on the trailing edge sells the motion.

*Ghosts* (20x20):
```
Fill 20x20 with ghost body color (BLINKY_BODY etc.)
Fill top 12x12 centered with ghost light color (highlight — gives roundedness)
Draw eyes: two 6x6 white rects at (4,4) and (12,4)
Draw pupils: two 3x3 dark rects inside eyes, offset toward movement direction
  - Moving right: pupils at right edge of eyes
  - Moving left: pupils at left edge
  - Moving up: pupils at top edge
  - Moving down: pupils at bottom edge
Bottom edge: draw 3 "bumps" (alternating 4px strips of body color and darker shade)
  to suggest the wavy ghost skirt
```

*Frightened ghost* (20x20):
```
Fill 20x20 with FRIGHT_BODY (deep indigo)
Draw two 4x4 FRIGHT_FACE white eye dots
Draw wavy "mouth" — 6 alternating 2x2 white dots in a zigzag at y=14
```
During flash phase (last 2 seconds), alternate between FRIGHT_BODY and FRIGHT_FLASH every 250ms.

*Eaten ghost — eyes only*:
```
Just the two 6x6 white eye rects with pupils, on a black background.
Moving toward ghost house at double speed.
```

**Dot rendering**: 4x4 DOT_COLOR square at cell center. Warm peach tone is distinctive and visible against black paths.

**Power pellet rendering**: 10x10 PELLET_COLOR square at cell center, with a 14x14 PELLET_GLOW behind it. Pulsing: alternate between drawn and erased every 500ms (use `get_time()` check).

**Fruit rendering**: 12x12 colored square at cell center. Cherry: red square with 2x4 green stem on top. Strawberry: red-orange triangle approximation (wider at top). Orange: orange square.

**Death animation** — warm fade:
```
Phase 1 (0-200ms):  20x20 PAC_BODY (normal)
Phase 2 (200-400ms): 16x16 PAC_DEATH1 (orange, shrinking)
Phase 3 (400-600ms): 12x12 PAC_DEATH2 (red-orange, smaller)
Phase 4 (600-800ms): 8x8 PAC_DEATH2 (red, small)
Phase 5 (800-1000ms): 4x4 WALL_EDGE (blue flash, vanishing)
Phase 6: gone (erase cell)
```

**Level complete animation**: maze walls flash between WALL_FACE and FRIGHT_FLASH (white), 3 cycles over 2 seconds. Characters frozen in place. Creates a satisfying "level clear" strobe.

**HUD panel** (right side, x=640..1240, full height):
```
y=30:   "1UP" label in HUD_LABEL, then current score in HUD_SCORE (large 2x 7-seg digits)
y=120:  "HIGH SCORE" label in HUD_LABEL, then high score in HUD_HISCORE (green)
y=210:  "LEVEL" label + level number in HUD_SCORE
y=280:  Lives: row of small Pac-Man icons (12x12 PAC_BODY), one per remaining life
y=340:  Current fruit icon for this level
y=420:  Ghost roster: 4 colored dots (8x8) showing ghost status:
        - Solid color = active, Blue = frightened, Gray = in ghost house, Eyes icon = eaten
```
Panel bordered on left by a 2px WALL_EDGE vertical line.

**"READY!" text**: Rendered in PELLET_COLOR using the 7-segment font at 2x scale, centered in the maze area below the ghost house. Displayed for 2 seconds at level start.

**"GAME OVER" text**: Rendered in BLINKY_BODY (red) at 2x scale, centered in maze. Background cells behind it darkened.

**Rendering order** (back to front):
1. Path cells (black)
2. Dots and power pellets
3. Fruit (if visible)
4. Ghosts (drawn in order: Clyde, Inky, Pinky, Blinky — so Blinky is on top, matching classic game)
5. Pac-Man (always on top of ghosts for collision visibility)

**Incremental rendering rules**:
- Moving character: erase old cell (redraw path + dot if dot was there), draw character at new cell
- Eaten dot: redraw cell as empty path
- Ghost mode change: redraw ghost at current position with new colors
- Score change: erase and redraw only the changed digits in HUD
- Full redraw only on: level start, game restart, load from file

## Lessons Learned from ch1-ch6 (apply ALL of these)

1. **virtio-drivers 0.3.0, not 0.1.0** — v0.1.0 has a confirmed bug where `VirtIOInput::pop_pending_event()` doesn't notify the device after recycling buffers. Keyboard stops after 32 events. Fixed in v0.3.0 (commit b559de30). Update the Hal trait impl for the new API (`BufferDirection`, `share`/`unshare`/`mmio_phys_to_virt`).

2. **FB_WRITE must NOT flush GPU** — Decouple pixel writes from GPU flush. Add a separate FB_FLUSH syscall (ID 2003). Game calls `fb_flush()` once per frame after all draws. Without this, VNC gets thousands of partial updates per frame and displays garbage.

3. **DMA pool needs explicit mapping** — Map 0x8500_0000..0x8550_0000 in `kernel_space()`. Without this, GPU init hangs because the framebuffer is allocated in unmapped DMA memory.

4. **Keyboard: return press AND release, drain sync events** — Return keycode for press, keycode|0x80 for release. Loop to drain EV_SYN events. Game tracks held-key state via `KeyState` struct for smooth movement.

5. **Single keyboard reader** — Only one process should read the VirtIO keyboard (there's one event queue). Parent reads all input, dispatches to children via pipes/shared memory.

6. **IO::read and the fd conflict** — In ch5/ch6, fd 3 (STDIN_BUFFERED) was used for non-blocking VirtIO keyboard. But in ch7, fd 3+ are used for pipes! The `Fd` enum dispatches read/write based on type (File, PipeRead, PipeWrite, Empty). For keyboard: add a new `Fd::Keyboard` variant, or use a dedicated high fd number (e.g., 100) that won't conflict with pipe allocations, or use `Fd::Empty` for fd 0 and make it poll VirtIO keyboard instead of SBI. The cleanest approach: when `Fd::Empty { read: true }` is read (which is stdin), try VirtIO keyboard first (non-blocking), then fall back to SBI console. The shell gets SBI input via serial, the game gets VirtIO keyboard events. Pipe fds are `Fd::PipeRead` and work unchanged.

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
2. Pac-Man moves with WASD/arrow keys with direction queuing (responsive cornering)
3. 4 ghost child processes communicate via pipes (verified by PID prints at startup)
4. Each ghost has distinct AI personality (Blinky chases, Pinky ambushes, Inky is erratic, Clyde is shy)
5. Ghost modes cycle: scatter (7s) → chase (20s) → repeat
6. Power pellet triggers SIGUSR1 to ghosts — they turn blue, move randomly, can be eaten for escalating points (200/400/800/1600)
7. Eaten ghosts return to ghost house as "eyes" then respawn
8. Tunnel wrap-around works (ghosts slow in tunnels)
9. Fruit bonus appears twice per level at the center
10. Score, high score, lives, level displayed in HUD panel
11. High score saved to filesystem (persists across restarts via open/write/close)
12. Death animation (Pac-Man shrinks), 1-second freeze, then continue/game over
13. Level progression when all dots cleared (maze flashes, ghosts get faster)
14. Game over when all lives lost, high score saved, any key restarts
15. Space pauses/unpauses the game
16. Existing ch7 test programs still pass (serial output correct)
17. `cargo check` and `cargo publish --dry-run` pass

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine game design, pipe protocol, signal usage, ghost AI. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task
4. `superpowers:requesting-code-review` — final review

Also load `frontend-design:frontend-design` for the visual design phase.

Do NOT publish the crate — the user will review and publish manually.

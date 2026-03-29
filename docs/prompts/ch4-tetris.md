Extend `tg-rcore-tutorial-ch4` with a user-space Tetris game rendered via VirtIO-GPU, with VirtIO keyboard input from VNC. The game runs as a user program under ch4's virtual-memory-enabled scheduler alongside existing test programs. Supports piece rotation, line clearing, scoring, and speed progression.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch4-tetris.gif

## Setup

Copy the ch4 directory to create the working crate:

```bash
cp -r tg-rcore-tutorial-ch4 tg-rcore-tutorial-ch4-tetris
rm -rf tg-rcore-tutorial-ch4-tetris/{target,Cargo.lock,.gitrepo}
```

Then update `tg-rcore-tutorial-ch4-tetris/Cargo.toml`:
- Change `name` to a unique crate name for publishing
- Update `authors` and `description`

**All kernel modifications go into `tg-rcore-tutorial-ch4-tetris/`** — do NOT modify the original `tg-rcore-tutorial-ch4/` or shared crates. User-side changes go in `tg-rcore-tutorial-user/`.

## Goal

Add VirtIO-GPU framebuffer rendering and VirtIO keyboard input to the ch4 virtual-memory OS kernel. Create a Tetris user program that renders to the GPU framebuffer and receives keyboard input from VNC. The game runs alongside existing ch4 test programs under round-robin scheduling with per-process Sv39 virtual address spaces.

## Architecture Overview — How ch4 differs from ch3

Ch4 is a **major architectural jump** from ch3. Understanding these differences is critical:

| Feature | Ch3 | Ch4 |
|---------|-----|-----|
| Address space | Shared physical memory | **Per-process Sv39 page tables** |
| Program loading | Raw binary at fixed address | **ELF parsing, segments mapped into isolated address space** |
| Context switch | `LocalContext::execute()` | **`ForeignContext::execute(portal, ())` via MultislotPortal** |
| Heap | None in kernel | **`tg-kernel-alloc` provides dynamic allocation** |
| Memory syscalls | None | **mmap, munmap, sbrk** |
| User pointers | Direct access (identity-mapped) | **Must translate via `address_space.translate()` before dereferencing** |

### Key ch4 mechanisms

- **MultislotPortal**: A gateway page mapped at `VPN::MAX` in both kernel and user address spaces. Enables seamless trap handling across address spaces by switching `satp` while maintaining code flow.
- **Address translation**: Every user pointer passed to the kernel via syscall must be translated through the process's page table before the kernel can read/write it. The existing `IO::write` handler already does this — follow the same pattern for `IO::read` and FB_WRITE.
- **Per-process structure**: `Process { context: ForeignContext, address_space: AddressSpace<Sv39>, heap_bottom, program_brk }`. Each process has its own page table root.
- **Kernel heap**: `tg-kernel-alloc` provides a proper heap allocator. The kernel can dynamically allocate buffers — no need for the fixed-address bump allocator used in ch2/ch3.

### What needs to be added

**Kernel-side:**

1. **VirtIO device drivers** — VirtIO-GPU and VirtIO keyboard. The `virtio-drivers = "0.1.0"` crate provides `VirtIOGpu`, `VirtIOInput`, and the `Hal` trait.

   **Critical: Hal trait for ch4 is different from ch2/ch3.** In ch2/ch3, `Hal` uses identity mapping (`phys_to_virt(p) = p`). In ch4, the kernel uses Sv39 page tables. However, the kernel's address space uses **identity mapping for physical memory** (kernel maps physical RAM at the same virtual addresses). So `phys_to_virt(p) = p` still works for kernel-side DMA access. The `Hal` implementation can use the kernel heap allocator (`tg-kernel-alloc`) for DMA allocation instead of a bump allocator, OR use a fixed-address bump allocator as in ch3 — either works since the kernel identity-maps physical RAM.

   **DMA pool address**: The kernel identity-maps its own address space. DMA buffers must be in physical memory accessible to VirtIO devices. With the kernel heap, you can allocate via `alloc::alloc::alloc()` and the physical address equals the virtual address (kernel identity mapping). Alternatively, use a fixed pool address well above the kernel image.

   **MMIO mapping**: VirtIO MMIO registers at `0x10001000`-`0x10008000` must be accessible to the kernel. In ch4, the kernel's `kernel_space()` function maps physical RAM. Check whether the MMIO range is already mapped. If not, add a mapping for it (identity-mapped with Read+Write flags, no caching).

2. **FB_INFO and FB_WRITE syscalls** (IDs 2000/2001) — Same interface as ch2/ch3, BUT:
   - **FB_WRITE must translate the user data pointer** through the calling process's page table before reading pixel data. Use `address_space.translate::<u8>(VAddr::new(data_ptr), READABLE)`. Follow the pattern in the existing `IO::write` handler.
   - If the user buffer spans multiple pages, you may need to translate and copy page by page, since virtual pages may not be physically contiguous.

3. **VirtIO keyboard input** — Use `VirtIOInput` from `virtio-drivers 0.1.0`:
   - `VirtIOInput::new(transport)` initializes the device with 32 event buffers
   - `pop_pending_event() -> Option<InputEvent>` is non-blocking
   - `InputEvent { event_type: u16, code: u16, value: u32 }` — filter for `event_type == 1` (EV_KEY) and `value == 1` (press)
   - Translate keycodes to game-meaningful bytes (see keycode table below)
   - **Do NOT use `tg_sbi::console_getchar()`** — it blocks in M-mode (busy-waits in a loop until a character arrives). This was a critical bug in ch3-snake development.

4. **Non-blocking IO::read** — Implement `IO::read` for fd=0 (STDIN): poll VirtIO keyboard directly, return 0 if no key. **Translate the user buffer pointer** before writing to it.

5. **Kernel stack size** — Ch4 already has a larger kernel setup. Ensure the stack is large enough for VirtIO init + page table operations.

6. **QEMU config** — Add `-device virtio-gpu-device` and `-device virtio-keyboard-device` to the runner. Replace `-nographic` with `-serial stdio`.

**User-side:**

1. **Tetris game module** (`tg-rcore-tutorial-user/src/tetris.rs`, feature-gated with `tetris`) — Implements all game logic, rendering, and input handling.

2. **One binary** (`tetris.rs`) — Calls `tetris::run_game()`. **Gate the binary body with `#[cfg(feature = "tetris")]`** so `cargo publish --dry-run` passes without the feature.

3. **Rendering via `fb_write`** — Same `fb_info()`/`fb_write()` wrappers in `lib.rs` as ch3. These use inline asm ecall with IDs 2000/2001.

## VirtIO Keyboard — Keycode Translation

```
EV_KEY (event_type == 1), value == 1 (key press):
  Left arrow = 105, A = 30  → move piece left
  Right arrow = 106, D = 32 → move piece right
  Down arrow = 108, S = 31  → soft drop
  Up arrow = 103, W = 17    → rotate piece
  Space = 57                → hard drop
  Enter = 28                → any-key for restart
```

## Tetris Game Design

### Board
- **10 columns × 20 rows** (standard Tetris)
- Cell size: 28-32px (fits in 1280×800 framebuffer with room for side panels)
- Board position: left-center of screen
- Right panel: next piece preview, score, level, lines cleared

### Pieces (7 standard tetrominoes)
```
I: ████    O: ██    T: ███    S:  ██    Z: ██     J: █      L:   █
              ██        █        ██        ██      ███      ███
```
Each piece: array of 4 (row, col) offsets relative to a pivot. Store all 4 rotation states per piece (or compute from a rotation matrix).

### Game mechanics
- **Gravity**: Piece drops one row per tick. Tick speed decreases with level (start ~800ms, minimum ~100ms).
- **Soft drop** (down/S): Accelerate drop to ~50ms per row.
- **Hard drop** (space): Instantly place piece at the lowest valid position.
- **Rotation**: Rotate clockwise. Simple rotation (no wall kicks) is fine for a teaching OS; SRS wall kicks are a bonus.
- **Line clearing**: When a row is full, remove it, shift above rows down, increment score.
- **Scoring**: 1 line = 100×level, 2 = 300×level, 3 = 500×level, 4 (Tetris) = 800×level.
- **Level up**: Every 10 lines cleared.
- **Game over**: When a new piece cannot spawn (top rows occupied).
- **Restart**: Show game over screen, wait for any key, restart.

### Rendering approach
- **Incremental rendering**: Only redraw cells that changed (placed piece, cleared lines, new falling piece position). Avoid full-board redraws — each `fb_write` triggers a GPU flush.
- **Ghost piece**: Show where the piece would land as a dim outline.
- **Use a static render buffer** (32×32×4 bytes) for cell drawing to avoid heap allocation on the hot path.
- **Background fill in user space** — the kernel should NOT fill the screen with game-specific colors. Draw background via fb_write strips in the game's init function.

### Data structures
```
Board: [u8; 10 * 20]  — 0 = empty, 1-7 = piece color index
Piece: { shape: u8, rotation: u8, row: i8, col: i8 }
Game: { board, current_piece, next_piece, score, level, lines, state, rng, ... }
```

### PRNG
Xorshift64 seeded from `rdtime`. Use for piece selection (random bag or simple random).

## Lessons Learned from ch3-snake (apply these)

1. **SBI console_getchar BLOCKS** — Never use it for non-blocking input. Use VirtIO keyboard `pop_pending_event()` which is naturally non-blocking.

2. **VirtIO keyboard for VNC input** — Add `-device virtio-keyboard-device` to QEMU. The device appears on an MMIO slot alongside the GPU. Scan all slots and check `transport.device_type()` to distinguish GPU (`DeviceType::GPU`) vs keyboard (`DeviceType::Input`).

3. **build.rs caching** — Add `println!("cargo:rerun-if-env-changed=TG_SKIP_USER_APPS");` to avoid stale dummy app.asm after `TG_SKIP_USER_APPS=1 cargo check`.

4. **Event::None timer reset** — Syscalls returning `Event::None` (fb_write, read, clock_gettime) cause `continue` in the scheduling loop, resetting the timer. This means the game monopolizes CPU during rendering bursts (initial screen draw). Other programs don't get scheduled until the game calls `sched_yield`. This is expected behavior — the game yields during `sleep()`.

5. **Feature-gate binary bodies** — Snake/tetris binary bodies must use `#[cfg(feature = "tetris")] { ... }` so `cargo publish --dry-run` compiles without the feature (same pattern as tangram binaries).

6. **User stack is 8 KiB** — Don't put large buffers on the stack. Use static buffers or heap allocation for pixel data.

7. **Integer-only rendering** — Use integer arithmetic for all pixel calculations (distance squared instead of sqrt, 256-level integer color interpolation). No floating point.

## Address Translation for FB_WRITE

This is the most important ch4-specific concern. In ch3, user pointers are physical addresses (identity-mapped). In ch4, user pointers are **virtual addresses in the process's page table**. The kernel must translate them before reading.

For FB_WRITE (user sends a pixel buffer pointer):
```rust
// In the FB_WRITE handler:
let process = &PROCESSES[caller.entity];
let user_data_ptr = args[4]; // User virtual address

// Translate to kernel-accessible pointer
const READABLE: VmFlags<Sv39> = build_flags("RV");
if let Some(kernel_ptr) = process.address_space.translate::<u8>(
    VAddr::new(user_data_ptr), READABLE
) {
    // kernel_ptr is now safe to read from
    let user_data = unsafe { core::slice::from_raw_parts(kernel_ptr.as_ptr(), w * h * 4) };
    // Copy pixels to framebuffer...
}
```

**Page boundary crossing**: If the user buffer spans multiple pages (likely for large rectangles), the translated physical addresses may not be contiguous. You may need to copy row-by-row or page-by-page. For small cell-sized fb_write calls (32×32×4 = 4 KiB = 1 page), this is usually not an issue. For larger writes (background strips), handle page boundaries.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch4-tetris/Cargo.toml` | Modify | New name, add `virtio-drivers = "0.1.0"` |
| `ch4-tetris/.cargo/config.toml` | Modify | Add GPU + keyboard devices, `-serial stdio` |
| `ch4-tetris/src/main.rs` | Modify | VirtIO init, FB/read syscalls with address translation, keyboard driver |
| `ch4-tetris/src/process.rs` | Modify (maybe) | If MMIO mapping needed for VirtIO |
| `ch4-tetris/build.rs` | Modify | Tetris case_key selection, `--features tetris`, rerun triggers |
| `ch4-tetris/test.sh` | Modify | Headless CI with timeout + GPU device |
| `user/src/tetris.rs` | Create | Game logic, rendering, input (feature-gated) |
| `user/src/lib.rs` | Modify | Add tetris module |
| `user/Cargo.toml` | Modify | Add `tetris` feature |
| `user/src/bin/tetris.rs` | Create | Tetris binary (feature-gated body) |
| `user/cases.toml` | Modify | Add ch4_tetris section |

## Acceptance Criteria

1. `cargo run` opens QEMU with a playable Tetris game visible on VNC
2. Arrow keys / WASD control piece movement and rotation, Space for hard drop
3. Pieces fall with gravity, lines clear, score/level increase
4. Speed increases with level (starts ~800ms, gets faster)
5. Game over when pieces stack to the top, any key restarts
6. Existing ch4 test programs still pass (serial output correct)
7. `cargo check` and `cargo publish --dry-run` pass
8. Score, level, and lines displayed on screen

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine game design, ch4-specific architecture (address translation, MMIO mapping, Hal implementation). Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task
4. `superpowers:requesting-code-review` — final review

Also load `frontend-design:frontend-design` for the visual design phase.

Do NOT publish the crate — the user will review and publish manually.

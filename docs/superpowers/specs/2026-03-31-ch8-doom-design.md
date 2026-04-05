# Ch8 Doom Port — Design Spec

## Overview

Port the classic Doom game (1993) to run as a user-space program on the ch8 concurrency kernel. Uses the [doomgeneric](https://github.com/ozkl/doomgeneric) cross-platform abstraction layer with C code cross-compiled for RISC-V and linked via FFI. Renders via VirtIO-GPU framebuffer, reads input from VirtIO keyboard over VNC.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch8-doom.gif

## Architecture

### Approach: C cross-compilation via `cc` crate (Approach A)

- Vendor doomgeneric C sources into `tg-rcore-tutorial-user/doomgeneric/`
- Use the `cc` crate in the user crate's `build.rs` (gated on `#[cfg(feature = "doom")]`) to compile all `.c` files with `riscv64-unknown-elf-gcc`
- Exclude `i_main.c` (conflicts with Rust entry point)
- Provide stub C headers for `<stdio.h>`, `<stdlib.h>`, `<string.h>`, `<ctype.h>`, `<stdarg.h>`, `<errno.h>` that declare functions implemented in Rust
- Rust binary (`doom.rs`) calls `doomgeneric_Create()` + `doomgeneric_Tick()` loop via FFI
- Platform functions (`DG_Init`, `DG_DrawFrame`, etc.) implemented in Rust, exported as `#[no_mangle] extern "C"`

### Component Diagram

```
User Space (doom binary)
├── doom.rs                    — Entry point, game loop
├── doom/mod.rs                — Module root, run_game()
├── doom/platform.rs           — DG_ function implementations (framebuffer, keyboard, timing)
├── doom/libc_shim.rs          — Minimal libc: malloc/free, string ops, printf, file I/O
├── doom/keymap.rs             — VirtIO → Doom keycode translation
└── doomgeneric/ (C sources)   — Cross-compiled static library (libdoomgeneric.a)

Kernel Space (ch8-doom)
├── main.rs                    — GPU/keyboard init, FB syscalls, keyboard IO::read, increased MEMORY
├── allocator.rs               — DMA bump allocator + v0.3.0 Hal trait
├── virtio.rs                  — MMIO scanning for GPU/keyboard
├── virtio_block.rs            — Updated to use shared Hal
└── process.rs                 — Increased user stack (16+ pages)
```

## Kernel Changes

### 1. VirtIO-GPU and Keyboard Drivers

Same pattern as ch7-pacman. Scan MMIO range 0x10001000-0x10008000 for GPU and keyboard devices. Use `virtio-drivers = "0.3.0"` (v0.1.0 has keyboard bug).

### 2. DMA Bump Allocator

Dedicated DMA pool at 0x8500_0000, size 5 MiB. Simple atomic bump allocator (no dealloc needed). Implements `virtio_drivers::Hal` trait for v0.3.0 API.

### 3. FB Syscalls (2000, 2001, 2003)

- **FB_INFO (2000)**: Returns `(width << 32) | height`
- **FB_WRITE (2001)**: Translates user data pointer through process page table row-by-row. In ch8, use `PROCESSOR.get_mut().get_current_proc().unwrap()` for process access (not `current()` which returns Thread).
- **FB_FLUSH (2003)**: Triggers GPU display update. Called once per frame (not per fb_write).

### 4. Non-blocking Keyboard IO::read

Replace `sbi::console_getchar()` for fd 0 (STDIN) with VirtIO keyboard polling. Return keycode for press, keycode|0x80 for release. Drain sync events. Do NOT break pipe/file reads.

### 5. Memory Increases

| Parameter | Default | Doom Requirement |
|-----------|---------|-----------------|
| MEMORY constant | 48 MiB | 128 MiB |
| QEMU RAM (-m) | 128 MiB | 256 MiB |
| User stack | 2 pages (8 KiB) | 16 pages (64 KiB) |
| User heap | 16 KiB | 8+ MiB via sbrk |

### 6. MMIO & DMA Mapping

```rust
const MMIO: &[(usize, usize)] = &[
    (0x1000_0000, 0x00_9000),   // VirtIO MMIO range (GPU + keyboard + block)
    (0x8500_0000, 0x0050_0000), // DMA pool
];
```

Map both ranges in `kernel_space()` with `_WRV` flags.

### 7. QEMU Config

Add: `-device virtio-gpu-device -device virtio-keyboard-device -vnc :0 -serial stdio -m 256M`

### 8. CHAPTER Mechanism

Set `CHAPTER=doom` in build.rs. Add `"doom" => "doom"` to initproc.rs.

## User-Space Changes

### 1. Minimal libc (libc_shim.rs)

**Memory**: `malloc`/`free`/`realloc`/`calloc` backed by bump allocator over `sbrk()`. Initialize with 8 MiB heap at startup.

**String/memory**: `memcpy`, `memset`, `memmove`, `strlen`, `strcpy`, `strncpy`, `strcmp`, `strncmp`, `strncasecmp`, `strcat`, `strncat`, `strchr`, `strrchr`, `strstr` — implemented from scratch.

**Formatted I/O**: Minimal `snprintf`/`sprintf`/`printf`/`puts`/`fprintf`/`vsnprintf` — enough for HUD text. `sscanf` with minimal format support.

**File I/O**: `fopen`/`fclose`/`fread`/`fwrite`/`fseek`/`ftell`/`remove` — WAD loaded entirely into memory at startup (avoids lseek). Regular files (save games) use rCore `open`/`read`/`write`/`close` syscalls.

**Character**: `toupper`/`tolower`/`isdigit`/`isspace`/`isalnum`/`isprint`/`isupper`

**Other**: `qsort`, `atoi`/`strtol`/`abs`, `exit`/`abort`, `errno` (dummy), `signal`/`atexit` (no-op), `mkdir`/`stat`/`access`/`getenv` (stub)

### 2. WAD File Handling

**In-memory WAD cache**: Load entire doom1.wad (~4 MiB) into heap at startup via `open`/`read`/`close`. Implement `FILE` abstraction over memory buffer supporting `fseek`/`ftell`/`fread`.

doom1.wad is packed into the easy-fs image during build. The build script downloads it if not present.

### 3. DG_ Platform Functions

- **`DG_Init()`**: Call `fb_info()` for screen dimensions. No other init needed.
- **`DG_DrawFrame()`**: OR each pixel with 0xFF000000 (set alpha). Write to framebuffer centered (offset 320,200 for 640x400 on 1280x800). Call `fb_write()` + `fb_flush()`.
- **`DG_SleepMs(ms)`**: Call `sleep(ms)` (poll-based from user lib).
- **`DG_GetTicksMs()`**: Call `get_time()` (returns ms).
- **`DG_GetKey(pressed, key)`**: Poll keyboard via `read(STDIN, buf)`. Translate VirtIO→Doom keycodes. Return 1 if event, 0 if none.
- **`DG_SetWindowTitle(title)`**: No-op.

### 4. Keycode Translation

VirtIO scancodes → Doom keycodes mapping table (arrows, ctrl/fire, space/use, shift/run, alt/strafe, enter, escape, F-keys, number keys, letter keys).

### 5. Build Integration

- Feature `doom` in user Cargo.toml
- `cc` build-dependency, gated compilation in build.rs
- Stub headers in `doomgeneric/include/` directory
- cases.toml section `[ch8_doom]`
- doom.rs binary with `#[cfg(feature = "doom")]` body

## Resolution Strategy

Doom renders at 640x400. Framebuffer is 1280x800. Center the output: offset_x=320, offset_y=200. No scaling needed (1:1 pixel mapping).

## Pixel Format

Doom XRGB8888 and GPU BGRA8888 share byte layout (B,G,R in bytes 0-2). Set byte 3 (alpha) to 0xFF via `pixel | 0xFF000000`.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch8-doom/Cargo.toml` | Modify | New name, virtio-drivers 0.3.0 |
| `ch8-doom/.cargo/config.toml` | Modify | GPU + keyboard + VNC, -m 256M |
| `ch8-doom/src/main.rs` | Modify | GPU/keyboard init, FB syscalls, keyboard IO::read, MEMORY=128M, MMIO |
| `ch8-doom/src/allocator.rs` | Create | DMA bump allocator + v0.3.0 Hal |
| `ch8-doom/src/virtio.rs` | Create | MMIO scanning for GPU/keyboard |
| `ch8-doom/src/virtio_block.rs` | Modify | Use shared Hal |
| `ch8-doom/src/process.rs` | Modify | 16-page user stack |
| `ch8-doom/build.rs` | Modify | ch8_doom case_key, --features doom, CHAPTER=doom |
| `user/Cargo.toml` | Modify | doom feature, cc build-dep |
| `user/src/lib.rs` | Modify | doom module |
| `user/src/doom/mod.rs` | Create | Module entry, run_game() |
| `user/src/doom/libc_shim.rs` | Create | Minimal libc |
| `user/src/doom/platform.rs` | Create | DG_ implementations |
| `user/src/doom/keymap.rs` | Create | Keycode translation |
| `user/src/bin/doom.rs` | Create | Binary entry |
| `user/src/bin/initproc.rs` | Modify | Add "doom" branch |
| `user/cases.toml` | Modify | Add ch8_doom section |
| `user/build.rs` | Modify | cc compilation when doom feature active |
| `user/doomgeneric/` | Create | Vendored C sources + stub headers |

## Acceptance Criteria

1. `cargo run` boots QEMU, loads doom1.wad from filesystem, starts Doom title screen on VNC
2. Main menu navigable with arrow keys and Enter
3. New game starts, player moves through E1M1
4. Arrow keys move, Ctrl fires, Space opens doors, Shift runs
5. 3D rendering correct (walls, floors, ceilings, sprites)
6. No sound required (nosound mode)
7. Playable frame rate (>5 FPS at 640x400)
8. Escape opens pause menu, F-keys work
9. Existing ch8 tests still pass
10. `TG_SKIP_USER_APPS=1 cargo check` passes
11. doom1.wad loaded from filesystem (not embedded in binary)

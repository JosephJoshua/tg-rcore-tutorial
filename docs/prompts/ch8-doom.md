Extend `tg-rcore-tutorial-ch8` with a user-space port of the classic Doom game (1993), rendered via VirtIO-GPU framebuffer with VirtIO keyboard input from VNC. The port uses the [doomgeneric](https://github.com/ozkl/doomgeneric) cross-platform abstraction layer, demonstrating ch8's **threads** for game logic, **mutexes** for shared state protection, **filesystem** for WAD file loading, and **VirtIO device drivers** for display and input — the most ambitious integration in the tutorial series.

Demo reference: https://github.com/rcore-os/tg-rcore-tutorial-game-demo/blob/main/ch8-doom.gif

## Setup

Copy the ch8 directory to create the working crate:

```bash
cp -r tg-rcore-tutorial-ch8 tg-rcore-tutorial-ch8-doom
rm -rf tg-rcore-tutorial-ch8-doom/{target,Cargo.lock,.gitrepo}
```

Then update `tg-rcore-tutorial-ch8-doom/Cargo.toml`:
- Change `name` to a unique crate name for publishing
- Update `authors` and `description`

**All kernel modifications go into `tg-rcore-tutorial-ch8-doom/`** — do NOT modify the original `tg-rcore-tutorial-ch8/` or shared crates. User-side changes go in `tg-rcore-tutorial-user/`.

## Goal

Port the Doom game engine to run as a user-space program on the ch8 concurrency kernel. The game loads its WAD data file from the easy-fs filesystem, renders 3D scenes via software rendering to the VirtIO-GPU framebuffer, and reads keyboard input from the VirtIO keyboard device. Optionally demonstrate ch8's threading and synchronization primitives by running the game loop and input handling on separate threads.

## Architecture Overview — How ch8 differs from ch7

Ch8 adds **threads** (lightweight execution units within a process), **mutexes**, **semaphores**, and **condition variables** to the ch7 IPC/signal OS. The process model is split: `Process` holds shared resources (address space, fd_table, sync primitives), `Thread` holds execution state (TID, context). Multiple threads in one process share the same address space.

| Feature | Ch7 | Ch8 |
|---------|-----|-----|
| Execution model | Single-threaded processes | Multi-threaded processes |
| Scheduling unit | Process (ProcId) | Thread (ThreadId) |
| Manager type | `PManager` (processes only) | `PThreadManager` (processes + threads) |
| Resource sharing | Processes isolated | Threads share: address space, fd_table, sync primitives |
| Synchronization | Signals only | + Mutex, Semaphore, Condvar |
| Blocking | Only exit/wait | Blocking via sync primitives (returns -1 → `make_current_blocked()`) |
| Stack allocation | One per process | Multiple (one per thread, 2 pages each) |
| Deadlock detection | None | Wait-for graph + banker's algorithm |
| task-manage feature | `proc` | `thread` |
| New dependency | — | tg-sync |

### Key ch8 mechanisms

- **`thread_create(entry, arg)`**: Creates a new thread in the current process. Allocates a 2-page user stack, sets entry point and argument (a0). Returns TID.
- **`gettid()`**: Returns current thread's TID.
- **`waittid(tid)`**: Blocks until thread exits, returns its exit code.
- **`mutex_create(blocking)`**: Creates a mutex, returns mutex_id.
- **`mutex_lock(mutex_id)`**: Acquires mutex. Returns 0 on success, -1 if blocked (kernel removes thread from ready queue until mutex is released).
- **`mutex_unlock(mutex_id)`**: Releases mutex, wakes one waiting thread.
- **`semaphore_create(count)`**, **`semaphore_down(id)`**, **`semaphore_up(id)`**: Counting semaphore.
- **`condvar_create()`**, **`condvar_wait(condvar_id, mutex_id)`**, **`condvar_signal(condvar_id)`**: Condition variable (releases mutex while waiting).
- **Dual-layer manager**: `PThreadManager<Process, Thread, ThreadManager, ProcManager>`. Threads are the scheduling unit; processes hold resources. `get_current_proc()` returns the process owning the current thread.
- **Blocking pattern**: Sync syscalls return -1 when the thread must block. The trap loop detects this and calls `make_current_blocked()`. When the resource becomes available, the releasing thread calls `re_enque(woken_tid)`.
- **Thread stacks**: Each new thread gets 2 pages (8 KiB) of user stack, allocated by searching downward from VPN `(1<<26)-2` for unmapped page table entries.

### The doomgeneric porting approach

[doomgeneric](https://github.com/ozkl/doomgeneric) is a cross-platform Doom source port that abstracts all platform dependencies into 6 functions. To port Doom to a new platform, you implement these 6 functions and link with the Doom engine code:

```c
void     DG_Init();                                    // Platform init (create window/framebuffer)
void     DG_DrawFrame();                               // Blit DG_ScreenBuffer to display
void     DG_SleepMs(uint32_t ms);                      // Sleep N milliseconds
uint32_t DG_GetTicksMs();                              // Milliseconds since program start
int      DG_GetKey(int* pressed, unsigned char* key);  // Return 1 if key event, 0 if none
void     DG_SetWindowTitle(const char* title);         // Set window title (can be no-op)
```

The engine renders into a global `DG_ScreenBuffer` (a `uint32_t*` array in **XRGB8888** format) at `DOOMGENERIC_RESX × DOOMGENERIC_RESY` resolution (default 640×400). The main loop is:

```c
doomgeneric_Create(argc, argv);  // Init engine, allocate DG_ScreenBuffer, call DG_Init()
while (1) {
    doomgeneric_Tick();          // Process one frame: input → game logic → render → DG_DrawFrame()
}
```

### What needs to be added

**Kernel-side:**

1. **VirtIO-GPU and keyboard drivers** — Same pattern as ch6-breakout and ch7-pacman. Ch8 already has VirtIO-blk at MMIO slot 0x10001000 with a `VirtioHal` in `virtio_block.rs`. Extend MMIO scanning for GPU and keyboard. Reuse the existing Hal (kernel heap DMA) or add a dedicated DMA bump allocator as in ch7-pacman.

   **IMPORTANT**: Use `virtio-drivers = "0.3.0"` (not 0.1.0) — v0.1.0 has a bug where `VirtIOInput::pop_pending_event()` stops working after 32 events.

2. **FB_INFO (2000), FB_WRITE (2001), FB_FLUSH (2003) syscalls** — Same as ch5-ch7. FB_WRITE must translate user data pointer through the process's page table row-by-row. In ch8, use `PROCESSOR.get_mut().get_current_proc().unwrap()` for process access (not `current()` which returns a Thread). FB_FLUSH triggers GPU display update.

3. **Non-blocking keyboard IO::read** — Ch8's `IO::read` handles multiple fd types via the `Fd` enum (same as ch7). For fd 0 (STDIN/Empty), replace blocking `sbi::console_getchar()` with VirtIO keyboard polling. Return keycode for press, keycode|0x80 for release, drain sync events. Do NOT break pipe reads or file reads.

4. **Increased user heap** — Doom needs approximately **8 MiB** of user heap (6 MiB zone memory + framebuffer + overhead). The default `customizable-buddy` user heap is only 16 KiB. You MUST significantly increase the user heap size. Options:
   - Increase the user-side heap init to 8+ MiB via `sbrk()` calls at startup
   - Or provide a large pre-mapped heap region
   - The `sbrk` syscall in ch8 (`change_program_brk`) grows the heap by mapping new pages. Doom's `malloc` (from a minimal libc or custom allocator) must ultimately call `sbrk`.

5. **Increased user stack** — Doom's call depth is deeper than simple games. Increase user stack from 2 pages (8 KiB) to at least 16 pages (64 KiB), or even 32 pages (128 KiB). Modify `Process::from_elf()` and `Thread::new()` stack allocation.

6. **QEMU config** — Add `-device virtio-gpu-device -device virtio-keyboard-device -vnc :0 -serial stdio`. Keep the existing `-drive file=fs.img,...` for filesystem. Add `-m 256M` to increase QEMU RAM from the default 128 MiB to 256 MiB (Doom user process + kernel + GPU framebuffer need more than 128 MiB).

7. **MEMORY increase** — Increase the kernel's `MEMORY` constant to at least 128 MiB (currently 48 MiB in ch8). This is the kernel heap size, which backs all physical page allocations including user process pages (via sbrk → map new pages from kernel heap). Doom's user process alone needs ~12 MiB of mapped pages, plus kernel overhead, GPU framebuffer (~4 MiB), page tables, and other processes. Ensure QEMU's `-m` flag provides enough physical RAM to cover this.

8. **DMA pool mapping** — Add identity mapping for DMA pool region (0x8500_0000..0x8550_0000) in `kernel_space()`. Also ensure VirtIO MMIO range covers GPU/keyboard slots.

9. **Larger filesystem image** — doom1.wad is ~4 MiB. The default easy-fs image (64×2048 blocks = 64 MiB) should be sufficient, but verify.

**User-side:**

1. **C cross-compilation toolchain** — The doomgeneric engine is written in C (~15,000 lines across ~50 files). You need `riscv64-unknown-elf-gcc` (or `riscv64-linux-gnu-gcc`) to cross-compile the C code into a static library. The user Rust program links against this library via FFI.

2. **Minimal libc** — Doom's C code uses standard library functions: `malloc`/`free`, `memcpy`/`memset`/`memmove`, `strlen`/`strcpy`/`strcmp`/`strncpy`/`strncasecmp`, `sprintf`/`snprintf`/`sscanf`, `printf`/`puts`/`fprintf(stderr,...)`, `atoi`/`abs`, `fopen`/`fread`/`fwrite`/`fseek`/`ftell`/`fclose`/`remove`, `qsort`, `toupper`/`tolower`/`isdigit`/`isspace`. You must provide implementations that delegate to rCore syscalls:
   - **Memory**: `malloc`/`free` backed by a bump allocator or buddy allocator over `sbrk()`
   - **File I/O**: `fopen`→`open()`, `fread`→`read()`, `fseek`→(track offset manually or add lseek), `fclose`→`close()`. Note: easy-fs does NOT have `lseek` — you may need to add it to the kernel, or load the entire WAD into memory and serve reads from RAM.
   - **String/memory**: Implement from scratch (no_std) or use compiler builtins
   - **printf family**: Minimal `snprintf`/`sprintf` implementation (Doom uses these for HUD text). Can be simplified heavily.
   - **Math**: No floating point is needed — Doom uses fixed-point math internally

3. **WAD file handling** — The shareware `doom1.wad` (~4 MiB) is [freely distributable](https://doomwiki.org/wiki/DOOM1.WAD) and can be downloaded from various sources (e.g., `archive.org`). It must be accessible to the game:
   - **Option A (filesystem)**: Pack doom1.wad into the easy-fs image during build. The game opens and reads it via `open`/`read`/`close` syscalls. This requires either `lseek` support (Doom seeks within the WAD) or loading the entire WAD into memory at startup.
   - **Option B (embedded)**: Embed doom1.wad as a static byte array in the binary (via `include_bytes!` or linker section). Simple but makes the binary ~4 MiB larger.
   - **Recommended**: Option A with an in-memory WAD cache — load the entire 4 MiB WAD into heap at startup, then serve all file reads from the in-memory copy. This avoids the need for `lseek`.

4. **DG_ platform functions** — Implement the 6 doomgeneric functions:
   - **`DG_Init()`**: Call `fb_info()` to get screen dimensions. The framebuffer is 1280×800 but Doom renders at 640×400 (or 320×200 upscaled). May need to center the Doom output or scale.
   - **`DG_DrawFrame()`**: Doom's XRGB8888 and the GPU's BGRA8888 share the same little-endian byte layout (B, G, R in bytes 0-2). Just set byte 3 (alpha) to 0xFF for each pixel, then call `fb_write()` + `fb_flush()`. No channel swapping needed.
   - **`DG_SleepMs(ms)`**: Call `sleep(ms)` or a yield-loop with `get_time()`.
   - **`DG_GetTicksMs()`**: Call `get_time()` (returns milliseconds).
   - **`DG_GetKey(pressed, key)`**: Poll keyboard via `read(STDIN, buf)`. Translate VirtIO keycodes to Doom keycodes (defined in `doomkeys.h`). Maintain a ring buffer of key events. Return 1 if event available, 0 if not.
   - **`DG_SetWindowTitle(title)`**: No-op (no window title on VNC framebuffer).

5. **Doom keycode translation** — VirtIO keycodes → Doom keycodes:
   ```
   VirtIO   → Doom
   103 (Up)    → KEY_UPARROW (0xad)
   108 (Down)  → KEY_DOWNARROW (0xaf)
   105 (Left)  → KEY_LEFTARROW (0xac)
   106 (Right) → KEY_RIGHTARROW (0xae)
   29 (Ctrl)   → KEY_FIRE (0xa3)
   57 (Space)  → KEY_USE (0xa2)
   28 (Enter)  → KEY_ENTER (13)
   1 (Escape)  → KEY_ESCAPE (27)
   42 (LShift) → KEY_RSHIFT (0xb6)
   56 (Alt)    → KEY_LALT (0xb8)
   59-68 (F1-F10) → KEY_F1-KEY_F10 (0x80+offset)
   2-11 (1-0)  → '1'-'0' (ASCII)
   Normal keys → lowercase ASCII
   ```

6. **Build system** — Three approaches:
   - **Approach A (cc crate)**: Use the `cc` Rust crate in the **ch8-doom kernel crate's** `build.rs` (not the user crate — the user crate's build.rs handles app compilation). Add `cc = "1"` to `[build-dependencies]`. Compile all doomgeneric `.c` files with flags: `-march=rv64gc -mabi=lp64d -O2 -ffreestanding -nostdlib -DNORMALUNIX -I<doomgeneric/include>`. The `cc` crate produces a static library that gets linked into the user binary. **Note**: the user binary is compiled separately by the kernel's `build.rs` — you need to arrange for the C library to be available when linking the Doom user binary. The cleanest way is to have the **user crate's** `build.rs` use `cc` gated on `#[cfg(feature = "doom")]`.
   - **Approach B (Makefile + static library)**: Clone doomgeneric, write a standalone Makefile that cross-compiles all `.c` files with `riscv64-unknown-elf-gcc` into `libdoom.a`. The kernel's `build.rs` invokes this Makefile before building the `doom` user binary. Link via `println!("cargo:rustc-link-lib=static=doom")` and `println!("cargo:rustc-link-search=native=<path>")`.
   - **Approach C (Rust rewrite)**: Manually translate the C code to Rust or use a Rust Doom port. Most work but eliminates C toolchain dependency entirely.
   - **Recommended**: Approach A (cc crate in user crate's build.rs). Requires `riscv64-unknown-elf-gcc` or `riscv64-linux-gnu-gcc` installed.

   **Important**: Exclude `i_main.c` from the C sources — it defines `main()` which conflicts with the Rust binary's entry point. The Rust binary calls `doomgeneric_Create()` and `doomgeneric_Tick()` directly.

   **Important**: The C code expects standard library headers (`<stdio.h>`, `<stdlib.h>`, `<string.h>`). With `-ffreestanding -nostdlib`, these won't be available. You must either: (a) provide stub headers that declare the functions your libc_shim implements, or (b) use `--specs=nano.specs` with newlib-nano (which provides headers) and override the actual implementations at link time with your Rust shims.

7. **One binary** (`doom.rs`) — Feature-gated with `#[cfg(feature = "doom")]`. The binary calls `doomgeneric_Create()` and enters the tick loop.

8. **Optional threading** — Doom is single-threaded by default. To demonstrate ch8's threading:
   - Spawn a dedicated input thread that continuously polls keyboard events and pushes them into a shared ring buffer protected by a mutex
   - The main thread runs the game loop, reading from the shared buffer
   - This demonstrates `thread_create`, `mutex_create`/`lock`/`unlock`, and shared-memory concurrency

## doomgeneric Porting API — Detailed Reference

### Pixel Format

Doom's internal renderer produces an 8-bit palettized 320×200 image in `I_VideoBuffer`. The doomgeneric layer upscales this to `DOOMGENERIC_RESX × DOOMGENERIC_RESY` (default 640×400) and converts palette indices to 32-bit pixels stored in `DG_ScreenBuffer`.

**Pixel layout in memory** (little-endian `uint32_t` = 0xXXRRGGBB):
- Byte 0 (lowest address) = Blue
- Byte 1 = Green
- Byte 2 = Red
- Byte 3 (highest address) = Unused (X)

The rCore VirtIO-GPU framebuffer uses **B8G8R8A8_UNORM** which has the same byte layout: B, G, R, A. Since bytes 0-2 match exactly, you only need to set byte 3 to 0xFF (opaque alpha) for each pixel. No channel swapping is needed. In practice, you can just OR each pixel with `0xFF000000` before writing.

### Resolution

- Doom internal: 320×200 (8-bit palettized)
- DG_ScreenBuffer: 640×400 (XRGB8888, upscaled 2×)
- rCore framebuffer: 1280×800

The 640×400 Doom output should be centered on the 1280×800 display, or scaled 2× to fill the screen. For centering: offset_x = (1280 - 640) / 2 = 320, offset_y = (800 - 400) / 2 = 200.

Alternatively, set `DOOMGENERIC_RESX=320 DOOMGENERIC_RESY=200` and scale 4× (320×4=1280, 200×4=800) to exactly fill the display. This requires manual upscaling in `DG_DrawFrame()` but gives perfect pixel-art scaling.

### Doom Keycodes (from doomkeys.h)

```c
#define KEY_RIGHTARROW  0xae
#define KEY_LEFTARROW   0xac
#define KEY_UPARROW     0xad
#define KEY_DOWNARROW   0xaf
#define KEY_FIRE        0xa3   // Ctrl
#define KEY_USE         0xa2   // Space
#define KEY_ESCAPE      27
#define KEY_ENTER       13
#define KEY_TAB         9
#define KEY_BACKSPACE   127
#define KEY_RSHIFT      0xb6
#define KEY_RCTRL       0xb5
#define KEY_RALT        0xb9
#define KEY_LALT        0xb8
#define KEY_F1          (0x80+0x3b)  // 0xbb
#define KEY_F2          (0x80+0x3c)  // 0xbc
// ... F3-F12 follow sequentially
```

### Memory Requirements

| Component | Size |
|-----------|------|
| Zone memory (Doom heap) | 6 MiB (`I_ZoneBase` calls `malloc(6*1024*1024)`) |
| DG_ScreenBuffer (640×400×4) | ~1 MiB |
| WAD file (doom1.wad, loaded into RAM) | ~4 MiB |
| C stack depth | ~64 KiB recommended |
| Code + BSS | ~500 KiB |
| **Total user process** | **~12 MiB** |

### libc Functions Used by Doom

**Must implement** (Doom will not link without these):
- `malloc`, `free`, `realloc`, `calloc` — memory allocation (backed by sbrk)
- `memcpy`, `memset`, `memmove` — memory operations (compiler builtins or manual)
- `strlen`, `strcpy`, `strncpy`, `strcmp`, `strncmp`, `strncasecmp`, `strcat`, `strncat`, `strchr`, `strrchr`, `strstr` — string operations
- `sprintf`, `snprintf`, `sscanf`, `printf`, `puts`, `fprintf`, `vsnprintf` — formatted I/O (can use a minimal implementation)
- `fopen`, `fclose`, `fread`, `fwrite`, `fseek`, `ftell`, `remove` — file I/O
- `atoi`, `strtol`, `abs` — numeric conversion
- `toupper`, `tolower`, `isdigit`, `isspace`, `isalnum`, `isprint`, `isupper` — character classification
- `qsort` — sorting
- `exit`, `abort` — process control
- `errno` — global error number (can be a dummy)

**Can stub/no-op** (non-critical):
- `signal()`, `atexit()` — no-op
- `mkdir()`, `stat()`, `access()` — return -1
- `getenv()` — return NULL

### Save Game Files

Doom's F2/F3 save/load functionality writes save game files (`doomsav0.dsg` through `doomsav5.dsg`, each ~200 KiB). These require `fopen`/`fwrite`/`fclose` for writing and `fopen`/`fread`/`fclose` for reading. Since save files are written sequentially (no seeking needed for writes), they work naturally with easy-fs's `open(CREATE|WRONLY|TRUNC)` + `write()` + `close()`. For reading, open with `RDONLY` and read sequentially — no seek needed.

The libc shim must support opening multiple files simultaneously (the WAD stays open while save files are read/written). Track a small table of open files (8 slots is plenty).

### easy-fs lseek Limitation

Doom's WAD loader heavily uses `fseek()` to jump around in the WAD file. easy-fs's `FileHandle` tracks an internal read offset that advances on each `read_at()`, but there is **no `lseek` syscall** in the rCore tutorial.

**Workaround — in-memory WAD**: At game startup, open doom1.wad, read the entire ~4 MiB into a heap buffer, close the file. Implement a `FILE` abstraction over this buffer that supports seek/read/tell operations internally. This is the simplest and most performant approach — all subsequent WAD reads are memory copies.

```rust
struct MemFile {
    data: &'static [u8],  // WAD data loaded into heap
    pos: usize,           // current read position
}

// fopen("doom1.wad", "rb") → load from fs into heap, return MemFile
// fread(buf, size, count, file) → memcpy from file.data[file.pos..], advance pos
// fseek(file, offset, SEEK_SET/CUR/END) → adjust file.pos
// ftell(file) → return file.pos
// fclose(file) → drop (don't free the WAD buffer — it's needed for the whole game)
```

## VirtIO Keyboard — Keycode Translation

```
EV_KEY (event_type == 1):
  value == 1 (press)   → keycode (0-127)
  value == 0 (release) → keycode | 0x80 (128-255)

Key mappings (VirtIO scancode → game action):
  Up=103, Down=108, Left=105, Right=106  → movement
  LCtrl=29                                → fire
  Space=57                                → use/open doors
  LShift=42                               → run
  LAlt=56                                 → strafe
  Enter=28                                → menu confirm
  Escape=1                                → menu/pause
  Tab=15                                  → automap
  F1-F10 = 59-68                          → function keys
  1-0 = 2-11                              → weapon select
  A-Z = 30,48,46,32,18,33,34,35,23,36,37,38,50,49,24,25,16,19,31,20,22,47,17,45,21,44
```

## Lessons Learned from ch1-ch7 (apply ALL of these)

1. **virtio-drivers 0.3.0, not 0.1.0** — v0.1.0 keyboard bug. Update Hal trait for v0.3.0 API.

2. **FB_WRITE must NOT flush GPU** — Separate FB_WRITE (2001) and FB_FLUSH (2003). Doom renders many rectangles per frame; flushing once at the end of DG_DrawFrame is critical for performance.

3. **DMA pool needs explicit mapping** — Map 0x8500_0000..0x8550_0000 in `kernel_space()`.

4. **Keyboard: return press AND release, drain sync events** — Doom needs both press and release events for smooth control.

5. **User heap MUST be large** — Doom needs ~12 MiB of user memory. The default 16 KiB heap will not work. Ensure `sbrk()` can grow the heap to at least 16 MiB. The kernel's `MEMORY` must be large enough to back this.

6. **User stack MUST be large** — Doom's call stack is deep (recursive rendering, BSP traversal). Use at least 64 KiB (16 pages) for the main thread stack.

7. **XRGB to BGRA pixel format** — Doom's pixel layout (XRGB8888) happens to match the VirtIO-GPU's BGRA layout in the B, G, R bytes. Just ensure the alpha byte is 0xFF.

8. **In-memory WAD avoids lseek** — Load the entire WAD into heap, implement FILE over a memory buffer.

9. **MEMORY = 128+ MiB** — Doom user process (~12 MiB) + kernel + GPU framebuffer + page tables.

10. **C cross-compilation** — Use `cc` crate in build.rs for cleanest integration. Target `riscv64gc-unknown-none-elf`.

11. **Feature-gate binary body** — `#[cfg(feature = "doom")]` for `cargo publish --dry-run`.

12. **build.rs caching** — `cargo:rerun-if-env-changed=TG_SKIP_USER_APPS`.

13. **initproc CHAPTER mechanism** — Set `CHAPTER=doom` in build.rs for initproc. Add `"doom" => "doom"` branch to initproc.rs.

14. **FB_WRITE in ch8 uses get_current_proc()** — Ch8's process/thread split means the address space is on Process, not Thread. Use `PROCESSOR.get_mut().get_current_proc().unwrap()` to access the page table for user pointer translation.

15. **Doom is single-threaded internally** — The engine is not thread-safe. If using threads for input polling, protect shared state with a mutex. Do not call Doom engine functions from multiple threads.

## Files to Create/Modify

| File | Action | Purpose |
|------|--------|---------|
| `ch8-doom/Cargo.toml` | Modify | New name, `virtio-drivers = "0.3.0"` |
| `ch8-doom/.cargo/config.toml` | Modify | Add GPU + keyboard + VNC, `-serial stdio`, `-m 256M`, `TG_USER_DIR` |
| `ch8-doom/src/main.rs` | Modify | GPU/keyboard init, FB syscalls, keyboard IO::read, increase MEMORY, expand MMIO |
| `ch8-doom/src/allocator.rs` | Create | DMA bump allocator + v0.3.0 Hal trait |
| `ch8-doom/src/virtio.rs` | Create | MMIO scanning for GPU/keyboard |
| `ch8-doom/src/virtio_block.rs` | Modify | Use shared Hal from allocator.rs |
| `ch8-doom/src/process.rs` | Modify | Increase user stack size to 64 KiB (16 pages) |
| `ch8-doom/build.rs` | Modify | ch8_doom case_key, `--features doom`, `CHAPTER=doom` |
| `ch8-doom/test.sh` | Modify | Headless CI |
| `user/Cargo.toml` | Modify | Add `doom` feature, `cc` build-dependency |
| `user/src/lib.rs` | Modify | Add doom module |
| `user/src/doom/` | Create | Doom port: libc shims, DG_ functions, FFI bindings |
| `user/src/doom/mod.rs` | Create | Module entry, `run_game()` function |
| `user/src/doom/libc_shim.rs` | Create | Minimal libc: malloc/free, string ops, printf, file I/O over rCore syscalls |
| `user/src/doom/platform.rs` | Create | DG_ function implementations (framebuffer, keyboard, timing) |
| `user/src/doom/keymap.rs` | Create | VirtIO → Doom keycode translation table |
| `user/src/bin/doom.rs` | Create | Binary entry (feature-gated body) |
| `user/cases.toml` | Modify | Add ch8_doom section |
| `user/build.rs` | Modify | Compile doomgeneric C sources via `cc` crate when `doom` feature active |
| `user/doomgeneric/` | Create (vendored) | Clone of doomgeneric C source (or git submodule) |

## Acceptance Criteria

1. `cargo run` boots QEMU, loads doom1.wad from filesystem, and starts the Doom title screen on VNC
2. Doom's main menu is navigable with arrow keys and Enter
3. A new game can be started and the player can move through E1M1
4. WASD or arrow keys move the player; Ctrl fires; Space opens doors; Shift runs
5. 3D rendering is correct (walls, floors, ceilings, sprites visible)
6. Sound is not required (Doom runs fine with no sound driver — `nosound` mode)
7. Frame rate is playable (>5 FPS at 640×400, >10 FPS at 320×200)
8. Escape opens the pause/options menu
9. F-keys work (F1=help, F2=save, F3=load, etc.)
10. Game does not crash or hang during normal play
11. Existing ch8 test programs still pass (serial output correct)
12. `TG_SKIP_USER_APPS=1 cargo check` passes
13. doom1.wad is loaded from the filesystem (not hardcoded in binary)

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine porting strategy, libc scope, WAD loading approach. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — implementation plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task
4. `superpowers:requesting-code-review` — final review

Do NOT publish the crate — the user will review and publish manually.

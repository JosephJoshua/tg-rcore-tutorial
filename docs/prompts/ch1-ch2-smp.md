Extend `tg-rcore-tutorial-ch1` and `tg-rcore-tutorial-ch2` with multi-core (SMP) support, including the M-mode boot layer. Each chapter becomes its own standalone crate that boots all available harts from the ground up, demonstrates multi-core output, and exposes the synchronization challenges inherent in concurrent execution.

## Setup

Create three working crates by copying the originals:

```bash
cp -r tg-rcore-tutorial-sbi jsph-tg-rcore-tutorial-sbi-smp
rm -rf jsph-tg-rcore-tutorial-sbi-smp/{target,Cargo.lock,.gitrepo}

cp -r tg-rcore-tutorial-ch1 jsph-tg-rcore-tutorial-ch1-smp
rm -rf jsph-tg-rcore-tutorial-ch1-smp/{target,Cargo.lock,.gitrepo}

cp -r tg-rcore-tutorial-ch2 jsph-tg-rcore-tutorial-ch2-smp
rm -rf jsph-tg-rcore-tutorial-ch2-smp/{target,Cargo.lock,.gitrepo}
```

Then update each crate's `Cargo.toml`:

**SBI crate** (`jsph-tg-rcore-tutorial-sbi-smp/Cargo.toml`):
- Change `name` to `"jsph-tg-rcore-tutorial-sbi-smp"`
- Change `authors` to `["Joseph Joshua Anggita <jj.anggita@gmail.com>"]`
- Change `description` to describe the SMP-capable SBI

**Ch1-SMP** (`jsph-tg-rcore-tutorial-ch1-smp/Cargo.toml`):
- Change `name` to `"jsph-tg-rcore-tutorial-ch1-smp"`
- Change `authors` to `["Joseph Joshua Anggita <jj.anggita@gmail.com>"]`
- Point `tg-sbi` dependency to local SBI copy:
  ```toml
  tg-sbi = { package = "jsph-tg-rcore-tutorial-sbi-smp", path = "../jsph-tg-rcore-tutorial-sbi-smp", features = ["nobios"] }
  ```
- Change other path dependencies to version-only (for crates.io publishing compatibility)

**Ch2-SMP** (`jsph-tg-rcore-tutorial-ch2-smp/Cargo.toml`):
- Same pattern as ch1: rename, repoint tg-sbi to the local SBI copy with `nobios` feature

**All modifications go into the three new crate directories** — do NOT modify the originals or any shared crates.

## Goal

**Ch1-SMP**: Boot 4 harts on a bare-metal RISC-V system, all the way from M-mode. All harts print identification messages to serial. Demonstrate both unsynchronized (garbled) and synchronized (ordered) output, making the case for why synchronization matters. Optionally run a parallel computation benchmark.

**Ch2-SMP**: Boot 4 harts on the batch OS. All harts initialize and announce themselves, but only hart 0 runs user programs — the others enter a wait loop. This demonstrates multi-core boot infrastructure without multi-core scheduling (which comes in later chapters).

## Architecture Overview

### Boot Model: Full M-mode Multi-Hart Boot

With QEMU `-bios none -smp 4`, **all 4 harts start executing at `_m_start` (0x80000000) simultaneously**. This is the rawest form of multi-core boot — no firmware does the hard work for us. Each hart must:

1. Identify itself (read `mhartid` CSR)
2. Set up its own M-mode stack
3. Configure its own M-mode CSRs (PMP, delegation, trap vector, counters)
4. Transition to S-mode (`mret`)

The original `tg-rcore-tutorial-sbi` does none of this — it assumes a single hart. The copied SBI crate (`jsph-tg-rcore-tutorial-sbi-smp`) gets the multi-hart treatment.

### What changes in the SBI crate

| Aspect | Original SBI | SMP SBI |
|--------|-------------|---------|
| `mhartid` | Never read | Read at entry, used for stack/timer indexing |
| M-mode stack | Single 16 KiB stack | NUM_HARTS × 4 KiB stacks, indexed by hart ID |
| `mscratch` | Points to single stack top | Per-hart: each hart's `mscratch` → its own M-mode stack top |
| `mepc` target | `_start` (assumes one hart) | `_start` (all harts jump here, but each carries its hart ID) |
| Hart ID passing | None | `a0 = mhartid` on `mret` to S-mode |
| Hart overflow guard | None | Harts with `id >= NUM_HARTS` enter WFI park loop |
| CLINT mtimecmp | Hardcoded `0x2004000` (hart 0 only) | `0x2004000 + 8 * hartid` (per-hart timer) |

### M-mode Boot Sequence (m_entry.asm)

```asm
    .equ NUM_HARTS,     4
    .equ M_STACK_SIZE,  4096     # 4 KiB per-hart M-mode stack

    .section .text.m_entry
    .globl _m_start
_m_start:
    # === Step 1: Identify this hart ===
    csrr  t0, mhartid           # t0 = hart ID

    # === Step 2: Guard against overflow ===
    li    t1, NUM_HARTS
    bge   t0, t1, .park_hart    # If hartid >= NUM_HARTS, park forever

    # === Step 3: Per-hart M-mode stack ===
    la    t1, m_stacks_base
    li    t2, M_STACK_SIZE
    addi  t3, t0, 1             # (hartid + 1)
    mul   t2, t2, t3            # offset = (hartid + 1) * stack_size
    add   sp, t1, t2            # sp = base + offset (top of this hart's stack)
    csrw  mscratch, sp          # Save for M-mode trap entry (csrrw sp, mscratch, sp)

    # === Step 4: M-mode configuration (each hart does its own) ===

    # mstatus: MPP=01 (S-mode), MPIE=1
    li    t1, (1 << 11) | (1 << 7)
    csrw  mstatus, t1

    # mepc: S-mode entry point
    la    t1, _start
    csrw  mepc, t1

    # mtvec: M-mode trap handler
    la    t1, m_trap_vector
    csrw  mtvec, t1

    # Interrupt/exception delegation to S-mode
    # (except ecall from S-mode, which stays in M-mode for SBI calls)
    li    t1, 0xffff
    csrw  mideleg, t1
    li    t1, 0xffff
    li    t2, (1 << 9)          # Exception 9: ecall from S-mode
    not   t2, t2
    and   t1, t1, t2
    csrw  medeleg, t1

    # PMP: allow S-mode full access (teaching simplification)
    li    t1, -1
    csrw  pmpaddr0, t1
    li    t1, 0x0f              # TOR + RWX
    csrw  pmpcfg0, t1

    # Allow S-mode to read counters (rdtime, rdcycle)
    li    t1, -1
    csrw  mcounteren, t1

    # === Step 5: Pass hart ID to S-mode and jump ===
    csrr  a0, mhartid           # a0 = hart ID (S-mode receives this)
    mret                        # Jump to _start in S-mode

.park_hart:
    wfi
    j     .park_hart

    # === Per-hart M-mode stacks ===
    .section .bss.m_stack
    .globl m_stacks_base
m_stacks_base:
    .space M_STACK_SIZE * NUM_HARTS   # 4 KiB × 4 = 16 KiB total
    .globl m_stacks_top
m_stacks_top:
```

**Key insight for students**: Every CSR write (`csrw mstatus`, `csrw mideleg`, etc.) is local to the hart executing it. CSRs are NOT shared memory — each hart has its own copy. This is why every hart can independently configure itself without locks.

### M-mode Trap Handler (msbi.rs) Changes

The trap handler (`m_trap_handler`) needs one change: **per-hart CLINT timer address**.

The CLINT (Core Local Interruptor) on QEMU virt has per-hart registers:
- `mtimecmp` for hart N at address `0x2004000 + 8 * N`
- `mtime` (shared, read-only) at `0x200BFF8`

Change `handle_timer()`:
```rust
fn handle_timer(time: u64) -> SbiRet {
    let hartid: usize;
    unsafe { core::arch::asm!("csrr {}, mhartid", out(reg) hartid) };
    let mtimecmp_addr = 0x200_4000 + 8 * hartid;
    unsafe { (mtimecmp_addr as *mut u64).write_volatile(time) };
    // Clear pending S-mode timer interrupt
    unsafe { asm!("csrc mip, {}", in(reg) (1 << 5)) };
    SbiRet::success(0)
}
```

The M-mode trap vector already works per-hart because `mscratch` is per-hart (set up in `_m_start`). Each hart swaps to its own M-mode stack on trap entry.

Everything else in msbi.rs (UART putchar/getchar, shutdown) is hart-independent and works unchanged. Note: UART writes from multiple harts WILL interleave — this is by design for the demos.

### Linker Script Changes

The linker script (`NOBIOS_SCRIPT` pattern) stays the same structure but the M-mode stack section grows:

```ld
/* M-mode stacks: 4 KiB × NUM_HARTS instead of a single 16 KiB stack */
.bss.m_stack : {
    *(.bss.m_stack)
}
```

The size increase is handled by the assembly `.space M_STACK_SIZE * NUM_HARTS` directive — the linker script just places the section. Keep the existing `m_stack_lower_bound` / `m_stack_top` symbols or replace with `m_stacks_base` / `m_stacks_top` as needed by the new assembly.

Note: The existing linker script uses `ENTRY(_m_start)` and places M-mode code at `0x80000000`, S-mode code at `0x80200000`. This remains correct for SMP — all harts enter at `_m_start`.

### S-mode Boot Sequence

After `mret`, all harts arrive at `_start` in S-mode with `a0 = hartid`. The S-mode entry needs to:

1. **Save hart ID** before any instruction that clobbers `a0`
2. **Set up per-hart S-mode stack** (separate from the M-mode stacks)
3. **Branch**: hart 0 → `rust_main()`, others → `secondary_main()`

```rust
#[naked]
#[no_mangle]
#[link_section = ".text.entry"]
unsafe extern "C" fn _start() -> ! {
    // a0 = hart ID (from M-mode mret)
    const STACK_SIZE: usize = 64 * 1024;
    const NUM_HARTS: usize = 4;

    #[link_section = ".bss.uninit"]
    static mut HART_STACKS: [[u8; STACK_SIZE]; NUM_HARTS] = [[0; STACK_SIZE]; NUM_HARTS];

    core::arch::naked_asm!(
        // Save hart ID to tp register immediately
        "mv   tp, a0",

        // Calculate per-hart stack: sp = HART_STACKS + (hartid + 1) * STACK_SIZE
        "la   t0, {stacks}",
        "li   t1, {stack_size}",
        "addi t2, a0, 1",
        "mul  t1, t1, t2",
        "add  sp, t0, t1",

        // Branch: hart 0 → rust_main, others → secondary_main
        "bnez a0, {secondary}",
        "j    {main}",

        stacks     = sym HART_STACKS,
        stack_size = const STACK_SIZE,
        main       = sym rust_main,
        secondary  = sym secondary_main,
    )
}
```

**Hart 0** (`rust_main`):
1. Clear BSS (`zero_bss()`)
2. Initialize console
3. Initialize syscall handlers (ch2 only)
4. Signal secondary harts: `BOOT_HART_DONE.store(true, Release)`
5. Run demos (ch1) or batch processing (ch2)
6. Shutdown

**Secondary harts** (`secondary_main`):
1. Spin-wait on `BOOT_HART_DONE` (Acquire ordering)
2. Print identification message (with spinlock)
3. Participate in demos (ch1) or enter WFI loop (ch2)

### Synchronization Primitives

Implement from scratch in `src/smp.rs` (no external crate):

**1. Atomic Boot Flag**

```rust
use core::sync::atomic::{AtomicBool, Ordering};

static BOOT_HART_DONE: AtomicBool = AtomicBool::new(false);
```

The `Release` store on hart 0 pairs with `Acquire` loads on secondary harts. This ensures all of hart 0's initialization (BSS clearing, console init) is visible before secondaries proceed. On RISC-V, this compiles to `amoswap` or `fence` instructions — students can inspect the disassembly to see real memory barriers.

**2. Spinlock (for UART output)**

```rust
pub struct SpinLock {
    locked: AtomicBool,
}

impl SpinLock {
    pub const fn new() -> Self {
        Self { locked: AtomicBool::new(false) }
    }

    pub fn lock(&self) {
        while self.locked.compare_exchange_weak(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_err() {
            // TTAS: spin on read (no bus contention) until lock appears free
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }

    pub fn unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}
```

The test-and-test-and-set (TTAS) inner loop is important: a naive `compare_exchange` loop hammers the bus with atomic RMW operations. The inner `load(Relaxed)` reads the cache line without bus transactions until the lock is released.

**3. Barrier (for demo synchronization)**

```rust
use core::sync::atomic::{AtomicUsize, Ordering};

pub struct Barrier {
    count: AtomicUsize,
    generation: AtomicUsize,
}

impl Barrier {
    pub const fn new() -> Self {
        Self { count: AtomicUsize::new(0), generation: AtomicUsize::new(0) }
    }

    pub fn wait(&self, num_harts: usize) {
        let gen = self.generation.load(Ordering::Acquire);
        if self.count.fetch_add(1, Ordering::AcqRel) + 1 == num_harts {
            // Last arrival: reset count and advance generation
            self.count.store(0, Ordering::Relaxed);
            self.generation.store(gen.wrapping_add(1), Ordering::Release);
        } else {
            // Wait for generation to advance
            while self.generation.load(Ordering::Acquire) == gen {
                core::hint::spin_loop();
            }
        }
    }
}
```

The generation counter prevents the race where a fast hart re-enters the barrier before a slow hart exits the previous one.

**4. Hart ID accessor**

```rust
#[inline(always)]
pub fn hart_id() -> usize {
    let id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) id) };
    id
}
```

## Ch1-SMP Detailed Design

Ch1 is a bare-metal S-mode program with no user/kernel separation.

### Demonstrations (in order of execution)

**Demo 1: Unsynchronized Output**

All 4 harts print a multi-line message simultaneously WITHOUT a lock. The output will be interleaved/garbled. This is shown first to immediately demonstrate the problem:

```
== Demo 1: Unsynchronized Output ==
[Hart 2] Hel[Hart 0] He[Hart 1] Hello[Hart 3]lo from hart 0
[Hart 1]llo from hart 1
...
```

Each hart prints something like: `"Hello from hart N!\nThis is line 2.\nThis is line 3.\n"`. Without a lock, individual characters from different harts intermix because `console_putchar` is per-byte.

**Demo 2: Synchronized Output**

Same messages, but each hart acquires the spinlock before printing its entire block. Output is clean:

```
== Demo 2: Synchronized Output ==
[Hart 0] Hello from hart 0!
[Hart 0] This is line 2.
[Hart 0] This is line 3.
[Hart 2] Hello from hart 2!
...
```

Hart ordering may vary between runs — this itself is a teaching point about non-determinism.

**Demo 3: Parallel Computation**

Each hart independently computes a CPU-bound workload (e.g., sum 1..10_000_000 using volatile reads to prevent optimization). Hart 0 measures wall-clock time for:
- Sequential: hart 0 does all 4 workloads one after another
- Parallel: all 4 harts work simultaneously

Report speedup. Use `rdtime` for timing (QEMU virt timer = 10 MHz).

```
== Demo 3: Parallel Computation ==
Sequential: 4 workloads took 800ms
Parallel:   4 workloads took 210ms
Speedup: 3.81x
```

**After demos**: Hart 0 optionally renders tangram "OS" on VirtIO-GPU (other harts are idle). Then shutdown.

### Hart Discovery

Rather than hardcoding 4 harts, discover the actual count at runtime. After hart 0 completes initialization, it can probe which harts exist by checking which hart stacks were used (or simply count up to NUM_HARTS and let the demos use `ACTIVE_HARTS` count). For simplicity, use the compile-time `NUM_HARTS` constant but handle the case where QEMU starts fewer harts gracefully.

### Barrier Usage Between Demos

```rust
static DEMO_BARRIER: Barrier = Barrier::new();

fn demo_sequence(hartid: usize, num_harts: usize) {
    // Demo 1: unsynchronized
    if hartid == 0 { println!("== Demo 1: Unsynchronized Output =="); }
    DEMO_BARRIER.wait(num_harts);
    print_without_lock(hartid);
    DEMO_BARRIER.wait(num_harts);

    // Demo 2: synchronized
    if hartid == 0 { println!("\n== Demo 2: Synchronized Output =="); }
    DEMO_BARRIER.wait(num_harts);
    print_with_lock(hartid);
    DEMO_BARRIER.wait(num_harts);

    // Demo 3: parallel computation (only hart 0 drives this)
    if hartid == 0 {
        println!("\n== Demo 3: Parallel Computation ==");
        run_parallel_benchmark(num_harts);
    }
}
```

## Ch2-SMP Detailed Design

Ch2 is a batch processing OS with U-mode/S-mode separation and syscalls.

### Design

- All harts boot through M-mode and arrive at `_start` in S-mode
- Hart 0: clear BSS, init console, init syscall handlers, signal BOOT_DONE, run batch processing
- Secondary harts: wait for BOOT_DONE, print presence message (with lock), enter WFI loop
- All existing user programs run on hart 0 only — output is identical to original ch2

### Secondary Hart Trap Vector

Hart 0 sets up `stvec` for trap handling (user-mode ecall/exceptions). Secondary harts don't execute user code, but should have a basic trap handler for safety:

```rust
extern "C" fn secondary_main() -> ! {
    let hartid = hart_id();

    while !BOOT_HART_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    PRINT_LOCK.lock();
    // Use SBI putchar directly (console may not be hart-safe for println!)
    // Or use println! if the console is initialized by hart 0
    println!("[Hart {}] online, entering idle loop", hartid);
    PRINT_LOCK.unlock();

    // Set stvec to a panic handler for this hart
    unsafe {
        core::arch::asm!(
            "csrw stvec, {}",
            in(reg) secondary_trap_panic as usize,
        );
    }

    loop { unsafe { core::arch::asm!("wfi") }; }
}

extern "C" fn secondary_trap_panic() -> ! {
    // If a secondary hart traps unexpectedly, halt
    loop { unsafe { core::arch::asm!("wfi") }; }
}
```

### Console Thread-Safety

The `tg_console` crate uses a global `Console` trait object. `println!` calls go through this → `Console::put_char()` → `tg_sbi::console_putchar()`. This path is inherently NOT thread-safe (no locking). For ch2-smp, only hart 0 uses `println!` after the initial boot messages, so this is fine. The boot messages from secondary harts use the spinlock wrapper.

For a more robust design, the spinlock could wrap the Console trait — but this is beyond ch2's scope.

## QEMU Configuration

Both ch1-smp and ch2-smp:

```toml
[target.riscv64gc-unknown-none-elf]
runner = [
    "qemu-system-riscv64",
    "-machine", "virt",
    "-smp", "4",              # 4 hardware threads
    "-bios", "none",          # Use our SBI (no OpenSBI)
    "-serial", "stdio",       # Serial console to terminal
    "-kernel",
]
```

Ch1-smp also needs `-device virtio-gpu-device` and `-display none` for headless testing (same as original ch1-tangram).

**Key**: `-smp 4` tells QEMU to start 4 harts. All 4 begin executing at `_m_start` simultaneously. Without our SBI multi-hart changes, this would crash.

## Key Constraints

- All modifications go in `jsph-tg-rcore-tutorial-sbi-smp/`, `jsph-tg-rcore-tutorial-ch1-smp/`, and `jsph-tg-rcore-tutorial-ch2-smp/`
- Do NOT modify the original `tg-rcore-tutorial-sbi`, `tg-rcore-tutorial-linker`, or other shared crates
- Keep `-bios none` — the custom SBI with `nobios` feature handles M-mode
- Ch1-SMP: no page tables (identity mapping), static allocator for VirtIO GPU
- Ch2-SMP: no page tables, `tg_linker::KernelLayout::locate()` still works (linker symbols are identical)
- `cargo check` and `cargo publish --dry-run` must pass on host (Mac x86_64/ARM64). Non-riscv64 stubs remain.
- `NUM_HARTS = 4` as a compile-time constant. Harts with `id >= NUM_HARTS` are parked in M-mode (`wfi` loop). Test with `-smp 1` through `-smp 8` to verify graceful handling.
- The `tp` register stores hart ID. In `no_std` bare-metal code, the Rust compiler does not generate code that touches `tp` (no TLS support). Verify in disassembly if issues arise.

## Files to Create/Modify

### SBI-SMP (jsph-tg-rcore-tutorial-sbi-smp/)

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` | Modify | New name/author/description |
| `src/m_entry.asm` | Rewrite | Multi-hart: read mhartid, per-hart M-mode stacks, hart overflow guard, pass hartid via a0 |
| `src/msbi.rs` | Modify | Per-hart CLINT timer address: `0x2004000 + 8 * hartid` |
| `src/lib.rs` | Keep | SBI call wrappers unchanged |

### Ch1-SMP (jsph-tg-rcore-tutorial-ch1-smp/)

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` | Modify | New name/author, point tg-sbi to local SBI-SMP |
| `.cargo/config.toml` | Modify | Add `-smp 4`, keep `-bios none`, keep GPU device |
| `build.rs` | Modify | Keep existing linker script structure (it already has M-mode sections) |
| `src/main.rs` | Rewrite | Multi-hart `_start`, hart 0 init, demo sequence, shutdown |
| `src/smp.rs` | Create | SpinLock, Barrier, `hart_id()`, boot flag, `secondary_main()` |
| `src/allocator.rs` | Keep | VirtIO GPU allocator (unchanged) |
| `src/gpu.rs` | Keep | VirtIO GPU driver (hart 0 only) |
| `src/tangram.rs` | Keep | Tangram rendering (hart 0 only) |
| `test.sh` | Modify | `-smp 4`, check all 4 harts print, check demos |

### Ch2-SMP (jsph-tg-rcore-tutorial-ch2-smp/)

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` | Modify | New name/author, point tg-sbi to local SBI-SMP |
| `.cargo/config.toml` | Modify | Add `-smp 4`, keep `-bios none`, keep TG_USER env |
| `build.rs` | Modify | Keep linker script and user app build logic |
| `src/main.rs` | Rewrite | Multi-hart `_start`, hart 0 init + batch loop, secondary WFI |
| `src/smp.rs` | Create | Same primitives as ch1-smp |
| `test.sh` | Modify | `-smp 4`, check multi-hart boot + existing ch2 output |

## Testing & Demonstrations

### Test Scripts

**Ch1-SMP** (`test.sh`):
```bash
#!/bin/bash
cargo build 2>&1
BINARY="target/riscv64gc-unknown-none-elf/debug/jsph-tg-rcore-tutorial-ch1-smp"

OUTPUT=$(timeout 30 qemu-system-riscv64 \
    -machine virt \
    -smp 4 \
    -display none \
    -serial stdio \
    -bios none \
    -device virtio-gpu-device \
    -kernel "$BINARY" 2>&1)

PASS=true
for i in 0 1 2 3; do
    if ! echo "$OUTPUT" | grep -q "Hart $i"; then
        echo "FAIL: Hart $i output not found"
        PASS=false
    fi
done

if echo "$OUTPUT" | grep -q "Demo 2"; then
    echo "PASS: Demo 2 section found"
fi

if [ "$PASS" = true ]; then
    echo "Test PASSED: All 4 harts produced output"
    exit 0
else
    echo "Test FAILED"
    echo "$OUTPUT"
    exit 1
fi
```

**Ch2-SMP** (`test.sh`):
```bash
#!/bin/bash
set -e
cargo build 2>&1

OUTPUT=$(timeout 30 qemu-system-riscv64 \
    -machine virt \
    -smp 4 \
    -nographic \
    -bios none \
    -kernel "target/riscv64gc-unknown-none-elf/debug/jsph-tg-rcore-tutorial-ch2-smp" 2>&1)

echo "$OUTPUT"

PASS=true
for i in 0 1 2 3; do
    if ! echo "$OUTPUT" | grep -q "Hart $i"; then
        echo "FAIL: Hart $i not found"
        PASS=false
    fi
done

if echo "$OUTPUT" | grep -q "Hello, world!"; then
    echo "PASS: User programs executed"
else
    echo "FAIL: User program output missing"
    PASS=false
fi

[ "$PASS" = true ] && echo "Test PASSED" && exit 0
echo "Test FAILED" && exit 1
```

### Demonstration Scenarios for Report

1. **UART Race Condition** (ch1 Demo 1): Capture garbled output. Show specific character interleaving. Explain: each `println!` decomposes to N individual `console_putchar()` SBI calls. Between any two SBI calls, another hart can execute its own `console_putchar()`.

2. **Spinlock Correctness** (ch1 Demo 2): Clean output with lock. Discuss: what if we used `Relaxed` ordering? (Answer: on RISC-V RVWMO, stores from hart 0 might not be visible to other harts without proper fences → secondary harts could see stale data.)

3. **Parallel Speedup** (ch1 Demo 3): Measure wall-clock time with 1, 2, 3, 4 harts. Expect near-linear speedup on QEMU for CPU-bound work.

4. **Boot Order Non-determinism**: Run ch1-smp multiple times. Note that hart ordering in Demo 1 varies. This demonstrates that multi-core execution is inherently non-deterministic.

5. **Varying `-smp` Count**: Run with `-smp 1`, `-smp 2`, `-smp 4`, `-smp 8`. Verify: with `-smp 1`, only hart 0 runs (degenerates to single-core). With `-smp 8`, harts 4-7 are parked by the M-mode guard.

6. **Memory Fence Demonstration**: Optionally show what happens without `Release`/`Acquire` on `BOOT_HART_DONE`. On QEMU (which often has stronger memory model than real hardware), this may still work — document why the fences are theoretically necessary on RVWMO hardware.

## Acceptance Criteria

### Ch1-SMP
1. `cargo run` boots 4 harts from M-mode, all print identification messages
2. Demo 1 shows visibly interleaved multi-hart output (without lock)
3. Demo 2 shows clean per-hart output (with spinlock)
4. Demo 3 shows measurable parallel speedup (optional but recommended)
5. Tangram "OS" still renders on VirtIO-GPU (hart 0 only)
6. Clean shutdown after all demos
7. Works with `-smp 1` (single-hart) through `-smp 8` (excess harts parked)
8. `bash test.sh` passes
9. `cargo check` and `cargo publish --dry-run` pass on host

### Ch2-SMP
1. `cargo run` boots 4 harts, all print boot messages
2. Only hart 0 runs user programs — existing ch2 output identical
3. Secondary harts print presence and enter WFI
4. Clean shutdown after batch processing
5. Works with `-smp 1` (degenerates to original behavior)
6. `bash test.sh` passes
7. `cargo check` and `cargo publish --dry-run` pass on host

## Likely Pitfalls

1. **All harts hit `_m_start` simultaneously**: This is the fundamental challenge. Without per-hart stacks, all harts write to the same `sp`, corrupting each other's stack frames. The very first thing `_m_start` must do is `csrr t0, mhartid` and compute a per-hart stack. No instructions before `csrr mhartid` may use the stack.

2. **`mscratch` is per-hart**: CSRs are local to each hart — `csrw mscratch, sp` on hart 0 does NOT affect hart 1's `mscratch`. This is correct behavior and means the M-mode trap handler naturally gets per-hart stacks. But if you forget to set `mscratch` on secondary harts, their first M-mode trap (e.g., timer interrupt) will use garbage as a stack pointer.

3. **BSS race with `BOOT_HART_DONE`**: The flag is in BSS. Hart 0 clears BSS, then sets the flag to `true`. Secondary harts spin-read the flag. Since `AtomicBool::new(false)` is all-zeros, and BSS starts as zeros on QEMU (RAM is zeroed), secondaries correctly read `false` even before hart 0's `zero_bss()` runs. **But**: if any code between `_start` and the flag spin-loop touches a non-zero-initialized BSS variable, that variable might not be cleared yet. Keep secondary harts' pre-BOOT_DONE path minimal.

4. **CLINT timer per-hart**: The original SBI hardcodes `CLINT_MTIMECMP = 0x2004000` (hart 0's timer). Hart 1's timer is at `0x2004008`, hart 2 at `0x2004010`, etc. If you forget this change, `set_timer()` from hart 1 will set hart 0's timer, causing hart 0 to get unexpected interrupts and hart 1 to never get its timer interrupt.

5. **Stack collision with user programs (ch2)**: Ch2 loads user binaries at `0x80400000`. With 4 harts × 64 KiB S-mode stacks (256 KiB) in BSS, plus the kernel image at `0x80200000`, the kernel end (`__end`) must be below `0x80400000`. If the kernel grows too large (e.g., tangram geometry data), stacks collide with user memory. Check with `riscv64-linux-gnu-objdump -t | grep __end`. **Alternative for ch2**: Use smaller stacks for secondary harts (they only spin/WFI, so 4 KiB is enough).

6. **UART garbling is per-byte, not per-line**: SBI `console_putchar` writes one byte at a time. Between any two bytes of "Hello", another hart can insert its own byte. The spinlock must protect the entire message, not individual characters. This means the lock scope is around `println!` or a custom `locked_print()`, not around `console_putchar`.

7. **`tp` register in naked functions**: In the naked `_start` function, you control every instruction. But once you jump to Rust code (`rust_main`), the compiler owns the register allocator. Verify via `objdump -d` that the compiler never writes to `tp`. In practice, `no_std` code without TLS won't touch `tp`, but if you link against any crate that uses `#[thread_local]`, it might.

8. **`a0` clobbering between `mret` and `mv tp, a0`**: After `mret`, `a0` contains the hart ID. The very first instruction in `_start` must be `mv tp, a0` (or save it somewhere). If `_start` begins with `la sp, STACKS` and that pseudo-instruction uses `a0` as a temporary... check the actual RISC-V expansion. `la` typically expands to `auipc` + `addi` using the destination register only, so `la t0, STACKS` won't touch `a0`. But verify.

9. **Macro expansion of `naked_asm!`**: Rust's `naked_asm!` does not allow symbolic operand references in the same way as `asm!`. The `{stacks}`, `{stack_size}`, etc. syntax has restrictions. If the compiler complains about the `mul` instruction (RV64 requires M extension), verify the target is `riscv64gc` (which includes M). Also, `mul` may not be available in RV64I — but `riscv64gc` includes everything.

10. **Cargo incremental compilation**: Modifying `m_entry.asm` in the SBI crate might not trigger recompilation of the chapter crate. Use `cargo clean` in the chapter crate if M-mode behavior seems stale.

11. **macOS QEMU**: On macOS via Homebrew, `qemu-system-riscv64` should work out of the box. `-bios none` doesn't require any firmware files. Verify installation with `qemu-system-riscv64 --version` (should be >= 7.0).

12. **Demo 3 optimization**: If the computation workload uses a simple loop, the compiler might optimize it away. Use `core::hint::black_box()` or `read_volatile` to prevent elision. Otherwise "parallel speedup" will show 0ms for both sequential and parallel.

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine M-mode boot, demo design, synchronization. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — create step-by-step plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task with review checkpoints. Start with the SBI crate, then ch1-smp, then ch2-smp.
4. `superpowers:requesting-code-review` — final review

After implementation, write a design analysis report to `docs/reports/ch1-ch2-smp.md` covering:
- M-mode multi-hart boot design (with diagrams or pseudocode)
- S-mode synchronization: boot flag, spinlock, barrier
- All bugs encountered and how they were fixed
- Test results with `-smp 1`, 2, 4, 8
- Garbled UART output examples (before/after spinlock)
- Parallel computation speedup measurements
- Discussion: what would change for multi-core *scheduling* (preview of ch3+)
- Common multi-core programming pitfalls

Do NOT publish the crates — the user will review and publish manually.

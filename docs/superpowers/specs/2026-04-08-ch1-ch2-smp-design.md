# Ch1-Ch2 SMP (Multi-Core) — Design Spec

## Overview

Extend ch1 and ch2 with symmetric multiprocessing (SMP) support, including full M-mode multi-hart boot. Three new crates — `jsph-tg-rcore-tutorial-sbi-smp`, `jsph-tg-rcore-tutorial-ch1-smp`, `jsph-tg-rcore-tutorial-ch2-smp` — boot all 4 QEMU harts from `_m_start` in M-mode, demonstrate multi-core output and synchronization primitives (ch1), and show multi-hart boot infrastructure for a batch OS (ch2).

No original crates are modified. The SBI-SMP crate is the multi-hart fork of `tg-rcore-tutorial-sbi`; both chapter crates depend on it via local path with `nobios` feature.

## Architecture

### Approach: Full M-mode Multi-Hart Boot

With QEMU `-bios none -smp 4`, all 4 harts start executing at `_m_start` (0x80000000) simultaneously. Each hart independently:
1. Reads `mhartid` CSR to identify itself
2. Sets up a per-hart M-mode stack (4 KiB each, indexed by hart ID)
3. Configures its own M-mode CSRs (PMP, delegation, trap vector, counters)
4. Transitions to S-mode via `mret` with `a0 = hartid`

Harts with `id >= NUM_HARTS` (4) are parked in a WFI loop in M-mode.

### Component Diagram

```
M-mode (jsph-tg-rcore-tutorial-sbi-smp)
├── m_entry.asm          — Multi-hart entry: per-hart stacks, CSR config, mret
├── msbi.rs              — M-mode trap handler with per-hart CLINT timer
└── lib.rs               — SBI call wrappers (unchanged)

S-mode: Ch1-SMP (jsph-tg-rcore-tutorial-ch1-smp)
├── main.rs              — Multi-hart _start, rust_main (hart 0), demo sequence
├── smp.rs               — SpinLock, Barrier, hart_id(), boot flag, secondary_main
├── allocator.rs         — VirtIO GPU allocator (unchanged, hart 0 only)
├── gpu.rs               — VirtIO GPU driver (unchanged, hart 0 only)
└── tangram.rs           — Tangram rendering (unchanged, hart 0 only)

S-mode: Ch2-SMP (jsph-tg-rcore-tutorial-ch2-smp)
├── main.rs              — Multi-hart _start, hart 0 batch processing, secondary WFI
├── smp.rs               — SpinLock, hart_id(), boot flag, secondary_main
└── (batch/trap/syscall) — Existing ch2 logic on hart 0 only
```

## SBI-SMP Changes

### M-mode Boot (m_entry.asm)

| Aspect | Original SBI | SMP SBI |
|--------|-------------|---------|
| `mhartid` | Never read | Read at entry, used for stack/timer indexing |
| M-mode stack | Single 16 KiB stack | 4 × 4 KiB stacks, indexed by hart ID |
| `mscratch` | Points to single stack top | Per-hart: each hart's `mscratch` → its own stack top |
| `mepc` target | `_start` (assumes one hart) | `_start` (all harts, each carries hartid in `a0`) |
| Hart overflow | None | `id >= NUM_HARTS` → WFI park loop |
| CLINT mtimecmp | Hardcoded `0x2004000` (hart 0) | `0x2004000 + 8 * hartid` (per-hart) |

### M-mode Trap Handler (msbi.rs)

Only change: `handle_timer()` reads `mhartid` and computes per-hart CLINT address:
```rust
let mtimecmp_addr = 0x200_4000 + 8 * hartid;
```

Everything else (UART, shutdown, base) is hart-independent.

## S-mode Boot Sequence

After `mret`, all harts arrive at `_start` with `a0 = hartid`:

1. **Save hart ID**: `mv tp, a0` (immediately, before any clobber)
2. **Per-hart stack**: `sp = HART_STACKS + (hartid + 1) * STACK_SIZE`
3. **Branch**: hart 0 → `rust_main()`, others → `secondary_main()`

Hart 0 clears BSS, initializes console, signals `BOOT_HART_DONE` (Release), runs demos/batch.
Secondary harts spin on `BOOT_HART_DONE` (Acquire), print identification, then participate in demos (ch1) or enter WFI (ch2).

## Synchronization Primitives (smp.rs)

### 1. Boot Flag

```rust
static BOOT_HART_DONE: AtomicBool = AtomicBool::new(false);
```

Release/Acquire pair ensures hart 0's initialization is visible before secondaries proceed. On RISC-V RVWMO, this compiles to fence instructions.

### 2. SpinLock (TTAS)

Test-and-test-and-set design: inner `load(Relaxed)` spin avoids bus contention from repeated `compare_exchange_weak` operations. Protects UART output so multi-line messages print atomically.

### 3. Barrier (generation-based)

Counter + generation prevents the ABA race where a fast hart re-enters the barrier before a slow hart exits. Used between demos in ch1.

### 4. Hart ID

Reads `tp` register (set once in `_start`, never touched by `no_std` compiler).

## Ch1-SMP Demos

**Demo 1 — Unsynchronized Output**: All 4 harts print multi-line messages without lock. Output is garbled (character-level interleaving from per-byte `console_putchar`). Demonstrates the problem.

**Demo 2 — Synchronized Output**: Same messages with spinlock. Output is clean per-hart blocks. Hart ordering may vary (non-determinism teaching point).

**Demo 3 — Parallel Computation**: Each hart sums 1..N using `read_volatile` to prevent optimization. Hart 0 measures sequential (4 workloads on hart 0 alone) vs parallel (all 4 harts) wall-clock time via `rdtime` (10 MHz). Reports speedup (~3.8x expected).

Barriers synchronize between demos. After demos, hart 0 renders tangram "OS" on VirtIO-GPU, then shuts down.

## Ch2-SMP Design

- All 4 harts boot through M-mode → S-mode
- Hart 0: clear BSS, init console, init syscall handlers, signal BOOT_DONE, run batch processing (existing ch2 logic)
- Secondary harts: wait for BOOT_DONE, print presence with spinlock, set `stvec` to panic handler, enter WFI loop
- All user programs run on hart 0 only — output identical to original ch2
- Console thread-safety: only hart 0 uses `println!` after boot messages

## QEMU Configuration

Both chapters use:
```
-machine virt -smp 4 -bios none -serial stdio
```
Ch1 also: `-device virtio-gpu-device -display none` (headless test).

## Key Constraints

- All modifications in the three new crate directories only
- `-bios none` with `nobios` feature handles M-mode
- `NUM_HARTS = 4` compile-time constant
- `tp` register stores hart ID (safe in `no_std` without TLS)
- `cargo check` and `cargo publish --dry-run` must pass on host (non-riscv64 stubs)
- Graceful handling of `-smp 1` through `-smp 8`

## Acceptance Criteria

### Ch1-SMP
1. `cargo run` boots 4 harts from M-mode, all print identification
2. Demo 1: visibly interleaved output (no lock)
3. Demo 2: clean per-hart output (spinlock)
4. Demo 3: measurable parallel speedup
5. Tangram "OS" renders on VirtIO-GPU (hart 0 only)
6. Clean shutdown, works with `-smp 1` through `-smp 8`
7. `bash test.sh` passes
8. `cargo check` passes on host

### Ch2-SMP
1. `cargo run` boots 4 harts, all print boot messages
2. Only hart 0 runs user programs — existing ch2 output preserved
3. Secondary harts print presence + enter WFI
4. Clean shutdown, works with `-smp 1`
5. `bash test.sh` passes
6. `cargo check` passes on host

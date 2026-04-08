# Ch3-Ch5 SMP Design Specification

## Overview

Extend chapters 3, 4, and 5 with symmetric multi-processing (SMP) support. All 4 QEMU harts participate in scheduling and executing tasks/processes. Each chapter becomes a standalone crate (`jsph-tg-rcore-tutorial-ch{3,4,5}-smp`) that does NOT modify any original or shared crates.

## Approach: Coarse-Grained Locking

After evaluating coarse-grained vs fine-grained locking, **coarse-grained** is the right choice:
- Single `SpinLockIrq` around shared scheduler state
- Simple, correct, minimal deadlock surface
- 4 harts with short critical sections = negligible contention
- Teaching-appropriate: demonstrates the core SMP concepts without lock-ordering complexity

## Shared Infrastructure (smp.rs)

All three chapters share identical `smp.rs` containing:

### SpinLockIrq (interrupt-safe spinlock)
- Disables S-mode interrupts (`sstatus.SIE`) before acquiring
- TTAS (test-and-test-and-set) spin pattern with `core::hint::spin_loop()`
- Guard-based API: `SpinLockIrqGuard` restores interrupt state on Drop
- This prevents the #1 SMP kernel bug: deadlock when timer fires while holding lock

### SpinLockIrq<T> (generic wrapper)
- Wraps arbitrary data `T` behind `SpinLockIrq` + `UnsafeCell`
- `lock()` returns `SpinLockIrqGuard<T>` with `Deref`/`DerefMut`
- Used for: scheduler state, process list, print lock

### hart_id()
- Reads `tp` register (set once in `_start`)
- Returns `usize` hart ID

### BOOT_HART_DONE
- `AtomicBool` flag for boot handshake (same as ch2-smp)
- Hart 0: store with Release after init
- Secondary harts: spin with Acquire

### PRINT_LOCK
- `SpinLockIrq` (no data, just lock/unlock) for console output
- Wraps all `println!` calls to prevent interleaved multi-hart output

### Per-Hart State
```rust
const NUM_HARTS: usize = 4;
struct PerHart { current_task: Option<usize> }  // ch3: task index, ch5: ProcId
static PER_HART: [UnsafeCell<PerHart>; NUM_HARTS] = ...;
```

## Ch3-SMP: Multi-Core Round-Robin

### Data Design
```rust
struct SharedState {
    ready_queue: VecDeque<usize>,     // task indices
    tcbs: [TaskControlBlock; APP_CAPACITY],
    finished_count: usize,
    total_tasks: usize,
}
static SCHEDULER: SpinLockIrq<SharedState> = ...;
```

### Execution Flow
1. Hart 0: zero BSS, init console/syscall, load apps into TCBs, populate ready queue, signal BOOT_HART_DONE
2. All harts: enter `scheduling_loop()`
3. Loop: lock -> pop from ready queue -> unlock -> execute task -> lock -> handle result -> unlock
4. Task execution happens OUTSIDE the lock (the long-running part has zero contention)
5. When `finished_count >= total_tasks`: hart 0 shuts down, others WFI

### Timer
- Each hart calls `set_timer()` independently (per-hart CLINT via SBI-SMP)
- Timer interrupt returns to scheduling loop (task was preempted)
- `sie::set_stimer()` called by each hart

### SYSCALL_COUNTS
- Wrap in `SpinLockIrq` or use per-hart arrays merged at shutdown
- Simple approach: lock around increment (short critical section)

## Ch4-SMP: Multi-Core + Virtual Memory

### Additional Complexity
- **satp is per-hart**: each hart tracks its own active page table, no coordination needed
- **Portal (MultislotPortal)**: mapped at same VA in all address spaces, works with concurrent satp switches
- **Allocator**: `tg-kernel-alloc` must be wrapped in SpinLockIrq for concurrent page allocation
- **Process list**: `Vec<Process>` wrapped in SpinLockIrq

### Data Design
```rust
static PROCESSES: SpinLockIrq<Vec<Process>> = ...;
```

### Execution Flow
Same scheduling pattern as ch3 but:
1. Task execution goes through `ForeignContext.execute(portal, ())` (address space switch)
2. Each hart's `satp` switch is independent
3. Page allocation during sbrk/mmap protected by allocator lock
4. `sfence.vma` after page table modifications (local only, no remote shootdown)

### Allocator Thread Safety
Wrap the frame allocator in SpinLockIrq. The heap allocator (`tg-kernel-alloc`) likely uses a linked-list allocator internally - wrap its global allocator in a lock.

## Ch5-SMP: Multi-Core Process Management

### Additional Complexity
- **Per-hart current**: `PManager.current` is single-valued; replace with per-hart array
- **Process tree**: `ProcRel` parent-child relationships mutated by fork/wait/exit concurrently
- **Stride scheduling**: `fetch()` scans queue for minimum stride - must be atomic

### Data Design
```rust
// Wrap entire PManager in SpinLockIrq (coarse-grained)
static PROCESSOR: SpinLockIrq<PManager<Process, ProcManager>> = ...;

// Per-hart current process tracking
static PER_HART_CURRENT: [AtomicUsize; NUM_HARTS] = ...; // 0 = no process
```

### Process State Machine
```
Ready -> Running(hart)    [scheduler picks, remove from queue]
Running(hart) -> Ready    [timer preemption, push back to queue]
Running(hart) -> Blocked  [wait() with no dead children]
Running(hart) -> Exited   [exit() called]
Blocked -> Ready          [child exits, wakes parent]
```

### Fork/Exec/Wait Under SMP
- **fork()**: Lock PROCESSOR, allocate new ProcId, deep-copy address space (allocator locked separately), insert child, add to ready queue, unlock
- **exec()**: Lock PROCESSOR, get current process, replace address space, unlock
- **wait()**: Lock PROCESSOR, check dead_children. If found: collect exit code, remove child, unlock. If not: mark parent as blocked, unlock, yield
- **exit()**: Lock PROCESSOR, mark self as exited, notify parent (unblock if waiting), unlock

### Handling `current`
The original `PManager` stores `current: Option<ProcId>` as a single field. For SMP:
- Before `execute()`: set `PER_HART_CURRENT[hartid]` 
- In syscall handlers: read `PER_HART_CURRENT[hartid]` to know which process is running
- The PManager's `current` field is not used in SMP mode

## Boot Assembly (_start)

All three chapters use identical boot assembly (from ch2-smp pattern):

```rust
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    // a0 = hartid from M-mode
    // 1. mv tp, a0 (save hartid)
    // 2. Calculate per-hart stack from HART_STACKS
    // 3. bnez a0, secondary_main
    // 4. j rust_main
}
```

**HART_STACKS in .boot.stack section** (not .bss) - prevents zero_bss() corruption.

## QEMU Configuration

All chapters: `-smp 4` added to runner in `.cargo/config.toml`.

## Files Per Chapter

| File | Action |
|------|--------|
| `Cargo.toml` | New name, author, sbi-smp dependency |
| `.cargo/config.toml` | Add `-smp 4`, fix TG_USER_DIR |
| `src/main.rs` | Rewrite: multi-hart boot, scheduling loop, SpinLockIrq wrappers |
| `src/smp.rs` | Create: SpinLockIrq, hart_id, BOOT_HART_DONE, PerHart, PRINT_LOCK |
| `src/task.rs` | Minor: remove global mutable SYSCALL_COUNTS (ch3) |
| `test.sh` | Adjust checker invocation if needed |

## Acceptance Criteria

1. `cargo run` boots 4 harts, all participate in scheduling
2. All existing test output is correct (test.sh passes)
3. `cargo check` passes on host (non-riscv64 stubs)
4. Works with `-smp 1` (degrades gracefully)
5. No deadlocks under normal operation
6. Console output is not interleaved (PRINT_LOCK)

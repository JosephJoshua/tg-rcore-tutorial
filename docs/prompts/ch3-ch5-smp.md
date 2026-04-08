Extend `tg-rcore-tutorial-ch3`, `tg-rcore-tutorial-ch4`, and `tg-rcore-tutorial-ch5` with multi-core (SMP) scheduling support. This builds on the SBI-SMP crate from ch1-ch2-smp (multi-hart M-mode boot, per-hart stacks, per-hart CLINT timer). Each chapter becomes its own standalone crate where ALL harts participate in scheduling and executing tasks/processes — not just booting and idling.

This is the hardest SMP work in the tutorial: shared scheduler state, per-hart kernel stacks, concurrent trap handling, and process tree mutations under contention. These are among the most bug-prone areas in real OS kernels.

## Prerequisites

The SBI-SMP crate (`jsph-tg-rcore-tutorial-sbi-smp`) from the ch1-ch2-smp task must be complete. It provides:
- Multi-hart M-mode boot (per-hart stacks, `mhartid` routing, overflow guard)
- Per-hart CLINT timer (`0x2004000 + 8 * hartid`)
- Hart ID passed to S-mode via `a0` on `mret`

## Setup

Create three working crates by copying the originals:

```bash
cp -r tg-rcore-tutorial-ch3 jsph-tg-rcore-tutorial-ch3-smp
rm -rf jsph-tg-rcore-tutorial-ch3-smp/{target,Cargo.lock,.gitrepo}

cp -r tg-rcore-tutorial-ch4 jsph-tg-rcore-tutorial-ch4-smp
rm -rf jsph-tg-rcore-tutorial-ch4-smp/{target,Cargo.lock,.gitrepo}

cp -r tg-rcore-tutorial-ch5 jsph-tg-rcore-tutorial-ch5-smp
rm -rf jsph-tg-rcore-tutorial-ch5-smp/{target,Cargo.lock,.gitrepo}
```

For each crate's `Cargo.toml`:
- Change `name` to `"jsph-tg-rcore-tutorial-chN-smp"` (N=3,4,5)
- Change `authors` to `["Joseph Joshua Anggita <jj.anggita@gmail.com>"]`
- Point `tg-sbi` dependency to local SBI-SMP:
  ```toml
  tg-sbi = { package = "jsph-tg-rcore-tutorial-sbi-smp", path = "../jsph-tg-rcore-tutorial-sbi-smp", features = ["nobios"] }
  ```
- Change other path dependencies to version-only (remove `path = "..."`)

For each `.cargo/config.toml`:
- Add `-smp`, `"4"` to the QEMU runner array
- Set `TG_USER_DIR` to point to `../tg-rcore-tutorial-user` if needed

**All modifications go into the new crate directories** — do NOT modify originals or shared crates.

## Goals

**Ch3-SMP**: All 4 harts run the round-robin scheduler. Tasks are distributed across harts — each hart picks the next ready task from a shared run queue (protected by a spinlock). Timer interrupts fire independently on each hart. Demonstrates true multi-core time-sharing.

**Ch4-SMP**: Same multi-hart scheduling but with virtual memory. Each hart maintains its own `satp` CSR (page table base). The portal mechanism (`MultislotPortal`) must work correctly when multiple harts switch address spaces concurrently. The kernel page table is shared (read-only after init); user page tables are per-process.

**Ch5-SMP**: Full multi-core process management. Fork/exec/wait work correctly under concurrent execution. The process tree (`ProcRel`) and ready queue are protected by fine-grained locks. This is the most complex chapter — process state transitions (running, suspended, exited) must be atomic.

## Architecture Overview

### Shared Infrastructure (all three chapters)

#### 1. Per-Hart S-mode Stacks and Boot Handshake

Same pattern as ch1-ch2-smp:

```rust
#[unsafe(naked)]
unsafe extern "C" fn _start() -> ! {
    // a0 = hartid from M-mode
    core::arch::naked_asm!(
        "mv   tp, a0",
        "la   t0, {stacks}",
        "li   t1, {stack_size}",
        "addi t2, a0, 1",
        "mul  t1, t1, t2",
        "add  sp, t0, t1",
        "bnez a0, {secondary}",
        "j    {main}",
        ...
    )
}
```

**CRITICAL for ch3/ch4/ch5**: `HART_STACKS` must be in `.boot.stack` section (not `.bss`), because `zero_bss()` would destroy stacks that secondary harts are already using. This was a real bug in ch2-smp.

#### 2. SpinLock with Interrupt Disable (SpinLockIrq)

The ch1-ch2 `SpinLock` is insufficient for kernel code. When a hart holds a spinlock and a timer interrupt fires on that hart, the interrupt handler might try to acquire the same lock, causing **deadlock** (spinlocks are not recursive).

Implement `SpinLockIrq` that disables S-mode interrupts before acquiring:

```rust
pub struct SpinLockIrq {
    locked: AtomicBool,
}

impl SpinLockIrq {
    pub fn lock(&self) -> SpinLockIrqGuard<'_> {
        // Save and disable S-mode interrupts (sstatus.SIE)
        let sie_was_enabled: bool;
        unsafe {
            let sstatus: usize;
            core::arch::asm!("csrr {}, sstatus", out(reg) sstatus);
            sie_was_enabled = (sstatus & (1 << 1)) != 0;
            if sie_was_enabled {
                core::arch::asm!("csrc sstatus, {}", in(reg) (1 << 1));
            }
        }

        // Now acquire the spinlock (TTAS pattern)
        while self.locked.compare_exchange_weak(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_err() {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }

        SpinLockIrqGuard { lock: self, sie_was_enabled }
    }
}

pub struct SpinLockIrqGuard<'a> {
    lock: &'a SpinLockIrq,
    sie_was_enabled: bool,
}

impl Drop for SpinLockIrqGuard<'_> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
        if self.sie_was_enabled {
            unsafe { core::arch::asm!("csrs sstatus, {}", in(reg) (1 << 1)); }
        }
    }
}
```

This is the pattern ArceOS uses (`SpinLockNoIrq`). The guard-based API ensures interrupts are restored even on early returns or panics.

#### 3. Per-Hart State

Each hart needs its own:
- Kernel stack (set up in `_start`, saved via trap mechanism)
- Current task/process ID (which task this hart is currently running)
- Timer configuration (each hart's timer interrupt fires independently)

Use hart ID (from `tp` register) to index into per-hart arrays:

```rust
const NUM_HARTS: usize = 4;

struct PerHart {
    current_task: Option<TaskId>,
}

static PER_HART: [UnsafeCell<PerHart>; NUM_HARTS] = ...;

fn current_hart() -> &'static mut PerHart {
    let hartid = hart_id();
    unsafe { &mut *PER_HART[hartid].get() }
}
```

#### 4. Hart ID Accessor

```rust
#[inline(always)]
pub fn hart_id() -> usize {
    let id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) id) };
    id
}
```

### Ch3-SMP: Multi-Core Round-Robin Scheduling

#### Current Ch3 Architecture (single-core)

Ch3 uses a flat loop in `rust_main`:
```rust
let mut remain = tasks.len();
let mut i = 0;
while remain > 0 {
    if !tcbs[i].finish {
        set_timer(12500);
        tcbs[i].ctx.execute();
        match scause {
            Timer => { /* preempted, move to next task */ }
            Syscall => { /* handle, continue or finish */ }
            Exception => { tcbs[i].finish = true; remain -= 1; }
        }
    }
    i = (i + 1) % tasks.len();
}
```

Key data structures:
- `tcbs: [TaskControlBlock; APP_CAPACITY]` — static array of task contexts
- `TaskControlBlock { ctx: LocalContext, finish: bool, stack: [usize; 1024] }` — per-task state
- `SYSCALL_COUNTS: [[u32; 500]; APP_CAPACITY]` — global mutable, no synchronization

#### Multi-Core Design

Replace the single scheduling loop with a shared run queue:

```rust
struct SharedScheduler {
    lock: SpinLockIrq,
    ready_queue: VecDeque<usize>,  // indices into tcbs[]
    tcbs: [Option<TaskControlBlock>; APP_CAPACITY],
    finished_count: AtomicUsize,
    total_tasks: usize,
}

static SCHEDULER: SharedScheduler = ...;
```

Each hart runs a scheduling loop:

```rust
fn scheduling_loop(hartid: usize) -> ! {
    loop {
        // Try to get a task from the shared queue
        let task_idx = {
            let guard = SCHEDULER.lock.lock();
            SCHEDULER.ready_queue.pop_front()
        }; // lock released here

        match task_idx {
            Some(idx) => {
                set_timer(12500);
                let tcb = &mut SCHEDULER.tcbs[idx].as_mut().unwrap();
                unsafe { tcb.ctx.execute() };

                match scause::read().cause() {
                    Timer => {
                        // Preempted: put back in ready queue
                        let guard = SCHEDULER.lock.lock();
                        SCHEDULER.ready_queue.push_back(idx);
                    }
                    UserEnvCall => {
                        match handle_syscall(&mut tcb.ctx, idx) {
                            Done => {
                                let guard = SCHEDULER.lock.lock();
                                SCHEDULER.ready_queue.push_back(idx);
                            }
                            Exit(_) => {
                                SCHEDULER.finished_count.fetch_add(1, Ordering::AcqRel);
                            }
                        }
                    }
                    _ => {
                        SCHEDULER.finished_count.fetch_add(1, Ordering::AcqRel);
                    }
                }
            }
            None => {
                if SCHEDULER.finished_count.load(Ordering::Acquire) >= SCHEDULER.total_tasks {
                    if hartid == 0 { shutdown(false); }
                    else { loop { unsafe { core::arch::asm!("wfi") }; } }
                }
                core::hint::spin_loop();
            }
        }
    }
}
```

**Key design points:**
- The lock is held only while accessing the queue (short critical section)
- Task execution happens OUTSIDE the lock (long-running, no contention)
- Each hart has its own timer interrupt (per-hart CLINT)
- `finished_count` is atomic — no lock needed

#### Timer Interrupt Handling

Each hart's timer fires independently. The interrupt handler:
1. Reads `scause` to confirm it's a timer interrupt
2. Calls `set_timer()` for the next interval (per-hart CLINT address)
3. Returns to the scheduling loop (the task was preempted)

#### Console Output

`println!` via SBI `console_putchar` is not thread-safe. Wrap console output in a global `PRINT_LOCK: SpinLockIrq`. This lock must disable interrupts because the timer handler or panic handler might also print.

### Ch4-SMP: Multi-Core with Virtual Memory

#### What Ch4 Adds

Ch4 introduces Sv39 page tables:
- **Kernel address space**: Identity-mapped (VA == PA), shared across all harts (read-only after init)
- **User address space**: Each process has its own page table tree
- **MultislotPortal**: A code page mapped at the same VA in all address spaces, used during `satp` switching

#### Multi-Core Considerations

**`satp` is per-hart.** Each hart has its own `satp` CSR pointing to its active page table. When hart 0 runs process A and hart 1 runs process B, they have different `satp` values. Hardware handles this correctly.

**The portal.** When switching `satp`, the instruction stream must continue executing. The portal page is mapped at the same VA in ALL address spaces, so code on that page remains valid regardless of `satp`. With multiple harts switching `satp` concurrently, each hart goes through the portal independently — no coordination needed, because each hart has its own `satp`.

**Page table allocation.** Creating new page tables calls the kernel memory allocator. With multiple harts allocating simultaneously, the allocator MUST be protected by a lock. Wrap allocator calls in a `SpinLockIrq`.

**TLB consistency.** When one hart modifies a page table (sbrk, mmap), other harts in the same address space might have stale TLB entries. Fix: `sfence.vma` after modifications. For ch4-smp, assume each process runs on only one hart at a time, making local `sfence.vma` sufficient. Full remote TLB shootdown via IPI is beyond scope.

**`PROCESSES` list.** Must be wrapped in a `SpinLockIrq`:

```rust
static PROCESSES: SpinLockIrq<Vec<Process>> = SpinLockIrq::new(Vec::new());
```

### Ch5-SMP: Multi-Core Process Management

This is the most complex chapter.

#### What Ch5 Adds

- **`PROCESSOR`**: A global `PManager<Process, ProcManager>` containing:
  - `tasks: BTreeMap<ProcId, Process>` — all processes
  - `ready_queue: VecDeque<ProcId>` — FIFO ready queue
  - `rel_map: BTreeMap<ProcId, ProcRel>` — parent-child relationships
  - `current: Option<ProcId>` — currently running process
- **Stride scheduling**: Each process has `stride` and `priority`; scheduler picks minimum stride
- **fork()**: Deep-copies the entire address space
- **exec()**: Replaces address space with new ELF
- **wait()**: Parent blocks until child exits, collects exit code

#### Multi-Core Design

**Per-hart `current`.** The single `current: Option<ProcId>` must become per-hart:

```rust
struct PerHartState {
    current: Option<ProcId>,
}
static PER_HART: [UnsafeCell<PerHartState>; NUM_HARTS] = ...;
```

**Locking the process manager.** Two approaches:

**(A) Coarse-grained** (recommended for teaching): Single `SpinLockIrq` around the entire `PManager`. Simple, correct, but serializes all scheduling decisions. Good enough for 4 harts.

**(B) Fine-grained**: Separate locks for ready queue, process table, and relationship map. More realistic but much harder to get right (lock ordering, deadlock avoidance).

**Fork concurrency.** Two harts forking simultaneously both call the memory allocator. With the allocator lock, this works correctly but serializes allocation. The copy itself (memcpy of page contents) can proceed in parallel.

**Wait/exit race.** Child calls `exit()` on hart 1, parent calls `wait()` on hart 0. Both modify `dead_children` in `ProcRel`. With the coarse-grained lock, this is serialized.

**Process state transitions.** Must be atomic:

```
Ready -> Running(hart)    [scheduler picks it]
Running(hart) -> Ready    [timer preemption or yield]
Running(hart) -> Suspended [wait() with no dead children]
Running(hart) -> Exited   [exit() called]
Suspended -> Ready        [child exits, notifies parent]
```

## QEMU Configuration

All three chapters:

```toml
[target.riscv64gc-unknown-none-elf]
runner = [
    "qemu-system-riscv64",
    "-machine", "virt",
    "-smp", "4",
    "-nographic",
    "-bios", "none",
    "-kernel",
]
```

## Key Constraints

- All modifications go in `jsph-tg-rcore-tutorial-ch{3,4,5}-smp/` directories
- Do NOT modify originals or shared crates
- Keep `-bios none` — the SBI-SMP crate handles M-mode
- `NUM_HARTS = 4` compile-time constant; handle `-smp 1` gracefully
- `HART_STACKS` in `.boot.stack` section (not `.bss`) for chapters with `zero_bss()`
- `cargo check` must pass on host (non-riscv64 stubs)
- Each hart gets its own timer interrupt via per-hart CLINT
- `tp` register stores hart ID (set once in `_start`)
- Console output protected by `SpinLockIrq`
- Memory allocator must be thread-safe (wrap in lock)

## Files to Create/Modify

### Per Chapter (ch3/ch4/ch5)

| File | Action | Purpose |
|------|--------|---------|
| `Cargo.toml` | Modify | New name/author, point tg-sbi to SBI-SMP |
| `.cargo/config.toml` | Modify | Add `-smp 4`, set `TG_USER_DIR` |
| `src/main.rs` | Rewrite | Multi-hart `_start`, per-hart scheduling loop, boot handshake |
| `src/smp.rs` | Create | `SpinLockIrq`, `hart_id()`, `BOOT_HART_DONE`, per-hart state |
| `test.sh` | Modify | Check multi-hart output + existing test output |

### Ch3-SMP Additional

| File | Action | Purpose |
|------|--------|---------|
| `src/main.rs` | Rewrite | `SharedScheduler` with `SpinLockIrq`-protected run queue; all harts run `scheduling_loop` |

### Ch4-SMP Additional

| File | Action | Purpose |
|------|--------|---------|
| `src/main.rs` | Rewrite | Same scheduler pattern + per-hart `satp` tracking; allocator lock |
| `src/process.rs` | Modify | Process struct unchanged but accessed through lock |

### Ch5-SMP Additional

| File | Action | Purpose |
|------|--------|---------|
| `src/main.rs` | Rewrite | `PROCESSOR` wrapped in `SpinLockIrq`; per-hart `current` |
| `src/processor.rs` | Modify | Per-hart current tracking |

## Testing and Demonstrations

### Test Scenarios

**Ch3-SMP:**
1. Run existing ch3 test programs with `-smp 4`. All should produce correct output.
2. Add a test that prints which hart is running each task (demonstrates work distribution).
3. Run with `-smp 1` — degrades to single-core behavior.
4. Exercise tests should still pass with `--features exercise`.

**Ch4-SMP:**
1. All ch4 test programs pass with `-smp 4` (virtual memory + multi-core).
2. `sbrk` test works correctly (allocator under contention).
3. Run with `-smp 1`.

**Ch5-SMP:**
1. All ch5 test programs pass with `-smp 4` (fork/exec/wait under multi-core).
2. Fork bomb test: parent forks multiple children rapidly, all run on different harts.
3. Wait/exit race test: parent and child exit/wait simultaneously.
4. Run with `-smp 1`.

### Demonstration Programs (User-Space)

Create test programs (in `tg-rcore-tutorial-user/`) that specifically exercise multi-core:

1. **`smp_hello.rs`**: Each forked child prints its PID and which hart it's running on (use a new `gethartid` syscall or embed hart ID in output). Demonstrates work distribution.

2. **`smp_compute.rs`**: Fork N children, each computing a partial sum. Parent waits and aggregates. Measures wall-clock time vs sequential. Demonstrates parallel speedup on a process-based OS.

3. **`smp_stress.rs`**: Rapidly fork/exec/exit to stress-test the process manager under contention. Catches race conditions in the process tree.

## Acceptance Criteria

### Ch3-SMP
1. `cargo run` boots 4 harts, all participate in round-robin scheduling
2. Tasks visibly run on different harts (add hart ID to output or separate log)
3. All existing ch3 test output is correct
4. Timer interrupts fire independently on each hart
5. `bash test.sh` passes
6. `cargo check` passes on host
7. Works with `-smp 1`

### Ch4-SMP
1. All ch4 test programs pass with `-smp 4`
2. Virtual memory works correctly with multiple harts switching address spaces
3. Memory allocator doesn't corrupt under concurrent allocation
4. `bash test.sh` passes
5. `cargo check` passes on host
6. Works with `-smp 1`

### Ch5-SMP
1. All ch5 test programs pass with `-smp 4`
2. Fork/exec/wait work correctly under concurrent execution
3. Process tree (parent-child relationships) remains consistent
4. No deadlocks under normal operation
5. `bash test.sh` passes
6. `cargo check` passes on host
7. Works with `-smp 1`

## Likely Pitfalls

1. **Spinlock deadlock in interrupt handler**: If the scheduler lock is held when a timer interrupt fires on the same hart, and the interrupt handler tries to acquire the same lock, it deadlocks. **Fix**: Use `SpinLockIrq` which disables interrupts before acquiring. This is the #1 SMP kernel bug.

2. **BSS race (again)**: `HART_STACKS` in `.bss` gets zeroed by `zero_bss()` while secondary harts are using their stacks. **Fix**: `.boot.stack` section. Already solved in ch2-smp but easy to forget.

3. **`current` task is global**: Ch5's `PManager` has a single `current: Option<ProcId>`. With 4 harts, each needs its own `current`. If two harts both set `current`, they overwrite each other. **Fix**: Per-hart `current` array indexed by `hart_id()`.

4. **Memory allocator not thread-safe**: `tg-kernel-alloc` uses interior mutability without locks. Two harts calling `alloc()` simultaneously corrupt the free list. **Fix**: Wrap allocator calls in a `SpinLockIrq`.

5. **Process accessed by two harts simultaneously**: If the scheduler gives the same process to two harts (queue race), both write to the same `LocalContext` — corruption. **Fix**: Remove process from queue before executing; only re-add after execution completes.

6. **Lock ordering violation (deadlock)**: Hart 0 holds lock A, waits for lock B. Hart 1 holds lock B, waits for lock A. Neither proceeds. **Fix**: Always acquire locks in the same order. With coarse-grained locking, there's only one lock.

7. **TLB stale entries**: When one hart modifies a page table, other harts in the same address space have cached old translations. **Fix**: `sfence.vma` on the modifying hart. Assume no process migration (local flush sufficient).

8. **Console print interleaving**: `println!` without a lock produces garbled multi-hart output. **Fix**: `PRINT_LOCK: SpinLockIrq`. Must be `SpinLockIrq` because the timer or panic handler might print.

9. **fork() under contention**: Deep-copying an address space allocates many pages. Two harts forking simultaneously both call the allocator concurrently. With the allocator lock, this works but serializes. Without it, heap corruption.

10. **wait/exit notification race**: Child calls `exit()` on hart 1, parent calls `wait()` on hart 0. Both modify `dead_children` in `ProcRel`. Without a lock, one update is lost.

11. **Hart starvation**: If one hart always gets the scheduler lock first, other harts spin wastefully. **Mitigation**: Keep critical sections short (lock only while accessing the queue, not during execution).

12. **Per-hart timer**: Each hart must call `set_timer()` for its own CLINT address. Already fixed in SBI-SMP but the S-mode code must also call `set_timer()` from the correct hart.

13. **Kernel stack overflow with SMP**: 4 harts x N KiB stacks increases total kernel stack usage 4x. Check that `HART_STACKS` doesn't collide with user program load addresses.

14. **`rdtime` not synchronized**: RISC-V's `mtime` counter is shared across all harts (single CLINT register at `0x200BFF8`). However, `rdtime` reads may be slightly skewed due to pipeline effects. Fine for scheduling, but note for precise benchmarks.

## Workflow

Use superpowers skills in order:
1. `superpowers:brainstorming` — refine scheduler locking strategy, per-hart state design. Save spec to `docs/superpowers/specs/`
2. `superpowers:writing-plans` — create step-by-step plan. Save to `docs/superpowers/plans/`
3. `superpowers:subagent-driven-development` or `superpowers:executing-plans` — implement task-by-task with review checkpoints. Start with ch3-smp (simplest scheduler), then ch4-smp (add VM), then ch5-smp (add process tree).
4. `superpowers:requesting-code-review` — final review

After implementation, write a design analysis report to `docs/reports/ch3-ch5-smp.md` covering:
- Multi-core scheduler design (shared run queue, lock strategy)
- Per-hart state management
- All bugs encountered and how they were fixed (with root cause analysis)
- Test results with `-smp 1`, 2, 4
- Demonstration of tasks running on different harts
- Performance comparison: single-core vs multi-core for parallel workloads
- Lock contention analysis (how long are critical sections?)
- Discussion: coarse-grained vs fine-grained locking trade-offs
- Common multi-core scheduling pitfalls and how to avoid them
- ArceOS comparison: how does our approach differ from ArceOS's per-CPU run queues?

Do NOT publish the crates — the user will review and publish manually.

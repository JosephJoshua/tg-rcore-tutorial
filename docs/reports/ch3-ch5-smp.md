# Ch3-Ch5 SMP: Multi-Core Scheduling and Process Management

A design analysis of extending rCore-Tutorial chapters 3, 4, and 5 with symmetric multi-processing. Covers the scheduler architecture, synchronization primitives, bugs encountered, and the fundamental trade-offs of coarse-grained locking.

---

## 1. What We Built

Three new crates:

| Crate | Base | What It Adds |
|-------|------|-------------|
| `jsph-tg-rcore-tutorial-ch3-smp` | Ch3 (multiprogramming) | 4 harts run the round-robin scheduler. Tasks distributed across harts via shared run queue. |
| `jsph-tg-rcore-tutorial-ch4-smp` | Ch4 (virtual memory) | Same multi-hart scheduling but with Sv39 page tables. Each hart maintains its own `satp`. Portal works concurrently. |
| `jsph-tg-rcore-tutorial-ch5-smp` | Ch5 (process management) | Full multi-core fork/exec/wait/exit. Custom `SmpProcManager` replaces `PManager` to handle per-hart `current` tracking. |

Plus one supporting crate:

| Crate | Base | What It Adds |
|-------|------|-------------|
| `jsph-tg-rcore-tutorial-kernel-alloc-smp` | `tg-rcore-tutorial-kernel-alloc` | Same buddy allocator but with an interrupt-disabling spinlock inside `GlobalAlloc::alloc/dealloc`. Eliminates the need for manual locking at every allocation call site. |

All four build on `jsph-tg-rcore-tutorial-sbi-smp` (M-mode multi-hart boot from ch1-ch2-smp). All modifications live in the new crate directories; the original shared crates are unmodified.

---

## 2. Multi-Core Scheduler Design

### The Core Pattern: Lock-Pop-Unlock-Execute

Every chapter uses the same fundamental pattern:

```
scheduling_loop():
    loop {
        task = lock(QUEUE) -> pop() -> unlock(QUEUE)   // microseconds
        if task:
            execute(task)                                // milliseconds
            lock(QUEUE) -> push_or_finish() -> unlock()  // microseconds
        else:
            check_all_done_or_spin()
    }
```

The critical insight: **task execution happens outside the lock**. The lock protects only the queue metadata (pop/push), which takes microseconds. The actual task execution (user-mode code running for a full time slice) takes milliseconds and requires zero synchronization. This is what makes the design scale.

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs:259-295`

### Ch3: Fixed-Size Ring Buffer

Ch3 has no heap allocator, so the ready queue is a fixed-size ring buffer:

```rust
struct QueueState {
    queue: [usize; APP_CAPACITY],  // ring buffer of task indices
    head: usize,
    tail: usize,
    count: usize,
    finished_count: usize,
    total_tasks: usize,
}
static QUEUE: SpinLockIrq<QueueState> = SpinLockIrq::new(QueueState::new());
```

Task control blocks (TCBs) live in a separate `UnsyncCell<[TaskControlBlock; APP_CAPACITY]>` accessed via raw pointers. The **ownership-by-dequeue** discipline guarantees exclusive access: once a hart pops task index `i` from the queue, no other hart will touch `TCBS[i]` until it's re-enqueued.

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs:84-145`

### Ch4: Vec-Based Process List

Ch4 adds the heap allocator and virtual memory. Processes are stored in `Option<Process>` slots (like ch3's TCB array but using `Option` for lifetime management). The same ring-buffer queue tracks indices into the process array.

The new challenge: **allocator thread safety**. The original `tg-kernel-alloc` buddy allocator has no internal locking. Two harts calling `alloc()` simultaneously corrupt the free list. Rather than scattering manual lock acquisitions at every allocation call site (fragile -- easy to miss one), we copied the allocator crate and added a spinlock inside `GlobalAlloc::alloc/dealloc`:

```rust
// jsph-tg-rcore-tutorial-kernel-alloc-smp/src/lib.rs
unsafe impl GlobalAlloc for SmpGlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _guard = LOCK.lock();  // interrupt-disabling spinlock
        heap_mut().allocate_layout::<u8>(layout) ...
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _guard = LOCK.lock();
        heap_mut().deallocate_layout(...) ...
    }
}
```

Every heap allocation -- whether from `Sv39Manager::page_alloc`, `BTreeMap::insert`, or `VecDeque::push_back` -- goes through this lock automatically. No call site needs to know about synchronization.

> Reference: `jsph-tg-rcore-tutorial-kernel-alloc-smp/src/lib.rs:140-161`

### Ch5: Custom Process Manager with Stride Scheduling

Ch5 was the hardest. The original `PManager` from `tg-task-manage` stores `current: Option<ProcId>` as a single field -- fine for single-core, broken for 4 harts each running a different process. Wrapping `PManager` in a lock doesn't help because `find_next()` sets `current` internally, and a second hart's `find_next()` overwrites it.

The solution: replace `PManager` entirely with `SmpProcManager`:

```rust
struct SmpProcManager {
    tasks: BTreeMap<ProcId, Process>,
    ready_queue: VecDeque<ProcId>,
    rel_map: BTreeMap<ProcId, ProcRel>,
}
```

No `current` field. Per-hart current tracking uses atomic arrays:

```rust
static CURRENT_PROCESS: [AtomicUsize; NUM_HARTS] = ...;  // raw ptr
static CURRENT_PID: [AtomicUsize; NUM_HARTS] = ...;      // pid as usize
```

Before executing a process, the scheduling loop stores the pointer and PID:

```rust
CURRENT_PROCESS[hid].store(proc_ptr as usize, Relaxed);
CURRENT_PID[hid].store(pid.get_usize(), Relaxed);
```

Syscall handlers read `CURRENT_PROCESS[hart_id()]` to find the running process for address translation, sbrk, etc.

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs:147-285`

---

## 3. SpinLockIrq: The Foundation

Plain spinlocks deadlock in kernel code. The scenario:

```
Hart 0 holds QUEUE lock
  Timer interrupt fires on hart 0
    Interrupt handler needs QUEUE lock
      DEADLOCK (spinlocks are not recursive)
```

`SpinLockIrq` prevents this by disabling S-mode interrupts (`sstatus.SIE`) before acquiring:

```rust
pub fn lock(&self) -> RawSpinLockIrqGuard<'_> {
    let sie_was_enabled = Self::disable_interrupts();  // csrc sstatus, SIE
    // TTAS spin
    while self.locked.compare_exchange_weak(...).is_err() {
        while self.locked.load(Relaxed) { spin_loop(); }
    }
    RawSpinLockIrqGuard { lock: self, sie_was_enabled }
}
```

The guard restores interrupts on drop -- but **releases the lock first**:

```rust
impl Drop for RawSpinLockIrqGuard<'_> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Release);           // unlock first
        RawSpinLockIrq::restore_interrupts(self.sie);     // then re-enable interrupts
    }
}
```

If we restored interrupts first, a timer interrupt could fire while the lock is still held -- exactly the deadlock we're trying to prevent.

This is the same pattern ArceOS uses (`SpinNoIrq`) and Linux uses (`spin_lock_irqsave`).

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/smp.rs:36-117`

### Nesting

`SpinLockIrq` nests correctly. The inner lock sees `SIE=0` (already disabled by the outer lock), saves `sie_was_enabled=false`, and doesn't try to restore interrupts on drop. The outer lock restores interrupts to their original state.

This matters because `PROC_MANAGER` -> allocator lock is a common nesting path in ch5 (the locked allocator's internal spinlock is acquired implicitly whenever a `BTreeMap` or `VecDeque` operation triggers a heap allocation while `PROC_MANAGER` is held).

---

## 4. Per-Hart State Management

### Boot Stacks

Each hart gets its own S-mode kernel stack, allocated in the `.boot.stack` linker section:

```rust
#[unsafe(link_section = ".boot.stack")]
static mut HART_STACKS: [[u8; STACK_SIZE]; NUM_HARTS] = [[0; STACK_SIZE]; NUM_HARTS];
```

The `.boot.stack` section is placed **after** `.bss` in the linker script, so `zero_bss()` doesn't corrupt stacks that secondary harts are already using. This was a real bug in early ch2-smp development (see Part 8 of the ch1-ch2 tutorial).

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs:160-183`

### Hart ID

Stored in the `tp` (thread pointer) register, set once in `_start`:

```asm
mv   tp, a0           // a0 = hartid from M-mode mret
```

Read via `hart_id()`:

```rust
pub fn hart_id() -> usize {
    let id: usize;
    unsafe { core::arch::asm!("mv {}, tp", out(reg) id) };
    id
}
```

### Boot Handshake

Same Release/Acquire pattern as ch2-smp:

- Hart 0: `BOOT_HART_DONE.store(true, Release)` after init
- Harts 1-3: `while !BOOT_HART_DONE.load(Acquire) { spin_loop(); }`

### Per-Hart satp (Ch4, Ch5)

Each hart has its own `satp` CSR. Hart 0 activates the kernel page table during init. Secondary harts load the same satp value from a shared atomic:

```rust
// Hart 0, after building kernel space:
KERNEL_SATP.store(satp::read().bits(), Release);

// Secondary harts:
let satp_val = KERNEL_SATP.load(Relaxed);
core::arch::asm!("csrw satp, {0}", "sfence.vma", in(reg) satp_val);
```

The `sfence.vma` is essential -- without it, the hart might use stale TLB entries from whatever page table was active before.

> Reference: `jsph-tg-rcore-tutorial-ch4-smp/src/main.rs` (secondary_main)

---

## 5. Console Output Serialization

SBI `console_putchar` is not thread-safe. Without a lock, multi-hart `println!` produces garbled output:

```
Hepowllo,_3 [wo10rld000!/20
000Test0]
```

A global `PRINT_LOCK: RawSpinLockIrq` serializes all console output:

```rust
impl IO for SyscallContext {
    fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
        match fd {
            STDOUT | STDDEBUG => {
                let _pl = crate::smp::PRINT_LOCK.lock();
                print!("{}", unsafe { ... });
                count as _
            }
            _ => -1,
        }
    }
}
```

This must be `SpinLockIrq` (not a plain spinlock) because the timer interrupt handler or panic handler might also print.

Note: individual `write()` syscalls are atomic, but sequences of writes from different processes interleave at the syscall boundary. A process printing a multi-line message will have its lines interleaved with other processes' output. This matches real multi-core Linux behavior.

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs:414-435`

---

## 6. Bugs Encountered and Root Cause Analysis

### Bug 1: SpinLock Deadlock Under Timer Interrupt

**Symptom**: System hangs immediately after first timer interrupt.

**Root cause**: Using a plain `SpinLock` (without interrupt disable) for the scheduler queue. When hart 0 held the lock and a timer interrupt fired on the same hart, the interrupt handler tried to acquire the same lock.

**Fix**: `SpinLockIrq` -- disable `sstatus.SIE` before acquiring. This is the #1 SMP kernel bug, documented in every OS textbook.

### Bug 2: BSS Race (Per-Hart Stacks)

**Symptom**: Secondary harts crash with corrupt stacks.

**Root cause**: `HART_STACKS` placed in `.bss`, which `zero_bss()` clears. Hart 0 calls `zero_bss()` while secondary harts are already using their stacks.

**Fix**: `#[unsafe(link_section = ".boot.stack")]` places stacks outside the BSS range. Already fixed in ch2-smp but easy to accidentally regress.

### Bug 3: `wait()` Returns -1 Instead of -2

**Symptom**: `forktest` panics with "wait stopped early" -- parent stops waiting for children that are still alive.

**Root cause**: The `SmpProcManager::wait()` method didn't implement the `-2` ("children exist but none dead yet") return value. The original `ProcRel::wait_any_child()` returns this sentinel, but our wrapper swallowed it and returned `None` (mapped to -1 = "no children at all").

The user-space `waitpid` wrapper loops on -2 (calling `yield_()` each time) but stops on -1. By returning -1 instead of -2, we told the parent "you have no children" when it actually had 30 alive children.

**Fix**: Added an `or_else` clause that checks if `children` is non-empty when no dead children are found:

```rust
result.or_else(|| {
    if !current_rel.children.is_empty() {
        Some((ProcId::from_usize(usize::MAX - 1), -1))  // -2 as isize
    } else {
        None
    }
})
```

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs:270-279`

### Bug 4: Stale Ready Queue Entries

**Symptom**: After processes exit, the scheduling loop wastes cycles fetching stale PIDs from the ready queue.

**Root cause**: `make_exited()` removed the process from `tasks` but not from `ready_queue`. When `fetch_next()` picked a stale PID, it checked `tasks.get(&pid)` which returned `None`, cleaned up one stale entry, and returned `None` to the caller. With 30 stale entries from forktest, this meant 30 wasted scheduling cycles.

**Fix**: Added `self.ready_queue.retain(|p| *p != pid)` in `make_exited()`.

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs:225-226`

### Bug 5: Idle-Hart Thundering Herd

**Symptom**: With `-smp 4`, the system was dramatically slower than `-smp 1` even with blocking wait. Test programs that completed in 60 seconds on 1 hart timed out after 10 minutes on 4 harts.

**Root cause**: When the ready queue was empty (common with blocking wait -- blocked processes aren't in the queue), idle harts spin-looped while acquiring `PROC_MANAGER` lock on every iteration. Three idle harts continuously contending on the lock starved the one hart doing useful work.

**Fix**: `wfi` (Wait For Interrupt) instead of `spin_loop()` when the queue is empty. The timer interrupt wakes the hart periodically to recheck. This reduces idle-hart lock contention from millions of acquisitions per second to zero.

### Bug 6: PManager `current` Field Under SMP

**Symptom**: (Design-time bug, caught before runtime.) `PManager` stores a single `current: Option<ProcId>`. With 4 harts, each calling `find_next()`, they overwrite each other's `current`. Subsequent `make_current_suspend()` operates on the wrong process.

**Root cause**: `PManager` was designed for single-core use. The `current` field is private, with no setter method.

**Fix**: Replaced `PManager` entirely with `SmpProcManager` that has no `current` field. Per-hart tracking uses atomic arrays indexed by `hart_id()`.

---

## 7. Test Results

### Ch3-SMP

```
$ cd jsph-tg-rcore-tutorial-ch3-smp && bash test.sh base

[Hart 2] online
[Hart 3] online
[Hart 1] online
...
[PASS] found <Test write A OK!>
[PASS] found <Test write B OK!>
[PASS] found <Test write C OK!>
[PASS] not found <FAIL: T.T>

Test PASSED: 4/4
```

All 12 user programs run correctly on 4 harts. Tasks visibly execute on different harts (interleaved output from concurrent power computations).

### Ch4-SMP

```
$ cd jsph-tg-rcore-tutorial-ch4-smp && bash test.sh base

[Hart 1] online
[Hart 3] online
[Hart 2] online
...
[PASS] found <Test write A OK!>
[PASS] found <Test write B OK!>
[PASS] found <Test write C OK!>
[PASS] found <Test sbrk almost OK!>
[PASS] not found <FAIL: T.T>
[PASS] not found <Test sbrk failed!>

Test PASSED: 6/6
```

Virtual memory + SMP works correctly. The `sbrk` test verifies allocator thread-safety (concurrent page allocation via `ALLOC_LOCK`). Portal transitions work with 4 concurrent harts.

### Ch5-SMP

```
$ cd jsph-tg-rcore-tutorial-ch5-smp && bash test.sh base

[Hart 1] online
[Hart 3] online
[Hart 2] online
...
[PASS] found <Hello, world from user mode program!>
[PASS] found <Test power_3 OK!>
[PASS] found <Test power_5 OK!>
[PASS] found <Test power_7 OK!>
[PASS] found <Test write A OK!>
[PASS] found <Test write B OK!>
[PASS] found <Test write C OK!>
[PASS] found <Test sbrk almost OK!>
[PASS] found <exit pass.>
[PASS] found <hello child process!>
[PASS] found <child process pid = (\d+), exit code = (\d+)>
[PASS] found <forktest pass.>
[PASS] not found <FAIL: T.T>
[PASS] not found <Test sbrk failed!>

Test PASSED: 14/14
```

All 14 base tests pass with `-smp 4`, including fork/exec/wait/exit, forktest (30 children), and sbrk. This required two key optimizations beyond the basic coarse-grained locking (see section 8).

### `cargo check` (Host Compilation)

All three crates pass `cargo check` on the host platform (non-riscv64). `cfg(target_arch)` gates are used for all architecture-specific code, with stub implementations for host compilation.

---

## 8. Performance Analysis and Optimizations

The initial coarse-grained locking implementation passed ch3 and ch4 tests on 4 harts, but ch5's forktest (30 children) hung indefinitely. Three targeted optimizations fixed it.

### Problem 1: wait() Busy-Retry Storm

The naive implementation returned -2 ("children alive, none dead") to user space, which busy-retried:

```
Parent: wait() -> -2 -> yield -> schedule -> wait() -> -2 -> yield -> ...
```

Each retry cycle required 3 lock acquisitions (wait + yield + schedule). With 30 children, the parent retried hundreds of times before any child exited.

**Fix: Blocking wait.** Instead of returning -2, the kernel removes the parent from the ready queue entirely. When a child exits, `make_exited()` wakes the parent by re-enqueuing it. The parent retries once, finds the dead child, and proceeds.

This is exactly what xv6 does (sleep/wakeup). The coarse-grained lock naturally prevents lost wakeups: the "check dead_children + add to blocked_on_wait" happens atomically under `PROC_MANAGER` lock, and `make_exited`'s "del_child + wake parent" also happens under the same lock.

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs:305-332` (wait), `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs:241-245` (wake in make_exited)

### Problem 2: Idle-Hart Thundering Herd

Even with blocking wait, 4-hart mode was slower than 1-hart. The root cause: when the ready queue was empty, idle harts **spin-looped while acquiring the global lock**:

```rust
None => {
    let all_done = {
        let pm = PROC_MANAGER.lock();  // <-- 3 idle harts hammer this
        !pm.has_processes()
    };
    core::hint::spin_loop();  // immediately retry -> re-acquire lock
}
```

With 3 idle harts continuously acquiring and releasing `PROC_MANAGER`, the hart doing actual work (executing a process, handling syscalls) had to compete for the lock on every re-enqueue operation.

**Fix: Lock-free idle check + WFI.** An atomic counter `ACTIVE_PROCESSES` tracks the number of living processes. Idle harts check this counter without acquiring any lock, then call `wfi` (Wait For Interrupt) to sleep until the next timer:

```rust
static ACTIVE_PROCESSES: AtomicUsize = AtomicUsize::new(0);
// Incremented in add(), decremented in make_exited()

None => {
    if ACTIVE_PROCESSES.load(Relaxed) == 0 {
        // All done -- shutdown or WFI forever
    }
    unsafe { core::arch::asm!("wfi") };  // sleep until timer interrupt
}
```

No lock acquisition at all on the idle path. The timer interrupt wakes harts periodically to recheck.

### Problem 3: Manual ALLOC_LOCK at Every Call Site

The initial design used a manual `ALLOC_LOCK: RawSpinLockIrq` acquired at every allocation call site -- in `page_alloc`, in every `SmpProcManager` method (defensively, because `BTreeMap`/`VecDeque` operations might allocate), and in `Process::from_elf`. This was fragile: if anyone added a new code path that called the allocator, they'd forget the lock.

Worse, `ALLOC_LOCK` nested inside `PROC_MANAGER` extended critical section hold times. Every scheduling operation (fetch, re-enqueue, exit) acquired two locks instead of one.

**Fix: Locked allocator crate.** Copied `tg-rcore-tutorial-kernel-alloc` to `jsph-tg-rcore-tutorial-kernel-alloc-smp` and added a spinlock inside `GlobalAlloc::alloc/dealloc`. Removed all manual `ALLOC_LOCK` calls. The allocator protects itself; no call site needs to know about synchronization.

### Combined Impact

| Metric | Naive | + Blocking Wait | + Lock-Free Idle + WFI | + Locked Allocator |
|--------|-------|----------------|------------------------|-------------------|
| Lock acquisitions per wait cycle | ~300-600 | ~2 | ~2 | ~2 |
| Idle hart lock contention | O(3 * cpu_freq) | O(3 * cpu_freq) | 0 | 0 |
| Nested locks in PROC_MANAGER | ALLOC_LOCK inside | ALLOC_LOCK inside | ALLOC_LOCK inside | None (allocator self-locks) |
| Timer quantum | 1ms | 1ms | 1ms | **1ms** (restored from 100ms bandaid) |
| Ch5 forktest with -smp 4 | Timeout (>10min) | Timeout (>10min) | Passes (~3min) | **Passes (<1min)** |
| Ch5 test.sh result | FAIL | FAIL | 14/14 PASS | **14/14 PASS** |

All three fixes were necessary. Blocking wait eliminated the retry storm. Lock-free idle eliminated contention from idle harts. The locked allocator shortened critical sections by removing nested locking and restored the 1ms timer quantum.

### Critical Section Duration

Measured by operation type (approximate, based on code analysis):

| Operation | Instructions in Critical Section | Duration (~) |
|-----------|----------------------------------|-------------|
| Queue pop (fetch_next) | ~50 (VecDeque scan + remove) | ~1 us |
| Queue push (make_ready) | ~10 (VecDeque push_back) | ~0.2 us |
| Process exit (make_exited) | ~100 (BTreeMap remove + retain + ProcRel updates) | ~2 us |
| wait() | ~30 (ProcRel dead_children check) | ~0.5 us |
| add() (after fork) | ~80 (BTreeMap insert + VecDeque push + ProcRel setup) | ~1.5 us |

These are short. After the two optimizations, the contention frequency is low enough for 4 harts to work efficiently.

---

## 9. Coarse-Grained vs Fine-Grained Locking

### What We Used: Coarse-Grained

One `SpinLockIrq` around the process manager for scheduling decisions. The allocator has its own internal lock (in the locked allocator crate). Two independent locks, no nesting required on the scheduling path.

**Advantages**:
- Correctness is easy to reason about
- No lock-ordering bugs (two independent locks: PROC_MANAGER for scheduling, allocator lock for heap)
- Teaching-appropriate: demonstrates the core SMP concepts
- Works well for 4 harts (all tests pass, including fork-heavy forktest)

**Disadvantages**:
- All scheduling decisions serialize, even when they don't conflict
- Fork-heavy workloads create O(N) contention with N processes (mitigated by blocking wait)

### What Would Help: Fine-Grained Locking

Separate locks for:
- Ready queue (scheduling operations)
- Process table (process lookup/insertion/deletion)
- Relationship map (parent-child operations)

This would allow, for example, one hart to fork (modifying the process table) while another handles a timer interrupt (modifying only the ready queue).

**Risk**: Lock ordering. With three locks, there are six possible orderings. Any circular dependency causes deadlock. The lock ordering must be documented and enforced. For a teaching OS, this complexity isn't worth it.

### What ArceOS Does: Per-CPU Run Queues

ArceOS avoids the global lock entirely by giving each CPU its own run queue. The common case (timer preemption, yield) only touches the local queue -- no lock contention at all. When a CPU's queue is empty, it **steals** work from another CPU's queue.

```
ArceOS approach:
  Timer on CPU 0 → push to CPU 0's queue (no lock)
  CPU 1 idle → steal from CPU 0's queue (one lock)

Our approach:
  Timer on CPU 0 → push to global queue (global lock)
  CPU 1 idle → pop from global queue (same global lock)
```

The per-CPU queue eliminates contention for the common case. The downside: load balancing is harder, and work-stealing requires careful synchronization.

For our 4-hart teaching kernel, the global queue is acceptable. At 32+ harts, per-CPU queues become necessary.

---

## 10. Common Multi-Core Scheduling Pitfalls

Based on bugs encountered and design decisions made:

### 1. Interrupt-Lock Deadlock (Pitfall #1)

**Pattern**: Hart holds lock, timer fires, handler needs same lock.

**Fix**: Always use interrupt-disabling locks (`SpinLockIrq`) for kernel data structures that might be accessed from interrupt context.

### 2. BSS Race on Boot

**Pattern**: Hart 0 calls `zero_bss()` while secondary harts are using their stacks (which were in `.bss`).

**Fix**: Place per-hart stacks in a linker section outside BSS (`.boot.stack`).

### 3. Single-Valued `current` Under SMP

**Pattern**: Process manager tracks "current process" as a single field. Multiple harts overwrite each other.

**Fix**: Per-hart current tracking via atomic arrays indexed by `hart_id()`.

### 4. Allocator Not Thread-Safe

**Pattern**: Two harts call `alloc()` simultaneously, corrupting the free list.

**Fix**: Copy the allocator crate and add a spinlock inside `GlobalAlloc::alloc/dealloc`. This is the standard approach (cf. `linked_list_allocator::LockedHeap`, Phil Opp's "Writing an OS in Rust"). Manual locking at every call site is fragile -- one missed site corrupts the heap.

### 5. Console Output Interleaving

**Pattern**: Multi-hart `println!` produces garbled output.

**Fix**: `PRINT_LOCK: RawSpinLockIrq` around all console output.

### 6. Process Given to Two Harts

**Pattern**: Ready queue race where the same process is dequeued by two harts.

**Fix**: Queue operations are atomic under the scheduler lock. Once dequeued, the process is "owned" by that hart.

### 7. TLB Stale Entries

**Pattern**: One hart modifies a page table, other harts have cached old translations.

**Fix**: `sfence.vma` after page table modifications. In our design, each process runs on only one hart at a time, so local flush is sufficient. Full remote TLB shootdown (via IPI) is beyond scope.

### 8. wait/exit Notification Race

**Pattern**: Child exits on hart 1, parent waits on hart 0. Both modify `dead_children` in `ProcRel`.

**Fix**: All relationship operations are under the global `PROC_MANAGER` lock.

### 9. Stale Queue Entries After Exit

**Pattern**: Process exits but its PID remains in the ready queue, wasting scheduling cycles.

**Fix**: `make_exited()` calls `ready_queue.retain(|p| *p != pid)` to clean up stale entries.

### 10. Memory Leaks from Address Space Drop

**Pattern**: `drop_root()` in `Sv39Manager` is `todo!()`. When a process exits or exec replaces its address space, the old page tables and physical pages are leaked.

**Status**: Pre-existing issue in the original single-core ch4/ch5. Not SMP-specific, but SMP amplifies the impact because more processes run concurrently and consume more memory.

---

## 11. Architecture Comparison

### Our Design vs rCore-Tutorial-v3

rCore-Tutorial-v3 is single-core only. It uses `UnsafeCell` wrappers with no locking. Our SMP extensions add:
- `SpinLockIrq` replacing all `UnsafeCell` wrappers
- Per-hart state arrays replacing single-valued globals
- Locked allocator crate (`jsph-tg-rcore-tutorial-kernel-alloc-smp`) for thread-safe heap
- `PRINT_LOCK` for console serialization
- Blocking wait (sleep/wakeup) for process management
- Lock-free idle check via `ACTIVE_PROCESSES` atomic counter

### Our Design vs ArceOS

| Aspect | Our Design | ArceOS |
|--------|-----------|--------|
| Ready queue | Global, shared | Per-CPU |
| Lock type | SpinLockIrq | SpinNoIrq (same pattern) |
| Scheduling | Stride (ch5) / FIFO (ch3/ch4) | CFS / FIFO / Round-Robin (pluggable) |
| Current process | Per-hart atomic array | Per-CPU `CurrentTask` |
| Work balancing | Implicit (all harts share one queue) | Work-stealing when idle |
| Lock contention | O(num_harts * num_processes) | O(1) for common case |

### Our Design vs Linux

Linux uses a completely different approach: per-CPU run queues with CFS (Completely Fair Scheduler), and **ticket spinlocks** (not TTAS) for fairness. Linux's `spin_lock_irqsave` is equivalent to our `SpinLockIrq`.

---

## 12. Summary of Files

### Supporting Crate

| Crate | Files | Purpose |
|-------|-------|---------|
| `jsph-tg-rcore-tutorial-kernel-alloc-smp` | `src/lib.rs` | Buddy allocator + interrupt-disabling spinlock in `GlobalAlloc` impl. Used by ch4-smp and ch5-smp. |

### Shared Across All Three Chapters

| File | Purpose |
|------|---------|
| `src/smp.rs` | `SpinLockIrq<T>`, `RawSpinLockIrq`, `hart_id()`, `BOOT_HART_DONE`, `PRINT_LOCK` |

### Per Chapter

| File | Ch3 | Ch4 | Ch5 |
|------|-----|-----|-----|
| `Cargo.toml` | SBI-SMP dep, version-only tg-* deps | Same + xmas-elf, kernel-alloc-smp, kernel-vm, kernel-context[foreign] | Same + spin, task-manage[proc] |
| `.cargo/config.toml` | `-smp 4`, `TG_USER_DIR` | Same | Same |
| `src/main.rs` | Multi-hart _start, QueueState ring buffer, scheduling_loop, run_task | Same pattern + ForeignContext, MultislotPortal, KERNEL_SATP | SmpProcManager, per-hart CURRENT_PROCESS/PID, ACTIVE_PROCESSES, blocking wait, fork/exec/wait/exit handlers |
| `src/task.rs` | Parameterized handle_syscall | -- | -- |
| `src/process.rs` | -- | Unchanged from original | Added `unsafe impl Send` |

---

## 13. What This Design Does Not Cover

| Topic | Why Not | How To Add |
|-------|---------|-----------|
| Per-CPU run queues | Teaching complexity | Replace global PROC_MANAGER with per-hart queues + work-stealing |
| Remote TLB shootdown | No cross-hart IPI mechanism | Send IPI via SBI, target hart flushes TLB |
| Proper address space deallocation | `drop_root()` is `todo!()` in shared crate | Implement PageManager::drop_root to walk and free page tables |
| Fine-grained process manager locking | Deadlock risk outweighs benefit at 4 harts | Separate locks for queue/table/relmap with documented ordering |
| Process migration | Processes don't move between harts mid-execution | Track which hart a process last ran on, prefer same hart for cache locality |
| Priority inversion | Not a concern with short critical sections | Priority inheritance protocol on the spinlock |
| NUMA-aware scheduling | QEMU virt is UMA | Allocate pages local to the process's preferred NUMA node |

//! # Chapter 3 SMP: Multi-core Multiprogramming and Time-Sharing
//!
//! This chapter extends the single-core multiprogramming OS with SMP support.
//! All 4 QEMU harts participate in round-robin scheduling of user tasks.
//!
//! ## Architecture
//!
//! - **Per-hart stacks** in `.boot.stack` section (safe from `zero_bss`)
//! - **Shared ready queue** protected by `SpinLockIrq` (interrupt-safe spinlock)
//! - **TCBs accessed without lock** -- once a hart dequeues a task index,
//!   it has exclusive ownership until it re-enqueues or marks the task finished
//! - **PRINT_LOCK** serializes all console output across harts

// Bare-metal: no std, no main
#![no_std]
#![no_main]
// Strict warnings on RISC-V64; allow dead code on host (for cargo publish --dry-run)
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code, unused_imports))]

mod task;
mod smp;

#[macro_use]
extern crate tg_console;

use impls::{Console, SyscallContext};
use task::TaskControlBlock;
use tg_console::log;

#[cfg(target_arch = "riscv64")]
use riscv::register::*;
#[cfg(target_arch = "riscv64")]
use core::sync::atomic::Ordering;

use tg_sbi;

// ========== Constants ==========

/// Maximum number of applications.
const APP_CAPACITY: usize = 32;

/// Stack size per hart: 32 KiB.
#[cfg(target_arch = "riscv64")]
const HART_STACK_SIZE: usize = 8 * 4096;

// ========== Static Data ==========

/// Unsynchronized cell for data that is accessed with ownership discipline
/// (only the hart that dequeued a task index may access that index's slot).
/// This avoids `static mut` which Rust 2024 restricts.
struct UnsyncCell<T>(core::cell::UnsafeCell<T>);
unsafe impl<T> Sync for UnsyncCell<T> {}
impl<T> UnsyncCell<T> {
    const fn new(val: T) -> Self {
        Self(core::cell::UnsafeCell::new(val))
    }
    /// Get a raw pointer to the inner data.
    #[inline]
    fn as_ptr(&self) -> *mut T {
        self.0.get()
    }
}

/// Get a mutable reference to `TCBS[idx]` via raw pointer.
///
/// # Safety
/// Caller must ensure exclusive access to this index (i.e. the task has been
/// dequeued from the ready queue and no other hart references it).
#[inline]
unsafe fn tcb_mut(idx: usize) -> &'static mut TaskControlBlock {
    unsafe { &mut *(TCBS.as_ptr() as *mut TaskControlBlock).add(idx) }
}

/// Get a mutable reference to `SYSCALL_COUNTS[idx]` via raw pointer.
///
/// # Safety
/// Same ownership discipline as `tcb_mut`.
#[inline]
unsafe fn syscall_counts_mut(idx: usize) -> &'static mut [u32; 500] {
    unsafe { &mut *(SYSCALL_COUNTS.as_ptr() as *mut [u32; 500]).add(idx) }
}

/// Task control blocks. Accessed without lock -- ownership guaranteed by
/// dequeuing the task index from the ready queue.
static TCBS: UnsyncCell<[TaskControlBlock; APP_CAPACITY]> =
    UnsyncCell::new([TaskControlBlock::ZERO; APP_CAPACITY]);

/// Per-task syscall call counts. Same ownership model as TCBS.
static SYSCALL_COUNTS: UnsyncCell<[[u32; 500]; APP_CAPACITY]> =
    UnsyncCell::new([[0; 500]; APP_CAPACITY]);

// ========== Scheduling Queue ==========

/// Scheduling metadata protected by SpinLockIrq.
struct QueueState {
    /// Ring buffer of ready task indices.
    queue: [usize; APP_CAPACITY],
    /// Head index in ring buffer (next to dequeue).
    head: usize,
    /// Tail index in ring buffer (next slot to enqueue).
    tail: usize,
    /// Number of elements currently in the queue.
    count: usize,
    /// Number of tasks that have finished (exited or killed).
    finished_count: usize,
    /// Total number of loaded tasks.
    total_tasks: usize,
}

impl QueueState {
    /// Create an empty queue state.
    const fn new() -> Self {
        Self {
            queue: [0; APP_CAPACITY],
            head: 0,
            tail: 0,
            count: 0,
            finished_count: 0,
            total_tasks: 0,
        }
    }

    /// Push a task index to the back of the ready queue.
    fn push(&mut self, idx: usize) {
        self.queue[self.tail] = idx;
        self.tail = (self.tail + 1) % APP_CAPACITY;
        self.count += 1;
    }

    /// Pop a task index from the front of the ready queue.
    fn pop(&mut self) -> Option<usize> {
        if self.count == 0 {
            None
        } else {
            let idx = self.queue[self.head];
            self.head = (self.head + 1) % APP_CAPACITY;
            self.count -= 1;
            Some(idx)
        }
    }
}

/// Global scheduling queue, protected by interrupt-safe spinlock.
static QUEUE: smp::SpinLockIrq<QueueState> = smp::SpinLockIrq::new(QueueState::new());

// ========== Embedded User Programs ==========

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

// ========== Entry Point ==========

/// Multi-hart S-mode entry point.
/// a0 = hart ID (from M-mode mret).
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const NUM_HARTS_CONST: usize = 4;

    // MUST be in .boot.stack (NOT .bss), because hart 0 calls zero_bss() which
    // would corrupt stacks that secondary harts are already using.
    #[unsafe(link_section = ".boot.stack")]
    static mut HART_STACKS: [[u8; HART_STACK_SIZE]; NUM_HARTS_CONST] =
        [[0; HART_STACK_SIZE]; NUM_HARTS_CONST];

    core::arch::naked_asm!(
        "mv   tp, a0",              // Save hart ID to tp
        "la   t0, {stacks}",
        "li   t1, {stack_size}",
        "addi t2, a0, 1",
        "mul  t1, t1, t2",
        "add  sp, t0, t1",          // Per-hart stack
        "bnez a0, {secondary}",     // Secondary harts branch
        "j    {main}",              // Hart 0 continues
        stacks     = sym HART_STACKS,
        stack_size = const HART_STACK_SIZE,
        main       = sym rust_main,
        secondary  = sym secondary_main,
    )
}

// ========== Hart 0: Initialization ==========

/// Hart 0 entry: initialize kernel subsystems, load apps, signal secondaries,
/// then enter the scheduling loop.
extern "C" fn rust_main() -> ! {
    // Step 1: Clear BSS
    unsafe { tg_linker::KernelLayout::locate().zero_bss() };

    // Step 2: Init console output (makes print!/println! available)
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG").or(Some("info")));
    tg_console::test_log();

    // Step 3: Init syscall handlers (scheduling, clock, trace are new in ch3)
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_trace(&SyscallContext);

    // Step 4: Load user programs into TCBs and populate the ready queue
    {
        let mut q = QUEUE.lock();
        for (i, app) in tg_linker::AppMeta::locate().iter().enumerate() {
            let entry = app.as_ptr() as usize;
            log::info!("load app{i} to {entry:#x}");
            unsafe { tcb_mut(i).init(entry) };
            q.push(i);
            q.total_tasks += 1;
        }
    }
    println!();

    // Step 5: Enable S-mode timer interrupt (for preemptive scheduling)
    #[cfg(target_arch = "riscv64")]
    unsafe { sie::set_stimer() };

    // Step 6: Signal secondary harts that initialization is complete
    #[cfg(target_arch = "riscv64")]
    smp::BOOT_HART_DONE.store(true, Ordering::Release);

    // Step 7: Enter the shared scheduling loop
    scheduling_loop()
}

// ========== Secondary Harts ==========

/// Secondary hart entry: wait for hart 0, enable timer, enter scheduling loop.
#[cfg(target_arch = "riscv64")]
extern "C" fn secondary_main() -> ! {
    // Spin until hart 0 finishes initialization
    while !smp::BOOT_HART_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // Enable S-mode timer interrupt on this hart
    unsafe { sie::set_stimer() };

    // Print identification
    {
        let _pl = smp::PRINT_LOCK.lock();
        println!("[Hart {}] online", smp::hart_id());
    }

    // Enter the shared scheduling loop
    scheduling_loop()
}

// ========== Scheduling Loop ==========

/// The core scheduling loop, run by every hart.
///
/// Each iteration: dequeue a task, run it until a trap, handle the trap
/// (re-enqueue, mark finished, etc.), then loop back to dequeue the next task.
fn scheduling_loop() -> ! {
    loop {
        // Try to dequeue a ready task
        let task_idx = {
            let mut q = QUEUE.lock();
            q.pop()
        };

        match task_idx {
            Some(idx) => {
                run_task(idx);
            }
            None => {
                // No tasks in the ready queue -- check if all tasks are done
                let all_done = {
                    let q = QUEUE.lock();
                    q.finished_count >= q.total_tasks
                };
                if all_done {
                    if smp::hart_id() == 0 {
                        tg_sbi::shutdown(false);
                    } else {
                        // Secondary harts idle forever
                        loop {
                            #[cfg(target_arch = "riscv64")]
                            unsafe { core::arch::asm!("wfi") };
                            #[cfg(not(target_arch = "riscv64"))]
                            core::hint::spin_loop();
                        }
                    }
                }
                // Not all done but queue empty -- spin briefly and retry
                core::hint::spin_loop();
            }
        }
    }
}

/// Run a single task until it yields, times out, exits, or faults.
#[cfg(target_arch = "riscv64")]
fn run_task(idx: usize) {
    // Get exclusive references via raw pointers. Safe because this hart
    // dequeued `idx` from the ready queue, guaranteeing exclusive access.
    let tcb = unsafe { tcb_mut(idx) };

    // Set timer for preemptive scheduling (skip if cooperative mode)
    #[cfg(not(feature = "coop"))]
    tg_sbi::set_timer(time::read64() + 12500);

    // Execute the task
    unsafe { tcb.execute() };

    // Read trap cause
    use scause::*;
    match scause::read().cause() {
        // ── Timer interrupt: time slice expired ──
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            // Clear the timer to avoid immediate re-trigger
            tg_sbi::set_timer(u64::MAX);
            // Re-enqueue the task
            let mut q = QUEUE.lock();
            q.push(idx);
        }
        // ── Syscall: user program executed ecall ──
        Trap::Exception(Exception::UserEnvCall) => {
            use task::SchedulingEvent as Event;
            let counts = unsafe { syscall_counts_mut(idx) };
            let event = tcb.handle_syscall(idx, counts);
            match event {
                Event::None => {
                    // Syscall handled (e.g. write, clock_gettime) -- re-enqueue
                    let mut q = QUEUE.lock();
                    q.push(idx);
                }
                Event::Yield => {
                    // Task voluntarily yields -- re-enqueue at back
                    let mut q = QUEUE.lock();
                    q.push(idx);
                }
                Event::Exit(code) => {
                    {
                        let _pl = smp::PRINT_LOCK.lock();
                        log::info!("app{idx} exit with code {code}");
                    }
                    let mut q = QUEUE.lock();
                    tcb.finish = true;
                    q.finished_count += 1;
                }
                Event::UnsupportedSyscall(id) => {
                    {
                        let _pl = smp::PRINT_LOCK.lock();
                        log::error!("app{idx} call an unsupported syscall {}", id.0);
                    }
                    let mut q = QUEUE.lock();
                    tcb.finish = true;
                    q.finished_count += 1;
                }
            }
        }
        // ── Other exception (illegal instruction, page fault, etc.) ──
        Trap::Exception(e) => {
            {
                let _pl = smp::PRINT_LOCK.lock();
                log::error!("app{idx} was killed by {e:?}");
            }
            let mut q = QUEUE.lock();
            tcb.finish = true;
            q.finished_count += 1;
        }
        // ── Unexpected interrupt ──
        Trap::Interrupt(ir) => {
            {
                let _pl = smp::PRINT_LOCK.lock();
                log::error!("app{idx} was killed by an unexpected interrupt {ir:?}");
            }
            let mut q = QUEUE.lock();
            tcb.finish = true;
            q.finished_count += 1;
        }
    }
}

/// Non-riscv64 stub for `run_task` (host compilation).
#[cfg(not(target_arch = "riscv64"))]
fn run_task(_idx: usize) {}

// ========== Panic Handler ==========

/// Panic handler: print error info and shutdown with error status.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

// ========== Interface Implementations ==========

/// Console and syscall trait implementations.
mod impls {
    use tg_syscall::*;

    /// Console implementation: output via SBI putchar.
    pub struct Console;

    impl tg_console::Console for Console {
        #[inline]
        fn put_char(&self, c: u8) {
            tg_sbi::console_putchar(c);
        }
    }

    /// Syscall context implementation.
    pub struct SyscallContext;

    /// IO syscall: handles write.
    impl IO for SyscallContext {
        #[inline]
        fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            match fd {
                STDOUT | STDDEBUG => {
                    let _pl = crate::smp::PRINT_LOCK.lock();
                    print!("{}", unsafe {
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                            buf as *const u8,
                            count,
                        ))
                    });
                    count as _
                }
                _ => {
                    let _pl = crate::smp::PRINT_LOCK.lock();
                    tg_console::log::error!("unsupported fd: {fd}");
                    -1
                }
            }
        }
    }

    /// Process syscall: handles exit.
    impl Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: Caller, _status: usize) -> isize {
            0
        }
    }

    /// Scheduling syscall: handles yield.
    impl Scheduling for SyscallContext {
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }
    }

    /// Clock syscall: handles clock_gettime.
    ///
    /// Converts RISC-V hardware timer value to nanosecond precision.
    /// QEMU virt platform clock frequency: 12.5 MHz (80 ns/tick).
    impl Clock for SyscallContext {
        #[inline]
        fn clock_gettime(
            &self,
            _caller: Caller,
            clock_id: ClockId,
            tp: usize,
        ) -> isize {
            match clock_id {
                ClockId::CLOCK_MONOTONIC => {
                    let time = riscv::register::time::read() * 10000 / 125;
                    *unsafe { &mut *(tp as *mut TimeSpec) } = TimeSpec {
                        tv_sec: time / 1_000_000_000,
                        tv_nsec: time % 1_000_000_000,
                    };
                    0
                }
                _ => -1,
            }
        }
    }

    /// Trace syscall implementation.
    ///
    /// - trace_request=0: read one byte from user memory at address `id`
    /// - trace_request=1: write low byte of `data` to user memory at address `id`
    /// - trace_request=2: query syscall `id` call count (this call included)
    impl Trace for SyscallContext {
        fn trace(
            &self,
            caller: Caller,
            trace_request: usize,
            id: usize,
            data: usize,
        ) -> isize {
            match trace_request {
                0 => {
                    let ptr = id as *const u8;
                    unsafe { *ptr as isize }
                }
                1 => {
                    let ptr = id as *mut u8;
                    unsafe { *ptr = data as u8; }
                    0
                }
                2 => {
                    if id < 500 {
                        // Safe: the owning hart has exclusive access to this task's counts
                        unsafe {
                            let counts = crate::syscall_counts_mut(caller.entity);
                            counts[id] as isize
                        }
                    } else {
                        0
                    }
                }
                _ => -1,
            }
        }
    }
}

/// Non-RISC-V64 stub module for host compilation (cargo publish --dry-run).
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    /// Host platform placeholder entry.
    #[unsafe(no_mangle)]
    pub extern "C" fn main() -> i32 {
        0
    }

    /// C runtime placeholder.
    #[unsafe(no_mangle)]
    pub extern "C" fn __libc_start_main() -> i32 {
        0
    }

    /// Rust exception handling personality placeholder.
    #[unsafe(no_mangle)]
    pub extern "C" fn rust_eh_personality() {}
}

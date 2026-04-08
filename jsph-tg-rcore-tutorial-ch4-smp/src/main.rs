//! # Chapter 4 SMP: Multi-core Address Space Management
//!
//! This chapter extends ch4 (Sv39 virtual memory) with SMP support.
//! All 4 QEMU harts participate in round-robin scheduling of user processes,
//! each running in its own independent address space.
//!
//! ## Architecture
//!
//! - **Per-hart stacks** in `.boot.stack` section (safe from `zero_bss`)
//! - **Shared ready queue** protected by `SpinLockIrq` (interrupt-safe spinlock)
//! - **Per-hart portal slots** via `MultislotPortal` with `TpReg` key
//! - **PRINT_LOCK** serializes all console output across harts
//! - **Per-hart CURRENT_PROCESS** pointers for syscall address translation

// Bare-metal: no std, no main
#![no_std]
#![no_main]
// Strict warnings on RISC-V64; allow dead code on host (for cargo publish --dry-run)
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code, unused_imports))]

// Process management module: defines Process struct with AddressSpace and ForeignContext
mod process;
// SMP synchronization primitives
mod smp;

// Console output macros (print! / println!)
#[macro_use]
extern crate tg_console;

// Enable alloc crate for heap allocation (Vec, VecDeque, etc.)
extern crate alloc;

// ========== Imports ==========

use crate::{
    impls::{Console, SyscallContext},
    process::Process,
};
use alloc::alloc::alloc;
use core::{alloc::Layout, sync::atomic::AtomicUsize};
// Re-exports for process.rs (which uses crate::build_flags, crate::Sv39, etc.)
use impls::{build_flags, parse_flags, Sv39Manager};

use tg_console::log;

#[cfg(target_arch = "riscv64")]
use core::sync::atomic::Ordering::{self, Acquire, Relaxed, Release};
#[cfg(target_arch = "riscv64")]
use riscv::register::*;

// Non-RISC-V64 uses stub Sv39 type
#[cfg(not(target_arch = "riscv64"))]
use stub::Sv39;
// RISC-V64 uses real Sv39 type
#[cfg(target_arch = "riscv64")]
use tg_kernel_vm::page_table::Sv39;

// Foreign context portal for cross-address-space context switching
use tg_kernel_context::foreign::MultislotPortal;
#[cfg(target_arch = "riscv64")]
use tg_kernel_context::foreign::TpReg;
use tg_kernel_vm::page_table::{MmuMeta, VAddr, VmMeta, VPN, PPN};
use tg_kernel_vm::AddressSpace;
use tg_sbi;
use tg_syscall::Caller;
use xmas_elf::ElfFile;

// ========== Constants ==========

/// Maximum number of applications.
const APP_CAPACITY: usize = 32;

/// Stack size per hart: 32 KiB.
#[cfg(target_arch = "riscv64")]
const HART_STACK_SIZE: usize = 8 * 4096;

/// Physical memory capacity = 24 MiB (QEMU virt platform).
const MEMORY: usize = 24 << 20;

/// Portal virtual page: the highest page of the virtual address space.
/// The portal is mapped at the same virtual address in both kernel and user
/// address spaces, so code can continue executing after satp switch.
const PROTAL_TRANSIT: VPN<Sv39> = VPN::MAX;

// ========== Unsynchronized Cell ==========

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

// ========== Static Data ==========

/// Kernel satp value for secondary harts to load.
static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);

/// Shared portal pointer (set by hart 0, read by all).
static PORTAL_PTR: AtomicUsize = AtomicUsize::new(0);

/// Per-hart current process pointer for syscall handlers.
/// Set before `execute()`, read by trait impls to access address_space.
static CURRENT_PROCESS: [AtomicUsize; smp::NUM_HARTS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; smp::NUM_HARTS]
};

/// Process slots: accessed without lock -- once a hart dequeues a process
/// index, it has exclusive ownership until it re-enqueues or marks it finished.
static PROCESSES: UnsyncCell<[Option<Process>; APP_CAPACITY]> =
    UnsyncCell::new([const { None }; APP_CAPACITY]);

/// Per-process syscall call counts. Same ownership model as PROCESSES.
static SYSCALL_COUNTS: UnsyncCell<[[u32; 500]; APP_CAPACITY]> =
    UnsyncCell::new([[0; 500]; APP_CAPACITY]);

// ========== Scheduling Queue ==========

/// Scheduling metadata protected by SpinLockIrq.
struct QueueState {
    /// Ring buffer of ready process indices.
    queue: [usize; APP_CAPACITY],
    /// Head index in ring buffer (next to dequeue).
    head: usize,
    /// Tail index in ring buffer (next slot to enqueue).
    tail: usize,
    /// Number of elements currently in the queue.
    count: usize,
    /// Number of processes that have finished (exited or killed).
    finished_count: usize,
    /// Total number of loaded processes.
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

    /// Push a process index to the back of the ready queue.
    fn push(&mut self, idx: usize) {
        self.queue[self.tail] = idx;
        self.tail = (self.tail + 1) % APP_CAPACITY;
        self.count += 1;
    }

    /// Pop a process index from the front of the ready queue.
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

// ========== Helper Functions ==========

/// Get a mutable reference to the process at `idx` via raw pointer.
///
/// # Safety
/// Caller must ensure exclusive access to this index (i.e. the process has been
/// dequeued from the ready queue and no other hart references it).
#[inline]
unsafe fn process_mut(idx: usize) -> &'static mut Option<Process> {
    unsafe { &mut *(PROCESSES.as_ptr() as *mut Option<Process>).add(idx) }
}

/// Get a mutable reference to `SYSCALL_COUNTS[idx]` via raw pointer.
///
/// # Safety
/// Same ownership discipline as `process_mut`.
#[inline]
unsafe fn syscall_counts_mut(idx: usize) -> &'static mut [u32; 500] {
    unsafe { &mut *(SYSCALL_COUNTS.as_ptr() as *mut [u32; 500]).add(idx) }
}

/// Get the current process for this hart (set during `run_task`).
///
/// # Safety
/// Must only be called from within a syscall handler while the process is
/// being executed by the current hart.
#[cfg(target_arch = "riscv64")]
fn current_process() -> &'static mut Process {
    let ptr = CURRENT_PROCESS[smp::hart_id()].load(Relaxed);
    unsafe { &mut *(ptr as *mut Process) }
}

// ========== Hart 0: Initialization ==========

/// Hart 0 entry: initialize kernel subsystems, build kernel address space,
/// load user processes, signal secondaries, then enter the scheduling loop.
extern "C" fn rust_main() -> ! {
    let layout = tg_linker::KernelLayout::locate();
    // Step 1: Clear BSS
    unsafe { layout.zero_bss() };

    // Step 2: Init console output
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG"));
    tg_console::test_log();

    // Step 3: Init heap allocator
    tg_kernel_alloc::init(layout.start() as _);
    unsafe {
        tg_kernel_alloc::transfer(core::slice::from_raw_parts_mut(
            layout.end() as _,
            MEMORY - layout.len(),
        ))
    };

    // Step 4: Allocate portal pages (NUM_HARTS slots for concurrent hart transitions)
    let portal_size = MultislotPortal::calculate_size(smp::NUM_HARTS);
    let portal_layout = Layout::from_size_align(portal_size, 1 << Sv39::PAGE_BITS).unwrap();
    let portal_ptr = unsafe { alloc(portal_layout) };
    let portal_pages = portal_layout.size().div_ceil(1 << Sv39::PAGE_BITS).max(1);

    // Step 5: Build kernel address space (identity-mapped + portal)
    #[allow(unused_mut)]
    let mut ks = kernel_space(layout, MEMORY, portal_ptr as _, portal_pages);
    let portal_idx = PROTAL_TRANSIT.index_in(Sv39::MAX_LEVEL);

    // Store kernel satp for secondary harts
    #[cfg(target_arch = "riscv64")]
    KERNEL_SATP.store(satp::read().bits(), Release);

    // Step 6: Init portal
    let portal = unsafe { MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), smp::NUM_HARTS) };
    PORTAL_PTR.store(portal as *const _ as usize, Ordering::Release);

    // Step 7: Init syscall handlers
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_trace(&SyscallContext);
    tg_syscall::init_memory(&SyscallContext);

    // Step 8: Load user processes
    {
        let mut q = QUEUE.lock();
        for (i, elf_data) in tg_linker::AppMeta::locate().iter().enumerate() {
            let base = elf_data.as_ptr() as usize;
            log::info!("detect app[{i}]: {base:#x}..{:#x}", base + elf_data.len());
            if let Some(process) = Process::new(ElfFile::new(elf_data).unwrap()) {
                // Share the kernel portal page table entry into user address space
                process.address_space.root()[portal_idx] = ks.root()[portal_idx];
                unsafe { *process_mut(i) = Some(process) };
                q.push(i);
                q.total_tasks += 1;
            }
        }
    }
    println!();

    // Step 9: Enable S-mode timer interrupt
    #[cfg(target_arch = "riscv64")]
    unsafe { sie::set_stimer() };

    // Step 10: Signal secondary harts
    #[cfg(target_arch = "riscv64")]
    smp::BOOT_HART_DONE.store(true, Release);

    // Step 11: Enter the shared scheduling loop
    scheduling_loop()
}

// ========== Secondary Harts ==========

/// Secondary hart entry: wait for hart 0, set satp, enable timer, enter scheduling loop.
#[cfg(target_arch = "riscv64")]
extern "C" fn secondary_main() -> ! {
    // Spin until hart 0 finishes initialization
    while !smp::BOOT_HART_DONE.load(Acquire) {
        core::hint::spin_loop();
    }

    // Set kernel satp on this hart (enable Sv39 paging)
    let satp_val = KERNEL_SATP.load(Relaxed);
    unsafe {
        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",
            in(reg) satp_val,
        );
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
/// Each iteration: dequeue a process, run it until a trap, handle the trap
/// (re-enqueue, mark finished, etc.), then loop back to dequeue the next.
fn scheduling_loop() -> ! {
    loop {
        // Try to dequeue a ready process
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

/// Run a single process until it yields, times out, exits, or faults.
#[cfg(target_arch = "riscv64")]
fn run_task(idx: usize) {
    // Get exclusive reference via raw pointer. Safe because this hart
    // dequeued `idx` from the ready queue, guaranteeing exclusive access.
    let process = unsafe { process_mut(idx) }.as_mut().unwrap();

    // Set per-hart current process pointer for syscall handlers
    CURRENT_PROCESS[smp::hart_id()].store(process as *mut Process as usize, Relaxed);

    // Get portal reference (safe: MultislotPortal with NUM_HARTS slots supports
    // concurrent access from different harts using different slot keys via TpReg)
    let portal = unsafe { &mut *(PORTAL_PTR.load(Relaxed) as *mut MultislotPortal) };

    // Set timer for preemptive scheduling
    tg_sbi::set_timer(time::read64() + 12500);

    // Execute the process through the portal (switches address space)
    unsafe { process.context.execute(portal, TpReg) };

    // Read trap cause
    use scause::*;
    match scause::read().cause() {
        // -- Timer interrupt: time slice expired --
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            // Clear the timer to avoid immediate re-trigger
            tg_sbi::set_timer(u64::MAX);
            // Re-enqueue the process
            let mut q = QUEUE.lock();
            q.push(idx);
        }
        // -- Syscall: user program executed ecall --
        Trap::Exception(Exception::UserEnvCall) => {
            use tg_syscall::{SyscallId as Id, SyscallResult as Ret};

            let ctx = &mut process.context.context;
            let id: Id = ctx.a(7).into();
            // Track syscall call count
            let id_num: usize = ctx.a(7);
            let counts = unsafe { syscall_counts_mut(idx) };
            if id_num < 500 {
                counts[id_num] += 1;
            }
            let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];
            match tg_syscall::handle(Caller { entity: idx, flow: 0 }, id, args) {
                Ret::Done(ret) => match id {
                    // exit: mark process as finished
                    Id::EXIT => {
                        {
                            let _pl = smp::PRINT_LOCK.lock();
                            log::info!("app{idx} exit with code {}", ctx.a(0));
                        }
                        // Drop the process (free address space)
                        unsafe { *process_mut(idx) = None };
                        // Reset syscall counts
                        *counts = [0; 500];
                        let mut q = QUEUE.lock();
                        q.finished_count += 1;
                    }
                    // Other syscalls: write return value, advance sepc
                    _ => {
                        *ctx.a_mut(0) = ret as _;
                        ctx.move_next();
                        let mut q = QUEUE.lock();
                        q.push(idx);
                    }
                },
                // Unsupported syscall: kill the process
                Ret::Unsupported(_) => {
                    {
                        let _pl = smp::PRINT_LOCK.lock();
                        log::error!("app{idx} call an unsupported syscall: {id:?}");
                    }
                    unsafe { *process_mut(idx) = None };
                    let mut q = QUEUE.lock();
                    q.finished_count += 1;
                }
            }
        }
        // -- Other exception (illegal instruction, page fault, etc.) --
        Trap::Exception(e) => {
            {
                let _pl = smp::PRINT_LOCK.lock();
                log::error!(
                    "app{idx} was killed by {e:?}, stval = {:#x}, sepc = {:#x}",
                    stval::read(),
                    process.context.context.pc()
                );
            }
            unsafe { *process_mut(idx) = None };
            let mut q = QUEUE.lock();
            q.finished_count += 1;
        }
        // -- Unexpected interrupt --
        Trap::Interrupt(ir) => {
            {
                let _pl = smp::PRINT_LOCK.lock();
                log::error!("app{idx} was killed by an unexpected interrupt {ir:?}");
            }
            unsafe { *process_mut(idx) = None };
            let mut q = QUEUE.lock();
            q.finished_count += 1;
        }
    }

    // Clear per-hart current process pointer
    CURRENT_PROCESS[smp::hart_id()].store(0, Relaxed);
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

// ========== Kernel Address Space ==========

/// Build the kernel address space.
///
/// Contains:
/// - **Identity mapping**: kernel code, data, heap regions (VA == PA)
/// - **Portal mapping**: portal physical pages mapped to highest virtual pages
fn kernel_space(
    layout: tg_linker::KernelLayout,
    memory: usize,
    portal: usize,
    portal_pages: usize,
) -> AddressSpace<Sv39, Sv39Manager> {
    let mut space = AddressSpace::<Sv39, Sv39Manager>::new();
    // Map kernel sections (identity mapping: VPN == PPN)
    for region in layout.iter() {
        log::info!("{region}");
        use tg_linker::KernelRegionTitle::*;
        let flags = match region.title {
            Text => "X_RV",
            Rodata => "__RV",
            Data | Boot => "_WRV",
        };
        let s = VAddr::<Sv39>::new(region.range.start);
        let e = VAddr::<Sv39>::new(region.range.end);
        space.map_extern(
            s.floor()..e.ceil(),
            PPN::new(s.floor().val()),
            build_flags(flags),
        )
    }
    // Map kernel heap region (identity mapping)
    log::info!(
        "(heap) ---> {:#10x}..{:#10x}",
        layout.end(),
        layout.start() + memory
    );
    let s = VAddr::<Sv39>::new(layout.end());
    let e = VAddr::<Sv39>::new(layout.start() + memory);
    space.map_extern(
        s.floor()..e.ceil(),
        PPN::new(s.floor().val()),
        build_flags("_WRV"),
    );
    // Map portal pages to highest virtual pages.
    // With NUM_HARTS slots, portal may span multiple pages.
    for p in 0..portal_pages {
        let vpn = VPN::new(PROTAL_TRANSIT.val() - (portal_pages - 1 - p));
        let ppn = PPN::new((portal + p * (1 << Sv39::PAGE_BITS)) >> Sv39::PAGE_BITS);
        space.map_extern(
            vpn..vpn + 1,
            ppn,
            build_flags("__G_XWRV"),
        );
    }
    println!();
    // Activate kernel address space
    unsafe { satp::set(satp::Mode::Sv39, 0, space.root_ppn().val()) };
    space
}

// ========== Interface Implementations ==========

/// Console and syscall trait implementations.
///
/// Key difference from ch3-smp: syscall handlers need address translation
/// because user pointers are virtual addresses.
mod impls {
    use super::Sv39;
    use alloc::alloc::alloc_zeroed;
    use core::{alloc::Layout, ptr::NonNull};
    use tg_console::log;
    use tg_kernel_vm::{
        page_table::{MmuMeta, Pte, VAddr, VmFlags, PPN, VPN},
        PageManager,
    };
    use tg_syscall::*;

    /// Build page table flags from string (compile-time constant).
    #[cfg(target_arch = "riscv64")]
    pub const fn build_flags(s: &str) -> VmFlags<Sv39> {
        VmFlags::build_from_str(s)
    }

    /// Parse page table flags from string (runtime).
    #[cfg(target_arch = "riscv64")]
    pub fn parse_flags(s: &str) -> Result<VmFlags<Sv39>, ()> {
        s.parse()
    }

    // Non-RISC-V64 stubs
    #[cfg(not(target_arch = "riscv64"))]
    pub const fn build_flags(_s: &str) -> VmFlags<Sv39> {
        unsafe { VmFlags::from_raw(0) }
    }

    #[cfg(not(target_arch = "riscv64"))]
    pub fn parse_flags(_s: &str) -> Result<VmFlags<Sv39>, ()> {
        Ok(unsafe { VmFlags::from_raw(0) })
    }

    /// Sv39 page table manager: handles physical page allocation and mapping.
    #[repr(transparent)]
    pub struct Sv39Manager(NonNull<Pte<Sv39>>);

    impl Sv39Manager {
        /// Custom flag bit: marks a page as kernel-allocated (for deallocate).
        const OWNED: VmFlags<Sv39> = unsafe { VmFlags::from_raw(1 << 8) };

        /// Allocate and zero physical pages.
        #[inline]
        fn page_alloc<T>(count: usize) -> *mut T {
            unsafe {
                alloc_zeroed(Layout::from_size_align_unchecked(
                    count << Sv39::PAGE_BITS,
                    1 << Sv39::PAGE_BITS,
                ))
            }
            .cast()
        }
    }

    /// Implement PageManager trait for address space operations.
    impl PageManager<Sv39> for Sv39Manager {
        #[inline]
        fn new_root() -> Self {
            Self(NonNull::new(Self::page_alloc(1)).unwrap())
        }

        #[inline]
        fn root_ppn(&self) -> PPN<Sv39> {
            PPN::new(self.0.as_ptr() as usize >> Sv39::PAGE_BITS)
        }

        #[inline]
        fn root_ptr(&self) -> NonNull<Pte<Sv39>> {
            self.0
        }

        #[inline]
        fn p_to_v<T>(&self, ppn: PPN<Sv39>) -> NonNull<T> {
            unsafe { NonNull::new_unchecked(VPN::<Sv39>::new(ppn.val()).base().as_mut_ptr()) }
        }

        #[inline]
        fn v_to_p<T>(&self, ptr: NonNull<T>) -> PPN<Sv39> {
            PPN::new(VAddr::<Sv39>::new(ptr.as_ptr() as _).floor().val())
        }

        #[inline]
        fn check_owned(&self, pte: Pte<Sv39>) -> bool {
            pte.flags().contains(Self::OWNED)
        }

        #[inline]
        fn allocate(&mut self, len: usize, flags: &mut VmFlags<Sv39>) -> NonNull<u8> {
            *flags |= Self::OWNED;
            NonNull::new(Self::page_alloc(len)).unwrap()
        }

        fn deallocate(&mut self, _pte: Pte<Sv39>, _len: usize) -> usize {
            todo!()
        }

        fn drop_root(&mut self) {
            todo!()
        }
    }

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

    /// IO syscall: handles write with address translation.
    impl IO for SyscallContext {
        fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            match fd {
                STDOUT | STDDEBUG => {
                    #[cfg(target_arch = "riscv64")]
                    {
                        const READABLE: VmFlags<Sv39> = build_flags("RV");
                        let process = super::current_process();
                        if let Some(ptr) = process
                            .address_space
                            .translate::<u8>(VAddr::new(buf), READABLE)
                        {
                            let _pl = crate::smp::PRINT_LOCK.lock();
                            print!("{}", unsafe {
                                core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                                    ptr.as_ptr(),
                                    count,
                                ))
                            });
                            count as _
                        } else {
                            let _pl = crate::smp::PRINT_LOCK.lock();
                            log::error!("ptr not readable");
                            -1
                        }
                    }
                    #[cfg(not(target_arch = "riscv64"))]
                    { count as _ }
                }
                _ => {
                    let _pl = crate::smp::PRINT_LOCK.lock();
                    log::error!("unsupported fd: {fd}");
                    -1
                }
            }
        }
    }

    /// Process syscall: handles exit and sbrk.
    impl Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: Caller, _status: usize) -> isize {
            0
        }

        /// sbrk: adjust process heap size.
        fn sbrk(&self, _caller: Caller, size: i32) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                let process = super::current_process();
                if let Some(old_brk) = process.change_program_brk(size as isize) {
                    old_brk as isize
                } else {
                    -1
                }
            }
            #[cfg(not(target_arch = "riscv64"))]
            { -1 }
        }
    }

    /// Scheduling syscall: handles yield.
    impl Scheduling for SyscallContext {
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }
    }

    /// Clock syscall: handles clock_gettime with address translation.
    impl Clock for SyscallContext {
        #[inline]
        fn clock_gettime(&self, _caller: Caller, clock_id: ClockId, tp: usize) -> isize {
            match clock_id {
                ClockId::CLOCK_MONOTONIC => {
                    #[cfg(target_arch = "riscv64")]
                    {
                        const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
                        let process = super::current_process();
                        if let Some(mut ptr) = process
                            .address_space
                            .translate::<TimeSpec>(VAddr::new(tp), WRITABLE)
                        {
                            let time = riscv::register::time::read() * 10000 / 125;
                            *unsafe { ptr.as_mut() } = TimeSpec {
                                tv_sec: time / 1_000_000_000,
                                tv_nsec: time % 1_000_000_000,
                            };
                            0
                        } else {
                            log::error!("ptr not readable");
                            -1
                        }
                    }
                    #[cfg(not(target_arch = "riscv64"))]
                    { -1 }
                }
                _ => -1,
            }
        }
    }

    /// Trace syscall: read/write user memory, query syscall counts.
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
                    #[cfg(target_arch = "riscv64")]
                    {
                        const READABLE: VmFlags<Sv39> = build_flags("U__RV");
                        let process = super::current_process();
                        if let Some(ptr) = process
                            .address_space
                            .translate::<u8>(VAddr::new(id), READABLE)
                        {
                            unsafe { *ptr.as_ptr() as isize }
                        } else {
                            -1
                        }
                    }
                    #[cfg(not(target_arch = "riscv64"))]
                    { -1 }
                }
                1 => {
                    #[cfg(target_arch = "riscv64")]
                    {
                        const WRITABLE: VmFlags<Sv39> = build_flags("U_W_V");
                        let process = super::current_process();
                        if let Some(mut ptr) = process
                            .address_space
                            .translate::<u8>(VAddr::new(id), WRITABLE)
                        {
                            unsafe { *ptr.as_mut() = data as u8; }
                            0
                        } else {
                            -1
                        }
                    }
                    #[cfg(not(target_arch = "riscv64"))]
                    { -1 }
                }
                2 => {
                    if id < 500 {
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

    /// Memory syscall: handles mmap and munmap with address translation.
    impl Memory for SyscallContext {
        fn mmap(
            &self,
            _caller: Caller,
            addr: usize,
            len: usize,
            prot: i32,
            _flags: i32,
            _fd: i32,
            _offset: usize,
        ) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;
                if addr % PAGE_SIZE != 0 { return -1; }
                if prot & !0x7 != 0 { return -1; }
                if prot & 0x7 == 0 { return -1; }

                let process = super::current_process();
                if len == 0 { return 0; }

                let len_aligned = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
                let start_vpn = VPN::<Sv39>::new(addr >> Sv39::PAGE_BITS);
                let end_vpn = VPN::<Sv39>::new((addr + len_aligned) >> Sv39::PAGE_BITS);

                const CHECK: VmFlags<Sv39> = build_flags("__V");
                for vpn_val in start_vpn.val()..end_vpn.val() {
                    let va = VAddr::<Sv39>::new(vpn_val << Sv39::PAGE_BITS);
                    if process.address_space.translate::<u8>(va, CHECK).is_some() {
                        return -1;
                    }
                }

                let mut flags_str: [u8; 5] = *b"U___V";
                if prot & 0x4 != 0 { flags_str[1] = b'X'; }
                if prot & 0x2 != 0 { flags_str[2] = b'W'; }
                if prot & 0x1 != 0 { flags_str[3] = b'R'; }
                let flags = parse_flags(unsafe { core::str::from_utf8_unchecked(&flags_str) }).unwrap();
                process.address_space.map(start_vpn..end_vpn, &[], 0, flags);
                0
            }
            #[cfg(not(target_arch = "riscv64"))]
            { -1 }
        }

        fn munmap(&self, _caller: Caller, addr: usize, len: usize) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;
                if addr % PAGE_SIZE != 0 { return -1; }

                let process = super::current_process();
                if len == 0 { return 0; }

                let len_aligned = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
                let start_vpn = VPN::<Sv39>::new(addr >> Sv39::PAGE_BITS);
                let end_vpn = VPN::<Sv39>::new((addr + len_aligned) >> Sv39::PAGE_BITS);

                const CHECK: VmFlags<Sv39> = build_flags("__V");
                for vpn_val in start_vpn.val()..end_vpn.val() {
                    let va = VAddr::<Sv39>::new(vpn_val << Sv39::PAGE_BITS);
                    if process.address_space.translate::<u8>(va, CHECK).is_none() {
                        return -1;
                    }
                }

                process.address_space.unmap(start_vpn..end_vpn);
                0
            }
            #[cfg(not(target_arch = "riscv64"))]
            { -1 }
        }
    }
}

/// Non-RISC-V64 stub module for host compilation (cargo publish --dry-run).
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    use tg_kernel_vm::page_table::{MmuMeta, VmFlags};

    /// Sv39 stub type for host platform.
    #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
    pub struct Sv39;

    impl MmuMeta for Sv39 {
        const P_ADDR_BITS: usize = 56;
        const PAGE_BITS: usize = 12;
        const LEVEL_BITS: &'static [usize] = &[9, 9, 9];
        const PPN_POS: usize = 10;

        #[inline]
        fn is_leaf(value: usize) -> bool {
            value & 0b1110 != 0
        }
    }

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

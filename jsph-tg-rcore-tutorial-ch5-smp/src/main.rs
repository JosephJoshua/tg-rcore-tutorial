//! # Chapter 5 SMP: Multi-core Process Management
//!
//! This chapter extends ch5 (process management) with SMP support.
//! All 4 QEMU harts participate in scheduling, with full fork/exec/wait/exit
//! semantics operating correctly under concurrent access.
//!
//! ## Architecture
//!
//! - **Per-hart stacks** in `.boot.stack` section (safe from `zero_bss`)
//! - **Custom SMP process manager** replacing PManager: BTreeMap-based storage
//!   with VecDeque ready queue and ProcRel parent-child tracking, all under
//!   a single `SpinLockIrq`
//! - **Per-hart portal slots** via `MultislotPortal` with `TpReg` key
//! - **Locked allocator** (`jsph-tg-rcore-tutorial-kernel-alloc-smp`) handles allocation synchronization internally
//! - **PRINT_LOCK** serializes console output across harts
//! - **Per-hart CURRENT_PROCESS/CURRENT_PID** atomics for syscall handlers

// Bare-metal: no std, no main
#![no_std]
#![no_main]
// Strict warnings on RISC-V64; allow dead code on host
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code, unused_imports))]

/// Process module: Process struct with fork, exec, from_elf.
mod process;
/// SMP synchronization primitives.
mod smp;

#[macro_use]
extern crate tg_console;

extern crate alloc;

use crate::{
    impls::{Console, Sv39Manager, SyscallContext},
    process::Process,
};
use alloc::{alloc::alloc, collections::BTreeMap, collections::BTreeSet};
use core::{
    alloc::Layout,
    cell::UnsafeCell,
    ffi::CStr,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicUsize},
};
use impls::{build_flags, parse_flags};

use tg_console::log;

#[cfg(target_arch = "riscv64")]
use core::sync::atomic::Ordering::{self, Acquire, Relaxed, Release};
#[cfg(target_arch = "riscv64")]
use riscv::register::*;

#[cfg(not(target_arch = "riscv64"))]
use stub::Sv39;
#[cfg(target_arch = "riscv64")]
use tg_kernel_vm::page_table::Sv39;

use tg_kernel_context::foreign::MultislotPortal;
#[cfg(target_arch = "riscv64")]
use tg_kernel_context::foreign::TpReg;
use tg_kernel_vm::{
    page_table::{MmuMeta, VAddr, VmMeta, PPN, VPN},
    AddressSpace,
};
use tg_sbi;
use tg_syscall::Caller;
use tg_task_manage::{ProcId, ProcRel};
use xmas_elf::ElfFile;

use alloc::collections::VecDeque;
use spin::Lazy;

// ========== Constants ==========

/// Stack size per hart: 32 KiB.
#[cfg(target_arch = "riscv64")]
const HART_STACK_SIZE: usize = 8 * 4096;

/// Physical memory capacity = 48 MiB.
const MEMORY: usize = 48 << 20;

/// Portal virtual page: highest page of the virtual address space.
const PROTAL_TRANSIT: VPN<Sv39> = VPN::MAX;

/// Big stride constant for stride scheduling.
const BIG_STRIDE: usize = 1_000_000;

// ========== Kernel Space ==========

/// Kernel address space global storage with lazy initialization.
struct KernelSpace {
    inner: UnsafeCell<MaybeUninit<AddressSpace<Sv39, Sv39Manager>>>,
}

unsafe impl Sync for KernelSpace {}

impl KernelSpace {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// Write kernel address space (called once during init).
    unsafe fn write(&self, space: AddressSpace<Sv39, Sv39Manager>) {
        unsafe { *self.inner.get() = MaybeUninit::new(space) };
    }

    /// Get immutable reference to kernel address space.
    unsafe fn assume_init_ref(&self) -> &AddressSpace<Sv39, Sv39Manager> {
        unsafe { &*(*self.inner.get()).as_ptr() }
    }
}

/// Kernel address space global instance.
static KERNEL_SPACE: KernelSpace = KernelSpace::new();

/// Application name to ELF data mapping table.
static APPS: Lazy<BTreeMap<&'static str, &'static [u8]>> = Lazy::new(|| {
    unsafe extern "C" {
        static app_names: u8;
    }
    unsafe {
        tg_linker::AppMeta::locate()
            .iter()
            .scan(&app_names as *const _ as usize, |addr, data| {
                let name = CStr::from_ptr(*addr as _).to_str().unwrap();
                *addr += name.as_bytes().len() + 1;
                Some((name, data))
            })
    }
    .collect()
});

// ========== Static Data ==========

/// Kernel satp value for secondary harts to load.
static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);

/// Shared portal pointer (set by hart 0, read by all).
static PORTAL_PTR: AtomicUsize = AtomicUsize::new(0);

/// Per-hart current process pointer for syscall handlers.
static CURRENT_PROCESS: [AtomicUsize; smp::NUM_HARTS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; smp::NUM_HARTS]
};

/// Per-hart current ProcId (stored as usize).
static CURRENT_PID: [AtomicUsize; smp::NUM_HARTS] = {
    const MAX: AtomicUsize = AtomicUsize::new(usize::MAX);
    [MAX; smp::NUM_HARTS]
};

/// Per-hart flag: set by wait() syscall when the process was blocked (not re-enqueued).
/// Checked by run_process() to skip re-enqueueing.
static WAIT_BLOCKED: [AtomicBool; smp::NUM_HARTS] = {
    const FALSE: AtomicBool = AtomicBool::new(false);
    [FALSE; smp::NUM_HARTS]
};

/// Tracks total number of living processes (in tasks + blocked_on_wait).
/// Used for lock-free "all done?" check by idle harts.
static ACTIVE_PROCESSES: AtomicUsize = AtomicUsize::new(0);

// ========== SMP Process Manager ==========

/// SMP-safe process manager.
///
/// Replaces PManager to avoid the single `current` field problem.
/// All fields are accessed under a single SpinLockIrq.
struct SmpProcManager {
    /// All process entities indexed by ProcId.
    tasks: BTreeMap<ProcId, Process>,
    /// Ready queue (FIFO with stride scheduling in fetch).
    ready_queue: VecDeque<ProcId>,
    /// Parent-child relationships.
    rel_map: BTreeMap<ProcId, ProcRel>,
    /// Processes blocked in wait(), waiting for a child to exit.
    /// When a child exits, make_exited() wakes the parent by re-enqueuing it.
    blocked_on_wait: BTreeSet<ProcId>,
}

impl SmpProcManager {
    /// Create a new empty process manager.
    const fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            ready_queue: VecDeque::new(),
            rel_map: BTreeMap::new(),
            blocked_on_wait: BTreeSet::new(),
        }
    }

    /// Fetch next process from ready queue (stride scheduling).
    /// Returns (ProcId, raw pointer to Process) or None.
    fn fetch_next(&mut self) -> Option<(ProcId, *mut Process)> {
        if self.ready_queue.is_empty() {
            return None;
        }
        // Find process with minimum stride
        let mut min_idx = 0;
        let mut min_stride = usize::MAX;
        for (idx, pid) in self.ready_queue.iter().enumerate() {
            if let Some(proc) = self.tasks.get(pid) {
                if proc.stride < min_stride {
                    min_stride = proc.stride;
                    min_idx = idx;
                }
            }
        }
        let pid = self.ready_queue.remove(min_idx).unwrap();
        if let Some(proc) = self.tasks.get_mut(&pid) {
            proc.stride += BIG_STRIDE / proc.priority;
            let ptr = proc as *mut Process;
            Some((pid, ptr))
        } else {
            None
        }
    }

    /// Add process to ready queue.
    fn make_ready(&mut self, pid: ProcId) {
        self.ready_queue.push_back(pid);
    }

    /// Mark a process as exited. Handles orphan reparenting, parent notification,
    /// and waking parents blocked in wait().
    fn make_exited(&mut self, pid: ProcId, exit_code: isize) {
        // Delete the process entity
        self.tasks.remove(&pid);
        ACTIVE_PROCESSES.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
        // Clean up any stale entries in ready queue for this pid
        self.ready_queue.retain(|p| *p != pid);
        // Handle relationship cleanup
        if let Some(current_rel) = self.rel_map.remove(&pid) {
            let parent_pid = current_rel.parent;
            let children = current_rel.children;
            // Notify parent of child's death
            if let Some(parent_rel) = self.rel_map.get_mut(&parent_pid) {
                parent_rel.del_child(pid, exit_code);
            }
            // Wake parent if it was blocked in wait()
            if self.blocked_on_wait.remove(&parent_pid) {
                self.ready_queue.push_back(parent_pid);
            }
            // Reparent orphaned children to PID 0 (init process)
            let init_pid = ProcId::from_usize(0);
            for child_id in children {
                if let Some(child_rel) = self.rel_map.get_mut(&child_id) {
                    child_rel.parent = init_pid;
                }
                if let Some(init_rel) = self.rel_map.get_mut(&init_pid) {
                    init_rel.add_child(child_id);
                }
            }
        }
    }

    /// Add a new process with parent relationship.
    fn add(&mut self, pid: ProcId, task: Process, parent: ProcId) {
        self.tasks.insert(pid, task);
        self.ready_queue.push_back(pid);
        ACTIVE_PROCESSES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // Only add to parent's children if parent is not the special "no parent" value
        if parent.get_usize() != usize::MAX {
            if let Some(parent_rel) = self.rel_map.get_mut(&parent) {
                parent_rel.add_child(pid);
            }
        }
        self.rel_map.insert(pid, ProcRel::new(parent));
    }

    /// Get mutable reference to a process by PID.
    #[allow(dead_code)]
    fn get_task(&mut self, pid: ProcId) -> Option<&mut Process> {
        self.tasks.get_mut(&pid)
    }

    /// Wait for a child process to exit.
    ///
    /// Returns `Some((dead_pid, exit_code))` if a dead child is found.
    /// Returns `None` if no matching children exist at all (-1 to user).
    ///
    /// If children exist but none are dead yet, **blocks the parent**: adds it
    /// to `blocked_on_wait` and sets the per-hart `WAIT_BLOCKED` flag. The caller
    /// (run_process) must NOT re-enqueue the process. The parent will be woken
    /// when a child exits (in make_exited).
    fn wait(&mut self, parent_pid: ProcId, child_pid: ProcId) -> Option<(ProcId, isize)> {
        let current_rel = self.rel_map.get_mut(&parent_pid)?;
        let result = if child_pid.get_usize() == usize::MAX {
            current_rel.wait_any_child()
        } else {
            current_rel.wait_child(child_pid)
        };
        match &result {
            Some((pid, _)) if pid.get_usize() != usize::MAX - 1 => {
                // Dead child found -- return it directly
                result
            }
            Some(_) => {
                // Children exist but none dead -- block the parent
                self.blocked_on_wait.insert(parent_pid);
                WAIT_BLOCKED[smp::hart_id()].store(true, core::sync::atomic::Ordering::Relaxed);
                // Return -2 sentinel; run_process will see WAIT_BLOCKED and skip re-enqueue
                result
            }
            None => {
                // No children at all (or no matching child)
                // Check if children exist (for the "children alive" case)
                if !current_rel.children.is_empty() {
                    self.blocked_on_wait.insert(parent_pid);
                    WAIT_BLOCKED[smp::hart_id()].store(true, core::sync::atomic::Ordering::Relaxed);
                    Some((ProcId::from_usize(usize::MAX - 1), -1))
                } else {
                    None
                }
            }
        }
    }

}

/// Global SMP process manager, protected by interrupt-safe spinlock.
static PROC_MANAGER: smp::SpinLockIrq<SmpProcManager> =
    smp::SpinLockIrq::new(SmpProcManager::new());

// ========== Embedded User Programs ==========

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

// ========== Entry Point ==========

/// Multi-hart S-mode entry point.
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const NUM_HARTS_CONST: usize = 4;

    #[unsafe(link_section = ".boot.stack")]
    static mut HART_STACKS: [[u8; HART_STACK_SIZE]; NUM_HARTS_CONST] =
        [[0; HART_STACK_SIZE]; NUM_HARTS_CONST];

    core::arch::naked_asm!(
        "mv   tp, a0",
        "la   t0, {stacks}",
        "li   t1, {stack_size}",
        "addi t2, a0, 1",
        "mul  t1, t1, t2",
        "add  sp, t0, t1",
        "bnez a0, {secondary}",
        "j    {main}",
        stacks     = sym HART_STACKS,
        stack_size = const HART_STACK_SIZE,
        main       = sym rust_main,
        secondary  = sym secondary_main,
    )
}

// ========== Helper Functions ==========

/// Get the current process for this hart.
///
/// # Safety
/// Must only be called from within a syscall handler while the process is
/// being run by the current hart.
#[cfg(target_arch = "riscv64")]
fn current_process() -> &'static mut Process {
    let ptr = CURRENT_PROCESS[smp::hart_id()].load(Relaxed);
    unsafe { &mut *(ptr as *mut Process) }
}

/// Get the current ProcId for this hart.
#[cfg(target_arch = "riscv64")]
fn current_pid() -> ProcId {
    ProcId::from_usize(CURRENT_PID[smp::hart_id()].load(Relaxed))
}

// ========== Hart 0: Initialization ==========

/// Hart 0 entry: initialize kernel subsystems, load initproc, signal secondaries.
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

    // Step 4: Allocate portal pages (NUM_HARTS slots)
    let portal_size = MultislotPortal::calculate_size(smp::NUM_HARTS);
    let portal_layout = Layout::from_size_align(portal_size, 1 << Sv39::PAGE_BITS).unwrap();
    let portal_ptr = unsafe { alloc(portal_layout) };
    let portal_pages = portal_layout.size().div_ceil(1 << Sv39::PAGE_BITS).max(1);

    // Step 5: Build kernel address space and activate Sv39 paging
    kernel_space(layout, MEMORY, portal_ptr as _, portal_pages);

    // Store kernel satp for secondary harts
    #[cfg(target_arch = "riscv64")]
    KERNEL_SATP.store(satp::read().bits(), Release);

    // Step 6: Init portal
    let portal =
        unsafe { MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), smp::NUM_HARTS) };
    PORTAL_PTR.store(portal as *const _ as usize, Ordering::Release);

    // Step 7: Init syscall handlers
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_memory(&SyscallContext);

    // Step 8: Load initproc
    let initproc_data = APPS.get("initproc").unwrap();
    if let Some(process) = Process::from_elf(ElfFile::new(initproc_data).unwrap()) {
        let mut pm = PROC_MANAGER.lock();
        let pid = process.pid;
        pm.add(pid, process, ProcId::from_usize(usize::MAX));
    }

    println!();

    // Step 9: Enable S-mode timer interrupt
    #[cfg(target_arch = "riscv64")]
    unsafe {
        sie::set_stimer();
    }

    // Step 10: Signal secondary harts
    #[cfg(target_arch = "riscv64")]
    smp::BOOT_HART_DONE.store(true, Release);

    // Step 11: Enter shared scheduling loop
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

    // Set kernel satp on this hart
    let satp_val = KERNEL_SATP.load(Relaxed);
    unsafe {
        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",
            in(reg) satp_val,
        );
    }

    // Enable S-mode timer interrupt
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
/// Each iteration: dequeue a process via stride scheduling, run it until a trap,
/// handle the trap (re-enqueue, mark exited, etc.), then loop.
fn scheduling_loop() -> ! {
    loop {
        // Phase 1: fetch next process from ready queue (lock held briefly)
        let task_info = {
            let mut pm = PROC_MANAGER.lock();
            pm.fetch_next()
        };

        match task_info {
            Some((pid, proc_ptr)) => {
                run_process(pid, proc_ptr);
            }
            None => {
                if ACTIVE_PROCESSES.load(core::sync::atomic::Ordering::Relaxed) == 0 {
                    if smp::hart_id() == 0 {
                        tg_sbi::shutdown(false);
                    } else {
                        loop {
                            #[cfg(target_arch = "riscv64")]
                            unsafe { core::arch::asm!("wfi") };
                            #[cfg(not(target_arch = "riscv64"))]
                            core::hint::spin_loop();
                        }
                    }
                }
                #[cfg(target_arch = "riscv64")]
                unsafe { core::arch::asm!("wfi") };
                #[cfg(not(target_arch = "riscv64"))]
                core::hint::spin_loop();
            }
        }
    }
}

/// Run a single process until it yields, times out, exits, or faults.
#[cfg(target_arch = "riscv64")]
fn run_process(pid: ProcId, proc_ptr: *mut Process) {
    let hid = smp::hart_id();

    // Set per-hart current process pointer and PID
    CURRENT_PROCESS[hid].store(proc_ptr as usize, Relaxed);
    CURRENT_PID[hid].store(pid.get_usize(), Relaxed);

    // Get process reference (safe: this hart has exclusive access via dequeue)
    let process = unsafe { &mut *proc_ptr };

    // Get portal reference
    let portal = unsafe { &mut *(PORTAL_PTR.load(Relaxed) as *mut MultislotPortal) };

    // Set timer for preemptive scheduling
    tg_sbi::set_timer(time::read64() + 12500);

    // Run the process through the portal
    unsafe { process.context.execute(portal, TpReg) };

    // Handle trap
    use scause::*;
    match scause::read().cause() {
        // Timer interrupt: time slice expired
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            tg_sbi::set_timer(u64::MAX);
            // Re-enqueue
            let mut pm = PROC_MANAGER.lock();
            pm.make_ready(pid);
        }
        // Syscall: user program ran ecall
        Trap::Exception(Exception::UserEnvCall) => {
            use tg_syscall::{SyscallId as Id, SyscallResult as Ret};

            let ctx = &mut process.context.context;
            ctx.move_next();
            let id: Id = ctx.a(7).into();
            let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];

            match tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args) {
                Ret::Done(ret) => match id {
                    // exit: mark current process as exited
                    Id::EXIT => {
                        let mut pm = PROC_MANAGER.lock();
                        pm.make_exited(pid, ret);
                    }
                    _ => {
                        let ctx = &mut process.context.context;
                        *ctx.a_mut(0) = ret as _;
                        // Check if the syscall blocked this process (e.g., wait with no dead children).
                        // If WAIT_BLOCKED was set by the syscall handler, the process is already
                        // in blocked_on_wait and must NOT be re-enqueued. It will be woken by
                        // make_exited() when a child exits.
                        if WAIT_BLOCKED[smp::hart_id()].swap(false, core::sync::atomic::Ordering::Relaxed) {
                            // Process blocked -- do not re-enqueue
                        } else {
                            let mut pm = PROC_MANAGER.lock();
                            pm.make_ready(pid);
                        }
                    }
                },
                Ret::Unsupported(_) => {
                    {
                        let _pl = smp::PRINT_LOCK.lock();
                        log::info!("id = {id:?}");
                    }
                    let mut pm = PROC_MANAGER.lock();
                    pm.make_exited(pid, -2);
                }
            }
        }
        // Other traps: kill the process
        e => {
            {
                let _pl = smp::PRINT_LOCK.lock();
                log::error!("unsupported trap: {e:?}");
            }
            let mut pm = PROC_MANAGER.lock();
            pm.make_exited(pid, -3);
        }
    }

    // Clear per-hart current process/PID
    CURRENT_PROCESS[hid].store(0, Relaxed);
    CURRENT_PID[hid].store(usize::MAX, Relaxed);
}

/// Non-riscv64 stub for `run_process`.
#[cfg(not(target_arch = "riscv64"))]
fn run_process(_pid: ProcId, _proc_ptr: *mut Process) {}

// ========== Panic Handler ==========

/// Panic handler: print error info and shutdown with error status.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

// ========== Kernel Address Space ==========

/// Build the kernel address space (identity-mapped + portal).
fn kernel_space(
    layout: tg_linker::KernelLayout,
    memory: usize,
    portal: usize,
    portal_pages: usize,
) {
    let mut space = AddressSpace::new();
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
    // Map heap region
    let s = VAddr::<Sv39>::new(layout.end());
    let e = VAddr::<Sv39>::new(layout.start() + memory);
    log::info!("(heap) ---> {:#10x}..{:#10x}", s.val(), e.val());
    space.map_extern(
        s.floor()..e.ceil(),
        PPN::new(s.floor().val()),
        build_flags("_WRV"),
    );
    // Map portal pages
    for p in 0..portal_pages {
        let vpn = VPN::new(PROTAL_TRANSIT.val() - (portal_pages - 1 - p));
        let ppn = PPN::new((portal + p * (1 << Sv39::PAGE_BITS)) >> Sv39::PAGE_BITS);
        space.map_extern(vpn..vpn + 1, ppn, build_flags("__G_XWRV"));
    }
    println!();
    // Activate kernel address space
    unsafe { satp::set(satp::Mode::Sv39, 0, space.root_ppn().val()) };
    // Save kernel address space
    unsafe { KERNEL_SPACE.write(space) };
}

/// Copy the portal page table entry from kernel to user address space.
fn map_portal(space: &AddressSpace<Sv39, Sv39Manager>) {
    let portal_idx = PROTAL_TRANSIT.index_in(Sv39::MAX_LEVEL);
    space.root()[portal_idx] = unsafe { KERNEL_SPACE.assume_init_ref() }.root()[portal_idx];
}

// ========== Interface Implementations ==========

/// Console and syscall trait implementations for SMP.
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
    use tg_task_manage::ProcId;
    use xmas_elf::ElfFile;

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

    #[cfg(not(target_arch = "riscv64"))]
    pub const fn build_flags(_s: &str) -> VmFlags<Sv39> {
        unsafe { VmFlags::from_raw(0) }
    }

    #[cfg(not(target_arch = "riscv64"))]
    pub fn parse_flags(_s: &str) -> Result<VmFlags<Sv39>, ()> {
        Ok(unsafe { VmFlags::from_raw(0) })
    }

    /// Sv39 page table manager with SMP-safe allocation.
    #[repr(transparent)]
    pub struct Sv39Manager(NonNull<Pte<Sv39>>);

    impl Sv39Manager {
        /// Custom flag bit: marks a page as kernel-allocated.
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

    /// Syscall context implementation for SMP.
    pub struct SyscallContext;

    /// IO syscall: handles write and read with address translation.
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
                    {
                        count as _
                    }
                }
                _ => {
                    let _pl = crate::smp::PRINT_LOCK.lock();
                    log::error!("unsupported fd: {fd}");
                    -1
                }
            }
        }

        fn read(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            if fd == STDIN {
                #[cfg(target_arch = "riscv64")]
                {
                    const WRITEABLE: VmFlags<Sv39> = build_flags("W_V");
                    let process = super::current_process();
                    if let Some(mut ptr) = process
                        .address_space
                        .translate::<u8>(VAddr::new(buf), WRITEABLE)
                    {
                        let mut ptr = unsafe { ptr.as_mut() } as *mut u8;
                        for _ in 0..count {
                            let c = tg_sbi::console_getchar() as u8;
                            unsafe {
                                *ptr = c;
                                ptr = ptr.add(1);
                            }
                        }
                        count as _
                    } else {
                        log::error!("ptr not writeable");
                        -1
                    }
                }
                #[cfg(not(target_arch = "riscv64"))]
                {
                    count as _
                }
            } else {
                log::error!("unsupported fd: {fd}");
                -1
            }
        }
    }

    /// Process management syscall implementation for SMP.
    impl Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: Caller, exit_code: usize) -> isize {
            exit_code as isize
        }

        /// fork: create child process by deep-copying parent's address space.
        fn fork(&self, _caller: Caller) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                let process = super::current_process();
                let parent_pid = process.pid;
                let mut child_proc = process.fork().unwrap();
                let child_pid = child_proc.pid;
                // Child's a0 = 0 (fork return value)
                *child_proc.context.context.a_mut(0) = 0;
                // Add child to process manager
                let mut pm = super::PROC_MANAGER.lock();
                pm.add(child_pid, child_proc, parent_pid);
                child_pid.get_usize() as isize
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                -1
            }
        }

        /// exec: replace current process's address space with new ELF program.
        fn exec(&self, _caller: Caller, path: usize, count: usize) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                const READABLE: VmFlags<Sv39> = build_flags("RV");
                let process = super::current_process();
                process
                    .address_space
                    .translate::<u8>(VAddr::new(path), READABLE)
                    .map(|ptr| unsafe {
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                            ptr.as_ptr(),
                            count,
                        ))
                    })
                    .and_then(|name| super::APPS.get(name))
                    .and_then(|input| ElfFile::new(input).ok())
                    .map_or_else(
                        || {
                            let _pl = crate::smp::PRINT_LOCK.lock();
                            log::error!("unknown app, select one in the list: ");
                            super::APPS.keys().for_each(|app| println!("{app}"));
                            println!();
                            -1
                        },
                        |data| {
                            process.exec(data);
                            0
                        },
                    )
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                -1
            }
        }

        /// wait: wait for a child process to exit.
        fn wait(&self, _caller: Caller, pid: isize, exit_code_ptr: usize) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                let parent_pid = super::current_pid();
                let process = super::current_process();
                const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
                let mut pm = super::PROC_MANAGER.lock();
                if let Some((dead_pid, exit_code)) =
                    pm.wait(parent_pid, ProcId::from_usize(pid as usize))
                {
                    // Write exit code to user space pointer
                    if let Some(mut ptr) = process
                        .address_space
                        .translate::<i32>(VAddr::new(exit_code_ptr), WRITABLE)
                    {
                        unsafe { *ptr.as_mut() = exit_code as i32 };
                    }
                    dead_pid.get_usize() as isize
                } else {
                    -1
                }
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                -1
            }
        }

        /// getpid: return current process PID.
        fn getpid(&self, _caller: Caller) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                super::current_pid().get_usize() as isize
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                -1
            }
        }

        /// spawn: create new process directly from ELF (no fork+exec).
        fn spawn(&self, _caller: Caller, path: usize, count: usize) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                use crate::process::Process as ProcStruct;
                let parent_pid = super::current_pid();
                let process = super::current_process();
                const READABLE: VmFlags<Sv39> = build_flags("RV");
                let child = process
                    .address_space
                    .translate::<u8>(VAddr::new(path), READABLE)
                    .map(|ptr| unsafe {
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                            ptr.as_ptr(),
                            count,
                        ))
                    })
                    .and_then(|name| super::APPS.get(name))
                    .and_then(|input| ElfFile::new(input).ok())
                    .and_then(|elf| ProcStruct::from_elf(elf));
                match child {
                    Some(child_proc) => {
                        let child_pid = child_proc.pid;
                        let mut pm = super::PROC_MANAGER.lock();
                        pm.add(child_pid, child_proc, parent_pid);
                        child_pid.get_usize() as isize
                    }
                    None => -1,
                }
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                -1
            }
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
            {
                -1
            }
        }
    }

    /// Scheduling syscall implementation.
    impl Scheduling for SyscallContext {
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }

        /// set_priority: set current process priority for stride scheduling.
        fn set_priority(&self, _caller: Caller, prio: isize) -> isize {
            if prio < 2 {
                return -1;
            }
            #[cfg(target_arch = "riscv64")]
            {
                let process = super::current_process();
                process.priority = prio as usize;
                prio
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                prio
            }
        }
    }

    /// Clock syscall implementation.
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
                    {
                        -1
                    }
                }
                _ => -1,
            }
        }
    }

    /// Memory management syscall implementation.
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
                if addr % PAGE_SIZE != 0 {
                    return -1;
                }
                if prot & !0x7 != 0 {
                    return -1;
                }
                if prot & 0x7 == 0 {
                    return -1;
                }

                let process = super::current_process();
                if len == 0 {
                    return 0;
                }

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
                if prot & 0x4 != 0 {
                    flags_str[1] = b'X';
                }
                if prot & 0x2 != 0 {
                    flags_str[2] = b'W';
                }
                if prot & 0x1 != 0 {
                    flags_str[3] = b'R';
                }
                let flags =
                    parse_flags(unsafe { core::str::from_utf8_unchecked(&flags_str) }).unwrap();
                process
                    .address_space
                    .map(start_vpn..end_vpn, &[], 0, flags);
                0
            }
            #[cfg(not(target_arch = "riscv64"))]
            {
                -1
            }
        }

        fn munmap(&self, _caller: Caller, addr: usize, len: usize) -> isize {
            #[cfg(target_arch = "riscv64")]
            {
                const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;
                if addr % PAGE_SIZE != 0 {
                    return -1;
                }

                let process = super::current_process();
                if len == 0 {
                    return 0;
                }

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
            {
                -1
            }
        }
    }
}

/// Non-RISC-V64 stub module for host compilation.
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

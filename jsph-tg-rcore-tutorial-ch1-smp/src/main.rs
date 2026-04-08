//! Chapter 1 SMP: Multi-core bare-metal demo.
//!
//! Boots 4 harts, demonstrates unsynchronized vs synchronized output,
//! parallel computation speedup, and renders tangram on VirtIO-GPU.

#![no_std]
#![no_main]
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]

#[cfg(target_arch = "riscv64")]
extern crate alloc;

use tg_sbi::{console_putchar, shutdown};

#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "riscv64")]
mod allocator;
#[cfg(target_arch = "riscv64")]
mod gpu;
#[cfg(target_arch = "riscv64")]
mod smp;
#[cfg(target_arch = "riscv64")]
mod tangram;

#[cfg(target_arch = "riscv64")]
use smp::*;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Multi-hart S-mode entry point.
///
/// `a0 = hart ID` (passed from M-mode via `mret`).
/// Each hart gets its own stack from `HART_STACKS`; hart 0 jumps to
/// `rust_main`, all others jump to `secondary_main`.
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 64 * 1024;
    const NUM_HARTS_CONST: usize = 4;

    #[unsafe(link_section = ".bss.uninit")]
    static mut HART_STACKS: [[u8; STACK_SIZE]; NUM_HARTS_CONST] =
        [[0; STACK_SIZE]; NUM_HARTS_CONST];

    core::arch::naked_asm!(
        // Save hart ID to tp register immediately (before anything clobbers a0)
        "mv   tp, a0",
        // Per-hart stack: sp = HART_STACKS + (hartid + 1) * STACK_SIZE
        "la   t0, {stacks}",
        "li   t1, {stack_size}",
        "addi t2, a0, 1",
        "mul  t1, t1, t2",
        "add  sp, t0, t1",
        // Branch: hart 0 -> rust_main, others -> secondary_main
        "bnez a0, {secondary}",
        "j    {main}",
        stacks     = sym HART_STACKS,
        stack_size = const STACK_SIZE,
        main       = sym rust_main,
        secondary  = sym secondary_main,
    )
}

// ---------------------------------------------------------------------------
// Hart 0 main
// ---------------------------------------------------------------------------

/// Published hart count for demos (set by hart 0 after registration window).
#[cfg(target_arch = "riscv64")]
static DEMO_HART_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Hart 0 main function.
extern "C" fn rust_main() -> ! {
    // Print hello
    for c in b"Hello, world!\n" {
        console_putchar(*c);
    }

    #[cfg(target_arch = "riscv64")]
    {
        // Count hart 0 as active
        ACTIVE_HARTS.fetch_add(1, Ordering::AcqRel);

        // Signal secondary harts that initialization is done
        BOOT_HART_DONE.store(true, Ordering::Release);

        // Wait 10ms (100,000 ticks at 10 MHz) for secondary harts to register
        let start = rdtime();
        while rdtime() - start < 100_000 {
            core::hint::spin_loop();
        }

        // Publish the final hart count
        let active = ACTIVE_HARTS.load(Ordering::Acquire);
        DEMO_HART_COUNT.store(active, Ordering::Release);

        // Run demos with the actual number of active harts
        demo_sequence(0, active);

        // Render tangram (hart 0 only)
        let (mut gpu, fb_info) = gpu::init();
        let buf = unsafe { core::slice::from_raw_parts_mut(fb_info.ptr, fb_info.len) };

        // Fill white background
        for pixel in buf.chunks_exact_mut(4) {
            pixel[0] = tangram::WHITE.b;
            pixel[1] = tangram::WHITE.g;
            pixel[2] = tangram::WHITE.r;
            pixel[3] = tangram::WHITE.a;
        }

        // Draw tangram pieces
        for piece in &tangram::PIECES {
            let color = tangram::COLORS[piece.color_idx];
            tangram::fill_polygon(buf, fb_info.width, fb_info.height, piece.vertices, color);
        }

        gpu.flush().expect("flush failed");
    }

    shutdown(false)
}

// ---------------------------------------------------------------------------
// Secondary hart entry
// ---------------------------------------------------------------------------

/// Secondary hart entry point. Waits for hart 0, participates in demos,
/// then idles with WFI.
#[cfg(target_arch = "riscv64")]
extern "C" fn secondary_main() -> ! {
    let hartid = hart_id();

    // Wait for hart 0 to finish initialization
    while !BOOT_HART_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // Register this hart as active
    ACTIVE_HARTS.fetch_add(1, Ordering::AcqRel);

    // Wait for hart 0 to publish the final count
    let active = loop {
        let count = DEMO_HART_COUNT.load(Ordering::Acquire);
        if count > 0 {
            break count;
        }
        core::hint::spin_loop();
    };

    // Run demos
    demo_sequence(hartid, active);

    // Secondary harts idle after demos
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

// ---------------------------------------------------------------------------
// Demo infrastructure
// ---------------------------------------------------------------------------

/// Barrier for synchronizing between demo phases.
#[cfg(target_arch = "riscv64")]
static DEMO_BARRIER: Barrier = Barrier::new();

/// Storage for sequential benchmark time (set by hart 0).
#[cfg(target_arch = "riscv64")]
static SEQ_TIME: AtomicUsize = AtomicUsize::new(0);

/// Parallel phase start timestamp (set by hart 0).
#[cfg(target_arch = "riscv64")]
static PAR_START: AtomicUsize = AtomicUsize::new(0);

/// Result storage for parallel computation (one slot per hart).
#[cfg(target_arch = "riscv64")]
static PARALLEL_RESULTS: [AtomicUsize; 4] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];

// ---------------------------------------------------------------------------
// SBI printing helpers (not cfg-gated)
// ---------------------------------------------------------------------------

/// Print a string character by character via SBI (no lock).
fn sbi_print(s: &str) {
    for c in s.bytes() {
        console_putchar(c);
    }
}

/// Print a `usize` in decimal via SBI.
fn sbi_print_num(mut n: usize) {
    if n == 0 {
        console_putchar(b'0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 0;
    while n > 0 {
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    while i > 0 {
        i -= 1;
        console_putchar(buf[i]);
    }
}

// ---------------------------------------------------------------------------
// Compute workload (not cfg-gated)
// ---------------------------------------------------------------------------

/// CPU-bound workload: sum 1..=WORKLOAD_SIZE using volatile operations
/// to prevent the compiler from optimizing away the loop.
fn compute_workload() -> usize {
    const WORKLOAD_SIZE: usize = 5_000_000;
    let mut sum: usize = 0;
    for i in 1..=WORKLOAD_SIZE {
        sum = core::hint::black_box(sum.wrapping_add(core::hint::black_box(i)));
    }
    sum
}

// ---------------------------------------------------------------------------
// Demo sequence
// ---------------------------------------------------------------------------

/// Read the RISC-V `time` CSR.
#[cfg(target_arch = "riscv64")]
fn rdtime() -> usize {
    let time: usize;
    unsafe { core::arch::asm!("rdtime {}", out(reg) time) };
    time
}

/// Run all three demos in sequence, synchronized by barriers.
///
/// Every hart calls this function. Barriers ensure that demo phases
/// execute in order across all harts.
#[cfg(target_arch = "riscv64")]
fn demo_sequence(hartid: usize, num_harts: usize) {
    // =================================================================
    // Demo 1: Unsynchronized Output
    // =================================================================
    if hartid == 0 {
        sbi_print("\n== Demo 1: Unsynchronized Output ==\n");
    }
    DEMO_BARRIER.wait(num_harts);

    // All harts print without lock -- output will be interleaved/garbled.
    sbi_print("[Hart ");
    sbi_print_num(hartid);
    sbi_print("] Hello from hart ");
    sbi_print_num(hartid);
    sbi_print("!\n[Hart ");
    sbi_print_num(hartid);
    sbi_print("] This is line 2 from hart ");
    sbi_print_num(hartid);
    sbi_print(".\n[Hart ");
    sbi_print_num(hartid);
    sbi_print("] This is line 3 from hart ");
    sbi_print_num(hartid);
    sbi_print(".\n");

    DEMO_BARRIER.wait(num_harts);

    // =================================================================
    // Demo 2: Synchronized Output
    // =================================================================
    if hartid == 0 {
        sbi_print("\n== Demo 2: Synchronized Output ==\n");
    }
    DEMO_BARRIER.wait(num_harts);

    // Each hart acquires the lock before printing its entire block.
    PRINT_LOCK.lock();
    sbi_print("[Hart ");
    sbi_print_num(hartid);
    sbi_print("] Hello from hart ");
    sbi_print_num(hartid);
    sbi_print("!\n[Hart ");
    sbi_print_num(hartid);
    sbi_print("] This is line 2 from hart ");
    sbi_print_num(hartid);
    sbi_print(".\n[Hart ");
    sbi_print_num(hartid);
    sbi_print("] This is line 3 from hart ");
    sbi_print_num(hartid);
    sbi_print(".\n");
    PRINT_LOCK.unlock();

    DEMO_BARRIER.wait(num_harts);

    // =================================================================
    // Demo 3: Parallel Computation
    // =================================================================
    if hartid == 0 {
        sbi_print("\n== Demo 3: Parallel Computation ==\n");

        // Sequential phase: hart 0 does all workloads alone.
        let seq_start = rdtime();
        for _ in 0..num_harts {
            let _ = core::hint::black_box(compute_workload());
        }
        let seq_end = rdtime();
        SEQ_TIME.store(seq_end - seq_start, Ordering::Release);
    }

    // All harts synchronize before the parallel phase.
    DEMO_BARRIER.wait(num_harts);

    // Hart 0 records the start time.
    if hartid == 0 {
        PAR_START.store(rdtime(), Ordering::Release);
    }
    DEMO_BARRIER.wait(num_harts);

    // Parallel phase: each hart computes one workload.
    let result = compute_workload();
    PARALLEL_RESULTS[hartid].store(result, Ordering::Relaxed);

    // Wait for all harts to finish.
    DEMO_BARRIER.wait(num_harts);

    // Hart 0 reports results.
    if hartid == 0 {
        let par_end = rdtime();
        let par_time = par_end - PAR_START.load(Ordering::Acquire);
        let seq_time = SEQ_TIME.load(Ordering::Acquire);

        // QEMU virt timer runs at 10 MHz; convert ticks to milliseconds.
        let seq_ms = seq_time / 10_000;
        let par_ms = par_time / 10_000;

        sbi_print("Sequential: ");
        sbi_print_num(num_harts);
        sbi_print(" workloads took ");
        sbi_print_num(seq_ms);
        sbi_print("ms\nParallel:   ");
        sbi_print_num(num_harts);
        sbi_print(" workloads took ");
        sbi_print_num(par_ms);
        sbi_print("ms\n");

        if par_ms > 0 {
            let speedup_x10 = (seq_ms * 10) / par_ms;
            sbi_print("Speedup: ");
            sbi_print_num(speedup_x10 / 10);
            sbi_print(".");
            sbi_print_num(speedup_x10 % 10);
            sbi_print("x\n");
        }
    }
}

// ---------------------------------------------------------------------------
// Panic handler
// ---------------------------------------------------------------------------

/// Panic handler -- shuts down with error status.
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    shutdown(true)
}

// ---------------------------------------------------------------------------
// Non-RISC-V64 stub module
// ---------------------------------------------------------------------------

/// Stubs for host-platform compilation (`cargo publish --dry-run`, etc.).
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    /// Host-platform entry stub.
    #[unsafe(no_mangle)]
    pub extern "C" fn main() -> i32 {
        0
    }

    /// C runtime stub.
    #[unsafe(no_mangle)]
    pub extern "C" fn __libc_start_main() -> i32 {
        0
    }

    /// Rust exception-handling personality stub.
    #[unsafe(no_mangle)]
    pub extern "C" fn rust_eh_personality() {}
}

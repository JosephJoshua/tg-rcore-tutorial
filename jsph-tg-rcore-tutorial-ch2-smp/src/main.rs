//! Chapter 2 SMP: Multi-core batch processing OS.
//!
//! All 4 harts boot from M-mode. Hart 0 runs the batch processing loop.
//! Secondary harts print identification and enter WFI idle loop.

#![no_std]
#![no_main]
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]

#[macro_use]
extern crate tg_console;

use impls::{Console, SyscallContext};
use riscv::register::*;
use tg_console::log;
use tg_kernel_context::LocalContext;
use tg_sbi;
use tg_syscall::{Caller, SyscallId};
#[cfg(target_arch = "riscv64")]
use core::sync::atomic::Ordering;

#[cfg(target_arch = "riscv64")]
mod smp;
#[cfg(target_arch = "riscv64")]
use smp::*;

// Embed user program binaries
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

/// Multi-hart S-mode entry point.
/// a0 = hart ID (from M-mode mret).
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 8 * 4096; // 32 KiB per hart
    const NUM_HARTS_CONST: usize = 4;

    // MUST be in .boot.stack (NOT .bss), because hart 0 calls zero_bss() which
    // would corrupt stacks that secondary harts are already using.
    #[unsafe(link_section = ".boot.stack")]
    static mut HART_STACKS: [[u8; STACK_SIZE]; NUM_HARTS_CONST] = [[0; STACK_SIZE]; NUM_HARTS_CONST];

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
        stack_size = const STACK_SIZE,
        main       = sym rust_main,
        secondary  = sym secondary_main,
    )
}

/// Hart 0: Initialize kernel subsystems and run batch processing.
extern "C" fn rust_main() -> ! {
    // Clear BSS
    unsafe { tg_linker::KernelLayout::locate().zero_bss() };

    // Init console
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG"));
    tg_console::test_log();

    // Init syscall handlers
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);

    // Print hart 0 identification (before signaling secondaries to avoid interleaving)
    println!("[Hart 0] primary, running batch processing");

    // Signal secondary harts
    #[cfg(target_arch = "riscv64")]
    BOOT_HART_DONE.store(true, Ordering::Release);

    // Batch processing (same as original ch2)
    for (i, app) in tg_linker::AppMeta::locate().iter().enumerate() {
        let app_base = app.as_ptr() as usize;
        log::info!("load app{i} to {app_base:#x}");

        let mut ctx = LocalContext::user(app_base);
        let mut user_stack: core::mem::MaybeUninit<[usize; 512]> =
            core::mem::MaybeUninit::uninit();
        let user_stack_ptr = user_stack.as_mut_ptr() as *mut usize;
        *ctx.sp_mut() = unsafe { user_stack_ptr.add(512) } as usize;

        loop {
            unsafe { ctx.execute() };

            use scause::{Exception, Trap};
            match scause::read().cause() {
                Trap::Exception(Exception::UserEnvCall) => {
                    use SyscallResult::*;
                    match handle_syscall(&mut ctx) {
                        Done => continue,
                        Exit(code) => log::info!("app{i} exit with code {code}"),
                        Error(id) => {
                            log::error!("app{i} call an unsupported syscall {}", id.0)
                        }
                    }
                }
                trap => log::error!("app{i} was killed because of {trap:?}"),
            }
            unsafe { core::arch::asm!("fence.i") };
            break;
        }
        let _ = core::hint::black_box(&user_stack);
        println!();
    }

    tg_sbi::shutdown(false)
}

/// Secondary hart entry: print identification and enter WFI loop.
#[cfg(target_arch = "riscv64")]
extern "C" fn secondary_main() -> ! {
    let hartid = hart_id();

    // Wait for hart 0 to finish BSS clear and console init
    while !BOOT_HART_DONE.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    // Print identification with lock
    PRINT_LOCK.lock();
    println!("[Hart {}] online, entering idle loop", hartid);
    PRINT_LOCK.unlock();

    // Set stvec to a panic handler for this hart
    unsafe {
        core::arch::asm!(
            "csrw stvec, {}",
            in(reg) secondary_trap_panic as usize,
        );
    }

    // Idle loop
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

/// If a secondary hart traps unexpectedly, halt.
#[cfg(target_arch = "riscv64")]
extern "C" fn secondary_trap_panic() -> ! {
    loop {
        unsafe { core::arch::asm!("wfi") };
    }
}

// ========== Panic handler ==========

/// Panic handler: print error info and shutdown with error status.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

// ========== Syscall handling ==========

/// Syscall handling result.
enum SyscallResult {
    /// Syscall completed, continue user program.
    Done,
    /// User program requested exit with code.
    Exit(usize),
    /// Unsupported syscall.
    Error(SyscallId),
}

/// Handle a syscall from user context.
fn handle_syscall(ctx: &mut LocalContext) -> SyscallResult {
    use tg_syscall::{SyscallId as Id, SyscallResult as Ret};

    let id = ctx.a(7).into();
    let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];

    match tg_syscall::handle(Caller { entity: 0, flow: 0 }, id, args) {
        Ret::Done(ret) => match id {
            Id::EXIT => SyscallResult::Exit(ctx.a(0)),
            _ => {
                *ctx.a_mut(0) = ret as _;
                ctx.move_next();
                SyscallResult::Done
            }
        },
        Ret::Unsupported(id) => SyscallResult::Error(id),
    }
}

// ========== Interface implementations ==========

/// Console and syscall interface implementations.
mod impls {
    use tg_syscall::{STDDEBUG, STDOUT};

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

    impl tg_syscall::IO for SyscallContext {
        fn write(
            &self,
            _caller: tg_syscall::Caller,
            fd: usize,
            buf: usize,
            count: usize,
        ) -> isize {
            match fd {
                STDOUT | STDDEBUG => {
                    print!("{}", unsafe {
                        core::str::from_utf8_unchecked(core::slice::from_raw_parts(
                            buf as *const u8,
                            count,
                        ))
                    });
                    count as _
                }
                _ => {
                    tg_console::log::error!("unsupported fd: {fd}");
                    -1
                }
            }
        }
    }

    impl tg_syscall::Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: tg_syscall::Caller, _status: usize) -> isize {
            0
        }
    }
}

/// Non-RISC-V64 stub for host compilation.
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

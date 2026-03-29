//! Ch3-Snake: Multiprogramming kernel with VirtIO-GPU snake game.

#![no_std]
#![no_main]
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]

#[cfg(target_arch = "riscv64")]
extern crate alloc;

mod task;

#[cfg(target_arch = "riscv64")]
mod allocator;
#[cfg(target_arch = "riscv64")]
mod gpu;

#[macro_use]
extern crate tg_console;

use impls::{Console, SyscallContext};
use riscv::register::*;
use task::TaskControlBlock;
use tg_console::log;
use tg_sbi;

#[cfg(target_arch = "riscv64")]
use virtio_drivers::{MmioTransport, VirtIOGpu};

/// Global GPU driver.
#[cfg(target_arch = "riscv64")]
static mut GPU: Option<VirtIOGpu<'static, allocator::HalImpl, MmioTransport>> = None;

/// Global framebuffer info.
#[cfg(target_arch = "riscv64")]
static mut FRAMEBUFFER: Option<gpu::Framebuffer> = None;

/// Custom syscall IDs for framebuffer operations.
const SYSCALL_FB_INFO: usize = 2000;
/// Write pixels to framebuffer.
const SYSCALL_FB_WRITE: usize = 2001;

// ========== Keyboard ring buffer ==========

/// Ring buffer size for timer-polled keyboard input.
const KB_BUF_SIZE: usize = 64;
/// Ring buffer storage.
static mut KB_BUF: [u8; KB_BUF_SIZE] = [0; KB_BUF_SIZE];
/// Ring buffer head (read pointer).
static mut KB_HEAD: usize = 0;
/// Ring buffer tail (write pointer).
static mut KB_TAIL: usize = 0;

/// Poll SBI console and push available bytes into ring buffer.
#[cfg(all(target_arch = "riscv64", feature = "interrupt"))]
fn poll_keyboard() {
    unsafe {
        loop {
            let ch = tg_sbi::console_getchar();
            if ch == usize::MAX {
                break;
            }
            let next_tail = (KB_TAIL + 1) % KB_BUF_SIZE;
            if next_tail == KB_HEAD {
                KB_HEAD = (KB_HEAD + 1) % KB_BUF_SIZE;
            }
            KB_BUF[KB_TAIL] = ch as u8;
            KB_TAIL = next_tail;
        }
    }
}

/// Pop one byte from the ring buffer.
fn kb_pop() -> Option<u8> {
    unsafe {
        if KB_HEAD == KB_TAIL {
            None
        } else {
            let byte = KB_BUF[KB_HEAD];
            KB_HEAD = (KB_HEAD + 1) % KB_BUF_SIZE;
            Some(byte)
        }
    }
}

// ========== Syscall counting ==========

/// Per-task syscall counts.
pub static mut SYSCALL_COUNTS: [[u32; 500]; APP_CAPACITY] = [[0; 500]; APP_CAPACITY];

// ========== Boot ==========

#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

const APP_CAPACITY: usize = 32;

#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = (APP_CAPACITY + 10) * 8192; // 336 KiB — TCBs need ~272 KiB + GPU init overhead
    #[unsafe(link_section = ".boot.stack")]
    static mut STACK: [u8; STACK_SIZE] = [0u8; STACK_SIZE];

    core::arch::naked_asm!(
        "la sp, {stack} + {stack_size}",
        "j  {main}",
        stack = sym STACK,
        stack_size = const STACK_SIZE,
        main = sym rust_main,
    )
}

// ========== Kernel main ==========

/// Kernel main: init subsystems, init GPU, then round-robin schedule all tasks.
extern "C" fn rust_main() -> ! {
    unsafe { tg_linker::KernelLayout::locate().zero_bss() };

    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG").or(Some("info")));
    tg_console::test_log();

    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    tg_syscall::init_scheduling(&SyscallContext);
    tg_syscall::init_clock(&SyscallContext);
    tg_syscall::init_trace(&SyscallContext);

    #[cfg(target_arch = "riscv64")]
    {
        unsafe extern "C" { static __end: u8; }
        let kernel_end = (&raw const __end) as usize;
        assert!(kernel_end < 0x8200_0000, "kernel image ({kernel_end:#x}) overlaps DMA pool");
    }

    #[cfg(target_arch = "riscv64")]
    {
        let (gpu_driver, fb_info) = gpu::init();
        let buf = unsafe { core::slice::from_raw_parts_mut(fb_info.ptr, fb_info.len) };

        for byte in buf.iter_mut() {
            *byte = 0xFF;
        }

        let mut gpu_driver = gpu_driver;
        gpu_driver.flush().expect("GPU flush failed");

        unsafe {
            GPU = Some(gpu_driver);
            FRAMEBUFFER = Some(fb_info);
        }

        unsafe { core::arch::asm!("csrs scounteren, {}", in(reg) (1 << 1)) };
    }

    let mut tcbs = [TaskControlBlock::ZERO; APP_CAPACITY];
    let mut index_mod = 0;
    for (i, app) in tg_linker::AppMeta::locate().iter().enumerate() {
        let entry = app.as_ptr() as usize;
        log::info!("load app{i} to {entry:#x}");
        tcbs[i].init(entry);
        index_mod += 1;
    }
    println!();

    unsafe { sie::set_stimer() };

    let mut remain = index_mod;
    let mut i = 0usize;
    while remain > 0 {
        let tcb = &mut tcbs[i];
        if !tcb.finish {
            loop {
                #[cfg(not(feature = "coop"))]
                tg_sbi::set_timer(time::read64() + 12500);

                unsafe { tcb.execute() };

                #[cfg(all(target_arch = "riscv64", feature = "interrupt"))]
                poll_keyboard();

                use scause::*;
                let finish = match scause::read().cause() {
                    Trap::Interrupt(Interrupt::SupervisorTimer) => {
                        tg_sbi::set_timer(u64::MAX);
                        log::trace!("app{i} timeout");
                        false
                    }
                    Trap::Exception(Exception::UserEnvCall) => {
                        use task::SchedulingEvent as Event;
                        match tcb.handle_syscall(i) {
                            Event::None => continue,
                            Event::Exit(code) => {
                                log::info!("app{i} exit with code {code}");
                                true
                            }
                            Event::Yield => {
                                log::debug!("app{i} yield");
                                false
                            }
                            Event::UnsupportedSyscall(id) => {
                                log::error!("app{i} call an unsupported syscall {}", id.0);
                                true
                            }
                        }
                    }
                    Trap::Exception(e) => {
                        log::error!("app{i} was killed by {e:?}");
                        true
                    }
                    Trap::Interrupt(ir) => {
                        log::error!("app{i} was killed by an unexpected interrupt {ir:?}");
                        true
                    }
                };

                if finish {
                    tcb.finish = true;
                    remain -= 1;
                }
                break;
            }
        }
        i = (i + 1) % index_mod;
    }

    tg_sbi::shutdown(false)
}

// ========== Panic ==========

/// Panic handler.
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

// ========== FB syscall handlers ==========

/// FB_INFO: returns (width << 32) | height.
#[cfg(target_arch = "riscv64")]
pub fn handle_fb_info() -> usize {
    let (width, height) = unsafe {
        let p = &raw const FRAMEBUFFER;
        let fb = (*p).as_ref().expect("GPU not initialized");
        (fb.width, fb.height)
    };
    ((width as usize) << 32) | (height as usize)
}

/// FB_WRITE: copy BGRA pixels into framebuffer and flush.
#[cfg(target_arch = "riscv64")]
pub fn handle_fb_write(x: usize, y: usize, w: usize, h: usize, data_ptr: usize) -> usize {
    let (fb_ptr, fb_len, fb_w, fb_h) = unsafe {
        let p = &raw const FRAMEBUFFER;
        let fb = (*p).as_ref().expect("GPU not initialized");
        (fb.ptr, fb.len, fb.width as usize, fb.height as usize)
    };

    if x + w > fb_w || y + h > fb_h || w == 0 || h == 0 {
        return usize::MAX;
    }

    let fb_buf = unsafe { core::slice::from_raw_parts_mut(fb_ptr, fb_len) };
    let user_data = unsafe { core::slice::from_raw_parts(data_ptr as *const u8, w * h * 4) };

    for row in 0..h {
        for col in 0..w {
            let src_off = (row * w + col) * 4;
            let alpha = user_data[src_off + 3];
            if alpha == 0 {
                continue;
            }
            let fb_off = ((y + row) * fb_w + (x + col)) * 4;
            fb_buf[fb_off] = user_data[src_off];
            fb_buf[fb_off + 1] = user_data[src_off + 1];
            fb_buf[fb_off + 2] = user_data[src_off + 2];
            fb_buf[fb_off + 3] = user_data[src_off + 3];
        }
    }

    let gpu = unsafe {
        let p = &raw mut GPU;
        (*p).as_mut().expect("GPU not initialized")
    };
    gpu.flush().expect("GPU flush failed");

    0
}

// ========== Trait implementations ==========

/// Interface implementations.
mod impls {
    use tg_syscall::*;

    /// Console: SBI putchar.
    pub struct Console;

    impl tg_console::Console for Console {
        #[inline]
        fn put_char(&self, c: u8) {
            tg_sbi::console_putchar(c);
        }
    }

    /// Syscall handler context.
    pub struct SyscallContext;

    /// IO: write + non-blocking read.
    impl IO for SyscallContext {
        fn read(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
            if count == 0 {
                return 0;
            }
            match fd {
                STDIN => {
                    let ch = tg_sbi::console_getchar();
                    if ch == usize::MAX {
                        0
                    } else {
                        unsafe { *(buf as *mut u8) = ch as u8; }
                        1
                    }
                }
                3 => {
                    match crate::kb_pop() {
                        Some(byte) => {
                            unsafe { *(buf as *mut u8) = byte; }
                            1
                        }
                        None => 0,
                    }
                }
                _ => -1,
            }
        }

        #[inline]
        fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
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

    /// Process: exit.
    impl Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: Caller, _status: usize) -> isize {
            0
        }
    }

    /// Scheduling: yield.
    impl Scheduling for SyscallContext {
        #[inline]
        fn sched_yield(&self, _caller: Caller) -> isize {
            0
        }
    }

    /// Clock: clock_gettime.
    impl Clock for SyscallContext {
        #[inline]
        fn clock_gettime(&self, _caller: Caller, clock_id: ClockId, tp: usize) -> isize {
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

    /// Trace: read/write memory, count syscalls.
    impl Trace for SyscallContext {
        fn trace(&self, caller: Caller, trace_request: usize, id: usize, data: usize) -> isize {
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
                        unsafe { super::SYSCALL_COUNTS[caller.entity][id] as isize }
                    } else {
                        0
                    }
                }
                _ => -1,
            }
        }
    }
}

/// Stubs for non-RISC-V64 (cargo publish --dry-run).
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    /// Host platform placeholder entry.
    #[unsafe(no_mangle)]
    pub extern "C" fn main() -> i32 { 0 }
    /// C runtime placeholder.
    #[unsafe(no_mangle)]
    pub extern "C" fn __libc_start_main() -> i32 { 0 }
    /// Rust exception personality placeholder.
    #[unsafe(no_mangle)]
    pub extern "C" fn rust_eh_personality() {}
}

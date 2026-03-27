//! # 第二章：批处理系统
//!
//! 本章在第一章"最小执行环境"的基础上，实现了一个**批处理操作系统**，
//! 能够依次加载并运行多个用户程序。
//!
//! ## 核心概念
//!
//! - **批处理系统**：将多个用户程序打包，自动依次执行
//! - **特权级切换**：U-mode（用户态）与 S-mode（内核态）之间的切换
//! - **Trap 处理**：用户程序通过 `ecall` 触发系统调用，或因异常陷入内核
//! - **上下文保存与恢复**：进入/退出 Trap 时保存/恢复用户寄存器状态
//! - **系统调用**：`write`（输出）和 `exit`（退出）
//!
//! 教程阅读建议：
//!
//! - 先看 `rust_main` 的 for-loop：理解批处理“逐个装载、逐个执行”；
//! - 再看 `handle_syscall`：理解 a7/a0-a5/a0 的系统调用寄存器约定；
//! - 最后看 `impls`：把内核态 trait 接口与用户态 syscall 行为对齐起来。

// 不使用标准库，裸机环境没有操作系统提供系统调用支持
#![no_std]
// 不使用标准入口，裸机环境没有 C runtime 进行初始化
#![no_main]
// RISC-V64 架构下启用严格警告和文档检查
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
// 非 RISC-V64 架构允许死代码（用于 cargo publish --dry-run 在主机上通过编译）
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]
// Allocator needed for VirtIO DMA pool
#[cfg(target_arch = "riscv64")]
extern crate alloc;

// 引入控制台输出宏（print! / println!），由 tg_console 库提供
#[macro_use]
extern crate tg_console;

#[cfg(target_arch = "riscv64")]
mod allocator;

#[cfg(target_arch = "riscv64")]
mod gpu;

// 本地模块：Console 和 SyscallContext 的实现
use impls::{Console, SyscallContext};
// riscv 库：访问 RISC-V 控制状态寄存器（CSR），如 scause
use riscv::register::*;
// 日志模块
use tg_console::log;
// 用户上下文：保存/恢复用户态寄存器，实现特权级切换
use tg_kernel_context::LocalContext;
// SBI 调用：关机等
use tg_sbi;
// 系统调用相关：调用者信息、系统调用 ID
use tg_syscall::{Caller, SyscallId};

#[cfg(target_arch = "riscv64")]
use virtio_drivers::{MmioTransport, VirtIOGpu};

/// Global GPU driver, initialized once in rust_main before the batch loop.
#[cfg(target_arch = "riscv64")]
static mut GPU: Option<VirtIOGpu<'static, allocator::HalImpl, MmioTransport>> = None;

/// Global framebuffer info, initialized once in rust_main.
#[cfg(target_arch = "riscv64")]
static mut FRAMEBUFFER: Option<gpu::Framebuffer> = None;

/// Custom syscall IDs for framebuffer operations.
/// These are handled directly in handle_syscall, bypassing tg_syscall::handle.
const SYSCALL_FB_INFO: usize = 2000;
const SYSCALL_FB_WRITE: usize = 2001;

// ========== 启动相关 ==========

// 将用户程序的二进制数据内联到内核镜像的 .data 段中
// APP_ASM 由 build.rs 在编译时生成，包含所有用户程序的二进制数据
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(include_str!(env!("APP_ASM")));

// 定义内核入口点：设置 16 页（64 KiB）的内核栈，然后跳转到 rust_main。
//
// 这里不再调用 tg_linker::boot0! 宏，避免外部已发布版本与 Rust 2024
// 在属性语义上的兼容差异影响本 crate 的发布校验。
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 16 * 4096; // 64 KiB — VirtIO-GPU init needs extra stack
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

// ========== 内核主函数 ==========

/// 内核主函数：初始化各子系统，然后以批处理方式依次运行所有用户程序。
extern "C" fn rust_main() -> ! {
    // 第一步：清零 BSS 段（未初始化的全局变量区域）
    unsafe { tg_linker::KernelLayout::locate().zero_bss() };

    // 第二步：初始化控制台输出（使 print!/println! 可用）
    tg_console::init_console(&Console);
    tg_console::set_log_level(option_env!("LOG"));
    tg_console::test_log();

    // 第三步：初始化系统调用处理（注册 IO 和 Process 的实现）
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);

    // Verify kernel image doesn't overlap the DMA pool at 0x8100_0000
    #[cfg(target_arch = "riscv64")]
    {
        unsafe extern "C" { static __end: u8; }
        let kernel_end = unsafe { &raw const __end } as usize;
        assert!(kernel_end < 0x8100_0000, "kernel image ({kernel_end:#x}) overlaps DMA pool");
    }

    // Initialize VirtIO-GPU and fill white background
    #[cfg(target_arch = "riscv64")]
    {
        let (gpu_driver, fb_info) = gpu::init();
        let buf = unsafe { core::slice::from_raw_parts_mut(fb_info.ptr, fb_info.len) };

        // Fill white background (BGRA: all 0xFF)
        for byte in buf.iter_mut() {
            *byte = 0xFF;
        }

        // Initial flush to show white screen
        let mut gpu_driver = gpu_driver;
        gpu_driver.flush().expect("GPU flush failed");

        // Store globally for syscall handlers
        unsafe {
            GPU = Some(gpu_driver);
            FRAMEBUFFER = Some(fb_info);
        }

        // Enable rdtime in U-mode for user-space spin-wait timing
        unsafe { core::arch::asm!("csrs scounteren, {}", in(reg) (1 << 1)) };
    }

    // 第四步：批处理——依次加载并运行每个用户程序
    for (i, app) in tg_linker::AppMeta::locate().iter().enumerate() {
        let app_base = app.as_ptr() as usize;
        log::info!("load app{i} to {app_base:#x}");

        // 创建用户态上下文，入口地址为 app_base
        // LocalContext::user() 会设置 sstatus.SPP = User，
        // 使得 sret 后 CPU 进入 U-mode
        let mut ctx = LocalContext::user(app_base);

        // 分配用户栈（4 KiB），使用 MaybeUninit 避免不必要的零初始化
        let mut user_stack: core::mem::MaybeUninit<[usize; 512]> =
            core::mem::MaybeUninit::uninit();
        let user_stack_ptr = user_stack.as_mut_ptr() as *mut usize;
        // 将用户栈顶地址写入上下文的 sp 寄存器
        *ctx.sp_mut() = unsafe { user_stack_ptr.add(512) } as usize;

        // 循环执行用户程序，直到退出或出错
        loop {
            // execute() 会：
            // 1. 将当前上下文的寄存器恢复到 CPU
            // 2. 执行 sret 切换到 U-mode 运行用户程序
            // 3. 用户程序触发 Trap 后回到这里
            unsafe { ctx.execute() };

            // 读取 scause 寄存器判断 Trap 原因
            use scause::{Exception, Trap};
            match scause::read().cause() {
                // 用户态系统调用（ecall from U-mode）
                Trap::Exception(Exception::UserEnvCall) => {
                    use SyscallResult::*;
                    match handle_syscall(&mut ctx) {
                        Done => continue,           // 系统调用处理完成，继续执行
                        Exit(code) => log::info!("app{i} exit with code {code}"),
                        Error(id) => {
                            log::error!("app{i} call an unsupported syscall {}", id.0)
                        }
                    }
                }
                // 其他异常（如非法指令、页错误等）：杀死应用
                trap => log::error!("app{i} was killed because of {trap:?}"),
            }
            // 清除指令缓存：因为下一个用户程序会被加载到相同的内存区域，
            // 需要确保 i-cache 中不会残留旧的指令
            unsafe { core::arch::asm!("fence.i") };
            break;
        }
        // 防止编译器优化掉 user_stack
        let _ = core::hint::black_box(&user_stack);
        println!();
    }

    // 所有用户程序执行完毕，关机
    tg_sbi::shutdown(false)
}

// ========== panic 处理 ==========

/// panic 处理函数：打印错误信息后以异常状态关机。
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("{info}");
    tg_sbi::shutdown(true)
}

// ========== 系统调用处理 ==========

/// 系统调用处理结果
enum SyscallResult {
    /// 系统调用完成，继续执行用户程序
    Done,
    /// 用户程序请求退出，附带退出码
    Exit(usize),
    /// 不支持的系统调用
    Error(SyscallId),
}

fn handle_syscall(ctx: &mut LocalContext) -> SyscallResult {
    use tg_syscall::{SyscallId as Id, SyscallResult as Ret};

    let id_raw: usize = ctx.a(7);
    let args = [ctx.a(0), ctx.a(1), ctx.a(2), ctx.a(3), ctx.a(4), ctx.a(5)];

    // Intercept custom framebuffer syscalls before standard dispatch
    #[cfg(target_arch = "riscv64")]
    match id_raw {
        SYSCALL_FB_INFO => {
            let ret = handle_fb_info();
            *ctx.a_mut(0) = ret as usize;
            ctx.move_next();
            return SyscallResult::Done;
        }
        SYSCALL_FB_WRITE => {
            let ret = handle_fb_write(args[0], args[1], args[2], args[3], args[4]);
            *ctx.a_mut(0) = ret as usize;
            ctx.move_next();
            return SyscallResult::Done;
        }
        _ => {}
    }

    let id: Id = id_raw.into();
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

/// FB_INFO syscall: returns (width << 32) | height packed in a0.
#[cfg(target_arch = "riscv64")]
fn handle_fb_info() -> usize {
    // SAFETY: single-threaded kernel; FRAMEBUFFER is set once in rust_main before apps run.
    let (width, height) = unsafe {
        let p = &raw const FRAMEBUFFER;
        let fb = (*p).as_ref().expect("GPU not initialized");
        (fb.width, fb.height)
    };
    ((width as usize) << 32) | (height as usize)
}

/// FB_WRITE syscall: copy user pixel data into framebuffer and flush.
/// Args: a0=x, a1=y, a2=w, a3=h, a4=data_ptr
///
/// NOTE: `data_ptr` is not validated beyond bounds-checking the rectangle.
/// In ch2 (no virtual memory, trusted batch programs), this is acceptable.
/// A kernel with untrusted user programs would need to verify the pointer
/// falls within the user address space.
#[cfg(target_arch = "riscv64")]
fn handle_fb_write(x: usize, y: usize, w: usize, h: usize, data_ptr: usize) -> usize {
    // SAFETY: single-threaded kernel; GPU/FRAMEBUFFER are set once before apps run.
    let (fb_ptr, fb_len, fb_w, fb_h) = unsafe {
        let p = &raw const FRAMEBUFFER;
        let fb = (*p).as_ref().expect("GPU not initialized");
        (fb.ptr, fb.len, fb.width as usize, fb.height as usize)
    };

    // Bounds check (also prevents w*h*4 overflow since w <= fb_w and h <= fb_h)
    if x + w > fb_w || y + h > fb_h || w == 0 || h == 0 {
        return usize::MAX; // -1 as usize
    }

    let fb_buf = unsafe { core::slice::from_raw_parts_mut(fb_ptr, fb_len) };
    let user_data = unsafe { core::slice::from_raw_parts(data_ptr as *const u8, w * h * 4) };

    // Copy row-by-row into framebuffer
    for row in 0..h {
        let fb_offset = ((y + row) * fb_w + x) * 4;
        let src_offset = row * w * 4;
        fb_buf[fb_offset..fb_offset + w * 4]
            .copy_from_slice(&user_data[src_offset..src_offset + w * 4]);
    }

    // Flush display
    let gpu = unsafe {
        let p = &raw mut GPU;
        (*p).as_mut().expect("GPU not initialized")
    };
    gpu.flush().expect("GPU flush failed");

    0
}

// ========== 接口实现 ==========

/// 各依赖库所需接口的具体实现
mod impls {
    use tg_syscall::{STDDEBUG, STDOUT};

    /// 控制台实现：通过 SBI 逐字符输出
    pub struct Console;

    impl tg_console::Console for Console {
        #[inline]
        fn put_char(&self, c: u8) {
            tg_sbi::console_putchar(c);
        }
    }

    /// 系统调用上下文实现：处理 IO 和 Process 相关的系统调用
    pub struct SyscallContext;

    /// IO 系统调用实现：处理 write 系统调用
    impl tg_syscall::IO for SyscallContext {
        fn write(
            &self,
            _caller: tg_syscall::Caller,
            fd: usize,
            buf: usize,
            count: usize,
        ) -> isize {
            match fd {
                // 标准输出和调试输出：将缓冲区内容打印到控制台
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

    /// Process 系统调用实现：处理 exit 系统调用
    impl tg_syscall::Process for SyscallContext {
        #[inline]
        fn exit(&self, _caller: tg_syscall::Caller, _status: usize) -> isize {
            0
        }
    }
}

/// 非 RISC-V64 架构的占位模块。
///
/// 提供编译所需的符号，使得 `cargo publish --dry-run` 在主机平台上能通过编译。
#[cfg(not(target_arch = "riscv64"))]
mod stub {
    /// 主机平台占位入口
    #[unsafe(no_mangle)]
    pub extern "C" fn main() -> i32 {
        0
    }

    /// C 运行时占位
    #[unsafe(no_mangle)]
    pub extern "C" fn __libc_start_main() -> i32 {
        0
    }

    /// Rust 异常处理人格占位
    #[unsafe(no_mangle)]
    pub extern "C" fn rust_eh_personality() {}
}

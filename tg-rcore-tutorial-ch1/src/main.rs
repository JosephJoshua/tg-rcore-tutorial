//! # 第一章：应用程序与基本执行环境
//!
//! 本章实现了一个最简单的 RISC-V S 态裸机程序，展示操作系统的最小执行环境。
//!
//! ## 关键概念
//!
//! - `#![no_std]`：不使用 Rust 标准库，改用不依赖操作系统的核心库 `core`
//! - `#![no_main]`：不使用标准的 `main` 入口，自定义裸函数 `_start` 作为入口
//! - 裸函数（naked function）：不生成函数序言/尾声，可在无栈环境下执行
//! - SBI（Supervisor Binary Interface）：S 态软件向 M 态固件请求服务的标准接口
//!
//! 教程阅读建议：
//!
//! - 先看 `_start`：理解无运行时情况下的最小启动流程；
//! - 再看 `rust_main`：理解最小 I/O 路径（SBI 输出 + 关机）；
//! - 最后看 `panic_handler`：理解 no_std 程序的异常收口方式。

// 不使用标准库，因为裸机环境没有操作系统提供系统调用支持
#![no_std]
// 不使用标准入口，因为裸机环境没有 C runtime 进行初始化
#![no_main]
// RISC-V64 架构下启用严格警告和文档检查
#![cfg_attr(target_arch = "riscv64", deny(warnings, missing_docs))]
// 非 RISC-V64 架构允许死代码（用于 cargo publish --dry-run 在主机上通过编译）
#![cfg_attr(not(target_arch = "riscv64"), allow(dead_code))]

#[cfg(target_arch = "riscv64")]
extern crate alloc;

// 引入 SBI 调用库，提供 console_putchar（输出字符）和 shutdown（关机）功能
// 启用 nobios 特性后，tg_sbi 内建了 M-mode 启动代码，无需外部 SBI 固件
use tg_sbi::{console_putchar, shutdown};

#[cfg(target_arch = "riscv64")]
mod allocator;

#[cfg(target_arch = "riscv64")]
mod gpu;

#[cfg(target_arch = "riscv64")]
mod tangram;

/// S 态程序入口点。
///
/// 这是一个裸函数（naked function），放置在 `.text.entry` 段，
/// 链接脚本将其安排在地址 `0x80200000`。
///
/// 裸函数不生成函数序言和尾声，因此可以在没有栈的情况下执行。
/// 它完成两件事：
/// 1. 设置栈指针 `sp`，指向栈顶（栈从高地址向低地址增长）
/// 2. 跳转到 Rust 主函数 `rust_main`
#[cfg(target_arch = "riscv64")]
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    // 栈大小：64 KiB（VirtIO-GPU 初始化需要较大栈空间）
    const STACK_SIZE: usize = 64 * 1024;

    // 在 .bss.uninit 段中分配栈空间
    #[unsafe(link_section = ".bss.uninit")]
    static mut STACK: [u8; STACK_SIZE] = [0u8; STACK_SIZE];

    core::arch::naked_asm!(
        "la sp, {stack} + {stack_size}", // 将 sp 设置为栈顶地址
        "j  {main}",                      // 跳转到 rust_main
        stack_size = const STACK_SIZE,
        stack      =   sym STACK,
        main       =   sym rust_main,
    )
}

/// S 态主函数：打印 "Hello, world!"，显示 tangram "OS" 图案，然后关机。
extern "C" fn rust_main() -> ! {
    for c in b"Hello, world!\n" {
        console_putchar(*c);
    }

    #[cfg(target_arch = "riscv64")]
    {
        let (mut gpu, fb_info) = gpu::init();
        let buf = unsafe { core::slice::from_raw_parts_mut(fb_info.ptr, fb_info.len) };

        // Fill white background.
        for pixel in buf.chunks_exact_mut(4) {
            pixel[0] = tangram::WHITE.b;
            pixel[1] = tangram::WHITE.g;
            pixel[2] = tangram::WHITE.r;
            pixel[3] = tangram::WHITE.a;
        }

        // Draw all tangram pieces.
        for piece in &tangram::PIECES {
            let color = tangram::COLORS[piece.color_idx];
            tangram::fill_polygon(buf, fb_info.width, fb_info.height, piece.vertices, color);
        }

        gpu.flush().expect("flush failed");

        // Wait for keypress or ~10 second timeout, whichever comes first.
        // QEMU virt timer runs at 10 MHz.
        let timeout = 10 * 10_000_000;
        let start: usize;
        unsafe { core::arch::asm!("rdtime {}", out(reg) start) };
        loop {
            if tg_sbi::console_getchar() != usize::MAX {
                break;
            }
            let now: usize;
            unsafe { core::arch::asm!("rdtime {}", out(reg) now) };
            if now - start >= timeout {
                break;
            }
        }
    }

    shutdown(false)
}

/// panic 处理函数。
///
/// `#![no_std]` 环境下必须自行实现。发生 panic 时以异常状态关机。
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    shutdown(true) // true 表示异常关机
}

/// 非 RISC-V64 架构的占位模块。
///
/// 提供 `main` 等符号，使得在主机平台（如 x86_64）上也能通过编译，
/// 满足 `cargo publish --dry-run` 和 `cargo test` 的需求。
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

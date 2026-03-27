# 第二章扩展报告：VirtIO-GPU Tangram "OS" 逐帧动画

## 实现内容

### 目标
扩展 ch2 批处理系统，通过 VirtIO-GPU 帧缓冲区在 QEMU 图形窗口中逐帧显示 tangram（七巧板）"OS" 图案。14 个用户程序各自渲染一块七巧板，通过批处理系统依次执行，形成逐帧出现的动画效果。

### 架构设计

ch2 是批处理 OS：内核顺序加载嵌入的用户程序，逐个执行到退出。用户程序运行在 U-mode，通过 ecall 触发系统调用。核心挑战在于 VirtIO-GPU 驱动在内核（S-mode）中，而渲染请求来自用户程序（U-mode），需要通过系统调用桥接。

**方案：低层帧缓冲区系统调用**
- 内核初始化 GPU 并暴露两个自定义系统调用（ID 2000/2001）
- 用户程序在用户空间完成多边形光栅化，通过系统调用将像素数据推送到内核帧缓冲区
- 内核仅负责帧缓冲区的矩形写入和 GPU 刷新，不了解七巧板具体形状

### 内核侧新增模块（jsph-tg-rcore-tutorial-ch2-moving-tangram/）

**allocator.rs — 固定地址 bump 分配器**
- 使用 RAM 固定地址 0x8100_0000 处的 5 MiB 区域，避免与用户程序区域（0x8040_0000）重叠
- 实现 `GlobalAlloc`（满足 `extern crate alloc`）和 `Hal` trait（VirtIO DMA 分配）
- 恒等映射（ch2 无页表），仅分配不释放

**gpu.rs — VirtIO-GPU 初始化**
- 探测 QEMU virt 平台 8 个 MMIO 插槽（0x10001000–0x10008000）
- 返回 GPU 驱动和帧缓冲区描述符（指针、长度、宽高）

**main.rs 修改**
- `rust_main` 中批处理循环前初始化 GPU，填充白色背景
- GPU 驱动和帧缓冲区存储为 `static mut` 全局变量
- `handle_syscall` 在标准分发前拦截自定义系统调用 ID
- 设置 `scounteren.TM` 位，允许用户态访问 rdtime（用于动画计时）
- 内核栈从 32 KiB 增大到 64 KiB（VirtIO-GPU 初始化需要）

### 系统调用接口

| 系统调用 | ID | 参数 | 返回值 |
|---------|-----|------|--------|
| FB_INFO | 2000 | 无 | `(width << 32) \| height`（RV64 打包） |
| FB_WRITE | 2001 | a0=x, a1=y, a2=w, a3=h, a4=data_ptr | 0 成功，usize::MAX 失败 |

### 用户侧修改（tg-rcore-tutorial-user/）

**tangram.rs — 七巧板渲染模块（feature-gated）**
- 7 种颜色定义（BGRA 格式）
- 14 块七巧板几何定义（与 ch1-tangram 一致）
- 整数扫描线多边形填充算法（i64 中间值防溢出）
- `render_piece(idx)`: 计算边界框、堆分配像素缓冲区、光栅化、通过 FB_WRITE 推送
- `spin_wait_ms(ms)`: rdtime 自旋等待（QEMU virt 10 MHz 时钟）

**lib.rs — 系统调用包装**
- `fb_info() -> (u32, u32)`: 内联汇编 ecall，解包宽高
- `fb_write(x, y, w, h, data) -> isize`: 内联汇编 ecall，5 参数

**14 个用户程序（tangram_00.rs–tangram_13.rs）**
- 每个程序渲染一块七巧板，300ms 延迟后退出
- 通过 `#[cfg(feature = "tangram")]` 门控，无特性时编译为空操作

**Cargo.toml — tangram 特性**
- `[features] tangram = []`
- `cargo publish --dry-run` 不启用特性时通过编译

**heap.rs — 堆大小调整**
- 16 KiB → 512 KiB（七巧板像素缓冲区最大 ~210 KiB）

### 配置变更
- `.cargo/config.toml`：`-nographic` → `-serial stdio` + `-device virtio-gpu-device`；`TG_USER_LOCAL_DIR` 指向 `../tg-rcore-tutorial-user`
- `Cargo.toml`：所有依赖改为仅版本号（无 path），添加 `virtio-drivers = "0.1.0"`
- `build.rs`：用户程序构建添加 `--features tangram`；RUSTFLAGS 绕过本地 syscall crate 编译问题
- `test.sh`：60 秒超时 + grep 检查所有 14 块是否渲染
- `cases.toml`：ch2 节追加 tangram_00–tangram_13

## 遇到的问题

### 关键 bug：DMA 池与用户程序内存重叠

**症状**：第一个七巧板程序在 `render_piece()` 入口处挂起，无错误输出。

**根因**：内核的 5 MiB 静态 bump 分配器池位于 BSS 段（0x80311000–0x80811000），与用户程序加载地址（0x80400000）重叠。内核将用户二进制复制到 0x80400000 时覆盖了 VirtIO DMA 缓冲区，导致后续 GPU flush 挂起。

**排查过程**：
1. 增大用户堆（16 KiB → 512 KiB）排除内存不足
2. 添加调试打印定位挂起点
3. 发现 cargo 增量编译缓存导致修改不生效（`.incbin` 文件变更未触发重新链接）
4. 通过 `cargo clean` 强制完整重建
5. `riscv64-linux-gnu-objdump -t` 查看符号表确认 POOL 地址在用户区域内

**修复**：将分配器池从 BSS 静态数组改为使用 RAM 固定地址 0x8100_0000（16 MiB 偏移），完全避开用户程序区域。

### 其他问题

1. **Rust 2024 `unsafe_op_in_unsafe_fn` 编译错误**：本地 `tg-rcore-tutorial-syscall` crate（edition 2024）的 `asm!` 在 `unsafe fn` 中缺少 `unsafe {}` 块。crates.io 发布版本使用旧 edition 不受影响。通过 build.rs 中 `env_remove("CARGO_ENCODED_RUSTFLAGS")` + `env("RUSTFLAGS", "-A unsafe_op_in_unsafe_fn")` 绕过。

2. **Rust 2024 `static_mut_refs` lint**：内核的 `static mut GPU/FRAMEBUFFER` 不能直接用 `.as_ref()` 创建引用，需要通过 `&raw const`/`&raw mut` 裸指针间接访问。子代理在实现时自动处理。

3. **Cargo 增量编译缓存**：修改用户程序源码后，内核二进制未更新。原因是 `global_asm!(include_str!(env!("APP_ASM")))` 引用的 `.incbin` 文件变更不会触发 Cargo 重新编译。需要 `cargo clean` 强制完整重建。

4. **用户堆不足**：默认 16 KiB 堆无法分配七巧板像素缓冲区（最大 ~210 KiB），`vec!` 分配失败导致 panic 后 exit。增大到 512 KiB 解决。

5. **tangram 二进制 publish 兼容**：`cargo publish --dry-run` 编译所有 bin 目标，但 tangram 二进制依赖 `tangram` 特性下的模块。通过 `#[cfg(feature = "tangram")]` 门控 main 函数体解决。

## 文件结构

```
jsph-tg-rcore-tutorial-ch2-moving-tangram/
├── Cargo.toml          # 独立发布配置，版本号依赖
├── .cargo/config.toml  # QEMU GPU 启动参数
├── build.rs            # --features tangram + RUSTFLAGS 绕过
├── test.sh             # 60 秒超时 + grep 验证
└── src/
    ├── main.rs         # GPU 初始化 + FB_INFO/FB_WRITE 系统调用
    ├── allocator.rs    # 固定地址 bump 分配器 + VirtIO Hal
    └── gpu.rs          # VirtIO-GPU MMIO 探测和帧缓冲区

tg-rcore-tutorial-user/  (修改)
├── Cargo.toml          # 新增 tangram feature
├── cases.toml          # ch2 节追加 14 个 tangram 程序
└── src/
    ├── lib.rs          # fb_info/fb_write 系统调用包装
    ├── heap.rs         # 堆 16 KiB → 512 KiB
    ├── tangram.rs      # 颜色/几何/光栅化/渲染（feature-gated）
    └── bin/
        ├── tangram_00.rs ... tangram_13.rs  # 14 个用户程序
        └── (原有 ch2 程序不变)
```

## 工作流程

使用 superpowers 技能链：
1. **brainstorming** → 设计规格文档（`docs/superpowers/specs/2026-03-27-ch2-moving-tangram-design.md`）
2. **writing-plans** → 实现计划（`docs/superpowers/plans/2026-03-27-ch2-moving-tangram.md`），8 个任务
3. **subagent-driven-development** → 每个任务分派独立子代理实现
4. 验证阶段发现并修复 DMA 池重叠 bug

共产生 12 个提交。

## 测试结果

- `cargo build`：通过
- `cargo check`（内核 crate）：通过
- `cargo publish --dry-run`（内核 crate）：通过
- `cargo publish --dry-run`（用户 crate）：通过
- `cargo run`：8 个原有 ch2 测试正常 + 14 块七巧板逐帧渲染 + 正常关机
- 所有 14 块七巧板均成功渲染并 exit code 0

## 设计决策说明

1. **自定义系统调用 vs 修改 tg-rcore-tutorial-syscall**：选择在内核 handle_syscall 中直接拦截自定义 ID（2000/2001），避免修改共享 crate。用户侧通过内联汇编 ecall 绕过 tg_syscall 的 SyscallId 枚举。

2. **用户空间光栅化 vs 内核光栅化**：选择在用户空间完成多边形光栅化，内核仅负责矩形像素块写入。这使内核不了解七巧板具体形状，更通用。

3. **固定地址分配器 vs BSS 静态数组**：从 BSS 静态数组改为 RAM 固定地址 0x81000000，消除与用户程序的地址空间冲突。QEMU virt 128 MiB RAM 保证此地址可用。

4. **rdtime 自旋等待 vs sleep 系统调用**：ch2 内核不实现 clock_gettime/sched_yield 系统调用，因此不能使用用户库的 `sleep()` 函数。改为直接在用户空间使用 rdtime 自旋等待，内核通过设置 `scounteren.TM` 位允许 U-mode 访问。

5. **堆大小 512 KiB**：最大七巧板（S1）像素缓冲区约 210 KiB，加上 buddy allocator 开销和对齐，512 KiB 提供充足余量。对 ch3+（step ≥ 2 MiB）和 ch4+（虚拟内存）无影响。

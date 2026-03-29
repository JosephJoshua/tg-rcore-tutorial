# 第三章扩展报告：VirtIO-GPU 贪吃蛇游戏

## 实现内容

### 目标
扩展 ch3 多道程序系统，通过 VirtIO-GPU 帧缓冲区和 VirtIO 键盘设备实现一个完整的贪吃蛇游戏。游戏与 12 个标准 ch3 测试程序在轮转调度下并发运行，演示两种输入模式：**轮询式输入**（polling）和**中断缓冲式输入**（interrupt-buffered）。

### 架构设计

ch3 是多道程序 OS：多个用户程序同时驻留在内存中，内核通过时间片轮转在它们之间切换。本扩展在 ch3 基础上增加 VirtIO-GPU（帧缓冲区渲染）和 VirtIO 键盘（VNC 输入），实现交互式图形游戏。

**整体方案**
- 新建 `tg-rcore-tutorial-ch3-snake` 内核 crate，融合 ch3 调度器与 ch2-moving-tangram 的 GPU 驱动
- 新增 VirtIO 键盘驱动，将 VNC 键盘事件转换为游戏按键
- 游戏逻辑和像素渲染全部在用户空间完成（`snake.rs` 模块）
- 两个二进制变体（`snake_poll` / `snake_interrupt`）通过文件描述符选择输入模式
- 13 个用户程序（12 标准测试 + 1 贪吃蛇）在时间片轮转下并发执行

**两种输入模式**
- **轮询式**（`snake_poll`，fd=0）：每个游戏 tick 主动调用 `read(STDIN)` → 内核直接调用 VirtIO 键盘的 `pop_pending_event()`，返回 0（无数据）或 1（一字节按键）
- **中断缓冲式**（`snake_interrupt`，fd=3，`--features interrupt`）：内核定时器中断处理中（每 1ms）轮询 VirtIO 键盘，将按键事件推入内核环形缓冲区（64 字节）；用户通过 `read(STDIN_BUFFERED)` 从缓冲区读取

### 内核侧模块（tg-rcore-tutorial-ch3-snake/）

**allocator.rs — 固定地址 bump 分配器**
- DMA 池位于 0x8200_0000（5 MiB），避开 13 个用户程序（最后一个在 0x81C0_0000 结束于 0x81E0_0000）
- 实现 `GlobalAlloc` 和 `Hal` trait（VirtIO DMA），恒等映射
- 启动断言 `__end < 0x8200_0000` 防止内核镜像与 DMA 池重叠

**virtio.rs — VirtIO 设备初始化（GPU + 键盘）**
- 扫描所有 8 个 MMIO 插槽（0x10001000–0x10008000）
- 通过 `transport.device_type()` 区分 GPU（DeviceType::GPU）和键盘（DeviceType::Input）
- 返回 `VirtIODevices` 结构：GPU 驱动 + 帧缓冲区 + 可选键盘驱动
- 键盘设备不存在时警告但不 panic（优雅降级）

**main.rs 核心变更**
- 内核栈 `(APP_CAPACITY + 10) * 8192 = 336 KiB`（TCB 数组约 272 KiB + GPU/VirtIO 初始化开销）
- GPU 初始化后填充游戏背景色（#0D0D1A），用户侧无需昂贵的全屏 fb_write
- VirtIO 键盘驱动存储为 `static mut KEYBOARD`
- `keyboard_trygetchar()`：非阻塞轮询 VirtIO 键盘，过滤 EV_KEY（event_type=1）且 value=1（按下），通过 `translate_keycode()` 转换为 ASCII 字节
- `translate_keycode()`：W/Up→w, A/Left→a, S/Down→s, D/Right→d, Enter→'\r', Space→' ', 其他→'\x01'
- `poll_keyboard()`（cfg interrupt）：定时器中断中轮询键盘推入环形缓冲区
- `IO::read` 实现：fd=0 直接轮询 VirtIO 键盘，fd=3 读取内核环形缓冲区

**task.rs — 系统调用拦截**
- `handle_syscall` 在标准分发前拦截 FB_INFO（2000）和 FB_WRITE（2001）
- FB 系统调用计数不计入标准系统调用统计

### 系统调用接口

| 系统调用 | ID | 参数 | 返回值 |
|---------|-----|------|--------|
| FB_INFO | 2000 | 无 | `(width << 32) \| height` |
| FB_WRITE | 2001 | a0=x, a1=y, a2=w, a3=h, a4=data_ptr | 0 成功，usize::MAX 失败 |
| read | 63 | a0=fd(0或3), a1=buf, a2=count | 0（无数据）或 1（已读取）|

### 用户侧模块（tg-rcore-tutorial-user/）

**snake.rs — 游戏模块（feature-gated: `snake`）**

*数据结构*
- `Snake`：环形缓冲区存储蛇身坐标（最大 400 格），head/len 索引，方向和缓冲方向（防 180° 转向）
- `Game`：蛇、食物坐标、分数、状态（Playing/GameOver）、xorshift64 PRNG（rdtime 种子）
- `InputParser`：三态有限自动机解析 WASD 和方向键 ESC 序列

*渲染 — "Neon Arcade" 风格*
- 深空黑背景（#0D0D1A），微妙棋盘格棋盘（两种深靛蓝色交替）
- 蛇头：霓虹绿（#39FF14）+ 3D 斜面效果（边缘暗、中心亮、高光点）+ 方向感知眼睛（3x3 暗色 + 1x1 白色瞳孔）
- 蛇身：深绿（#00CC33）+ 同样的斜面深度效果
- 食物：圆形发光球体（热粉 #FF2255），利用整数距离平方实现同心环（外圈→主体→高光中心）
- 边框：三层发光效果（外辉光 → 间隙 → 亮内线）
- 分数面板：电蓝色标题、装饰线、暗灰标签、白色数字
- Game Over：440x240 居中覆盖层，电蓝边框，居中提示文字
- 所有渲染使用 32×32 静态 `RENDER_BUF`（避免热路径堆分配），大矩形使用 `vec!` 堆分配
- 5x7 位图字体：25 个字符（0-9, A-V, W, 空格, 冒号）+ 小写字母扩展
- 整数色彩插值（`lerp_color`），无浮点运算

*游戏循环*
- 每 tick（150ms）：drain_input → update → render_tick → sleep
- `drain_input`：非阻塞循环读取所有可用按键，保留最后有效方向
- `render_tick`：仅重绘变化的格子（新头、旧头变身体、旧尾清除、食物刷新）
- Game Over 后等待任意键重新开始

**两个二进制（snake_poll.rs / snake_interrupt.rs）**
- 薄包装：`run_game(STDIN)` 或 `run_game(STDIN_BUFFERED)`

### 配置变更
- `.cargo/config.toml`：添加 `-device virtio-gpu-device` 和 `-device virtio-keyboard-device`
- `Cargo.toml`：`virtio-drivers = "0.1.0"`（cfg riscv64），features: `interrupt`, `coop`
- `build.rs`：case_key 选择 `ch3_snake_poll`（默认）或 `ch3_snake_interrupt`（`--features interrupt`），用户程序构建添加 `--features snake`，`TG_SKIP_USER_APPS` 加入 rerun-if-env-changed
- `cases.toml`：新增 `ch3_snake_poll` 和 `ch3_snake_interrupt` 两个配置节（各含 12 标准程序 + 1 贪吃蛇）
- `test.sh`：30 秒超时，`-display none` 无头运行，输出通过 `tg-rcore-tutorial-checker --ch 3` 校验

## 遇到的问题

### 关键 bug 1：SBI console_getchar 阻塞导致游戏冻结

**症状**：游戏板正常渲染，但蛇不移动，按键无响应。

**根因**：`tg_sbi::console_getchar()` 的 SBI 实现（`handle_console_getchar`）在 M-mode 中忙等循环直到收到字符，永远不会返回"无数据"。当 `drain_input` 调用 `read(STDIN)` → 内核调用 `console_getchar()` 时，整个内核挂起等待 UART 输入。

**排查过程**：
1. 添加逐步调试打印定位挂起点
2. 确认 `drain_input` 是挂起位置
3. 阅读 tg-sbi 源码 `msbi.rs:126-136` 发现 `loop { if let Some(c) = uart::getchar() { return } else { continue } }`

**最初修复**：绕过 SBI，直接读取 16550 UART 的 LSR（0x10000005）和 RBR（0x10000000）寄存器实现非阻塞读取。

**最终修复**：改用 VirtIO 键盘设备（`pop_pending_event()` 天然非阻塞），完全移除 UART 输入依赖。

### 关键 bug 2：build.rs 缓存导致空应用程序

**症状**：内核启动正常但 `AppMeta` 显示 base=0, step=0, count=0。所有应用程序"丢失"。

**根因**：Task 4 验证编译时使用 `TG_SKIP_USER_APPS=1 cargo check`，生成了 dummy `app.asm`。后续 `cargo build`（无 TG_SKIP_USER_APPS）未重新运行 build.rs，因为 `TG_SKIP_USER_APPS` 不在 `cargo:rerun-if-env-changed` 列表中。

**修复**：在 build.rs 中添加 `println!("cargo:rerun-if-env-changed=TG_SKIP_USER_APPS")`。

### 关键 bug 3：内核栈溢出

**症状**：内核在 GPU 初始化后挂起（"loading apps..." 前无输出）。

**根因**：TCB 数组 `[TaskControlBlock::ZERO; 32]` 在内核栈上分配，每个 TCB 约 8.5 KiB（LocalContext + 8 KiB 用户栈），共约 272 KiB。最初的 128 KiB 内核栈不够。

**修复**：增大到 `(APP_CAPACITY + 10) * 8192 = 336 KiB`。

### 其他问题

1. **Rust 2024 `static mut` 访问**：`unsafe { &mut RENDER_BUF }` 在 edition 2024 中被拒绝，需使用 `unsafe { &mut *(&raw mut RENDER_BUF) }` 裸指针解引用模式。

2. **DMA 池地址**：ch2 使用 0x8100_0000，但 ch3 有 13 个程序（step=0x200000），程序 6 加载到 0x8100_0000 会覆盖 DMA 池。移至 0x8200_0000。

3. **VNC 键盘路由**：VNC 键盘输入不进入 UART，需要 `-device virtio-keyboard-device`。virtio-drivers 0.1.0 已有 `VirtIOInput` 驱动，`pop_pending_event()` 非阻塞返回 `InputEvent`（event_type, code, value）。

4. **初始渲染性能**：400 个格子的 fb_write 各触发一次 GPU flush，在 debug 模式下较慢。内核在初始化时预填充背景色，用户侧省去 40 次全屏条带 fb_write。

5. **Event::None 导致定时器重置**：fb_write 等返回 `Event::None` 的系统调用会 `continue` 内层循环（重置定时器），使当前任务独占 CPU。初始渲染期间蛇游戏执行 400+ fb_write 而不让出。其他程序在此期间不被调度。睡眠时 `sched_yield` 正常让出。

## 文件结构

```
tg-rcore-tutorial-ch3-snake/
├── Cargo.toml          # features: interrupt, coop; virtio-drivers dep
├── .cargo/config.toml  # QEMU: GPU + keyboard devices
├── build.rs            # ch3_snake_poll/interrupt case_key, --features snake
├── rust-toolchain.toml
├── test.sh             # 30s 超时无头 CI
└── src/
    ├── main.rs         # 内核主函数：VirtIO 初始化、FB/read 系统调用、键盘驱动、环形缓冲区
    ├── task.rs         # TCB + FB 系统调用拦截
    ├── allocator.rs    # DMA bump 分配器 (0x8200_0000)
    └── virtio.rs       # MMIO 扫描：GPU + 键盘设备发现

tg-rcore-tutorial-user/  (修改)
├── Cargo.toml          # 新增 snake feature
├── cases.toml          # 新增 ch3_snake_poll, ch3_snake_interrupt 配置节
└── src/
    ├── lib.rs          # 新增 snake 模块 (feature-gated) + STDIN_BUFFERED 常量
    ├── snake.rs        # 游戏逻辑、像素渲染、输入解析、位图字体 (~700 行)
    └── bin/
        ├── snake_poll.rs       # 轮询输入变体
        └── snake_interrupt.rs  # 中断缓冲输入变体
```

## 内存布局

```
0x80000000  内核 .text/.data/.bss + 336 KiB 栈
0x80400000  程序 0  (00hello_world)
0x80600000  程序 1  (01store_fault)
...
0x81A00000  程序 11 (11sleep)
0x81C00000  程序 12 (snake_poll 或 snake_interrupt)
0x81E00000  程序 12 结束
0x82000000  DMA 池 (5 MiB) — VirtIO GPU + 键盘
0x82500000  DMA 池结束
0x88000000  QEMU RAM 结束 (128 MiB)
```

## 测试

- `cargo build`：通过（内核 + 13 用户程序）
- `TG_SKIP_USER_APPS=1 cargo check`：通过
- `cargo run`：VNC 显示游戏画面，WASD/方向键控制蛇移动，吃食物得分，撞墙/自身 Game Over
- `cargo run --features interrupt`：中断缓冲输入模式正常
- 12 个标准 ch3 测试程序串口输出正确
- `bash test.sh`：ch3 checker 校验通过

## 设计决策

1. **VirtIO 键盘 vs UART 输入**：最终使用 VirtIO 键盘替代 UART。SBI console_getchar 阻塞使 UART 非阻塞读取需直接操作寄存器（不优雅），且 VNC 键盘输入不经过 UART。VirtIO 键盘的 `pop_pending_event()` 天然非阻塞，VNC 键盘事件自动路由到 virtio-keyboard-device。

2. **fd=0/fd=3 区分两种输入模式**：轮询模式使用 STDIN（fd=0）直接调用 VirtIO 键盘驱动；中断缓冲模式使用自定义 fd=3 读取内核环形缓冲区。用户侧代码完全相同，仅传入不同 fd。

3. **像素渲染全在用户空间**：内核仅提供 fb_write 矩形写入接口。蛇的 3D 斜面效果、食物圆形、位图字体等全部在用户空间逐像素计算。内核不了解游戏具体形状。

4. **增量渲染**：每 tick 仅重绘 3-4 个变化的格子（新头、旧头变身体、旧尾清除、食物刷新），避免全屏重绘。初始渲染需画 400+ 格子，后续每 tick 仅 3-4 次 fb_write。

5. **内核填充背景色**：初始化 GPU 后在内核中直接写入帧缓冲区填充游戏背景色（#0D0D1A），避免用户侧 40 次条带 fb_write（约 4 MB 数据通过系统调用传递）。

6. **整数渲染**：所有像素计算使用整数算术（距离平方代替 sqrt，256 级整数色彩插值），无浮点运算。适合裸机环境。

7. **DMA 池地址 0x82000000**：13 个用户程序（step=0x200000）最后一个结束于 0x81E00000，留 2 MiB 间隙后放置 DMA 池，避免 AppMeta 加载程序时覆盖。

## 工作流程

1. **brainstorming** → 设计规格（`docs/superpowers/specs/2026-03-28-ch3-snake-design.md`）
2. **writing-plans** → 实现计划（`docs/superpowers/plans/2026-03-28-ch3-snake.md`），8 个任务
3. **subagent-driven-development** → 子代理实现 + spec/质量审查
4. 调试阶段发现并修复 SBI 阻塞、build.rs 缓存、栈溢出等问题
5. 迭代增加 VirtIO 键盘支持替代 UART 输入
6. "Neon Arcade" 视觉重设计：3D 斜面蛇段、发光圆形食物、方向感知眼睛

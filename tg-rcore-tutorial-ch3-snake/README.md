# 第三章扩展：VirtIO-GPU 贪吃蛇游戏

基于 ch3 多道程序系统，通过 VirtIO-GPU 帧缓冲区和 VirtIO 键盘实现交互式贪吃蛇游戏。游戏与 12 个标准 ch3 测试程序在时间片轮转调度下并发运行，演示**轮询式输入**和**中断缓冲式输入**两种控制方式。

## 运行

```bash
# 轮询式输入（默认）
cargo run

# 中断缓冲式输入
cargo run --features interrupt
```

通过 VNC 客户端连接 QEMU 显示窗口（默认端口 5900），使用 **WASD** 或**方向键**控制蛇的移动。

## 两种输入模式

| 模式 | 特性标志 | 文件描述符 | 原理 |
|------|---------|-----------|------|
| 轮询式 | （默认） | fd=0 (STDIN) | 每个游戏 tick 主动调用 `read()` → 内核直接调用 VirtIO 键盘 `pop_pending_event()` |
| 中断缓冲式 | `--features interrupt` | fd=3 | 内核定时器中断（每 1ms）轮询 VirtIO 键盘 → 推入内核环形缓冲区 → 用户通过 `read()` 读取 |

**轮询式**：应用主动检查输入（"我准备好了才检查"）。
**中断缓冲式**：内核持续捕获输入（"内核帮我一直收集，我随时读取"）。

## 项目结构

```
ch3-snake/
├── .cargo/
│   └── config.toml     # QEMU runner：VirtIO-GPU + VirtIO 键盘设备
├── build.rs            # 构建脚本：按输入模式选择 cases.toml 配置节
├── Cargo.toml          # features: interrupt, coop
├── rust-toolchain.toml
├── test.sh             # 无头 CI 测试（30s 超时）
└── src/
    ├── main.rs         # 内核：VirtIO 初始化、FB/read 系统调用、键盘驱动、环形缓冲区
    ├── task.rs         # 任务控制块 + FB 系统调用拦截
    ├── allocator.rs    # DMA bump 分配器（0x8200_0000）
    └── virtio.rs       # MMIO 设备扫描：GPU + 键盘
```

游戏逻辑和渲染代码位于用户 crate：`tg-rcore-tutorial-user/src/snake.rs`。

## 架构

```
┌─────────────────────────────────────────────────┐
│  用户空间 (U-mode)                                │
│                                                   │
│  snake_poll / snake_interrupt                     │
│    └── snake.rs: 游戏逻辑、像素渲染、输入解析       │
│         ├── fb_write(2001): 写入 BGRA 像素块       │
│         ├── fb_info(2000): 查询帧缓冲区尺寸        │
│         └── read(fd): 非阻塞键盘输入              │
├─────────────────────────────────────────────────┤
│  内核空间 (S-mode)                                │
│                                                   │
│  main.rs: 时间片轮转调度器                         │
│    ├── virtio.rs: VirtIO-GPU + VirtIO 键盘初始化   │
│    ├── handle_fb_write: 像素复制 + GPU flush       │
│    ├── keyboard_trygetchar: 非阻塞键盘事件轮询     │
│    └── poll_keyboard (interrupt): 定时器驱动缓冲   │
└─────────────────────────────────────────────────┘
```

## 视觉设计

"Neon Arcade" 风格：深空黑背景、霓虹绿 3D 斜面蛇段（带方向感知眼睛）、热粉圆形发光食物、电蓝色 UI 装饰。所有渲染使用整数算术逐像素计算，无浮点运算。

## 自定义系统调用

| 系统调用 | ID | 参数 | 返回值 |
|---------|-----|------|--------|
| FB_INFO | 2000 | 无 | `(width << 32) \| height` |
| FB_WRITE | 2001 | a0=x, a1=y, a2=w, a3=h, a4=data_ptr | 0 成功，usize::MAX 失败 |

## 内存布局

```
0x80000000  内核镜像 + 336 KiB 栈
0x80400000  程序 0–12（12 标准测试 + 1 贪吃蛇），step=0x200000
0x82000000  DMA 池（5 MiB）— VirtIO GPU + 键盘
0x88000000  QEMU RAM 结束（128 MiB）
```

## 测试

```bash
bash test.sh          # 无头 CI：30s 超时，checker 校验 ch3 标准输出
```

详细实现报告见 `docs/reports/ch3-snake.md`。

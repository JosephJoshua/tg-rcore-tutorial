# 第七章扩展报告：VirtIO-GPU 吃豆人游戏（管道 IPC + 信号）

## 实现内容

### 目标
扩展 ch7 进程间通信内核，通过 VirtIO-GPU 帧缓冲区和 VirtIO 键盘实现一个简化版吃豆人（Pac-Man）游戏。游戏使用 ch7 的**管道（pipe）**实现父子进程间的锁步通信，**信号（signal）**实现异步事件通知（能量豆广播、幽灵击杀），**文件系统**实现最高分持久化，全面演示 ch7 的 IPC 和信号处理能力。

### 架构设计

ch7 在 ch6 文件系统基础上引入了两大 IPC 原语：**管道**（单向字节流）和**信号**（异步事件通知）。本扩展的核心亮点是利用这两种机制构建一个**多进程游戏架构**——4 个幽灵 AI 各运行在独立子进程中，通过管道与父进程交换游戏状态，通过信号接收异步事件。

**整体方案**
- 新建 `tg-rcore-tutorial-ch7-pacman` 内核 crate，在 ch7 基础上增加 VirtIO GPU/键盘驱动
- 父进程（游戏协调器）：拥有完整游戏状态，负责渲染、输入处理、碰撞检测、管道通信
- 4 个幽灵子进程：各自运行独立 AI 循环，通过管道接收状态、返回移动方向
- 管道实现自然的锁步同步——父进程写入阻塞直到子进程读取，子进程读取阻塞直到父进程写入
- SIGUSR1 广播能量豆事件（幽灵进入惊吓模式），SIGUSR2 通知被吃幽灵回巢
- 文件系统读写最高分（`open`/`write`/`read`/`close`）

**与 ch6-breakout 的关键区别**

| 关注点 | ch6-breakout | ch7-pacman |
|--------|-------------|------------|
| fd_table 类型 | `Vec<Option<Mutex<FileHandle>>>` | `Vec<Option<Mutex<Fd>>>` (File/PipeRead/PipeWrite/Empty) |
| IPC 机制 | 无 | 管道（每个幽灵 2 条管道，共 8 条） |
| 异步事件 | 无 | 信号（SIGUSR1/SIGUSR2） |
| 进程数量 | 单进程 | 5 进程（1 父 + 4 幽灵子进程） |
| AI 架构 | 无 AI | 4 种独立幽灵 AI（Blinky/Pinky/Inky/Clyde） |
| 游戏复杂度 | 物理引擎 + 碰撞 | 网格移动 + 多角色 + 模式切换 + 关卡系统 |
| 存档内容 | 完整游戏状态 | 仅最高分（4 字节） |
| virtio-drivers | 0.1.0 | 0.3.0（修复键盘事件队列 bug） |

**五进程管道 IPC 架构**

```
父进程（游戏协调器）：
  VirtIO 键盘 → 方向输入 → 移动吃豆人
  每 tick:
    → 写 7 字节到每个幽灵的输入管道 [pac_x, pac_y, pac_dir, ghost_x, ghost_y, blinky_x, blinky_y]
    ← 从每个幽灵的输出管道读 1 字节 [direction: 0=上 1=右 2=下 3=左]
    → 碰撞检测、计分、渲染、fb_flush

幽灵子进程 0 (Blinky)：                       信号流：
  loop:                                        父 → SIGUSR1 → 所有幽灵（能量豆）
    read(pipe) → 7 字节游戏状态               父 → SIGUSR2 → 被吃幽灵（回巢）
    AI计算（直接追踪吃豆人）
    write(pipe) → 1 字节方向

幽灵子进程 1 (Pinky)：目标吃豆人前方 4 格
幽灵子进程 2 (Inky)：交替追踪/随机游走
幽灵子进程 3 (Clyde)：远时追踪，近时散开
```

管道双向流动实现自然的锁步同步：`pipe_read` 在缓冲区空时阻塞，`pipe_write` 在缓冲区满时阻塞。无需显式同步原语。

### 内核侧模块（tg-rcore-tutorial-ch7-pacman/）

**allocator.rs — 固定地址 bump 分配器**
- DMA 池位于 0x8500_0000（5 MiB），高于内核堆上界
- 实现 virtio-drivers v0.3.0 的 `Hal` trait（`dma_alloc`/`dma_dealloc`/`mmio_phys_to_virt`/`share`/`unshare`）
- 恒等映射，无需地址转换

**virtio.rs — VirtIO 设备初始化（GPU + 键盘）**
- 扫描 MMIO 插槽 0x10001000–0x10008000（步长 0x1000）
- 通过 `DeviceType` 枚举匹配 GPU 和 Input 设备
- GPU 必需（panic if not found），键盘可选（warn if absent）
- 返回 `VirtIODevices` 结构体（GPU 驱动 + Framebuffer 描述符 + 可选键盘驱动）

**virtio_block.rs — 更新为 v0.3.0 API**
- 使用 `allocator::HalImpl` 替换原有内联 `VirtioHal`
- 导入路径从 `virtio_drivers::VirtIOBlk` 改为 `virtio_drivers::device::blk::VirtIOBlk`
- 方法名保持 `read_block`/`write_block`（v0.3.0 未更名）

**main.rs 核心变更**
- `MEMORY` 从 48 MiB 增至 70 MiB（GPU 帧缓冲区 + 多进程页表开销）
- `MMIO` 扩展为 `(0x1000_0000, 0x9000)` + `(0x8500_0000, 0x50_0000)`
- 全局 GPU/键盘/帧缓冲区变量（`static mut`）
- `rust_main` 中在 `kernel_space()` 之后初始化 VirtIO GPU/键盘设备
- FB_INFO/FB_WRITE/FB_FLUSH 三个自定义系统调用在 trap 循环中拦截（在 `tg_syscall::handle` 之前），不影响信号处理流程
- `keyboard_trygetchar()` 返回按键码（press）或按键码|0x80（release），排空 EV_SYN 事件
- `IO::read` 的 STDIN 分支：替换阻塞 SBI 为非阻塞 VirtIO 键盘轮询
- **关键**：管道读写路径（`Fd::PipeRead`/`Fd::PipeWrite`）完全不受影响
- trap 错误处理新增 `stval`/`sepc` 日志输出，便于调试页错误

### 系统调用接口

| 系统调用 | ID | 参数 | 说明 |
|---------|-----|------|------|
| FB_INFO | 2000 | 无 | 返回 (width << 32) \| height |
| FB_WRITE | 2001 | x, y, w, h, data_ptr | 逐行翻译用户虚拟地址，alpha 混合写入帧缓冲区 |
| FB_FLUSH | 2003 | 无 | 触发 GPU 显示刷新 |
| read | 63 | fd=0(STDIN) | 非阻塞 VirtIO 键盘轮询，返回 keycode 或 0 |
| pipe | 59 | pipe_fd | 创建管道对 (read_fd, write_fd) |
| fork | 220 | 无 | 深拷贝进程（含 fd_table 和信号配置） |
| kill | 129 | pid, signum | 发送信号（SIGUSR1=10, SIGUSR2=12） |
| sigaction | 134 | signum, action, old | 注册信号处理函数 |
| sigreturn | 139 | 无 | 从信号处理函数返回 |
| open | 56 | path, flags | 最高分文件读写 |

### 用户侧模块（tg-rcore-tutorial-user/）

**pacman.rs — 游戏模块（feature-gated: `pacman`，~1970 行）**

*数据结构*
- `INITIAL_MAZE: [u8; 441]`：21×21 静态迷宫布局（墙/豆/能量豆/空/幽灵巢/隧道）
- `GAME: static mut Game`：全局游戏状态（静态分配，避免栈溢出）
  - `maze: [u8; 441]`：运行时迷宫（豆被吃后改为空）
  - `ghosts: [GhostInfo; 4]`：幽灵状态（位置/方向/PID/管道 fd/惊吓计时/被吃标记）
  - 吃豆人位置/方向/排队方向、分数/最高分/生命/关卡、状态机计时器
- `GameState` 枚举：Ready / Playing / Dying / LevelComplete / GameOver / Paused

*迷宫设计*
- 21×21 网格，28×28 像素单元格
- 左右对称，包含两条隧道行（幽灵穿越减速）
- 中央幽灵巢（3×3），粉色栅门标记入口
- 4 颗能量豆分布在近角落位置
- 吃豆人起始点在幽灵巢下方

*幽灵 AI — 4 种独立个性*

| 幽灵 | 颜色 | 追踪目标 | 性格 |
|------|------|---------|------|
| Blinky | 红色 | 吃豆人当前位置 | 激进追击者 |
| Pinky | 粉色 | 吃豆人前方 4 格 | 伏击者 |
| Inky | 青色 | 交替追踪/随机游走 | 不可预测 |
| Clyde | 橙色 | 远追近散（曼哈顿距离 > 8 追踪，否则回角落） | 胆小鬼 |

每个幽灵作为独立子进程运行：
1. 注册 SIGUSR1 处理函数（能量豆 → 设置 `FRIGHTENED_TICKS_LEFT = 48`）
2. 注册 SIGUSR2 处理函数（被吃 → 设置 `RESPAWNING_FLAG = true`）
3. 循环：`pipe_read` 7 字节 → AI 计算 → `pipe_write` 1 字节方向
4. 信号处理函数末尾必须调用 `sigreturn()` 恢复上下文

*模式切换*
- 散开模式（56 tick = 7 秒）→ 追踪模式（160 tick = 20 秒）→ 循环
- 惊吓模式（能量豆触发，48 tick = 6 秒）：随机移动，闪烁蓝白色
- 被吃模式：仅显示眼睛，双倍速度回巢

*管道通信协议*
```
父→幽灵（7 字节/tick）：
  [pac_x, pac_y, pac_dir, ghost_x, ghost_y, blinky_x, blinky_y]

幽灵→父（1 字节/tick）：
  [direction: 0=上, 1=右, 2=下, 3=左]
```

*渲染 — "Warm Arcade Phosphor" 风格*
- 深黑背景，近黑色通道，钴蓝色 3D 斜面墙壁（暗蓝阴影底 + 亮蓝边缘面向通道 + 蓝色内部填充）
- 金黄色吃豆人（圆形近似 + 方向嘴巴 + 亮黄色高光中心）
- 4 色幽灵精灵（圆顶 + 方向性瞳孔 + 波浪裙摆）
- 惊吓幽灵（深靛蓝 + 白色锯齿嘴 + 闪烁警告）
- 被吃幽灵（仅白色眼睛和瞳孔）
- 暖桃色圆点（4×4），脉冲发光能量豆（8×8 核心 + 光晕）
- 粉色幽灵巢栅门
- 5×7 位图字体（2× 缩放），逐行渲染避免大缓冲区

*HUD 面板（右侧）*
- 深色背景面板，左侧 2px 亮蓝色边框
- "1UP" + 当前分数（暖奶油色）
- "HIGH SCORE" + 最高分（绿色）
- "LEVEL" + 关卡号
- "LIVES" + 小型吃豆人图标（每条命一个）
- 幽灵状态指示器（彩色圆点）

*Game Over 覆盖面板*
- 居中蓝色斜面边框面板
- "GAME OVER" 红色标题
- 最终分数和最高分并排显示
- 新纪录提示（金黄色 "NEW HIGH SCORE!"）
- 闪烁 "PRESS SPACE" 提示（每 8 tick 交替亮暗）

*栈安全策略*
- `Game` 结构体使用 `static mut` 全局存储（~700 字节离开栈）
- `fill_rect` 使用 8×8 瓦片缓冲区（256 字节，非 16×16 的 1024 字节）
- 角色渲染使用逐行 `fb_write`（每行 80 字节缓冲区，非 20×20×4 = 1600 字节精灵缓冲区）
- 字体渲染使用逐行缓冲区（40 字节/行）
- 总栈使用峰值 < 2 KiB，远在 8 KiB 限制内

*输入处理*
- 非阻塞 `read(STDIN)` 轮询 VirtIO 键盘
- 按键码 & 0x7F = 实际键码，& 0x80 = 释放标记
- WASD / 方向键移动，Space 暂停/继续，F5 保存最高分
- 方向排队机制：预存下一个转向方向，到达路口时自动转向（经典吃豆人操作手感）

*最高分持久化*
- `open("pacman_hi\0", RDONLY)` 读取 4 字节小端 u32
- `open("pacman_hi\0", CREATE|WRONLY|TRUNC)` 写入 4 字节
- 读取失败返回 0（首次运行）

### 配置变更

- `.cargo/config.toml`：QEMU 添加 `-device virtio-gpu-device -device virtio-keyboard-device -vnc :0 -serial stdio`，`TG_USER_DIR` 指向共享用户 crate
- `Cargo.toml`：crate 名 `jsph-tg-rcore-tutorial-ch7-pacman`，`virtio-drivers = "0.3.0"`
- `build.rs`：case_key 为 `ch7_pacman`，用户程序构建添加 `--features pacman`，initproc 构建设置 `CHAPTER=pacman`（仅当 `CHAPTER` 环境变量未预设时）
- `cases.toml`：新增 `ch7_pacman` 配置节（28 标准 ch7 程序 + pacman）
- `initproc.rs`：新增 `"pacman" => "pacman"` 匹配分支

## 遇到的问题

### 关键 bug 1：StorePageFault 栈溢出

**症状**：游戏启动后打印 "Pac-Man starting..." 即触发 `StorePageFault`，`stval = 0x3FFFFFDEE0`。

**根因**：用户栈仅 8 KiB（2 页，映射在 `[2^38 - 8192, 2^38)` 区间）。故障地址 `0x3FFFFFDEE0 = 2^38 - 8480` 已超出栈底 288 字节。分析栈使用：
- `run_game` 栈帧中 `Game` 结构体 ~700 字节
- `fill_rect` 中 16×16 瓦片缓冲区 1024 字节
- `draw_pacman`/`draw_ghost` 中 20×20×4 精灵缓冲区 1600 字节
- 函数调用链帧开销（寄存器保存等）~数百字节
- 总计 > 8 KiB

**修复**（三管齐下）：
1. `Game` 改为 `static mut GAME` 全局存储，新增 `reset()` 方法原地重置
2. `TILE_SIZE` 从 16 降为 8（瓦片缓冲区从 1024 降为 256 字节）
3. `draw_pacman`/`draw_ghost`/`draw_char` 改为逐行渲染（每行 80 字节缓冲区，消除 1600 字节精灵缓冲区）

### 关键 bug 2：initproc 未识别 CHAPTER=pacman

**症状**：启动后显示 `Rust user shell >>` 提示符而非游戏。

**根因**：`initproc.rs` 的 `option_env!("CHAPTER")` 匹配表中无 `"pacman"` 分支，fallthrough 到 `"user_shell"`。

**修复**：在 `initproc.rs` 中添加 `"pacman" => "pacman"` 匹配分支。

### 关键 bug 3：非阻塞 STDIN 与 shell 不兼容

**症状**：将 STDIN 改为非阻塞 VirtIO 键盘后，user_shell 的 `getchar()` 持续返回 0，导致死循环。

**分析**：STDIN 读路径只能选一种模式——阻塞 SBI（shell 需要）或非阻塞 VirtIO（游戏需要）。SBI `console_getchar` 是忙等循环（M 态 UART 轮询），会阻塞整个内核。

**解决方案**：
- STDIN 使用纯非阻塞 VirtIO 键盘（无事件返回 0）
- `build.rs` 中 `CHAPTER=pacman` 仅在环境变量未预设时设置
- `test.sh` 导出 `CHAPTER=-7` 触发 `ch7b_usertest`（不含交互式测试），不会运行 shell
- `ch7b_usertest` 的测试列表不包含 `sig_ctrlc`（该测试需要交互式键盘输入）
- 最终效果：游戏模式下 STDIN 服务 VirtIO 键盘，测试模式下 STDIN 无需工作

### 关键 bug 4：VirtIO 键盘按键码不匹配

**症状**：方向键无响应。

**根因**：`pacman.rs` 使用 IBM PC 扫描码（Up=72, Left=75, Down=80, Right=77），但内核 `translate_keycode` 透传 VirtIO 原始键码（Up=103, Left=105, Down=108, Right=106）。

**修复**：更新按键码常量为 VirtIO 标准键码。

### 其他问题

1. **virtio-drivers v0.3.0 API 差异**：计划文档误称 v0.3.0 将 `read_block` 重命名为 `read_blocks`，实际未更名。通过阅读实际 crate 源码确认。

2. **信号处理函数遗漏 `sigreturn()`**：此 OS 的信号处理流程要求处理函数末尾调用 `sigreturn()` 恢复被中断的上下文。遗漏会导致进程状态损坏。参考 `sig_simple.rs` 测试用例确认模式。

3. **幽灵巢寻路**：幽灵 AI 子进程没有迷宫数据，无法验证移动是否穿墙。解决方案：幽灵选择最接近目标的方向，父进程验证移动合法性（`is_walkable`），非法移动则保持原位。

## 文件结构

```
tg-rcore-tutorial-ch7-pacman/
├── Cargo.toml          # crate 名 jsph-tg-rcore-tutorial-ch7-pacman, virtio-drivers 0.3.0
├── .cargo/config.toml  # QEMU: GPU + 键盘 + VNC + blk, -serial stdio, TG_USER_DIR
├── build.rs            # ch7_pacman case_key, --features pacman, CHAPTER=pacman（可被覆盖）
├── test.sh             # CHAPTER=-7 基础测试
└── src/
    ├── main.rs         # MMIO/DMA 映射、VirtIO 初始化、FB/FLUSH 系统调用、键盘 IO::read (~1070 行)
    ├── fs.rs           # Fd 枚举 (File/PipeRead/PipeWrite/Empty)（继承自 ch7）
    ├── process.rs      # 进程结构 + fd_table + signal（继承自 ch7）
    ├── processor.rs    # PROCESSOR + ProcManager（继承自 ch7）
    ├── virtio_block.rs # VirtIO-blk 驱动（更新为 v0.3.0 HalImpl）
    ├── allocator.rs    # DMA bump 分配器 (0x8500_0000) + v0.3.0 Hal trait
    └── virtio.rs       # MMIO 扫描：GPU + 键盘设备发现

tg-rcore-tutorial-user/  (修改)
├── Cargo.toml          # 新增 pacman feature
├── cases.toml          # 新增 ch7_pacman 配置节
└── src/
    ├── lib.rs          # 新增 pacman 模块
    ├── bin/
    │   ├── pacman.rs   # 二进制入口 (feature-gated body)
    │   └── initproc.rs # 新增 "pacman" => "pacman" 匹配分支
    └── pacman.rs       # 游戏逻辑、迷宫、渲染、幽灵 AI、管道 IPC、信号 (~1970 行)
```

## 内存布局

```
0x80000000  内核 .text/.rodata
0x802xxxxx  内核 .data
0x80xxxxxx  内核堆起始
0x84600000  内核堆结束 (MEMORY = 70 MiB)
0x85000000  DMA 池 (5 MiB) — VirtIO GPU 帧缓冲区 + virtqueue
0x85500000  DMA 池结束
0x88000000  QEMU RAM 结束 (128 MiB)

0x10000000  VirtIO MMIO 区域（恒等映射入内核地址空间）
0x10001000  VirtIO-blk（ch7 已有）
0x10007000  VirtIO-Input（键盘）
0x10008000  VirtIO-GPU

用户虚拟地址空间（每进程独立）：
0x00010000  ELF 代码/数据/BSS（含 static mut GAME）
0x000xxxxx  用户堆（sbrk 扩展，游戏不使用）
  ...
VPN(1<<26)-2  用户栈 (8 KiB)
VPN::MAX      异界传送门（内核/用户共享）

运行时进程拓扑（5 个进程，8 条管道）：
  initproc ─fork→ pacman (父进程)
                    ├─pipe+fork→ Blinky  (PID=3)
                    ├─pipe+fork→ Pinky   (PID=4)
                    ├─pipe+fork→ Inky    (PID=5)
                    └─pipe+fork→ Clyde   (PID=6)
```

## 测试

- `cargo build`：通过（内核 + 30 用户程序 + fs.img 打包）
- `TG_SKIP_USER_APPS=1 cargo check`：通过
- `cargo run` + VNC 连接（localhost:5900）：游戏正常运行
  - 迷宫渲染：蓝色 3D 斜面墙壁，暖桃色圆点，脉冲能量豆
  - WASD / 方向键控制吃豆人，方向排队响应流畅
  - 4 个幽灵 AI 子进程独立运行（启动时打印 PID）
  - 吃圆点计分（10 分/点，50 分/能量豆）
  - 能量豆触发 SIGUSR1，幽灵进入惊吓模式（蓝色闪烁）
  - 吃惊吓幽灵获得递增分数（200/400/800/1600）
  - 生命用尽后显示 Game Over 面板（分数/最高分/新纪录提示/闪烁重开提示）
  - Space 暂停/继续，F5 保存最高分
  - 吃完全部圆点后关卡闪烁升级
- 基础测试（`CHAPTER=-7`）：ch7 管道/信号/fork 测试通过

## 设计决策

1. **管道锁步 vs 共享内存**：ch5-pingpong 使用共享内存，但 ch7 的核心教学目标是管道。管道的阻塞读写天然提供锁步同步——父进程写入后阻塞等待幽灵响应，无需额外同步原语。代价是每 tick 8 次系统调用（4 写 + 4 读），但对 8 Hz 游戏帧率完全可接受。

2. **信号用于广播 vs 碰撞检测**：原始设计考虑幽灵→父进程的 SIGUSR2 碰撞通知，但父进程已知所有位置，直接检测碰撞更简单可靠。最终信号仅用于父→幽灵方向：SIGUSR1 广播能量豆事件，SIGUSR2 通知单个幽灵被吃。

3. **幽灵 AI 无迷宫数据**：幽灵子进程只接收 7 字节状态，没有完整迷宫。AI 选择最优方向，父进程负责验证合法性。这简化了管道协议，避免传输 441 字节迷宫数据。代价是幽灵偶尔选择无效方向而停滞一拍，但因帧率低（8 Hz），视觉上不明显。

4. **静态 Game 结构体**：用户栈仅 8 KiB，`Game`（~700 字节）+ 渲染缓冲区 + 调用链开销易超限。将 `Game` 放入 `static mut` 后，栈峰值降至 ~2 KiB。`reset()` 方法原地重置避免栈上构造。

5. **逐行渲染替代精灵缓冲区**：经典方案是构建完整精灵缓冲区（20×20×4=1600 字节）再一次性 `fb_write`。改为逐行渲染（20 次 `fb_write`，每次 80 字节）牺牲系统调用次数换取栈安全。对 8 Hz 帧率无感知影响。

6. **CHAPTER 环境变量覆盖机制**：`build.rs` 仅在 `CHAPTER` 未设置时才设 `pacman`。这允许 `test.sh` 通过 `export CHAPTER=-7` 切换到测试模式，复用同一内核 crate 同时支持游戏和基础测试。

7. **virtio-drivers 0.3.0**：v0.1.0 的 `VirtIOInput::pop_pending_event()` 不回收缓冲区通知设备，32 个事件后键盘停止工作。v0.3.0 修复此 bug。代价是 Hal trait API 变化（`BufferDirection` 参数、`share`/`unshare`/`mmio_phys_to_virt` 新方法），需要更新 block 设备驱动共用新 Hal。

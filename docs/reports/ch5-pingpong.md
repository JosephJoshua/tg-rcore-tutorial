# 第五章扩展报告：VirtIO-GPU 双人乒乓球游戏（共享内存 IPC）

## 实现内容

### 目标
扩展 ch5 进程管理系统，通过 VirtIO-GPU 帧缓冲区渲染和 VirtIO 键盘输入实现一个双人 Pong 游戏。游戏使用 **fork 创建两个进程**，通过**内核管理的共享内存页**通信，演示 ch5 的 fork/wait/exit 进程管理与 Sv39 虚拟内存、VirtIO 设备驱动的结合。

### 架构设计

ch5 在 ch4 虚拟内存基础上引入完整的**进程管理**：fork 深拷贝地址空间、exec 替换程序映像、wait 回收子进程、stride 调度算法。本扩展的核心挑战在于**进程间通信**——ch5 没有管道（ch7）或信号（ch7），因此引入一个最小的共享内存机制：内核分配一个物理页，映射到父子进程的相同虚拟地址。

**整体方案**
- 新建 `tg-rcore-tutorial-ch5-pingpong` 内核 crate
- 新增 SHM_CREATE 系统调用（ID 2002）：分配物理页，映射到进程地址空间 0x3000_0000
- 修改 fork() 实现：检测共享页，子进程中 unmap 深拷贝副本，remap 到父进程的同一物理页号
- 新增 FB_FLUSH 系统调用（ID 2003）：将 GPU 刷新与像素写入解耦
- 父进程读取所有键盘事件，通过共享内存传递 P2 输入给子进程
- 子进程根据共享内存中的输入标志移动 paddle2_y

**与 ch4-tetris 的关键区别**

| 关注点 | ch4-tetris | ch5-pingpong |
|--------|------------|--------------|
| 进程访问 | `PROCESSES.get_mut().get_mut(entity)` | `PROCESSOR.get_mut().current().unwrap()` |
| 进程管理 | `Vec<Process>`，轮转调度 | `PManager<Process, ProcManager>`，stride 调度 |
| 进程创建 | 内核启动时加载 ELF | `fork()` 深拷贝 + `exec()` 替换 |
| IPC | 无（单进程游戏） | 共享内存页（同一物理页映射到两个进程） |
| GPU 刷新 | 每次 fb_write 后刷新 | 独立 FB_FLUSH 系统调用，每帧一次 |
| 键盘驱动版本 | virtio-drivers 0.1.0 | virtio-drivers 0.3.0（修复输入队列 bug） |

**双进程 IPC 架构**

```
父进程（player 1 + 物理引擎 + 渲染）：
  VirtIO 键盘 -> W/S 按住？ -> 直接移动 paddle1_y
               -> Up/Down 按住？ -> 写入 p2_up/p2_down 到共享内存
  从共享内存读取 paddle2_y -> 渲染两个球拍
  运行球物理 -> 渲染球 + 分数 -> fb_flush

子进程（player 2 球拍控制）：
  从共享内存读取 p2_up/p2_down -> 移动 paddle2_y -> 清除标志
  yield
```

共享内存双向流动：父进程写输入标志，子进程写球拍位置。

### 内核侧模块（tg-rcore-tutorial-ch5-pingpong/）

**allocator.rs -- DMA bump 分配器**
- DMA 池位于 0x8500_0000（5 MiB），通过恒等映射加入内核地址空间
- 实现 virtio-drivers 0.3.0 的 `Hal` trait（`dma_alloc`/`dma_dealloc`/`mmio_phys_to_virt`/`share`/`unshare`）

**virtio.rs -- VirtIO 设备初始化**
- 扫描 MMIO 插槽 0x10001000-0x10008000，发现 GPU 和键盘设备
- GPU 必需（panic if missing），键盘可选

**process.rs -- 共享内存支持**
- `Process` 结构新增 `shared_page: Option<PPN<Sv39>>` 字段
- `fork()` 修改：若父进程有共享页，子进程中先 unmap `cloneself()` 的深拷贝，再 `map_extern()` 父进程的同一物理 PPN
- `from_elf()` 初始化 `shared_page: None`

**main.rs 核心变更**
- `MEMORY` 增至 70 MiB（ch5 内核镜像含大量嵌入用户程序，约 29 MiB .data 段）
- `kernel_space()` 新增 MMIO 区域（0x1000_0000..0x1000_9000）和 DMA 池（0x8500_0000..0x8550_0000）的恒等映射
- 四个自定义系统调用在调度循环的 `UserEnvCall` 处理中拦截

### 系统调用接口

| 系统调用 | ID | 参数 | 说明 |
|---------|-----|------|------|
| FB_INFO | 2000 | 无 | 返回 `(width << 32) | height` |
| FB_WRITE | 2001 | x, y, w, h, data_ptr | 逐行翻译用户指针，写入帧缓冲区（不刷新） |
| SHM_CREATE | 2002 | 无 | 分配物理页，映射到 0x3000_0000，返回虚拟地址 |
| FB_FLUSH | 2003 | 无 | 触发 GPU 显示刷新 |

**IO::read 双模式**
- fd 0（STDIN）：阻塞 SBI console_getchar（shell 使用）
- fd 3（STDIN_BUFFERED）：非阻塞 VirtIO 键盘轮询，返回按键（keycode）和释放（keycode|0x80），排空所有 sync 事件

### 用户侧模块（tg-rcore-tutorial-user/）

**pingpong.rs -- 游戏模块（feature-gated: `pingpong`）**

*共享内存布局*
```rust
#[repr(C)]
struct SharedState {
    ball_x/y/vx/vy: i32,     // 定点数 (x256)
    paddle1_y/paddle2_y: i32, // 像素坐标
    score1/score2: u32,
    tick: u32,
    game_state: u32,          // 0=等待, 1=进行中, 2=得分暂停, 3=游戏结束
    p2_up/p2_down: u8,        // 输入标志
}
```

*按键状态跟踪*
- `KeyState` 结构追踪 W/S/Up/Down 的按住状态
- `update()` 每帧排空所有待处理键盘事件，根据按下/释放更新状态
- 内核返回 keycode 表示按下，keycode|0x80 表示释放

*物理引擎*
- 定点数算术（x256），仅整数运算
- 球速初始 4px/帧，碰撞后加速（+32 定点单位），上限 10px/帧
- 墙壁反弹（上下边界）、球拍反弹（角度随击中位置变化）
- 得分检测：球越过左/右边界

*渲染 -- "Neon Arcade" 风格*
- 深邃虚空背景（#040612），微亮场地（#060A18）
- 双层发光边框（暗外层 + 亮内层靛蓝色）
- Player 1：霓虹青色球拍 + 青色光晕
- Player 2：霓虹洋红色球拍 + 洋红光晕
- 球：白色核心 + 灰色光晕 + 2 帧渐隐拖尾
- 2x 缩放 7 段数码管分数显示（青色 vs 洋红色）
- 得分暂停：3 秒真实时间暂停 + 分数闪烁（400ms 周期）
- 游戏结束横幅：霓虹发光边框 + 大号 "P1"/"P2" 字样
- 全屏虚空填充（场地外区域）

*增量渲染*
- 仅重绘移动的球和球拍，避免全帧重绘
- 球拍始终重绘以修复与球的重叠伪影
- 球在所有元素之上绘制
- 得分暂停和游戏结束期间跳过正常渲染

### 配置变更
- `.cargo/config.toml`：QEMU 添加 `-device virtio-gpu-device -device virtio-keyboard-device -vnc :0 -serial stdio`，`TG_USER_DIR` 指向共享用户 crate
- `Cargo.toml`：`virtio-drivers = "0.3.0"`
- `build.rs`：case_key 选择 `ch5_pingpong`，用户程序构建添加 `--features pingpong`
- `cases.toml`：新增 `ch5_pingpong` 配置节（21 标准程序 + pingpong）
- `test.sh`：90 秒超时，`CHAPTER=-5`，`-display none` 无头运行，`tg-rcore-tutorial-checker --ch 5` 校验

## 遇到的问题

### 关键 bug 1：DMA 池未映射入内核地址空间

**症状**：GPU 初始化后内核挂起（`gpu.resolution()` 不返回）。

**根因**：`kernel_space()` 未映射 DMA 池区域（0x8500_0000..0x8550_0000）。GPU 帧缓冲区分配在 DMA 池中，CPU 访问未映射的虚拟地址导致页错误。ch4-tetris 的 `kernel_space()` 中有此映射，但被遗漏。

**修复**：在 `kernel_space()` 中添加 DMA 池区域的恒等映射。

### 关键 bug 2：VNC 渲染混乱——每次 fb_write 后 GPU 刷新

**症状**：VNC 上显示一堆闪烁的小方块，无法识别游戏画面。

**根因**：`handle_fb_write` 每次调用都执行 `gpu.flush()`。`fill_rect` 使用 16x16 像素分块，绘制背景（1100x700）产生约 3000 次 fb_write + 3000 次 GPU 刷新。VNC 客户端收到大量部分更新帧，显示混乱。

**修复**：将 GPU 刷新从 fb_write 中移除，新增独立的 FB_FLUSH 系统调用（ID 2003）。游戏每帧结束时调用一次 `fb_flush()`，将刷新次数从数千降至每帧一次。

### 关键 bug 3：virtio-drivers 0.1.0 键盘输入队列耗尽

**症状**：键盘输入正常工作约 3 秒后完全停止响应。

**根因**：virtio-drivers 0.1.0 的 `VirtIOInput::pop_pending_event()` 在回收事件缓冲区到可用环后未调用 `transport.notify()`——这是该 crate 中唯一遗漏此调用的驱动（blk/net/gpu/console 均正确调用）。VirtIO 输入设备初始有 32 个缓冲区，每次按键生成约 4 个事件（按下、同步、释放、同步）。约 8 次按键后缓冲区耗尽，设备不再发送事件。

**验证**：上游提交 `b559de30`（2023-01-12，作者 Andrew Walbran@Google）修复了此 bug，标题为 "Notify device after adding buffers to available ring"，首次发布于 v0.3.0。

**修复**：将 virtio-drivers 从 0.1.0 升级到 0.3.0。更新 `Hal` trait 实现以适配新 API（`BufferDirection` 参数、`share`/`unshare`/`mmio_phys_to_virt` 方法）。

### 关键 bug 4：双进程键盘队列竞争 + 球拍操控迟钝

**症状**：球拍移动极慢且无响应，无法同时操控两个球拍，球速远快于球拍速度。

**根因（竞争）**：父子进程均从同一 VirtIO 键盘队列读取。子进程消耗了本应由父进程处理的事件（反之亦然），每个进程仅处理自己的按键并丢弃对方的。

**根因（迟钝）**：内核仅返回按下事件（value=1），不返回释放事件。游戏无法追踪按键按住状态，球拍仅在离散的键盘重复事件上移动（远慢于每帧运行的球物理引擎）。

**修复**：
1. 内核返回按下（keycode）和释放（keycode|0x80）两种事件
2. 父进程读取所有键盘事件，追踪 W/S/Up/Down 的按住状态
3. 球拍在按键按住时每帧移动（与球速匹配）
4. 父进程将 P2 按住状态写入共享内存，子进程读取并移动 paddle2_y（保留 IPC 模式）

### 关键 bug 5：Shell 阻塞导致测试挂起

**症状**：ch5 checker 测试全部失败（0/14），shell 启动后无输出。

**根因**：最初将 IO::read 改为非阻塞 VirtIO 键盘轮询，shell 的 `read(STDIN)` 返回 0 后疯狂循环，耗尽用户堆。

**修复**：fd 0（STDIN）使用阻塞 SBI console_getchar（shell 需要），fd 3（STDIN_BUFFERED）使用非阻塞 VirtIO 键盘（游戏使用）。

### 其他问题

1. **游戏结束横幅被覆盖**：渲染循环在 STATE_GAME_OVER 期间继续绘制球拍和球，覆盖了横幅。修复：在暂停/游戏结束状态下 `continue` 跳过正常渲染。

2. **球拍消失**：球经过球拍位置时，球的擦除操作抹掉了球拍像素。修复：每帧始终重绘球拍，球在所有元素之上绘制。

3. **VNC 键盘事件未路由**：缺少 `-vnc :0` 参数导致 QEMU 未建立 VNC 键盘输入到 VirtIO 键盘设备的路由。修复：添加 `-vnc :0` 到 QEMU 参数。

4. **球拍 2 碰撞后球减速**：代码审查发现 paddle 2 碰撞时球速增量符号错误（`ball_vx - signum * 32` 应为 `+`）。修复：修正符号。

## 文件结构

```
tg-rcore-tutorial-ch5-pingpong/
  Cargo.toml          # virtio-drivers = "0.3.0"
  .cargo/config.toml  # QEMU: GPU + keyboard + VNC, TG_USER_DIR
  build.rs            # ch5_pingpong case_key, --features pingpong
  test.sh             # 90s 超时，CHAPTER=-5 无头 CI
  src/
    main.rs           # MMIO/DMA 映射、VirtIO 初始化、FB/SHM/FLUSH 系统调用、双模式 IO::read
    process.rs        # shared_page 字段、fork 共享内存处理
    processor.rs      # stride 调度（未修改）
    allocator.rs      # DMA bump 分配器 + Hal trait (v0.3.0 API)
    virtio.rs         # MMIO 扫描：GPU + 键盘设备发现

tg-rcore-tutorial-user/  (修改)
  Cargo.toml          # 新增 pingpong feature
  cases.toml          # 新增 ch5_pingpong 配置节
  src/
    lib.rs            # 新增 pingpong 模块、shm_create() 和 fb_flush() 系统调用封装
    pingpong.rs       # 双进程 Pong 游戏：物理引擎、IPC、霓虹渲染 (~560 行)
    bin/
      pingpong.rs     # 二进制入口 (feature-gated body)
```

## 内存布局

```
0x80200000  内核 .text/.rodata
0x80242000  内核 .data（含嵌入的 22 个用户 ELF，约 29 MiB）
0x81f53000  内核堆起始
0x84800000  内核堆结束 (MEMORY = 70 MiB)
0x85000000  DMA 池 (5 MiB) -- VirtIO GPU 帧缓冲区 + virtqueue
0x85500000  DMA 池结束
0x88000000  QEMU RAM 结束 (128 MiB)

0x10000000  VirtIO MMIO 区域（恒等映射入内核地址空间）

用户虚拟地址空间（每进程独立）：
0x00010000  ELF 代码/数据/BSS
  ...       用户堆（sbrk 扩展）
0x30000000  共享内存页 (4 KiB) -- 父子进程映射到同一物理页
  ...
VPN(1<<26)-2  用户栈 (8 KiB)
VPN::MAX      异界传送门（内核/用户共享）
```

## 测试

- `cargo build`：通过（内核 + 22 用户程序）
- `TG_SKIP_USER_APPS=1 cargo check`：通过
- `cargo publish --dry-run`（用户 crate）：通过
- `bash test.sh`：ch5 checker 校验 14/14 通过
- `cargo run` + VNC 连接：双人游戏画面正常，W/S 和 Up/Down 同时操控球拍，球碰撞/计分/加速正常，得分后 3 秒暂停 + 分数闪烁，游戏结束显示 "P1"/"P2" 霓虹横幅

## 设计决策

1. **FB_WRITE 与 FB_FLUSH 分离**：ch4-tetris 每次 fb_write 后刷新 GPU，在 VNC 上造成渲染混乱。分离后游戏每帧只刷新一次，VNC 显示稳定。

2. **升级 virtio-drivers 到 0.3.0**：而非在 0.1.0 上做 MMIO 写入 workaround。v0.3.0 修复了输入队列通知 bug，API 变更（新 Hal trait 方法）可控。

3. **父进程统一读取键盘**：VirtIO 键盘只有一个事件队列，双进程竞争读取会互相消耗事件。由父进程统一读取，通过共享内存将 P2 输入传递给子进程，保留了 IPC 教学模式。

4. **按键按住状态追踪**：内核返回按下/释放事件，用户空间维护 `KeyState` 结构。球拍在按键按住时每帧移动，与球物理引擎速度匹配，操控流畅。

5. **真实时间暂停**：得分暂停使用 `get_time()` 毫秒时钟（3000ms），而非不可靠的调度器 tick 计数。

# 第六章扩展报告：VirtIO-GPU 打砖块游戏（含文件系统存档）

## 实现内容

### 目标
扩展 ch6 文件系统内核，通过 VirtIO-GPU 帧缓冲区和 VirtIO 键盘设备实现一个完整的打砖块（Breakout）游戏。游戏通过 ch6 的 `open`/`write`/`read`/`close` 文件系统调用实现存档和读档功能，演示 VirtIO 设备驱动与 easy-fs 文件系统的协同工作。

### 架构设计

ch6 在 ch5 进程管理的基础上引入了 **磁盘文件系统**（easy-fs on VirtIO-blk），程序从 fs.img 磁盘镜像加载，每个进程拥有独立的文件描述符表。本扩展在 ch6 基础上增加 VirtIO-GPU（帧缓冲区渲染）和 VirtIO 键盘（VNC 输入），核心亮点是利用文件系统系统调用实现**游戏状态持久化**。

**整体方案**
- 新建 `tg-rcore-tutorial-ch6-breakout` 内核 crate，在 ch6 基础上增加 VirtIO GPU/键盘驱动
- VirtIO MMIO 区域（0x1000_0000）和 DMA 池（0x8500_0000）通过恒等映射加入内核地址空间
- FB_WRITE 系统调用逐行翻译用户虚拟地址（复用 ch4-tetris 模式，适配 ch6 的 PROCESSOR 架构）
- FB_WRITE 与 FB_FLUSH 分离——像素写入不触发 GPU 刷新，用户代码每帧调用一次 `fb_flush()`
- 游戏通过 `open("breakout_save\0", CREATE|WRONLY|TRUNC)` / `write()` / `close()` 存档
- 游戏通过 `open("breakout_save\0", RDONLY)` / `read()` / `close()` 读档
- initproc 通过 `CHAPTER=breakout` 编译时环境变量直接启动游戏

**与 ch4-tetris 的关键区别**

| 关注点 | ch4-tetris | ch6-breakout |
|--------|-----------|--------------|
| 进程管理 | `PROCESSES`（Vec） | `PROCESSOR`（PManager + BTreeMap） |
| 程序加载 | 内核内嵌（APP_ASM） | 磁盘镜像（fs.img） |
| 文件 I/O | 无 | fd_table + easy-fs |
| 游戏特色 | 无存档 | F5 存档 / F9 读档 |
| GPU 刷新 | 每次 fb_write 刷新 | fb_write 仅写入，fb_flush 单独刷新 |
| 启动方式 | 顺序加载（无 initproc） | initproc fork + exec breakout |
| STDIN 读取 | 非阻塞（无 shell 问题） | 非阻塞 + getchar yield-loop |

### 内核侧模块（tg-rcore-tutorial-ch6-breakout/）

**allocator.rs — 固定地址 bump 分配器**
- DMA 池位于 0x8500_0000（5 MiB），高于内核堆上界
- 实现 `Hal` trait（VirtIO DMA），恒等映射
- 与 ch6 已有的 `VirtioHal`（内核堆分配）并存——blk 设备使用内核堆 Hal，GPU/键盘使用 bump Hal

**virtio.rs — VirtIO 设备初始化（GPU + 键盘）**
- 扫描 MMIO 插槽 0x10001000–0x10008000
- GPU 必需，键盘可选

**main.rs 核心变更**
- `MMIO` 扩展为 `(0x1000_0000, 0x9000)` + `(0x8500_0000, 0x50_0000)`
- `MEMORY` 从 48 MiB 增至 70 MiB
- FB_INFO (2000) / FB_WRITE (2001) / FB_FLUSH (2003) 三个自定义系统调用
- FB_WRITE 仅复制像素到帧缓冲区，不触发 GPU 刷新
- FB_FLUSH 单独触发 GPU 刷新（每帧调用一次，大幅提升性能）
- `IO::read` 的 STDIN 分支：替换阻塞 `tg_sbi::console_getchar()` 为非阻塞 `keyboard_trygetchar()`
- `translate_keycode` 扩展：新增 F5（code 63 → `\x05`）和 F9（code 67 → `\x09`）映射

### 系统调用接口

| 系统调用 | ID | 参数 | ch6 特殊处理 |
|---------|-----|------|-------------|
| FB_INFO | 2000 | 无 | 返回 (width << 32) \| height |
| FB_WRITE | 2001 | x, y, w, h, data_ptr | data_ptr 逐行翻译，不刷新 GPU |
| FB_FLUSH | 2003 | 无 | 触发 GPU 帧缓冲区刷新 |
| read | 63 | fd=0, buf, count | VirtIO 键盘轮询，无按键返回 0 |
| open | 56 | path, flags | 存档用 CREATE\|WRONLY\|TRUNC，读档用 RDONLY |
| write | 64 | fd, buf, count | 存档数据写入 easy-fs |
| close | 57 | fd | 关闭文件描述符 |

### 用户侧模块（tg-rcore-tutorial-user/）

**breakout.rs — 游戏模块（feature-gated: `breakout`，~800 行）**

*数据结构*
- `bricks: [u8; 50]`：10×5 网格，0=已销毁，1=存活
- `Game`：定点数球位置/速度（×256）、挡板位置、分数/生命/等级、存档闪烁计时器
- `SaveData`：`#[repr(C)]` 结构体（~92 字节），magic=0x42524B4F ("BRKO")，含完整游戏状态

*游戏逻辑*
- 10 列 × 5 行砖块，每行不同颜色和分值（顶部 50 → 底部 10）
- 定点数物理（×256）：球位移、墙壁反弹、挡板角度反射、砖块碰撞检测
- 挡板反弹角度：基于球心与挡板中心偏移量，归一化后乘以 3 得到 vx
- 砖块碰撞方向判定：比较水平/垂直重叠量，选择最小重叠方向反弹
- 每销毁 5 块砖加速 10%（vx/vy × 11/10）
- 全部砖块销毁后升级：重置砖块、提升发射速度（每级 +FP/2）
- 3 条生命，球落下底部减 1 条命，0 条命 Game Over

*存档/读档*
- F5 存档：`SaveData` 序列化为裸字节 → `open("breakout_save\0", CREATE|WRONLY|TRUNC)` → `write()` → `close()`
- F9 读档：`open("breakout_save\0", RDONLY)` → `read()` → `close()` → magic 校验 → 恢复状态 → 全屏重绘
- 存档文件 ~92 字节，远在 easy-fs 限制内
- 游戏中央显示 "SAVED" / "LOADED" 霓虹闪烁提示（30 帧后自动消失）

*渲染 — "Neon Arcade" 风格*
- 深空背景（0x080612），三层霓虹橙色发光边框围绕游戏区域
- 3D 斜面砖块：2px 高光（左上）+ 2px 阴影（右下）+ 内部填色
- 球体光晕：4px 青色半透明光环 + 白色实心核心
- 斜面挡板：电琥珀色，亮顶暗底
- 标题 "BREAKOUT" 带暗色阴影偏移的热橙色文字
- Game Over 覆盖层：霓虹边框 + 红色标题阴影 + 青色 "PRESS ENTER"
- 5×7 位图字体（35 字符：0-9, A-B, C-H, I, K-N, O, P, R-U, V-Y, 空格, 连字符），3× 缩放
- 增量渲染：仅重绘变化元素，HUD 数值仅在变化时刷新

*输入*
- 非阻塞 `read(STDIN)` 通过 `poll_key()` 轮询
- A/D（或左/右箭头）移动挡板，Space 发射球
- F5 存档，F9 读档，Enter 重开
- Game Over 状态下禁用输入（防止存档/读档状态不一致）

### 配置变更
- `.cargo/config.toml`：QEMU 添加 `-device virtio-gpu-device -device virtio-keyboard-device -serial stdio`，`TG_USER_DIR` 指向共享用户 crate
- `Cargo.toml`：crate 名 `jsph-tg-rcore-tutorial-ch6-breakout`
- `build.rs`：case_key 为 `ch6_breakout`，用户程序构建添加 `--features breakout`，initproc 构建设置 `CHAPTER=breakout`
- `cases.toml`：新增 `ch6_breakout` 配置节（23 标准程序 + breakout）
- 用户 `lib.rs`：新增 `fb_flush()` 系统调用封装（syscall 2003）

## 遇到的问题

### 关键 bug 1：非阻塞 STDIN 导致 user_shell 堆溢出崩溃

**症状**：内核启动后 user_shell 打印 `>>` 提示符后立即 panic：`memory allocation of 16384 bytes failed`。

**根因**：将 ch6 的 STDIN 读取从阻塞式 `tg_sbi::console_getchar()` 替换为非阻塞 VirtIO 键盘轮询后，`read(STDIN, &mut buf)` 在无按键时返回 0 字节。`user_shell` 的 `getchar()` 返回 0（null 字节），shell 主循环将 null 字节 `push` 到命令行 `String`，在紧密循环中快速耗尽 16 KiB 用户堆。

**修复**：
1. `getchar()` 改为 yield-loop：`read` 返回 0 时调用 `sched_yield()` 后重试，直到读到非零字节。向后兼容——阻塞式内核中 `read` 已经阻塞返回，loop 首次迭代即退出。
2. `initproc` 新增 `"breakout" => "breakout"` 匹配分支
3. `build.rs` 构建 initproc 时设置 `CHAPTER=breakout`，使游戏直接启动

### 关键 bug 2：FB_WRITE 每次刷新 GPU 导致帧率过低

**症状**：游戏画面刷新明显卡顿，尤其是初始屏幕绘制和砖块销毁时。

**根因**：`handle_fb_write` 每次调用都执行 `gpu.flush()`。`draw_rect` 对每行像素调用一次 `fb_write`，一个 920×600 的矩形产生 600 次 GPU 刷新。增量帧中球体擦除/绘制 + HUD 更新也产生数十次刷新。

**修复**：将 FB_WRITE 和 FB_FLUSH 分离为独立系统调用（2001/2003）。FB_WRITE 仅复制像素到帧缓冲区，用户代码每帧末尾调用一次 `fb_flush()` 触发单次 GPU 刷新。同时 HUD 数值仅在 score/lives/level 变化时重绘。（注：ID 2002 由 ch5-pingpong 的 SHM_CREATE 使用。）

### 关键 bug 3：缺少 B 和 U 字形

**症状**："BREAKOUT" 显示为 "\_REAKO\_T"，"LAUNCH" 显示为 "LA\_NCH"。

**根因**：从 tetris.rs 复制的字形表（33 个字符）不包含 B 和 U。

**修复**：添加 B 和 U 的 5×7 位图字形，字形表从 33 扩至 35 个字符。

### 关键 bug 4：HUD 控制说明文字超出屏幕

**症状**：控制面板中 "LAUNCH" 文字被裁切，超出 1280px 屏幕右边界。

**根因**：HUD 起始 x = PLAY_X + PLAY_W + 30 = 1130px。"SPACE" 后的 "LAUNCH"（6 个字符 × 17px）从 x=1232 开始，延伸到 1334px。

**修复**：重新排列控制说明为 `KEY  DESC` 内联格式（如 `SPC  LAUNCH`），使每行总宽度不超过 150px 右边距。

### 关键 bug 5：FB_WRITE 双重 move_next 跳过指令

**症状**：代码审查发现潜在问题——FB 系统调用返回后用户程序可能跳过一条指令。

**根因**：ch6 调度循环在提取 syscall ID 前已调用 `ctx.move_next()`（sepc += 4）。FB 处理分支内部又调用了一次 `ctx.move_next()`，导致 sepc 总共增加 8 字节，跳过 ecall 后的下一条指令。

**修复**：移除 FB 处理分支内的 `ctx.move_next()` 调用，保留调度循环开头的统一调用。

### 其他问题

1. **easy-fs crate 名称不匹配**：从 ch6 复制时 easy-fs 子目录的 Cargo.toml 名称（`tg-rcore-tutorial-easy-fs`）与 ch6-breakout Cargo.toml 中引用的名称（`jsph-tg-rcore-tutorial-easy-fs-t1l4`）不一致。从原始 ch6 重新复制解决。

2. **Game Over 期间存档/读档状态不一致**：Game Over 时 `drain_input` 仍处理 F5/F9，读档后 `game_over` 局部变量仍为 `true`，导致游戏不可恢复。修复为 Game Over 时跳过 `drain_input`。

3. **升级后球速未提升**：升级重置球为附着状态，发射时始终使用 `INIT_BALL_SPEED`。规格要求"升级后加速"。修复为发射速度 = `INIT_BALL_SPEED + (level - 1) * FP / 2`。

## 文件结构

```
tg-rcore-tutorial-ch6-breakout/
├── Cargo.toml          # crate 名 jsph-tg-rcore-tutorial-ch6-breakout
├── .cargo/config.toml  # QEMU: GPU + 键盘 + blk, -serial stdio, TG_USER_DIR
├── build.rs            # ch6_breakout case_key, --features breakout, CHAPTER=breakout
└── src/
    ├── main.rs         # MMIO/DMA 映射、VirtIO 初始化、FB/FLUSH/read 系统调用
    ├── fs.rs           # easy-fs 文件系统管理器（继承自 ch6）
    ├── process.rs      # 进程结构 + fd_table（继承自 ch6）
    ├── processor.rs    # PROCESSOR + ProcManager（继承自 ch6）
    ├── virtio_block.rs # VirtIO-blk 驱动（继承自 ch6）
    ├── allocator.rs    # DMA bump 分配器 (0x8500_0000) + Hal trait
    └── virtio.rs       # MMIO 扫描：GPU + 键盘设备发现

tg-rcore-tutorial-user/  (修改)
├── Cargo.toml          # 新增 breakout feature
├── cases.toml          # 新增 ch6_breakout 配置节
└── src/
    ├── lib.rs          # 新增 breakout 模块、fb_flush()、getchar yield-loop
    ├── bin/
    │   ├── breakout.rs # 二进制入口 (feature-gated body)
    │   └── initproc.rs # 新增 "breakout" => "breakout" 匹配分支
    └── breakout.rs     # 游戏逻辑、物理引擎、渲染、存档/读档 (~800 行)
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
0x10001000  VirtIO-blk（ch6 已有）
0x10002000+ VirtIO-GPU / VirtIO-Input（本扩展新增）

用户虚拟地址空间（每进程独立）：
0x00010000  ELF 代码/数据/BSS
0x000xxxxx  用户堆（sbrk 扩展）
  ...
VPN(1<<26)-2  用户栈 (8 KiB)
VPN::MAX      异界传送门（内核/用户共享）
```

## 测试

- `cargo build`：通过（内核 + 24 用户程序 + fs.img 打包）
- `TG_SKIP_USER_APPS=1 cargo check`：通过
- `cargo publish --dry-run`（用户 crate）：通过
- `cargo run` + VNC 连接：游戏正常运行，A/D 控制挡板，Space 发射球
- 砖块碰撞销毁、计分、升级加速均正常
- F5 存档 → 退出 → 重启 → F9 读档：状态完整恢复
- Game Over 后 Enter 重开

## 设计决策

1. **FB_WRITE / FB_FLUSH 分离**：ch4-tetris 的每次 fb_write 都刷新 GPU 在简单场景可行，但打砖块每帧涉及更多绘制操作（球光晕、斜面砖块、HUD），逐次刷新严重拖慢帧率。分离后每帧仅一次 GPU 刷新，性能提升显著。

2. **双 Hal 架构**：ch6 的 `VirtioHal` 使用内核堆分配 DMA（适合小量 blk 请求），GPU 帧缓冲区 ~4 MiB 使用独立 bump 分配器避免堆碎片化。两种 Hal 共存，各自服务不同设备。

3. **getchar yield-loop 而非修改 initproc 跳过 shell**：游戏需要非阻塞 STDIN，shell 需要阻塞 STDIN。通过 `getchar()` yield-loop 在用户库层统一解决：游戏用 `poll_key()`（直接 `read()`）获得非阻塞行为，shell 用 `getchar()`（yield 直到有键）获得阻塞行为。向后兼容所有章节。

4. **CHAPTER 环境变量启动游戏**：利用 initproc 已有的 `option_env!("CHAPTER")` 机制，通过 build.rs 设置 `CHAPTER=breakout`，无需创建专用 initproc 二进制。新增的匹配分支向后兼容。

5. **存档格式设计**：`#[repr(C)]` 固定布局 + magic 校验，无需序列化库。92 字节的存档文件远小于 easy-fs 的文件大小限制。`_pad` 字段确保 `bricks_destroyed` 的 u32 对齐。

6. **增量 HUD 更新**：Game 结构体追踪 `prev_score/prev_lives/prev_level`，`draw_hud_values` 仅在数值变化时重绘，避免每帧的冗余 fb_write 调用。

# 第四章扩展报告：VirtIO-GPU 俄罗斯方块游戏

## 实现内容

### 目标
扩展 ch4 虚拟内存系统，通过 VirtIO-GPU 帧缓冲区和 VirtIO 键盘设备实现一个完整的俄罗斯方块游戏。游戏与 13 个标准 ch4 测试程序在调度下并发运行，演示 Sv39 虚拟内存环境下的 VirtIO 设备驱动集成。

### 架构设计

ch4 引入了 Sv39 三级页表和独立进程地址空间，与 ch3（恒等映射）有本质区别。本扩展在 ch4 基础上增加 VirtIO-GPU（帧缓冲区渲染）和 VirtIO 键盘（VNC 输入），核心挑战在于**地址翻译**——所有用户指针必须通过进程页表翻译后内核才能访问。

**整体方案**
- 新建 `tg-rcore-tutorial-ch4-tetris` 内核 crate，在 ch4 基础上增加 VirtIO 驱动
- VirtIO MMIO 寄存器区域（0x1000_0000）和 DMA 池（0x8500_0000）通过恒等映射加入内核地址空间
- FB_WRITE 系统调用逐行翻译用户虚拟地址，处理页边界跨越
- 游戏逻辑和像素渲染全部在用户空间完成（`tetris.rs` 模块）
- 14 个用户程序（13 标准测试 + 1 俄罗斯方块）并发执行

**与 ch3-snake 的关键区别**

| 关注点 | ch3-snake | ch4-tetris |
|--------|-----------|------------|
| 地址空间 | 恒等映射，用户指针直接访问 | Sv39 页表，必须 `translate()` |
| 上下文切换 | `LocalContext::execute()` | `ForeignContext::execute(portal, ())` |
| DMA 分配 | 固定 bump 分配器（0x8200_0000） | 固定 bump 分配器（0x8500_0000） |
| MMIO 访问 | 直接（无分页） | 需在 `kernel_space()` 中映射 |
| 内存 | 24 MiB（默认） | 70 MiB（14 进程页表 + GPU 开销） |

### 内核侧模块（tg-rcore-tutorial-ch4-tetris/）

**allocator.rs — 固定地址 bump 分配器**
- DMA 池位于 0x8500_0000（5 MiB），高于内核堆上界（~0x8480_0000）
- 实现 `Hal` trait（VirtIO DMA），恒等映射（内核地址空间恒等映射物理 RAM）

**virtio.rs — VirtIO 设备初始化（GPU + 键盘）**
- 扫描 MMIO 插槽 0x10001000–0x10008000
- GPU 必需，键盘可选（优雅降级）

**main.rs 核心变更**
- `kernel_space()` 新增 MMIO 区域（0x1000_0000..0x1000_9000）和 DMA 池的恒等映射
- `MEMORY` 增至 70 MiB，内核栈增至 64 KiB
- `rust_main()` 在内核地址空间激活后、用户程序加载前初始化 VirtIO 设备
- FB_INFO/FB_WRITE 在调度循环的 `UserEnvCall` 处理中拦截（在标准系统调用分发前）
- FB_WRITE **逐行翻译**用户数据指针：`process.address_space.translate::<u8>(VAddr::new(row_va), READABLE)`
- `IO::read` 实现：翻译用户缓冲区指针后写入键盘按键

### 系统调用接口

| 系统调用 | ID | 参数 | ch4 特殊处理 |
|---------|-----|------|-------------|
| FB_INFO | 2000 | 无 | 无需翻译 |
| FB_WRITE | 2001 | x, y, w, h, data_ptr | data_ptr 逐行通过页表翻译 |
| read | 63 | fd=0, buf, count | buf 指针翻译后写入 |

### 用户侧模块（tg-rcore-tutorial-user/）

**tetris.rs — 游戏模块（feature-gated: `tetris`）**

*数据结构*
- `board: [u8; 10*20]`：0=空，1-7=七种方块颜色
- `Piece { shape, rotation, row, col }`：4 旋转态 × 7 方块 = 28 种形态，预计算偏移表
- `Game`：棋盘、当前/下一方块、分数/等级/行数、xorshift64 PRNG

*游戏逻辑*
- 标准俄罗斯方块规则：重力下落、软降（S 50ms）、硬降（Space 即时）、顺时针旋转
- 行消除：1行=100×level, 2行=300×level, 3行=500×level, 4行=800×level
- 每 10 行升级，速度从 800ms 递减至 100ms
- 双重游戏结束检测：**lock out**（方块锁定时有单元在棋盘上方）+ **block out**（新方块生成时重叠）
- Ghost piece：显示方块降落位置的半透明轮廓

*渲染 — "Neon Arcade" 风格*
- 深海军蓝黑背景，3D 斜面方块单元格（2px 高光/阴影边框）
- 三层霓虹发光边框围绕棋盘（暗→中→亮橙色渐变）
- 霓虹色方块：电青（I）、金黄（O）、洋红（T）、霓虹绿（S）、鲜红（Z）、亮蓝（J）、橙色（L）
- 右侧面板：NEXT 预览、SCORE/LEVEL/LINES 数值、CONTROLS 按键说明
- "TETRIS" 霓虹青色标题居中显示
- Game Over 覆盖层显示最终分数和"PRESS ANY KEY"提示
- 5x7 位图字体（33 个字符：0-9, A-Z 子集, 空格, 连字符），3× 缩放
- 增量渲染：仅重绘变化的单元格，`restore_cell()` 正确恢复被 ghost 覆盖的已锁定方块

*输入*
- 非阻塞 `read(STDIN)`，内核端直接轮询 VirtIO 键盘 `pop_pending_event()`
- 按键映射：W=旋转, A/D=左右移动, S=软降, Space=硬降
- `drain_input()` 每帧处理所有待处理按键

### 配置变更
- `.cargo/config.toml`：QEMU 添加 `-device virtio-gpu-device -device virtio-keyboard-device -serial stdio`，`TG_USER_DIR` 指向共享用户 crate
- `Cargo.toml`：`virtio-drivers = "0.1.0"`
- `build.rs`：case_key 选择 `ch4_tetris`，用户程序构建添加 `--features tetris`
- `cases.toml`：新增 `ch4_tetris` 配置节（13 标准程序 + tetris）
- `test.sh`：60 秒超时，`-display none` 无头运行，`tg-rcore-tutorial-checker --ch 4` 校验

## 遇到的问题

### 关键 bug 1：customizable-buddy 0.0.2 空指针导致所有用户程序崩溃

**症状**：所有 14 个用户程序在 `heap::init()` 时 panic：`NonNull::new_unchecked requires that the pointer is non-null`（customizable-buddy lib.rs:370）。

**根因**：共享用户 crate 的堆缓冲区为 512 KiB，位于 BSS 段（起始地址 ~0x1ab70）。该区域跨越 0x20000 边界。`deallocate()` 在 0x20000 处生成 order-17 的块（idx=1），`LinkedListBuddy::put(1)` 计算伙伴 `idx ^ 1 = 0`，`Order::idx_to_ptr(0)` 通过 `NonNull::new_unchecked(0)` 产生空指针——未定义行为。

ch3 不触发此 bug 是因为用户程序加载在高物理地址（0x80400000+），buddy 序号远大于 0。ch4 使用 Sv39 虚拟内存，用户程序链接在低虚拟地址 0x10000，512 KiB 缓冲区跨越 2 的幂次边界。

**排查过程**：
1. 最初怀疑 VirtIO 初始化、MMIO 映射或大 MEMORY 导致内存损坏
2. 逐一禁用 VirtIO 初始化、MMIO 映射、DMA 映射——均无效
3. 发现原始 ch4 的本地用户 crate 使用 16 KiB 堆（正常），共享 crate 使用 512 KiB（崩溃）
4. 通过 ELF readelf 确认 BSS 段跨越 0x20000 边界
5. 追踪 customizable-buddy 源码确认 `idx_to_ptr(0)` 产生空指针的精确路径

**临时修复**：用户 crate 堆大小按 feature 条件编译——无游戏 feature 时 16 KiB，有 snake/tangram feature 时 512 KiB。tetris 游戏不使用堆分配（全部栈/静态缓冲区）。

**上游修复**：已向 [YdrMaster/buddy-allocator](https://github.com/YdrMaster/buddy-allocator) 提交 PR。修复方案：`Order::idx_to_ptr` 返回 `Option<NonNull<T>>`（使用 `NonNull::new` 替代 `new_unchecked`），`LinkedListBuddy::put` 和 `AvlBuddy::put` 在 buddy 为 None 时跳过合并。同时移除 `AvlBuddy` 中写入后从未读取的无用 `base` 字段。

### 关键 bug 2：DMA 池与内核堆重叠导致 OOM

**症状**：MEMORY=24 MiB 时第 5 个进程加载即 OOM；增至 96 MiB 后用户程序崩溃（buddy 空指针问题）。

**根因**：VirtIO GPU 帧缓冲区（1280×800×4 ≈ 4 MiB）+ 14 个进程的页表大幅增加内核堆消耗。DMA bump 池最初放在 0x8200_0000，限制了 MEMORY 上界（堆不能与 DMA 池重叠）。

**修复**：DMA 池移至 0x8500_0000，MEMORY 设为 70 MiB。堆上界 ~0x8480_0000，DMA 池起始 0x8500_0000，无重叠。

### 关键 bug 3：draw_next_piece 整数溢出

**症状**：绘制下一个方块预览时 panic：`attempt to add with overflow`。

**根因**：`let px = PANEL_X + (dc as usize + 1) * CELL`——当 `dc` 为负数（如 -1）时，`dc as usize` 回绕为极大值。

**修复**：`let px = PANEL_X + ((dc + 1) as usize) * CELL`——先在 i8 域内加 1（结果非负），再转 usize。

### 其他问题

1. **Game over 不完整**：原实现仅检测 block out（新方块生成重叠），未检测 lock out（方块锁定时部分在棋盘上方）。`lock_piece()` 改为返回是否有单元在 r < 0，调度循环同时检查两种条件。

2. **Ghost 擦除覆盖已锁定方块**：`erase_falling_piece` 原实现用空棋盘格覆盖所有 ghost/piece 单元格，会抹掉下方已锁定方块的显示。引入 `restore_cell()` 检查 `game.board[r][c]` 正确恢复。

3. **硬降后锁定方块不可见**：硬降时 `drain_input` 绘制方块在底部，tick 处理立即擦除并锁定，但 `redraw_board` 仅在行消除时调用。锁定前保存 `locked_cells` 和 `locked_shape`，无行消除时手动绘制锁定单元格。

4. **共享用户 crate 路径**：ch4-tetris 删除了从 ch4 复制的本地用户 crate 副本，通过 `.cargo/config.toml` 的 `TG_USER_DIR` 指向共享 `../tg-rcore-tutorial-user/`。

5. **SCORE 标签被裁切**：NEXT 方块预览区域（y=160, 高 94px）与 SCORE 标签（y=250）重叠。下移所有面板标签 15px 解决。

## 文件结构

```
tg-rcore-tutorial-ch4-tetris/
├── Cargo.toml          # virtio-drivers dep
├── .cargo/config.toml  # QEMU: GPU + keyboard, TG_USER_DIR
├── build.rs            # ch4_tetris case_key, --features tetris
├── test.sh             # 60s 超时无头 CI
└── src/
    ├── main.rs         # MMIO/DMA 映射、VirtIO 初始化、FB/read 系统调用（含地址翻译）
    ├── process.rs      # 进程结构（从 ch4 继承）
    ├── allocator.rs    # DMA bump 分配器 (0x8500_0000) + Hal trait
    └── virtio.rs       # MMIO 扫描：GPU + 键盘设备发现

tg-rcore-tutorial-user/  (修改)
├── Cargo.toml          # 新增 tetris feature, 版本 0.4.11
├── cases.toml          # 新增 ch4_tetris 配置节
└── src/
    ├── lib.rs          # 新增 tetris 模块 (feature-gated)
    ├── heap.rs         # 堆大小按 feature 条件编译（16 KiB / 512 KiB）
    ├── tetris.rs       # 游戏逻辑、3D 渲染、位图字体、输入处理 (~480 行)
    └── bin/
        └── tetris.rs   # 二进制入口 (feature-gated body)
```

## 内存布局

```
0x80000000  内核 .text/.rodata
0x8022b000  内核 .data（含嵌入的 14 个用户 ELF）
0x814d1000  内核堆起始
0x84800000  内核堆结束 (MEMORY = 70 MiB)
0x85000000  DMA 池 (5 MiB) — VirtIO GPU 帧缓冲区 + virtqueue
0x85500000  DMA 池结束
0x88000000  QEMU RAM 结束 (128 MiB)

0x10000000  VirtIO MMIO 区域（恒等映射入内核地址空间）

用户虚拟地址空间（每进程独立）：
0x00010000  ELF 代码/数据/BSS
0x00020000  用户堆（sbrk 扩展）
  ...
VPN(1<<26)-2  用户栈 (8 KiB)
VPN::MAX      异界传送门（内核/用户共享）
```

## 测试

- `cargo build`：通过（内核 + 14 用户程序）
- `TG_SKIP_USER_APPS=1 cargo check`：通过
- `cargo publish --dry-run`（用户 crate + ch4-tetris crate）：通过
- `bash test.sh`：ch4 checker 校验 6/6 通过
- `cargo run` + VNC 连接：游戏画面正常，WASD/Space 控制，行消除计分，升级加速，Game Over 后任意键重启
- 13 个标准 ch4 测试程序串口输出正确

## 设计决策

1. **DMA 使用 bump 分配器而非内核堆**：最初尝试从内核堆（`tg_kernel_alloc`）分配 DMA 缓冲区，但大 MEMORY 导致 `customizable-buddy` 空指针问题。改用独立 bump 池，堆与 DMA 物理隔离，消除相互影响。

2. **FB_WRITE 逐行翻译**：用户像素缓冲区在虚拟地址空间连续，但物理页面可能不连续。逐行调用 `translate()` 确保每行数据从正确的物理地址读取。对于 28×28 的单元格（112 字节/行），单次翻译即可覆盖一行。

3. **用户堆大小条件编译**：受 `customizable-buddy` bug 影响，ch4 虚拟内存环境下 512 KiB 静态堆会崩溃。tetris 游戏不使用堆分配（所有缓冲区在栈上或编译期常量），因此默认 16 KiB 堆即可满足标准测试程序需求。

4. **增量渲染 + restore_cell**：每帧仅重绘移动/旋转影响的单元格。`restore_cell()` 检查棋盘状态，正确恢复空格或已锁定方块，避免 ghost piece 擦除覆盖已有方块。

5. **双重游戏结束检测**：同时实现 lock out（方块锁定时超出棋盘顶部）和 block out（新方块生成重叠），比仅检测 block out 更符合标准俄罗斯方块规则。

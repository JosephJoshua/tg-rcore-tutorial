# 第八章扩展报告：Doom 游戏移植（VirtIO GPU/键盘 + doomgeneric C 引擎 FFI）

## 实现内容

### 目标
将经典 Doom 游戏（1993）移植到 ch8 并发内核上作为用户态程序运行。使用 [doomgeneric](https://github.com/ozkl/doomgeneric) 跨平台抽象层，通过 FFI 链接 C 引擎代码与 Rust 平台层。通过 VirtIO-GPU 帧缓冲区渲染，VirtIO 键盘输入，从 easy-fs 文件系统加载 WAD 数据文件。

### 架构设计

**整体方案**
- 新建 `tg-rcore-tutorial-ch8-doom` 内核 crate，在 ch8 基础上增加 VirtIO GPU/键盘驱动
- 使用 `cc` crate 在 `build.rs` 中交叉编译 doomgeneric 的约 90 个 C 源文件为静态库
- 在 Rust 中实现最小 libc shim（malloc/free、字符串操作、文件 I/O、字符分类等），通过 `#[no_mangle] extern "C"` 导出
- printf 系列函数在 C 中实现（`doom_libc.c`），避免 `va_list` 跨 FFI 边界的问题
- 平台函数（`DG_Init`/`DG_DrawFrame`/`DG_GetKey` 等 6 个）在 Rust 中实现
- doom1.wad（共享软件版，约 4 MiB）在构建时打包进 easy-fs 镜像，运行时整体加载到内存

**与 ch7-pacman 的关键区别**

| 关注点 | ch7-pacman | ch8-doom |
|--------|------------|----------|
| 游戏引擎 | 纯 Rust (约 1970 行) | C 引擎 (约 15000 行) + Rust FFI |
| 编译方式 | Rust only | cc crate 交叉编译 C 为静态库 + Rust 链接 |
| libc 依赖 | 无 | 完整 libc shim（malloc/printf/fopen/qsort 等） |
| 内存需求 | 约 16 KiB 用户堆 | 16 MiB sbrk 堆（zone 8M + WAD 5M + 开销） |
| 用户栈 | 2 页 (8 KiB) | 32 页 (128 KiB)，release 模式编译 |
| 文件 I/O | 仅最高分 (4 字节) | WAD 文件加载 (4 MiB)，内存缓存避免 lseek |
| 进程模型 | 多进程（5 个，管道 IPC） | 单进程（ch8 线程可选但 Doom 引擎非线程安全） |
| task-manage feature | `proc` | `thread` |

### 内核侧模块（tg-rcore-tutorial-ch8-doom/）

**allocator.rs / virtio.rs / virtio_block.rs** — 与 ch7-pacman 相同模式，使用 virtio-drivers v0.3.0。

**main.rs 核心变更**
- `MEMORY` = 72 MiB（必须低于 DMA 池 0x8500_0000，详见"遇到的问题"）
- `MMIO` 扩展为 VirtIO MMIO 区域 + DMA 池恒等映射
- FB_INFO/FB_WRITE/FB_FLUSH 三个自定义系统调用（同 ch7-pacman）
- `keyboard_trygetchar()`：每次调用前 `ack_interrupt()`，否则 QEMU 不交付事件
- `IO::read` STDIN 分支：非阻塞 VirtIO 键盘轮询
- `IO::write` FB_WRITE：使用 `get_current_proc()`（ch8 线程/进程分离，地址空间属于 Process）
- 新增 `sbrk` 系统调用实现（ch8 原版未实现 sbrk）：`change_program_brk()` 方法映射/取消映射用户堆页面

**process.rs 变更**
- 用户栈从 2 页增至 32 页（128 KiB），Doom 调用栈深度需要
- 新增 `heap_bottom`/`program_brk` 字段用于 sbrk
- `exec()` 同步复制 `heap_bottom`/`program_brk`（否则 exec 后 sbrk 失效）
- `fork()` 同步复制 `heap_bottom`/`program_brk`
- `from_elf()` 追踪 ELF 段最高地址作为堆起始

**build.rs 变更**
- case_key 为 `ch8_doom`
- 用户程序以 `--release` 模式编译（debug 模式栈帧过大导致栈溢出）
- 传递 `--features doom` 和 `CHAPTER=doom`
- `easy_fs_pack()` 额外打包 `doom1.wad` 到文件系统镜像

### 用户侧模块

**doomgeneric/ — 厂商化 C 源码（约 90 个 .c 文件 + stub headers）**
- 排除平台后端文件（SDL/Xlib/Win/Emscripten/Allegro 等）
- 排除依赖 SDL 的系统文件（i_sound.c/i_system.c/i_timer.c/i_input.c 等）
- `doom_stubs.c`：被排除文件的桩实现 + C 版 memcpy/memset/memmove/memcmp + `I_GetEvent` 实现（调用 `DG_GetKey` + `D_PostEvent`，替代被排除的 `i_input.c`）
- `doom_libc.c`：printf/snprintf/sprintf/fprintf/vsnprintf/sscanf/puts/putchar
- `include/`：17 个 stub 标准库头文件（stdio.h/stdlib.h/string.h 等）
- `doomtype.h` 修改：`boolean` 从 `unsigned char` 改为 `int`（详见"遇到的问题"）；`PACKEDATTR` 置空（避免 RISC-V 非对齐访问）
- `m_misc.c` 修改：`M_StringJoin` 替换为非变参版本 `M_StringJoin2/3/4`
- `w_file_stdc.c` 修改：WAD 文件句柄分配改用 `malloc` 替代 `Z_Malloc`

**doom/libc_shim.rs — 最小 libc 实现**
- 内存分配：bump allocator over sbrk（malloc/free/realloc/calloc）
- 字符串操作：strlen/strcpy/strncpy/strcmp/strncmp/strncasecmp/strcat/strncat/strchr/strrchr/strstr/strdup
- 字符分类：isdigit/isspace/isalpha/isalnum/isprint/isupper/islower/isxdigit/toupper/tolower
- 数值转换：atoi/strtol/abs
- 排序：qsort（插入排序）
- 文件 I/O：fopen/fclose/fread/fwrite/fseek/ftell（WAD 内存缓存 + fd 方式存档）
- 进程控制：exit/abort
- 桩函数：signal/atexit/getenv/mkdir/stat/access/time/localtime/errno

**doom/platform.rs — DG_ 平台函数**
- `DG_Init()`：调用 fb_info() 获取屏幕尺寸
- `DG_DrawFrame()`：设置 alpha 字节，640x400 居中于 1280x800 帧缓冲区，逐行 fb_write + fb_flush
- `DG_SleepMs()`/`DG_GetTicksMs()`：基于 clock_gettime 轮询
- `DG_GetKey()`：从环形缓冲区读取按键事件，每次调用前 poll_keyboard()
- 关键：环形缓冲区的 head/tail/data 必须使用 `read_volatile`/`write_volatile`（详见"遇到的问题"）

**doom/keymap.rs — VirtIO 到 Doom 按键码映射**
- 方向键、Ctrl/Space/Shift/Alt/Enter/Escape/Tab/Backspace
- F1-F10 功能键
- 数字键和字母键（通过 scancode 到 ASCII 查找表）

### 系统调用接口

| 系统调用 | ID | 说明 |
|---------|-----|------|
| FB_INFO | 2000 | 返回 (width << 32) \| height |
| FB_WRITE | 2001 | 逐行翻译用户地址写入帧缓冲区 |
| FB_FLUSH | 2003 | 触发 GPU 显示刷新 |
| read(STDIN) | 63 | 非阻塞 VirtIO 键盘轮询 |
| open/close/read/write | 56/57/63/64 | WAD 文件加载 + 存档读写 |
| sbrk | 214 | 用户堆扩展（16 MiB） |

## 遇到的问题

### Bug 1：memcpy/memset 无限递归

**症状**：游戏在 WAD 处理阶段无限挂起（10+ 分钟无输出）。

**根因**：Rust 的 `core::ptr::copy_nonoverlapping` 和 `core::ptr::write_bytes` 在 no_std 环境下被编译器降级为 `memcpy`/`memset` 调用。我们的 `#[no_mangle] extern "C" fn memcpy` 实现使用了 `copy_nonoverlapping`，形成无限递归：`memcpy -> copy_nonoverlapping -> memcpy -> ...`

**修复**：memcpy/memset/memmove/memcmp 全部在 C 中实现（`doom_stubs.c`），使用手写字节/字循环，避免 Rust 内建函数。字对齐路径（8 字节一次）使 WAD 加载时间从 8 分钟降至 4 秒。

### Bug 2：M_StringJoin va_list NULL 检测失败

**症状**：`M_LoadDefaults` 阶段无限挂起。

**根因**：`M_StringJoin(const char *s, ...)` 使用 NULL 作为变参终止标记。在裸机 RISC-V 64 位环境下，`va_arg(args, const char *)` 无法正确检测 NULL 终止符（可能是 GCC 裸机工具链的 va_list ABI 问题），导致无限循环读取栈上的垃圾指针。

**修复**：将所有 `M_StringJoin(a, b, NULL)` 调用替换为非变参版本 `M_StringJoin2(a, b)`（同理 3/4 参数版本）。在 `m_misc.c` 中新增 `M_StringJoin2/3/4` 函数，共替换 15 处调用。

### Bug 3：doomgeneric boolean 类型大小不匹配（上游 bug）

**症状**：游戏初始化完成后在状态栏渲染时触发 `LoadPageFault`，`stval = 0x2b00000030`。

**根因**：doomgeneric 的 `doomtype.h` 将 `boolean` 定义为 `unsigned char`（1 字节），但 `st_stuff.c:1267` 有 `(int *)&plyr->weaponowned[i+1]` 强转。`weaponowned` 是 `boolean[]`，解引用 `*mi->inum` 时按 `int`（4 字节）读取，将相邻的 `weaponowned` 元素也读入，产生越界的 patch 数组下标（例如 257，合法范围 0-1）。

具体地：玩家拥有手枪和霰弹枪时，`*(int *)&weaponowned[1]` 在小端序下读到 `[0x01, 0x01, 0x00, 0x00]` = 257。`arms[0][257]` 越界读到 BSS 段中 WAD 文件目录数据，拼成野指针 `0x2b00000030`（其中 `0x30` 来自某 lump 的 filepos，`0x2b`（即 `+`）来自相邻 lump 的 size 字段）。

**分析过程**：
1. 初始以为是 zone 内存分配器 64 位不兼容（memblock_t 头部从 24 字节增至 40 字节），尝试增大 zone（6->10->32 MiB）无效
2. 添加 `V_DrawPatch` 指针检查发现崩溃指针始终为 `0x2b00000030`（非随机），排除 zone 碎片化
3. 在 `W_CacheLumpNum` 添加返回值检查——返回时指针有效，使用时已损坏，排除 zone 分配问题
4. 在 `STlib_updateMultIcon` 中打印 `mi->p`、`mi->oldinum`、`mi->inum`：发现 `oldinum=257`、`inum=0x01000101`（4 个相邻 boolean 拼成的垃圾值），`mi->p=0xae250`（正是 `arms` 数组地址），确认越界访问
5. 追溯到 `(int *)&plyr->weaponowned[i+1]` 强转

**修复**：`typedef int boolean`（匹配 chocolate-doom）。原始 doomgeneric 使用 4 字节 `enum boolean`（含 `undef=0xFFFFFFFF` 哨兵），commit `3b1d530` 将其改为 `unsigned char` 时未审计 `(int *)` 强转。此 bug 在所有平台上存在，但仅在裸机环境（无虚拟内存保护网）下必现崩溃。已确认为上游 bug，计划提交 PR。

### Bug 4：DMA 池地址映射冲突

**症状**：VirtIO 键盘 `pop_pending_event()` 始终返回 None。

**根因**：`MEMORY = 128 MiB` 时，内核堆范围 `0x80292000..0x88200000` 覆盖了 DMA 池地址 `0x85000000`。堆映射使用 `map_extern`（恒等映射 VA = PA），但如果 DMA 池地址落在堆范围内，则不能再单独 `map_extern` DMA 池（页表条目冲突）。初始"修复"是省略 DMA 池映射（认为堆已覆盖）。

然而实际上，堆的 `map_extern` 恒等映射虽然覆盖了 DMA 池地址范围，但内核堆分配器从中分配新页面时，可能修改该区域的内容。VirtIO 设备通过 DMA 写入物理 `0x85000000`，但如果该区域被堆分配器用作普通堆内存，DMA 写入的数据可能被堆操作覆盖或读取不一致。

**修复**：将 `MEMORY` 降至 72 MiB（堆止于 `0x84A00000`，低于 DMA 池），并恢复 DMA 池 `0x85000000..0x85500000` 的单独恒等映射。确保 DMA 池区域专用于 VirtIO 设备，不被堆分配器使用。

### Bug 5：release 模式优化器消除键盘队列逻辑

**症状**：`DG_GetKey` 的反汇编显示函数直接返回 0，键盘环形缓冲区的 head/tail 比较和 queue 数据读取全部被优化掉。

**根因**：`static mut KEY_HEAD`/`KEY_TAIL`/`KEY_QUEUE` 未使用 volatile 访问。编译器在 release 模式下证明：
1. `poll_keyboard()` 中的 `read(STDIN)` 系统调用（ecall 指令）没有 memory clobber，优化器认为 `buf[0]` 调用后仍为 0
2. 因此 `poll_keyboard()` 永远在第一次迭代 break，不写入 KEY_QUEUE
3. KEY_HEAD 永远等于 KEY_TAIL，`DG_GetKey` 永远返回 0
4. 整个队列逻辑被死代码消除

**修复**（三处 volatile）：
- `read(STDIN)` 系统调用后使用 `read_volatile(buf.as_ptr())` 读取缓冲区（内核通过页表翻译写入用户缓冲区，ecall 指令没有 memory clobber，编译器无法看到此副作用）
- KEY_HEAD/KEY_TAIL 的读写使用 `read_volatile`/`write_volatile`（防止优化器证明 head == tail 恒成立）
- KEY_QUEUE 的读写使用 `read_volatile`/`write_volatile`（防止优化器将队列数据替换为零）

### Bug 6：VirtIO 键盘需要 ack_interrupt

**症状**：即使 DMA 映射正确，`pop_pending_event()` 仍返回 None。

**根因**：QEMU 的 VirtIO 键盘设备在中断未确认时不交付新事件。

**修复**：每次调用 `pop_pending_event()` 前调用 `kbd.ack_interrupt()`。

### Bug 7：I_GetEvent 未实现导致键盘事件不传递

**症状**：VirtIO 键盘事件到达内核，但 Doom 引擎不响应按键。

**根因**：排除 `i_input.c` 后，`I_GetEvent()` 被桩为空函数。Doom 的输入链是 `I_StartTic()` -> `I_GetEvent()` -> `DG_GetKey()` -> `D_PostEvent()`。空桩导致 `DG_GetKey` 永远不被调用，键盘事件从未进入引擎事件队列。

**修复**：在 `doom_stubs.c` 中实现 `I_GetEvent()`，循环调用 `DG_GetKey()` 获取按键事件，构造 `event_t`（区分 `ev_keydown`/`ev_keyup`），通过 `D_PostEvent()` 提交给引擎。

### 其他问题

1. **用户栈溢出（debug 模式）**：debug 模式的 Doom 二进制使用超过 128 KiB 栈空间（未优化的深层函数调用链），即使 32 页栈也不够。修复：用户程序以 `--release` 模式编译。

2. **WAD 文件重复加载**：`D_FindIWAD` 调用 `M_FileExists`（内部 fopen+fclose）检查文件存在性，加载整个 4 MiB WAD 到内存。随后 `W_AddFile` 再次 fopen 同一文件。修复：WAD 内存缓存——首次加载后保存 buffer 指针，重新 fopen 同一文件时直接复用缓存。

3. **`-iwad doom1.wad` 参数**：`D_FindIWAD` 默认搜索目录列表（调用 `M_StringJoin` 拼路径 + `access()` 检查），但我们的 `access()` 桩返回 -1。修复：传递 `-iwad doom1.wad` 命令行参数直接指定 WAD 路径。后续改进 `access()` 为实际 `open()`/`close()` 检查。

4. **sbrk 未实现**：ch8 原版内核未实现 `sbrk` 系统调用（Doom 需要 16 MiB 用户堆）。在 `process.rs` 新增 `heap_bottom`/`program_brk` 字段和 `change_program_brk()` 方法。

## 文件结构

```
tg-rcore-tutorial-ch8-doom/
|-- Cargo.toml          # crate 名 jsph-tg-rcore-tutorial-ch8-doom, virtio-drivers 0.3.0
|-- .cargo/config.toml  # QEMU: GPU+键盘+VNC+blk, -m 256M, -serial stdio, TG_USER_DIR
|-- build.rs            # ch8_doom case_key, --release --features doom, CHAPTER=doom, WAD 打包
|-- test.sh
|-- doom1.wad           # 共享软件 WAD (约 4 MiB, .gitignore)
|-- src/
    |-- main.rs         # MEMORY=72M, MMIO/DMA, VirtIO 初始化, FB 系统调用, sbrk, 键盘
    |-- allocator.rs    # DMA bump 分配器 (0x8500_0000) + v0.3.0 Hal trait
    |-- virtio.rs       # MMIO 扫描: GPU + 键盘
    |-- virtio_block.rs # VirtIO-blk (v0.3.0 HalImpl)
    |-- process.rs      # 32 页用户栈, heap_bottom/program_brk, sbrk
    |-- processor.rs    # PThreadManager (继承自 ch8)
    |-- fs.rs           # Fd 枚举 (继承自 ch8)

tg-rcore-tutorial-user/ (修改)
|-- Cargo.toml          # 新增 doom feature, cc build-dependency
|-- build.rs            # cc crate 编译 doomgeneric C 源码 (feature-gated)
|-- cases.toml          # 新增 ch8_doom 配置节
|-- doomgeneric/        # 厂商化 C 源码 (约 90 .c, 约 97 .h, 17 stub headers)
|   |-- doom_stubs.c    # 被排除文件的桩 + C 版 memcpy/memset/memmove
|   |-- doom_libc.c     # C 版 printf/snprintf/vsnprintf/sscanf
|   |-- include/        # stub 标准库头文件
|-- src/
    |-- lib.rs          # 新增 doom 模块
    |-- heap.rs         # doom feature 无需额外静态堆（使用 sbrk）
    |-- bin/
    |   |-- doom.rs     # 二进制入口 (feature-gated)
    |   |-- initproc.rs # 新增 "doom" => "doom"
    |-- doom/
        |-- mod.rs      # run_game(): init_heap + doomgeneric_Create + Tick 循环
        |-- libc_shim.rs # 最小 libc (约 780 行)
        |-- platform.rs # DG_ 平台函数 + 键盘轮询
        |-- keymap.rs   # VirtIO 到 Doom 按键码映射
```

## 内存布局

```
0x80200000  内核 .text/.rodata/.data
0x80292000  内核堆起始
0x84A00000  内核堆结束 (MEMORY = 72 MiB)
  -- 间隙 --
0x85000000  DMA 池 (5 MiB, 恒等映射) -- VirtIO virtqueue + 事件缓冲区
0x85500000  DMA 池结束
0x90000000  QEMU RAM 结束 (256 MiB, -m 256M)

0x10000000  VirtIO MMIO 区域
0x10001000  VirtIO-blk
0x10007000  VirtIO-Input (键盘)
0x10008000  VirtIO-GPU

用户虚拟地址空间：
0x00010000  ELF 代码段 (约 750 KiB release)
0x000xxxxx  ELF 数据/BSS (约 130 KiB, 含 static 变量)
0x00140000  用户堆起始 (sbrk)
0x01140000  用户堆结束 (sbrk 16 MiB: zone 8M + WAD 5M + 开销)
  ...
VPN(1<<26)-32  用户栈 (128 KiB)
VPN::MAX       异界传送门
```

## 测试

- `cargo build`：通过（内核 + 用户程序 + C 交叉编译 + fs.img 打包 + WAD）
- `TG_SKIP_USER_APPS=1 cargo check`：通过
- `cargo run` + VNC 连接（localhost:5900）：
  - 约 5 秒完成 WAD 加载和引擎初始化
  - 标题画面正常显示，demo 自动回放运行
  - 3D 渲染正确（墙壁、地板、天花板、精灵）
  - 状态栏 HUD 正常显示（弹药、生命、武器）
  - 640x400 居中于 1280x800 帧缓冲区
  - 无声音（nosound 模式，Doom 正常运行无声音驱动）

## 设计决策

1. **C 交叉编译 vs Rust 重写**：doomgeneric 有约 15000 行 C 代码。重写为 Rust 工作量巨大且无教学价值。使用 `cc` crate 交叉编译 + FFI 是最务实的方案，也演示了 Rust/C 混合编程。

2. **libc shim 在 Rust vs C 中实现**：大部分 libc 函数（malloc/strlen/fopen 等）在 Rust 中实现，可以直接调用 rCore 系统调用。但 memcpy/memset 必须在 C 中实现（避免 Rust 内建函数递归），printf 系列也必须在 C 中实现（va_list 跨 FFI 不可靠）。

3. **WAD 内存缓存**：easy-fs 不支持 lseek，而 Doom 的 WAD 加载器大量使用 fseek。解决方案：首次 fopen 时整体读入内存，后续 fseek/fread/ftell 在内存 buffer 上操作。WAD 缓存避免了二次磁盘读取。

4. **release 模式用户程序**：debug 模式下 Doom 二进制约 5.8 MiB、栈使用超过 128 KiB；release 模式下约 750 KiB、栈使用小于 128 KiB。build.rs 中用户程序统一 `--release` 编译。

5. **`typedef int boolean`**：doomgeneric 上游 bug 的正确修复。与 chocolate-doom（维护版 Doom 源码港）保持一致。1 行改动，无副作用，修复所有 `boolean*` 到 `int*` 强转。

6. **volatile 键盘队列**：release 模式优化器会消除未使用 volatile 的 `static mut` 访问。这不是 Doom 特有问题——任何通过系统调用修改用户内存的 no_std 程序都可能遇到。`read_volatile`/`write_volatile` 是标准解决方案。

7. **MEMORY = 72 MiB（非 128 MiB）**：内核堆不能覆盖 DMA 池地址。DMA 池必须是专用的恒等映射区域，不被堆分配器使用。72 MiB 为 Doom 进程页表 + 用户堆 sbrk 提供充足空间。

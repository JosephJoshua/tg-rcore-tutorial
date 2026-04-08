# Ch1-Ch2 SMP 扩展报告：多核启动与同步原语

## 实现内容

### 目标

将第一章（裸机最小执行环境）和第二章（批处理系统）扩展为多核（SMP）版本。所有 4 个 RISC-V hart 从 M 态启动，经过自定义 SBI 完成特权级配置后进入 S 态。Ch1-SMP 演示多核输出竞争、自旋锁同步和并行计算加速；Ch2-SMP 展示多核启动基础设施在批处理操作系统中的应用。

### 架构设计

**整体方案**
- 新建三个独立 crate：`jsph-tg-rcore-tutorial-sbi-smp`（多核 SBI）、`jsph-tg-rcore-tutorial-ch1-smp`、`jsph-tg-rcore-tutorial-ch2-smp`
- 不修改任何原始 crate（`tg-rcore-tutorial-sbi`、`tg-rcore-tutorial-ch1`、`tg-rcore-tutorial-ch2` 等）
- QEMU 配置：`-bios none -smp 4`，所有 4 个 hart 同时从 `_m_start`（0x80000000）开始执行

**与原始单核版本的关键区别**

| 关注点 | 单核版本 | SMP 版本 |
|--------|---------|---------|
| M 态栈 | 单一 16 KiB 栈 | 4 × 4 KiB 独立栈，按 hartid 索引 |
| `mhartid` | 从不读取 | 入口第一条指令即读取，用于栈计算和 hart 路由 |
| `mscratch` | 指向唯一栈顶 | 每个 hart 指向各自的 M 态栈顶 |
| CLINT mtimecmp | 硬编码 0x2004000（hart 0） | `0x2004000 + 8 * hartid`（每 hart 独立） |
| S 态入口 | 单一栈，直接跳转 `rust_main` | 每 hart 独立栈，hart 0→`rust_main`，其余→`secondary_main` |
| Hart ID | 无 | 通过 `tp` 寄存器保存（`mv tp, a0`） |
| 溢出保护 | 无 | hartid >= 4 的 hart 在 M 态进入 WFI 停泊循环 |

### M 态多核启动序列（m_entry.asm）

```
所有 hart 同时到达 _m_start (0x80000000)
    │
    ├─ csrr t0, mhartid          # 识别自身
    ├─ bge t0, NUM_HARTS, .park   # 溢出保护
    ├─ 计算 per-hart 栈: sp = base + (id+1) * 4096
    ├─ csrw mscratch, sp          # 保存 M 态陷阱栈
    ├─ 配置 mstatus/mepc/mtvec/mideleg/medeleg/PMP/mcounteren
    ├─ csrr a0, mhartid           # 传递 hartid 给 S 态
    └─ mret → _start (S 态)      # 4 个 hart 各自独立进入 S 态
```

关键认知：每条 `csrw` 指令仅影响执行它的 hart。CSR 不是共享内存——每个 hart 拥有独立的 CSR 副本。因此所有 hart 可以并行配置自身，无需任何锁。

### S 态启动序列

```rust
_start:
    mv   tp, a0              // 立即保存 hartid（mret 后 a0 = hartid）
    // 计算 per-hart 栈
    sp = HART_STACKS + (hartid + 1) * STACK_SIZE
    // 分支
    if hartid == 0 → rust_main()
    else           → secondary_main()
```

**Hart 0**（`rust_main`）：
1. 清零 BSS（仅 ch2，ch1 无 `tg_linker`）
2. 初始化控制台
3. 初始化系统调用处理器（仅 ch2）
4. `BOOT_HART_DONE.store(true, Release)` — 通知二级 hart
5. 运行演示（ch1）或批处理循环（ch2）
6. 关机

**二级 hart**（`secondary_main`）：
1. 自旋等待 `BOOT_HART_DONE.load(Acquire)`
2. 打印标识消息（持有 `PRINT_LOCK`）
3. 参与演示（ch1）或进入 WFI 空闲循环（ch2）

### 同步原语（smp.rs）

**1. 启动标志（AtomicBool）**
- `Release` 存储与 `Acquire` 加载配对
- 确保 hart 0 的所有初始化（BSS 清零、控制台初始化）对二级 hart 可见
- 在 RISC-V RVWMO 内存模型下编译为 `fence` 指令

**2. 自旋锁（TTAS — Test-and-Test-and-Set）**
```rust
pub fn lock(&self) {
    loop {
        // 内层循环：Relaxed load 仅读缓存行，无总线争用
        while self.locked.load(Relaxed) { spin_loop(); }
        // 外层：仅在锁看似空闲时才尝试原子 RMW
        if self.locked.compare_exchange_weak(false, true, Acquire, Relaxed).is_ok() {
            return;
        }
    }
}
```
为何 TTAS 优于朴素 `compare_exchange` 循环：朴素循环每次迭代都执行原子 RMW（在 RISC-V 上为 `amoswap` 或 LR/SC 序列），每次都会请求总线独占访问权。内层 `load(Relaxed)` 从本地缓存读取，不产生总线事务，直到锁释放时缓存行失效才重试原子操作。

**3. 屏障（generation-based Barrier）**
- 计数器 + 代际号，防止快 hart 在慢 hart 退出前重新进入屏障的 ABA 竞争
- 最后到达的 hart 重置计数并推进代际号
- 先到达的 hart 自旋等待代际号变化

**4. 动态 hart 计数**
- `ACTIVE_HARTS: AtomicUsize` — 每个启动的 hart 递增
- Hart 0 等待 10ms（rdtime）后读取最终计数，发布到 `DEMO_HART_COUNT`
- 屏障使用实际活跃 hart 数而非编译时常量 `NUM_HARTS`
- 这使得 `-smp 1` 到 `-smp 8` 均可正确运行

### Ch1-SMP 演示

**Demo 1：无锁输出**
所有 hart 同时打印多行消息，不使用锁。由于 SBI `console_putchar` 逐字节输出，不同 hart 的字符在字节级交错：

```
[H[[HaHr[artaHtr a tr t 0]1 32H)e]]   HHHleeelllo llofllor  ooffr ormofm rhma  hrhaoatr 1t !
```

教学意义：`println!` 分解为 N 次独立的 `console_putchar()` SBI 调用。在任意两次 SBI 调用之间，另一个 hart 都可以插入自己的输出。

**Demo 2：自旋锁同步输出**
相同消息，但每个 hart 在打印前获取 `PRINT_LOCK`。输出干净有序：

```
[Hart 0] Hello from hart 0!
[Hart 0] This is line 2 from hart 0.
[Hart 0] This is line 3 from hart 0.
[Hart 2] Hello from hart 2!
...
```

Hart 顺序在不同运行间可能变化——这本身是非确定性的教学点。

**Demo 3：并行计算加速**
- 每个 hart 计算 `sum(1..5_000_000)`，使用 `black_box()` 防止优化器消除
- Hart 0 先顺序执行 4 份工作负载（测量基线），再所有 hart 并行执行
- 使用 `rdtime`（QEMU virt 定时器 10 MHz）测量墙钟时间

### Ch2-SMP 设计

- 所有 4 个 hart 通过 M 态启动→S 态
- Hart 0：BSS 清零→控制台初始化→系统调用注册→信号 BOOT_DONE→批处理循环（原有 ch2 逻辑不变）
- 二级 hart：等待 BOOT_DONE→打印在线消息→设置 `stvec` 为 panic 处理器→WFI 循环
- 所有用户程序仅在 hart 0 运行——输出与原始 ch2 完全一致

## 遇到的问题

### Bug 1：BSS 竞争——zero_bss() 覆盖 hart 栈

**症状**：Ch2-SMP 启动后无任何输出，QEMU 直到超时才退出。`-smp 1` 同样无输出。

**根因**：`HART_STACKS` 最初放置在 `.bss.uninit` 段。链接脚本将所有 `.bss*` 段归入 `__sbss` 到 `__ebss` 范围。`tg_linker::KernelLayout::locate().zero_bss()` 将该范围全部清零。

时序问题：
1. 所有 hart 到达 `_start`，各自设置 `sp` 指向 `HART_STACKS` 中的栈
2. Hart 0 调用 `zero_bss()`，将包含 `HART_STACKS` 的 `.bss` 段全部清零
3. 所有 hart 的栈被破坏（包括 hart 0 自己的返回地址和局部变量）
4. 程序行为未定义——通常表现为静默挂起

`objdump -t` 确认 `HART_STACKS` 位于 `0x802d4f20`（即 `__sbss`）。

**修复**：将 `HART_STACKS` 的段从 `.bss.uninit` 改为 `.boot.stack`。链接脚本将 `.boot` 段置于 `.bss` 之后（`__ebss` 之后），不在 `zero_bss()` 的清零范围内。

```rust
// 修复前
#[unsafe(link_section = ".bss.uninit")]
static mut HART_STACKS: ...

// 修复后
#[unsafe(link_section = ".boot.stack")]
static mut HART_STACKS: ...
```

注意：Ch1-SMP 不受影响，因为 ch1 没有 `tg_linker` 依赖，不调用 `zero_bss()`。QEMU 启动时 RAM 全零，`.bss.uninit` 中的栈天然为零。

### Bug 2：屏障死锁——smp 数量不足时 hart 不够

**症状**：Ch1-SMP 使用 `-smp 1` 运行时，打印 Demo 1 标题后永久挂起。

**根因**：`DEMO_BARRIER.wait(NUM_HARTS)` 等待 4 个 hart 到达，但 `-smp 1` 只有 1 个 hart。其余 3 个永远不会到达，屏障死锁。

**修复**：引入动态 hart 计数机制：
1. `ACTIVE_HARTS: AtomicUsize` — 每个启动的 hart 执行 `fetch_add(1)`
2. Hart 0 设置 `BOOT_HART_DONE` 后等待 10ms（100,000 rdtime ticks），让二级 hart 有时间注册
3. Hart 0 读取 `ACTIVE_HARTS` 最终值，发布到 `DEMO_HART_COUNT`
4. 所有 hart 使用 `DEMO_HART_COUNT`（而非编译时常量 `NUM_HARTS`）作为屏障宽度

### Bug 3：Hart 0 打印与二级 hart 竞争（ch2）

**症状**：Ch2-SMP 测试脚本偶发失败，`grep "Hart 2"` 找不到对应输出。

**根因**：Hart 0 的 `println!("[Hart 0] primary, running batch processing")` 未持有 `PRINT_LOCK`，与同时获得锁的二级 hart 交错输出，导致 `"Hart 0"` 和 `"Hart 2"` 等字符串在字节级被打断。

**修复**：Hart 0 在发送 `BOOT_HART_DONE` 信号之前打印标识消息。此时二级 hart 仍在自旋等待，不会竞争 UART。

## 文件结构

```
jsph-tg-rcore-tutorial-sbi-smp/
├── Cargo.toml          # name: jsph-tg-rcore-tutorial-sbi-smp
├── src/
│   ├── lib.rs          # SBI 调用封装（不变）
│   ├── m_entry.asm     # 多核 M 态入口：mhartid/per-hart 栈/溢出保护/传递 hartid
│   └── msbi.rs         # M 态陷阱处理器：per-hart CLINT 定时器地址

jsph-tg-rcore-tutorial-ch1-smp/
├── Cargo.toml          # tg-sbi → 本地 SBI-SMP
├── .cargo/config.toml  # 新增 -smp 4
├── test.sh             # 检查 4 个 hart 输出 + Demo 2 存在
├── src/
│   ├── main.rs         # 多核 _start、rust_main、secondary_main、三个 Demo
│   ├── smp.rs          # SpinLock/Barrier/hart_id()/BOOT_HART_DONE/ACTIVE_HARTS
│   ├── allocator.rs    # VirtIO GPU 分配器（不变，hart 0 专用）
│   ├── gpu.rs          # VirtIO GPU 驱动（不变）
│   └── tangram.rs      # Tangram 渲染（不变）

jsph-tg-rcore-tutorial-ch2-smp/
├── Cargo.toml          # tg-sbi → 本地 SBI-SMP，其余依赖改为 version-only
├── .cargo/config.toml  # 新增 -smp 4，TG_USER_DIR 指向共享 user crate
├── build.rs            # 不变（链接脚本 + 用户程序构建）
├── test.sh             # 检查 4 个 hart + 用户程序输出
├── src/
│   ├── main.rs         # 多核 _start、hart 0 批处理、secondary WFI
│   └── smp.rs          # SpinLock/hart_id()/BOOT_HART_DONE（无 Barrier）
```

## 内存布局

### M 态（0x80000000 起）

```
0x80000000  .text.m_entry   M 态入口代码（_m_start，128 字节）
0x80000080  .text.m_trap    M 态陷阱向量（m_trap_vector，~80 字节）
0x800000ce  .bss.m_stack    Per-hart M 态栈（4 KiB × 4 = 16 KiB）
              hart 0 栈: [base, base+4096)
              hart 1 栈: [base+4096, base+8192)
              hart 2 栈: [base+8192, base+12288)
              hart 3 栈: [base+12288, base+16384)
```

### S 态（0x80200000 起）

**Ch1-SMP**:
```
0x80200000  .text.entry     _start（多核入口）
0x80200000  .text           内核代码
0x8020c000  .rodata         只读数据
0x80210000  .bss            BSS（含 HART_STACKS: 64 KiB × 4 = 256 KiB）
              注：ch1 无 zero_bss()，HART_STACKS 在 .bss 中安全
0x80260000  (约)            静态分配器（5 MiB，VirtIO GPU DMA 用）
```

**Ch2-SMP**:
```
0x80200000  .text.entry     _start（多核入口）
0x80200000  .text           内核代码（~44 KiB）
0x8020c000  .rodata         只读数据
0x80210000  .data           数据段（含嵌入的用户程序二进制，~800 KiB）
0x802d4f20  .bss            BSS（__sbss → __ebss，由 zero_bss() 清零）
0x802f6000  .boot.stack     HART_STACKS（32 KiB × 4 = 128 KiB）
              ⚠ 必须在 __ebss 之后，不被 zero_bss() 覆盖
0x80400000  用户程序加载地址
```

## 测试结果

### Ch1-SMP

| 场景 | 结果 |
|------|------|
| `-smp 4` | 4 个 hart 全部启动，Demo 1 输出交错，Demo 2 输出有序，Demo 3 加速 3.6-4.9x |
| `-smp 1` | 单 hart 正常运行，"加速" 0.8x（预期中的开销） |
| `-smp 8` | hart 0-3 正常运行，hart 4-7 在 M 态停泊（WFI） |
| `cargo check`（主机） | 通过 |
| `test.sh` | PASSED |

**Demo 3 并行加速数据**（多次运行）：

| `-smp` | 顺序耗时 | 并行耗时 | 加速比 |
|--------|---------|---------|--------|
| 1 | 1915ms | 2250ms | 0.8x |
| 4 | 9180ms | 2495ms | 3.6x |
| 4 | 6501ms | 1491ms | 4.3x |
| 8 | 7717ms | 1983ms | 3.8x |

加速比变动来自 QEMU 的主机侧调度。`-smp 8` 实际仅 4 个 hart 工作（其余停泊），与 `-smp 4` 一致。

### Ch2-SMP

| 场景 | 结果 |
|------|------|
| `-smp 4` | 4 个 hart 启动，22 个用户程序全部正常运行（输出与原版 ch2 一致） |
| `-smp 1` | 单 hart 正常运行批处理，无二级 hart 消息 |
| `cargo check`（主机） | 通过 |
| `test.sh` | PASSED |

Ch2-SMP 二级 hart 输出示例：
```
[Hart 0] primary, running batch processing
[Hart 2] online, entering idle loop
[Hart 1] online, entering idle loop
[Hart 3] online, entering idle loop
```

## 设计决策

1. **完全 M 态多核启动 vs OpenSBI 代启动**：使用 `-bios none` + 自定义 SBI，所有 hart 从 `_m_start` 冷启动。这比依赖 OpenSBI 的 hart 启动协议（HSM 扩展）更能揭示多核启动的底层细节——学生可以在反汇编中看到每个 hart 独立配置 CSR 的过程。

2. **TTAS 自旋锁 vs 朴素 CAS 循环**：选择 test-and-test-and-set 而非朴素 `compare_exchange` 循环，是因为在多核系统上朴素循环会在每次迭代中产生总线级原子 RMW 操作（RISC-V 上的 LR/SC 或 AMO），造成缓存行在核间"乒乓"。TTAS 的内层 `load(Relaxed)` 仅读取本地缓存副本，显著降低总线争用。这不是过度工程——它是标准的自旋锁最佳实践。

3. **Generation-based Barrier vs 简单计数屏障**：简单计数屏障有 ABA 问题——快 hart 在慢 hart 退出前重入屏障，计数器被错误递增。代际号解决这个问题：每个 hart 记录进入时的代际号，只在代际号变化后才认为屏障通过。我们的 Demo 序列中多次重用同一屏障，必须使用此设计。

4. **动态 hart 计数 vs 编译时常量**：`NUM_HARTS = 4` 作为编译时常量用于栈数组大小，但屏障宽度使用运行时发现的 `ACTIVE_HARTS`。这使得程序在 `-smp 1` 到 `-smp 8` 的范围内均可正确运行，符合教学场景中的灵活演示需求。

5. **Ch2 HART_STACKS 放 `.boot.stack` 而非 `.bss`**：ch2 的 `zero_bss()` 会清零整个 BSS 段。如果栈在 BSS 中，hart 0 清零 BSS 时会破坏所有 hart（包括自己）的栈。`.boot.stack` 在链接脚本中位于 BSS 之后，不受 `zero_bss()` 影响。这是多核启动中最常见的坑之一。

6. **`tp` 寄存器保存 hart ID**：RISC-V ABI 中 `tp`（x4）是线程指针寄存器。在 `no_std` 无 TLS 的裸机环境下，Rust 编译器不会生成写 `tp` 的代码。我们在 `_start` 的第一条指令 `mv tp, a0` 中保存 hartid，之后随时可通过 `hart_id()` 读取。通过反汇编验证编译器确实不触碰 `tp`。

7. **Ch2 二级 hart 的 stvec 设置**：二级 hart 不执行用户代码，但仍设置 `stvec` 指向一个 panic 处理器（WFI 循环）。这是防御性编程——如果因硬件或软件错误导致二级 hart 收到意外中断（如定时器中断），至少不会导致未定义行为或破坏 hart 0 的执行。

## 关于后续章节的多核调度预览

当前实现的 ch1-smp 和 ch2-smp 只展示了"多核启动"——所有 hart 独立启动并运行固定代码路径。真正的多核操作系统还需要：

- **多核调度器**（ch3+）：每个 hart 运行自己的调度循环，从共享就绪队列中取任务。就绪队列需要锁保护或无锁设计。
- **核间中断（IPI）**：一个 hart 需要唤醒另一个处于 WFI 的 hart 时，通过 CLINT 发送软件中断。
- **Per-hart 内核栈**：每个 hart 处理陷阱时使用独立的内核栈（当前 ch2-smp 中二级 hart 不处理陷阱，规避了此问题）。
- **页表共享与 TLB 一致性**：多个 hart 共享同一地址空间时，修改页表需要 `sfence.vma` 同步所有 hart 的 TLB。
- **锁竞争优化**：朴素自旋锁在高竞争下浪费 CPU。更高级的方案包括 ticket lock、MCS lock、或基于 WFI + IPI 的休眠锁。

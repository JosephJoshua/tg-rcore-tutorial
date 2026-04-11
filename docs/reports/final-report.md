# 与 AI 合作的任务 1-3 实现过程与学习效果总结报告

## 第一部分：与 AI 合作的实现过程

### 任务 1：基础实验 ch3 - ch8

任务 1 包含 5 个实验（ch3、ch4、ch5、ch6、ch8），分别涉及系统调用追踪、虚拟内存映射、进程创建与调度、文件系统硬链接、死锁检测。这些实验我之前自己手写完成过一遍，这次用 AI 重做同一套题来对比。

给 AI 的 Prompt（一条指令完成全部 5 章）：

```
Complete all 5 basic OS kernel exercises in this repository. Work through them in order: ch3, ch4, ch5, ch6, ch8.

For each chapter:
1. Read `tg-rcore-tutorial-chN/exercise.md` for the full specification
2. Read the existing source code in `tg-rcore-tutorial-chN/src/` before making changes
3. Implement the solution
4. Test with `cd tg-rcore-tutorial-chN && bash test.sh exercise`
5. Also run `bash test.sh base` to verify base tests still pass

After completing each chapter, briefly document what you implemented, any bugs you encountered, and how you resolved them.
```

AI 先读了全部 exercise.md 和相关源代码，生成实现计划。ch3 和 ch4 在主会话中实现，ch5、ch6、ch8 分别委托子 agent 并行完成。每章都跑了 `test.sh exercise` 和 `test.sh base`，全部通过，总耗时不到一小时。

AI 碰到的 bug 如下表：

| 章节 | Bug | 原因 | 解决 |
|------|-----|------|------|
| ch3 | 启动后崩溃 | `[u32; 500]` 放入栈上 TCB 数组，32 个 TCB 多占 64KB 栈空间 | 改为 BSS 段全局数组 |
| ch4 | 首个用户程序 StorePageFault | 同样的大数组放入堆上 Process 结构体，24MB RAM 下堆压力过大 | 改为全局数组 |
| ch4 | `trace_read(0x80200000)` 应返回失败却成功 | translate 权限检查仅用 `"RV"`，内核页面也有 R+V 标志 | 改为 `"U__RV"` 检查 User 位 |
| ch6 | easy-fs 私有字段编译失败 | `inode_area_start_block` 和 `FileSystem::root` 为 private | 在本地克隆中改为 pub |
| ch8 | 信号量创建后 down 导致 panic | 跟踪向量 `sem_available`/`sem_alloc` 未随 semaphore_create 扩展 | 在 create 中同步扩展 |
| ch8 | 银行家算法安全性误判 | semaphore_up 唤醒线程后未更新其分配计数 | 在 up/unlock 中同步更新被唤醒线程 |

这些 bug 有一个共同特征：AI 有能力写出实现，但缺乏工程判断。比如不考虑内存布局就往结构体里放大数组，不理解 RISC-V 页表权限位的含义。修复过程是靠测试反馈迭代的，不是从对底层机制的理解出发去推导。

测试全部通过后我人工审查了代码，发现两个设计问题。

ch3 的系统调用计数绕过了框架的封装。exercise.md 的 HINT 建议在 `TaskControlBlock::handle_syscall()` 内统计调用次数，AI 没有遵循，把计数逻辑放在 main.rs 的主循环中 handle_syscall 调用之外，新增了 `ctx_a7()` 访问器暴露 TCB 内部状态，还引入了一个 `CURRENT_TASK_IDX` 全局变量。我向 AI 描述了这个问题，它随后把计数移入 handle_syscall 内部，通过参数传递任务索引，经 `Caller.entity` 在 Trace trait 实现中访问。

ch8 的 `condvar_wait` 没有同步互斥锁跟踪状态。condvar_wait 内部先释放互斥锁再阻塞当前线程，但 AI 的实现没有相应更新 `mutex_holder` 和 `mutex_wait` 跟踪表。这导致已经被 condvar 释放的锁在跟踪中仍显示为"被持有"，后续如果触发涉及该锁的死锁检测就会基于过期数据做判断。测试用例没有涉及 condvar 和死锁检测的交叉场景，所以没暴露出来。

这两个问题的共同点是测试全部通过但实现存在缺陷。

---

### 任务 2：多核支持 ch1 - ch5

任务 2 将 ch1 到 ch5 扩展为 SMP 多核版本，4 个 RISC-V hart 同时启动运行。分两批实现：ch1-ch2 完成多核启动基础设施和同步原语，ch3-ch5 在此基础上实现多核调度与进程管理。

给 AI 的 Prompt（整理版）：

ch1-ch2 阶段要求将 ch1 和 ch2 扩展成多核版本，所有 hart 从 M 态冷启动经过自定义 SBI 进入 S 态，ch1 做三个演示（无锁输出竞争、自旋锁同步输出、屏障同步并行计算），ch2 让二级 hart 启动后进 WFI 空闲循环。同时要求编写面向学过 rCore 但没接触过多核的学生的教程文档。

ch3-ch5 阶段要求基于 ch1-ch2 的多核启动基础设施，让 ch3 到 ch5 支持真正的多核调度，每个 hart 运行独立的调度循环从共享就绪队列取任务。

总共新建了 7 个 crate：

| Crate | 功能 |
|-------|------|
| sbi-smp | 多核 M 态入口，per-hart 栈、CSR 配置、hartid 传递 |
| ch1-smp | 三个演示：无锁竞争、TTAS 自旋锁、屏障同步并行计算 |
| ch2-smp | 多核启动 + 原始 ch2 批处理逻辑 |
| kernel-alloc-smp | buddy allocator 内部加中断禁用自旋锁 |
| ch3-smp | 全局共享队列，lock-pop-unlock-execute 调度模式 |
| ch4-smp | Sv39 页表环境下的多核调度，每个 hart 独立维护 satp |
| ch5-smp | 自定义 SmpProcManager 替换 PManager，per-hart current 跟踪 |

调度的核心设计是 lock-pop-unlock-execute：锁只保护队列元数据的 pop/push 操作，任务执行在锁外完成。这使得每个 hart 大部分时间都在执行用户代码而不互相干扰。

遇到的主要问题：

第一个是 BSS 竞争。`HART_STACKS` 放在 `.bss.uninit` 段，`zero_bss()` 清零整个 BSS 时把所有 hart 的栈一起清掉了，包括 hart 0 自己正在使用的栈。改到 `.boot.stack` 段解决，该段在链接脚本中位于 BSS 之后。ch1 没触发此问题是因为 ch1 不依赖 `tg_linker`、不调用 `zero_bss()`。

第二个是自旋锁在定时器中断下死锁。Hart 持有锁时定时器中断触发，中断处理器尝试获取同一把锁。用 SpinLockIrq 解决，获取前禁用 sstatus.SIE。释放时先释放锁再恢复中断，顺序不能反过来，否则恢复中断的瞬间定时器可能立刻触发而锁尚未释放，再次死锁。这和 Linux 的 `spin_lock_irqsave` 原理一致。

第三个是空闲 hart 的惊群效应。ch5 的 forktest 创建 30 个子进程，实现了阻塞 wait（父进程等不到死子进程就从就绪队列移除，子进程退出时把父进程塞回去）之后，4 核模式反而比 1 核慢。原因是就绪队列经常为空时 3 个空闲 hart 不断争抢全局锁，挤占正在执行任务的 hart 获取锁的机会。用 WFI 替换 spin_loop() 之后，空闲 hart 休眠到下一个定时器中断再醒来，锁争用从每秒数百万次降为零。

第四个是 PManager 的单值 current 字段。原始 PManager 只有一个 `current: Option<ProcId>`，4 个 hart 各自 find_next() 时会互相覆盖。find_next 内部直接设置 current，无法简单地在外层加锁解决，所以另写了 SmpProcManager，不包含 current 字段，改用 per-hart 的原子数组 `CURRENT_PROCESS[hart_id()]` 跟踪当前进程。

第五个是内存分配器非线程安全。原始 `tg-kernel-alloc` 的 buddy allocator 没有内部锁，两个 hart 同时调用 alloc() 会损坏空闲链表。最初在每个分配调用点手动加锁，但容易遗漏。最终复制了 kernel-alloc crate，在 `GlobalAlloc::alloc/dealloc` 实现内部加入 SpinLockIrq，使得所有堆分配（BTreeMap::insert、VecDeque::push_back、page_alloc 等）自动受锁保护，调用方无需额外处理同步。

ch1-smp 的并行加速数据：每个 hart 计算 `sum(1..5_000_000)`，用 `black_box()` 防止优化器消除。

| -smp | 顺序耗时 | 并行耗时 | 加速比 |
|------|---------|---------|--------|
| 1 | 1915ms | 2250ms | 0.8x |
| 4 | 9180ms | 2495ms | 3.6x |
| 4 | 6501ms | 1491ms | 4.3x |

加速比波动来自 QEMU 宿主机调度。理论上限 4x，实测 3.6x-4.3x，基本符合预期。

---

### 任务 3：图形游戏 ch1 - ch8

任务 3 为每个章节实现一个 VirtIO-GPU 图形游戏，从 ch1 的静态图案到 ch8 的 Doom 移植。每个游戏利用对应章节新引入的内核功能。

每个章节的 prompt 结构类似：描述目标游戏和交互方式，指定使用 VirtIO-GPU 帧缓冲区和 VirtIO 键盘，要求与该章节的标准测试程序并发运行。各章具体内容为：ch1 在裸机程序中用 VirtIO-GPU 显示七巧板 "OS" 图案；ch2 由 14 个用户程序各渲染一块七巧板，批处理系统依次执行形成逐帧动画；ch3 多道程序环境下的贪吃蛇，与标准测试并发运行；ch4 虚拟内存环境下的俄罗斯方块，所有用户指针需通过页表翻译；ch5 进程管理环境下的双人乒乓球，fork 出三个进程通过共享内存通信；ch6 文件系统环境下的打砖块，通过 easy-fs 实现 F5/F9 存档读档；ch7 管道和信号环境下的吃豆人，4 个幽灵 AI 子进程通过管道锁步、信号广播能量豆事件；ch8 将 doomgeneric C 引擎通过 FFI 链接到 Rust 平台层，实现 Doom 移植。

各章节对内核特性的利用关系如下表：

| 章节 | 游戏 | 利用的内核特性 |
|------|------|-------------|
| ch1 | 七巧板 OS | VirtIO-GPU 帧缓冲区，裸机 MMIO |
| ch2 | 七巧板逐帧动画 | 批处理执行，自定义系统调用 |
| ch3 | 贪吃蛇 | 时间片轮转，VirtIO 键盘，非阻塞输入 |
| ch4 | 俄罗斯方块 | Sv39 虚拟内存，逐行地址翻译 |
| ch5 | 双人乒乓球 | fork、共享内存 IPC、按键按住状态跟踪 |
| ch6 | 打砖块 | easy-fs 文件读写，存档/读档 |
| ch7 | 吃豆人 | 管道 IPC，信号处理，多进程 AI |
| ch8 | Doom | sbrk 堆扩展，C FFI，WAD 文件加载 |

8 个章节碰到的问题很多，以下选取几个有代表性的。

ch3 的 SBI console_getchar 阻塞导致游戏冻结。`tg_sbi::console_getchar()` 在 M 态死循环轮询 UART 直到收到字符，游戏的 drain_input 一调用就把整个内核挂住。最初绕过 SBI 直接读 16550 UART 的 LSR 和 RBR 寄存器实现非阻塞读取，后来改用 VirtIO 键盘的 `pop_pending_event()`，该接口本身就是非阻塞的。

ch2 的 DMA 池与用户程序内存重叠。5 MiB 的静态 bump 分配器位于 BSS 段（0x80311000 起），用户程序加载地址为 0x80400000，两个区域重叠。内核向 0x80400000 复制用户二进制时覆盖了 VirtIO DMA 缓冲区，后续 GPU flush 挂住。把 DMA 池移到固定地址 0x81000000 解决。

ch5 的 virtio-drivers 0.1.0 键盘输入队列耗尽。按键正常工作约 3 秒后完全停止响应。定位到 `VirtIOInput::pop_pending_event()` 在回收事件缓冲区到可用环后遗漏了 `transport.notify()` 调用，这是该 crate 中唯一遗漏此调用的驱动（blk/net/gpu/console 均正确调用）。VirtIO 输入设备初始有 32 个缓冲区，每次按键生成约 4 个事件（按下、同步、释放、同步），约 8 次按键后缓冲区耗尽，设备不再交付事件。升级到 v0.3.0 解决。

ch8 的 release 模式优化器消除键盘队列逻辑。Doom 的 `DG_GetKey` 反汇编显示函数直接返回 0，环形缓冲区的 head/tail 比较和数据读取全部被消除。原因是 `read(STDIN)` 对应的 ecall 指令没有 memory clobber，编译器认定 buf 在调用后仍为初始值 0，由此推导 poll_keyboard() 永远不写入队列，进而推导 head 恒等于 tail，整个队列逻辑被判定为死代码并消除。在三处关键位置使用 `read_volatile`/`write_volatile` 修复。这个问题并非 Doom 特有，任何在 no_std 环境下通过系统调用修改用户内存的程序都可能遇到。后续给 tg-rcore-tutorial-user 的系统调用缓冲区也补上了 read_volatile（commit 181b0d4）。

ch8 的 doomgeneric boolean 类型大小不匹配。`doomtype.h` 将 `boolean` 定义为 `unsigned char`（1 字节），但 `st_stuff.c` 中有 `(int *)&plyr->weaponowned[i+1]` 强制类型转换。`weaponowned` 是 `boolean[]`，按 int（4 字节）解引用时会把相邻元素一并读入。玩家同时持有手枪和霰弹枪时读到 257，作为数组下标越界，最终拼出野指针 `0x2b00000030`（其中 `0x30` 来自某 lump 的 filepos，`0x2b` 来自相邻 lump 的 size 字段）触发 LoadPageFault。该 bug 存在于所有平台上，但只有裸机环境（没有虚拟内存保护页兜底）才必现崩溃。改为 `typedef int boolean`（与 chocolate-doom 保持一致）即可，一行改动。

ch4 的 customizable-buddy 空指针。ch4 引入 Sv39 虚拟内存后用户程序链接在低虚拟地址 0x10000，共享用户 crate 的 512 KiB 堆缓冲区位于 BSS 段（起始 ~0x1ab70），跨越 0x20000 边界。`deallocate` 在 0x20000 处产生 order-17 的块（idx=1），计算 buddy 序号 `idx ^ 1 = 0`，`Order::idx_to_ptr(0)` 通过 `NonNull::new_unchecked(0)` 生成空指针，debug 模式下触发 panic，release 模式下导致未定义行为。ch3 不触发是因为用户程序加载在高物理地址（0x80400000+），buddy 序号远大于 0。此 bug 来自 customizable-buddy crate 本身。已向上游 YdrMaster/buddy-allocator 提交修复 PR 并被合并（#3）。修复方案为 `idx_to_ptr` 返回 `Option<NonNull<T>>`，buddy 为 None 时跳过合并直接插入当前链表。同时移除了 AvlBuddy 中一个写入后从未读取的无用 `base` 字段。

关于 Doom 移植。Doom 和前 7 个游戏的性质完全不同。前面全部是纯 Rust 用户态程序，Doom 是约 15000 行 C 代码的 doomgeneric 引擎，通过 `cc` crate 交叉编译为 riscv64gc 静态库再与 Rust 链接。需要实现约 780 行的最小 libc shim，涵盖 malloc/free、字符串操作、文件 I/O、字符分类、qsort 等。printf 系列函数必须在 C 中实现（va_list 跨 FFI 边界不可靠），memcpy/memset 同样必须在 C 中实现（Rust 的 `core::ptr::copy_nonoverlapping` 在 no_std 下会被编译器降级为对 memcpy 的调用，导致无限递归）。

WAD 文件（约 4 MiB）在构建时打包进 easy-fs 镜像，运行时整体加载到内存。easy-fs 不支持 lseek，但 Doom 的 WAD 加载器大量使用 fseek，因此做了内存缓存：首次 fopen 时将文件整体读入内存，后续 fseek/fread/ftell 在 buffer 上操作。

最终通过 QEMU VNC 连接可以看到标题画面、demo 回放、完整的 3D 渲染和状态栏 HUD。无声音输出（nosound 模式，Doom 引擎在缺少声音驱动时可正常运行）。

---

## 第二部分：学习效果评估

### 知识和能力的变化

三个任务覆盖的系统层次从 M 态启动一直到用户态 C 程序的 FFI 链接，期间对操作系统各子系统的理解都有不同程度的加深。

在虚拟内存方面，ch4 的 translate 权限检查问题促使我仔细追查了 Sv39 页表项的 flags 语义。Sv39 的 PTE 中 R、W、X、U、V 各位独立控制权限，translate 的检查逻辑是"请求的所有 flags 必须全部存在于 PTE 中"。内核页面设了 R+V 但没有 U 位，所以仅检查 `"RV"` 的话内核地址也能通过，必须加上 U 位才能正确隔离用户态和内核态的地址空间。这个细节以前自己做的时候只是改到测试通过就没再深入，审查 AI 代码时才真正理解 U 位在页表权限模型中的作用：它不是简单的"用户可访问"标记，而是内核在 translate 时区分自身页面和用户页面的关键依据。

在进程同步方面，ch8 的死锁检测实现让我理解了互斥锁和信号量在死锁检测上的本质区别。互斥锁是二值资源，持有者唯一，用等待图做环路检测即可：线程 T 请求锁 M，M 被 T' 持有，追踪 T' 在等什么锁、那把锁又被谁持有，链路回到 T 就是死锁。信号量的计数可以大于 1，不存在唯一持有者，必须用银行家算法构建分配/需求矩阵来做安全性检查。AI 选择对两种同步原语分别使用不同检测策略这个设计决策本身是合理的。我审查出的 condvar_wait 跟踪遗漏则涉及另一个层面的问题：condvar_wait 的语义是"释放互斥锁并阻塞，被唤醒后重新获取互斥锁"，中间有一段互斥锁处于无主状态。如果跟踪表在这个窗口内仍然记录旧持有者，后续任何涉及该锁的死锁检测都会基于错误的前提做判断。这个问题说明死锁检测的正确性不仅取决于检测算法本身，还取决于影子状态是否在所有状态转换点上与真实状态保持同步。

在多核同步方面，任务 2 是收获最大的部分。粗粒度锁的调度器设计本身不复杂（lock-pop-unlock-execute），但实际跑在 4 个 hart 上之后暴露出的问题全都不是从代码逻辑上能直接看出来的。中断-锁死锁的根本原因在于 RISC-V 的中断处理是在当前 hart 的当前特权级上执行的，如果 S 态中断处理器和被中断的代码需要同一把锁，就必须在获取锁之前关闭 sstatus.SIE。SpinLockIrq 的 drop 实现先 store(false, Release) 释放锁再恢复 SIE，是因为如果先恢复 SIE，在恢复和释放之间的指令窗口内定时器中断到达就会造成同样的死锁。这和 Linux 内核的 `spin_lock_irqsave`/`spin_unlock_irqrestore` 完全是同一个设计原则。

惊群效应的调试过程让我理解了锁争用对吞吐量的非线性影响。forktest 中大部分子进程处于阻塞状态，就绪队列经常为空，3 个空闲 hart 以 CPU 频率不断获取释放全局锁。这不仅浪费了空闲 hart 的 CPU 时间，更关键的是每次原子 RMW 操作（RISC-V 上为 LR/SC 或 AMO 指令）都会请求对锁所在缓存行的独占访问，导致正在执行用户代码的 hart 在每次需要重新入队时都要等待缓存一致性协议的仲裁。用 WFI 替换 spin_loop() 后空闲 hart 不再参与锁争用，ch5 forktest 在 4 核上从超时（10 分钟以上）降到不到 1 分钟。这也是为什么 Linux 和 ArceOS 都使用 per-CPU 运行队列而不是全局队列的原因：全局队列在 4 核时通过优化还能接受，但核数增多后争用会成为瓶颈。

在编译器优化与硬件语义的交互方面，ch8 的 volatile 问题本质上是编译器的抽象机模型和实际硬件行为之间的差异。Rust/LLVM 的优化器按照抽象机语义推导：ecall 是一条没有 memory clobber 的内联汇编指令（在 tg-rcore-tutorial-syscall 的实现中，asm! 块的 clobber 列表只包含寄存器不包含 memory），所以优化器有权认为 ecall 不会修改栈上的局部变量。但实际上 ecall 触发了特权级切换，内核通过进程页表翻译用户态虚拟地址后直接写入了那块物理内存。这是一个用户态代码和内核之间的隐式数据通道，对编译器完全不可见。`read_volatile` 的作用是告诉编译器"这块内存可能被外部修改过，不要依赖之前的值"，从而阻止基于值不变假设的优化推导。这个问题不限于 Doom，tg-rcore-tutorial-syscall 本身的设计就存在这个缺陷，所有 release 模式编译的用户程序都可能受影响。

customizable-buddy 的空指针问题涉及伙伴分配器的地址计算。伙伴分配器将管理的地址空间按二叉树组织，order-n 的块在树中有一个序号 idx，其伙伴序号为 `idx ^ 1`。当 idx=1 时伙伴序号为 0，而 `idx_to_ptr` 的实现是将 idx 乘以块大小再加上基地址转化为指针，idx=0 的结果就是基地址的前一个块的位置。原实现使用 `NonNull::new_unchecked(0)` 跳过空指针检查直接构造指针。这种情况只在管理区域的起始地址低于某个 2^n 边界且该区域跨越这个边界时才会触发，因为只有这种布局才会在 transfer 过程中产生 idx=1 的块。rCore 用户程序链接在 0x10000，512 KiB 的静态堆跨越 0x20000 边界，正好满足触发条件。ch3 不触发是因为用户程序直接加载到 0x80400000 以上的高物理地址，相关 order 的 idx 远大于 1，不会算出 buddy=0。

### 两次做法的定量对比（任务 1）

| 维度 | 自己手写（第一次） | AI 辅助（本次） |
|------|-------------------|----------------|
| 总耗时 | 约一周（断续） | AI 写码不到一小时 + 审查修复约两小时 |
| 被卡最久的环节 | ch8 银行家算法实现 | ch4 内存布局崩溃（AI 自行修复，约 10 分钟） |
| 发现的问题类型 | 编译错误和运行时 crash | 设计层面（封装、状态同步） |

AI 辅助的主要优势在速度。ch5 和 ch6 的前向兼容移植（把 mmap/munmap/spawn/stride 从上一章搬过来）是机械劳动，API 差异多但没有新知识。ch8 的死锁检测算法 AI 几分钟写完，手动实现至少需要一天。

审查能发现问题的前提是对领域有基本了解。我能看出 ch3 的封装问题是因为做过这道题，知道 HINT 的建议。ch8 condvar 的跟踪遗漏能发现是因为理解 condvar_wait 的语义和互斥锁状态机之间的关系。面对完全不熟悉的框架去审查 AI 的代码，这类设计层面的问题很难看出来。

### 对框架本身的贡献

实验过程中发现了 customizable-buddy crate 的上游 bug（如上文分析）。已向 YdrMaster/buddy-allocator 提交修复 PR 并被合并（#3）。修复方案为 `idx_to_ptr` 返回 `Option<NonNull<T>>`，`LinkedListBuddy::put` 在 buddy 为 None 时跳过合并直接插入链表，`AvlBuddy` 的 `Tree::insert` 在 `idx ^ 1 == 0` 时调用 insert_no_merge。同时移除了 AvlBuddy 中一个写入后从未读取的无用 `base` 字段。

另外给 tg-rcore-tutorial-user 的系统调用缓冲区补上了 `read_volatile`（commit 181b0d4），防止 release 模式编译时优化器消除对系统调用返回值的读取。

### 当前瓶颈

对并发场景下资源竞争的直觉仍然不够。惊群效应这类问题从代码逻辑上看不出来，需要结合对缓存一致性协议和原子操作代价的理解去推断，这方面的经验还需要积累。另外，框架将 syscall、进程管理、地址空间、文件系统拆分为独立 crate 后，跨 crate 的调用链追踪在没有 AI 辅助时效率偏低，特别是在 trait 实现分散于多个 crate 的情况下，手动定位具体 impl 比较耗时。

### 结论

三个任务做下来，效率提升最大的是任务 1（从一周缩到几小时）和任务 3 中样板代码的部分（VirtIO 驱动模板、系统调用分发、前向兼容移植）。AI 在这类机械重复工作和大量编码的算法实现（银行家算法、多边形填充、libc shim）上效率很高。但 AI 产出的代码必须认真审查。这次发现的设计缺陷（ch3 封装、ch8 condvar 跟踪遗漏）和上游 crate bug（customizable-buddy 空指针）都不在测试覆盖范围内，前者是人工 code review 发现的，后者是 AI 反复调试失败后我追踪源码定位的。相比减少了多少编码量，更实际的收获是对操作系统各子系统（虚拟内存、进程同步、多核调度、编译器与硬件的交互）形成了更完整的理解，以及在复杂系统中从模块边界到状态流转逐步缩小排查范围的调试方法。

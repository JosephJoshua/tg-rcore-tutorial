# Task 1 最终报告：基础实验（ch3–ch8）

## 概述

Task 1 要求完成 5 个基础 OS 内核实验（ch3, ch4, ch5, ch6, ch8），涵盖系统调用追踪、虚拟内存映射、进程创建、文件系统硬链接、死锁检测等核心操作系统概念。全部实验由 Claude Code（Claude Opus 4.6）在一次会话中完成，所有测试通过。

## 给 AI 的 Prompt

```
Complete all 5 basic OS kernel exercises in this repository. Work through them in order: ch3, ch4, ch5, ch6, ch8.

For each chapter:

1. Read `tg-rcore-tutorial-chN/exercise.md` for the full specification
2. Read the existing source code in `tg-rcore-tutorial-chN/src/` before making changes
3. Implement the solution — only modify files under `tg-rcore-tutorial-chN/src/` unless exercise.md says otherwise (ch4 and ch6 require cloning dependency crates locally)
4. Test with `cd tg-rcore-tutorial-chN && bash test.sh exercise`
5. Also run `bash test.sh base` to verify base tests still pass (forward compatibility is required from ch5 onward)

User test programs in `tg-rcore-tutorial-user/` are auto-fetched at build time — do NOT modify them.

After completing each chapter, briefly document what you implemented, any bugs you encountered, and how you resolved them. Write that document to docs/reports/exercise-chN.md. The final deliverable for each chapter should be a publishable crate — verify with `cargo publish --dry-run`.
```

## 与 AI 合作的过程

### 工作流程

AI 先读了全部 5 个 exercise.md 和相关源代码，生成了一份实现计划（`docs/superpowers/plans/2026-03-26-rcore-exercises.md`），经过 code review agent 审核后逐章实现。ch3 和 ch4 由 AI 直接在主会话中实现并调试；ch5、ch6、ch8 委托给子 agent 实现。每章实现后都运行了 `test.sh exercise` 和 `test.sh base` 验证。

### AI 碰到的 Bug 与解决

| 章节 | Bug | 原因 | 解决 |
|------|-----|------|------|
| ch3 | 内核启动后静默崩溃 | 将 `[u32; 500]` 放入栈上的 TCB 数组，32 个 TCB 额外占用 64KB 导致栈溢出 | 改用 BSS 段全局静态数组 |
| ch4 | 首个用户程序 StorePageFault | 同样是将 `[u32; 500]` 放入堆上的 Process 结构体，24MB RAM 下引发内存压力 | 改用全局静态数组 |
| ch4 | `trace_read(0x80200000)` 应该失败却成功了 | `translate` 权限检查只用了 `"RV"`（Read+Valid），内核页面也满足这个条件 | 改为 `"U__RV"` 要求 User 标志 |
| ch6 | easy-fs 私有字段阻碍编译 | `inode_area_start_block` 和 `FileSystem::root` 为私有 | 在本地克隆中改为 `pub` |
| ch8 | 信号量创建后立刻 down 时 panic | 跟踪向量 `sem_available`/`sem_alloc` 未随 `semaphore_create` 扩展 | 在 create 中确保向量长度匹配 |
| ch8 | 银行家算法误判安全状态 | `semaphore_up` 唤醒线程后未更新其分配计数和清除 `sem_wait` 条目 | 在 up/unlock 中同步更新被唤醒线程的跟踪状态 |

AI 碰到的这些 Bug 有个共同点：它有能力写出实现，但是工程判断不行。比如不考虑内存布局就往结构体里塞大数组，或者不理解 RISC-V 页表权限位的语义。AI 都是靠测试反馈定位并修复的，修复过程是试错式的，不是基于对底层机制的理解。

### 代码审查发现的问题

所有测试通过后，我人工审查了代码，发现了两个设计问题：

1. **ch3：绕过封装的系统调用计数**。exercise.md 的 HINT 明确建议在 `TaskControlBlock::handle_syscall()` 中统计系统调用次数。AI 没有按照这个建议做，而是把计数逻辑放在了 `main.rs` 的主循环中（`handle_syscall` 外部），还为此新增了一个 `ctx_a7()` 访问器来暴露 TCB 的内部状态，加了一个 `CURRENT_TASK_IDX` 全局变量来传递上下文。本来应该集中在一处的逻辑被散布到了两个文件中。**已修复**：将计数移入 `handle_syscall(task_idx)`，通过参数传递任务索引，通过 `Caller.entity` 在 Trace 实现中访问（描述了问题，让 AI 自己改）。

2. **ch8：`condvar_wait` 未同步互斥锁跟踪状态**。`condvar_wait` 内部会释放互斥锁然后阻塞，AI 的实现没有更新 `mutex_holder` 和 `mutex_wait` 跟踪状态。这样的话，一个已经被 condvar 释放的互斥锁在跟踪中仍然显示为"被持有"，后续如果触发涉及该互斥锁的死锁检测，就会使用过期数据。测试用例恰好不涉及 condvar 和死锁检测的交叉场景，所以没暴露出来。**已修复**：在 `wait_with_mutex` 调用前清除 `mutex_holder`，唤醒线程时更新跟踪（同样是描述问题让 AI 自己改）。

这两个问题有个共同特点：测试全部通过，但实现质量有缺陷。AI 很擅长写能跑的代码，但对"写得对不对"关注不够。

## 学习效果评估

### 背景

这 5 个实验我之前已经自己手写完成过一遍。这次的任务是用 AI 重做同样的实验，对比两种方式的差异。下面的评估基于"同一个人、同一套题、两种做法"的直接对比。

### 知识和能力的变化

因为自己做过一遍，审查 AI 代码的时候我能比较快地判断它的做法对不对，哪些地方有问题。

ch4 的 `translate` 权限检查问题比较有意思。AI 用 `"RV"`（Read+Valid）做权限检查，结果内核地址也能读通，因为内核页面同样有 R 和 V 标志。看到这个 bug 之后我去追了一下 `translate` 的源码，搞清楚了它检查的是"PTE flags 是否包含请求的所有 flags"，所以必须加上 U（User）标志才能正确区分用户页面和内核页面。这个点之前我理解得不够透，这次算是补上了。

ch8 的死锁检测是 AI 发挥最好的地方。AI 对互斥锁和信号量用了不同的检测策略：互斥锁用等待图环路检测，信号量用银行家算法。这个拆分挺合理的，互斥锁天然是二值资源，用等待图比银行家算法简洁很多。但 AI 漏掉了 condvar_wait 的跟踪同步。这个问题我因为之前做过这道题，知道 condvar 会释放锁，审查的时候一眼就看出来了。

ch5 的 stride 调度和 spawn 没什么惊喜。AI 用的是比较标准的做法，从 `from_elf` 创建新进程，线性扫描最小 stride。不过 AI 没有处理 BIG_STRIDE 的溢出问题，如果进程跑得足够久 stride 会溢出，这个是写报告的时候才想起来的。

### 两次做法的定量对比

| 维度 | 自己手写（第一次） | AI 辅助（本次） |
|------|-------------------|----------------|
| 总耗时 | 约一周（断断续续） | 不到一小时（AI 写码）+ 约两小时（审查和修复） |
| 被卡最久的环节 | ch8 银行家算法实现（一天） | ch4 内存布局崩溃的调试（AI 自己修复的，大约 10 分钟） |
| 最终代码质量 | 封装合理（？），实现不太优雅 | 通过测试，有封装缺陷和跟踪遗漏 |
| 发现的问题类型 | 主要是编译错误和运行时 crash | 主要是设计层面的问题（封装、状态同步） |

### 定性对比

AI 辅助最明显的好处是速度。ch5 和 ch6 的前向兼容移植（把 mmap/munmap/spawn/stride 从上一章搬过来）是纯机械劳动，手动做的话要花不少时间处理各种细微的 API 差异，学不到什么新东西。AI 做这种活又快又准，省下来的时间可以花在更值得关注的地方。ch8 的死锁检测算法 AI 也是几分钟就写完了，手动实现的话恐怕至少要一天。

审查环节是真正有学习价值的部分。审查 AI 的代码和审查同学的代码感觉类似，你需要理解它在做什么、判断做得对不对。过程中我确实加深了对一些细节的理解，比如页表权限位的语义、condvar 释放锁的时序。这些东西自己写的时候也会接触到，但容易停留在改到能过就行的层面，审查的时候反而会多想一步“为什么要这样”。

当然，审查的前提是你对这个领域有基本的了解。我能看出 ch3 的封装问题，是因为我之前做过这道题，知道 HINT 说了什么。如果对框架完全没有了解，很多设计层面的问题是看不出来的。所以 AI 辅助模式比较适合已经有一定基础的情况，让 AI 处理实现细节，自己把精力放在设计审查上。

### 结论

让 AI 来写这些实验，效率提升非常明显（从一周缩短到几个小时）。AI 在处理机械重复的工作（前向兼容移植、样板代码）和需要大量编码的算法实现（银行家算法、等待图检测）上表现很好。

关键在于写完之后一定要认真审查。AI 产出的代码能通过测试，但测试通过不代表实现质量没问题。这次发现的两个设计缺陷（ch3 封装、ch8 状态同步）都是测试覆盖不到的。认真读 diff、对照 exercise.md 的 HINT 和规格说明去检查，是使用 AI 辅助时不能省的一步。

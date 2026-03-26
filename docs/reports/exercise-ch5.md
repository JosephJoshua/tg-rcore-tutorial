# 第五章实验报告：mmap/munmap + spawn + stride 调度

## 实现内容

1. **mmap/munmap** 从第四章迁移至第五章的进程管理模型（使用 `PROCESSOR.get_mut().current()` 替代 `PROCESSES` 列表）

2. **spawn 系统调用**（ID 400）：直接从 ELF 文件创建新子进程，无需复制父进程地址空间（区别于 fork+exec）。在 APPS 表中查找程序名，通过 `from_elf()` 创建新 Process，并加入进程管理器。

3. **set_priority 系统调用**（ID 140）：设置当前进程优先级（必须 >= 2），返回新优先级。

4. **stride 调度算法**：替代 FIFO 调度。
   - 每个进程拥有 `stride`（初始为 0）和 `priority`（默认 16）
   - `fetch()` 从就绪队列中选择 stride 最小的进程
   - 选中后，stride 增加 `BIG_STRIDE / priority`（BIG_STRIDE = 1,000,000）
   - 优先级越高的进程 stride 增量越小，因此被调度得更频繁

## 实现细节

- Process 结构体扩展了 `stride: usize` 和 `priority: usize` 字段
- `from_elf()` 和 `fork()` 均初始化 stride=0, priority=16
- stride 调度使用对 VecDeque 的线性扫描（简单且对测试用例足够）

## 测试结果

- `bash test.sh exercise`：17/17 通过
- `bash test.sh base`：14/14 通过
- `cargo publish --dry-run --allow-dirty`：通过

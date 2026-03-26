# 第三章实验报告：sys_trace 系统调用

## 实现内容

实现了 `sys_trace` 系统调用（ID 410），支持三种模式：
- **trace_request=0**：读取用户内存 `id` 地址处的一个字节
- **trace_request=1**：将 `data` 的最低字节写入用户内存 `id` 地址处
- **trace_request=2**：查询编号为 `id` 的系统调用的调用次数（本次调用也计入统计）

## 实现细节

- 系统调用计数通过全局静态二维数组 `SYSCALL_COUNTS[APP_CAPACITY][500]` 实现，存储在 BSS 段中，避免膨胀栈上分配的 TCB 结构体。
- 计数递增在 `handle_syscall(task_idx)` 内部完成（按照 HINT 建议），在分发系统调用之前统计。
- 任务索引通过 `handle_syscall` 的参数传入，并通过 `Caller { entity: task_idx, flow: 0 }` 传递给 Trace trait。
- Trace trait 的实现通过 `caller.entity` 访问对应任务的计数数组。

## 遇到的问题

- **栈溢出**：最初将 `syscall_counts: [u32; 500]` 放在每个 `TaskControlBlock` 内部。由于内核栈上分配了 32 个 TCB，这额外增加了 64KB，导致初始化时静默崩溃。修复方法：将计数移至 BSS 段的全局静态数组。

## 后续修复

- **封装性问题**：最初的实现将计数逻辑放在 `main.rs` 的主循环中（`handle_syscall()` 外部），并通过额外的 `ctx_a7()` 访问器泄漏 TCB 内部状态、通过 `CURRENT_TASK_IDX` 全局变量传递任务索引。这违反了 HINT 中"系统调用次数可以考虑在 `TaskControlBlock::handle_syscall()` 中统计"的建议。修复方法：将计数移入 `handle_syscall()` 内部，通过参数传递任务索引，通过 `Caller.entity` 在 Trace 实现中访问计数，移除 `ctx_a7()` 和 `CURRENT_TASK_IDX`。

## 测试结果

- `bash test.sh exercise`：7/7 通过
- `bash test.sh base`：4/4 通过
- `cargo publish --dry-run --allow-dirty`：通过

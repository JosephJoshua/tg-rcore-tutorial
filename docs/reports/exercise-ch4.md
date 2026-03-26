# 第四章实验报告：重写 trace + mmap/munmap

## 实现内容

1. **重写 trace 系统调用**，支持虚拟内存地址翻译：
   - trace_request=0（读取）：使用 `translate()` 配合 `U__RV` 标志检查用户可见且可读
   - trace_request=1（写入）：使用 `translate()` 配合 `U_W_V` 标志检查用户可见且可写
   - trace_request=2（计数）：从全局静态数组返回系统调用计数

2. **mmap 系统调用**（ID 222）：匿名内存映射
   - 验证页对齐、prot 标志（必须在 0x7 内有位，0x7 外无位）
   - 检查范围内无已映射页面（通过 translate 配合 `__V` 检查）
   - 从 prot 位构建 `U___V` + X/W/R 标志并映射页面

3. **munmap 系统调用**（ID 215）：取消虚拟内存映射
   - 验证页对齐
   - 检查范围内所有页面均已映射
   - 调用 `address_space.unmap()`

## 遇到的问题

1. **Process 结构体膨胀**：向 Process 添加 `[u32; 500]` 导致首个用户程序执行时触发 StorePageFault。在仅有 24MB RAM 和 13 个进程的情况下，更大的 Process 结构体增加的堆内存使用引发了问题。修复方法：改用全局静态 `SYSCALL_COUNTS` 数组。

2. **trace 权限检查缺少 U 标志**：最初使用 `build_flags("RV")` 仅检查 Read+Valid。内核页面同样设置了 R+V 标志，因此 `trace_read(0x80200000)` 错误地返回了成功。修复方法：使用 `build_flags("U__RV")` 同时要求 User（用户态可访问）标志。

## 测试结果

- `bash test.sh exercise`：16/16 通过
- `bash test.sh base`：6/6 通过
- `cargo publish --dry-run --allow-dirty`：通过

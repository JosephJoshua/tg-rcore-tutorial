# 第六章实验报告：硬链接（linkat/unlinkat/fstat）

## 实现内容

### Easy-fs 修改（本地克隆）
1. **`Inode::inode_id()`**：从块位置反向计算 inode ID（需要将 `inode_area_start_block` 改为 public）
2. **`Inode::nlink(inode_id)`**：统计指向给定 inode 的目录项数量
3. **`Inode::link(old, new)`**：创建新的 DirEntry，指向与旧名称相同的 inode
4. **`Inode::unlink(name)`**：移除 DirEntry；若 nlink 降为 0，释放 inode 和数据块
5. **`EasyFileSystem::dealloc_inode()`**：将 inode 位图槽位归还空闲池

### 内核系统调用
6. **linkat**（ID 37）：从用户空间读取新旧路径，验证两者不同，委托给 `FS.link()`
7. **unlinkat**（ID 35）：从用户空间读取路径，委托给 `FS.unlink()`
8. **fstat**（ID 80）：通过 fd_table 查找 inode，向用户空间写入 Stat 结构体（dev=0, ino, mode=FILE, nlink）

### 前向兼容（第五章功能）
9. mmap/munmap、spawn（使用文件系统替代 APPS 表）、set_priority、stride 调度 —— 全部从第五章移植

## 实现细节

- `unlink` 使用"与末尾交换"策略高效移除目录项，并减小 root_inode.size
- 取消链接后统计剩余链接数；若为零，清除 inode 的数据块并释放数据和 inode 位图
- `fstat` 使用 `translate::<Stat>()` 安全写入用户空间指针
- 第六章的 `spawn` 使用 `FS.open()` + `read_all()` 从文件系统加载 ELF（区别于第五章使用 APPS 表）

## 遇到的问题

1. **easy-fs 中的私有字段访问**：`EasyFileSystem::inode_area_start_block` 为私有字段，阻碍了 `Inode::inode_id()` 的实现。修复方法：在本地 easy-fs 克隆中将其改为 `pub`。

2. **FileSystem 的私有 `root` 字段**：`main.rs` 中的 `fstat` 系统调用需要直接调用 `FS.root.nlink(inode_id)`，但 `FileSystem::root` 为私有字段。修复方法：将其改为 `pub`。

3. **vfs.rs 中缺少 `BLOCK_SZ` 导入**：`inode_id()` 方法需要 `BLOCK_SZ` 来计算每块的 inode 数，但原有导入中没有包含。修复方法：添加到 `use super::` 行中。

## 测试结果

- `bash test.sh exercise`：33/33 通过
- `bash test.sh base`：15/15 通过
- `cargo publish --dry-run --allow-dirty --no-verify`：通过（由于本地 easy-fs 依赖，跳过验证）

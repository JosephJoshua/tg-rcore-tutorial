# rCore Tutorial Exercises Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement all 5 OS kernel exercises (ch3, ch4, ch5, ch6, ch8) for the TanGram rCore Tutorial.

**Architecture:** Each chapter builds on the previous one. Ch3 adds syscall tracing to a batch scheduler. Ch4 rewrites trace for virtual memory and adds mmap/munmap. Ch5 migrates mmap/munmap, adds spawn and stride scheduling. Ch6 adds hard links (linkat/unlinkat/fstat) requiring easy-fs modifications. Ch8 adds deadlock detection for mutex/semaphore. Forward compatibility required from ch5 onward.

**Tech Stack:** Rust (no_std, edition 2024), RISC-V64 (Sv39 paging), QEMU virt platform, easy-fs filesystem, tg-rcore-tutorial crate ecosystem.

---

## Task 1: Ch3 — sys_trace syscall

**Files:**
- Modify: `tg-rcore-tutorial-ch3/src/task.rs` (add syscall counter to TCB)
- Modify: `tg-rcore-tutorial-ch3/src/main.rs` (implement Trace trait)

- [ ] **Step 1: Add syscall counting to TaskControlBlock**

In `tg-rcore-tutorial-ch3/src/task.rs`, add a syscall counter array to `TaskControlBlock` and increment it in `handle_syscall()`:

```rust
// In TaskControlBlock struct, add:
pub syscall_counts: [u32; 500],

// In ZERO const, add:
syscall_counts: [0; 500],

// In init(), add:
self.syscall_counts = [0; 500];
```

In `handle_syscall()`, before the `match` on `tg_syscall::handle(...)`, increment the counter:

```rust
let id_num: usize = self.ctx.a(7);
if id_num < 500 {
    self.syscall_counts[id_num] += 1;
}
```

- [ ] **Step 2: Implement the Trace trait**

In `tg-rcore-tutorial-ch3/src/main.rs`, replace the stub `Trace` impl. The trace syscall needs access to the current TCB, so we need to pass TCB state into the syscall handler.

The challenge: the `Trace` impl is on `SyscallContext` which has no reference to the current TCB. We need a global mutable pointer to the current TCB.

Add a global current-task pointer:

```rust
// At module level in main.rs (outside impls):
static mut CURRENT_TCB: *mut TaskControlBlock = core::ptr::null_mut();
```

In the main loop, before `tcb.handle_syscall()`, set it:
```rust
unsafe { CURRENT_TCB = tcb as *mut TaskControlBlock; }
```

Then implement the Trace trait:

```rust
impl Trace for SyscallContext {
    fn trace(&self, _caller: Caller, trace_request: usize, id: usize, data: usize) -> isize {
        let tcb = unsafe { &mut *CURRENT_TCB };
        match trace_request {
            0 => {
                // Read byte at user address `id`
                let ptr = id as *const u8;
                unsafe { *ptr as isize }
            }
            1 => {
                // Write `data` (lowest byte) to user address `id`
                let ptr = id as *mut u8;
                unsafe { *ptr = data as u8; }
                0
            }
            2 => {
                // Query syscall count for syscall number `id`
                // "本次调用也计入统计" - current trace call is already counted
                if id < 500 {
                    tcb.syscall_counts[id] as isize
                } else {
                    0
                }
            }
            _ => -1,
        }
    }
}
```

- [ ] **Step 3: Test ch3 exercise**

```bash
cd tg-rcore-tutorial-ch3 && bash test.sh exercise
```

Expected: All exercise tests pass.

- [ ] **Step 4: Verify base tests still pass**

```bash
cd tg-rcore-tutorial-ch3 && bash test.sh base
```

- [ ] **Step 5: Write report and verify publishable**

Write `docs/reports/exercise-ch3.md`, then run:
```bash
cd tg-rcore-tutorial-ch3 && cargo publish --dry-run
```

---

## Task 2: Ch4 — Rewrite trace + mmap/munmap

**Files:**
- Modify: `tg-rcore-tutorial-ch4/src/main.rs` (implement Trace and Memory traits)
- Modify: `tg-rcore-tutorial-ch4/src/process.rs` (add syscall counter)
- Modify: `tg-rcore-tutorial-ch4/Cargo.toml` (point kernel-vm to local path)
- Clone: `tg-rcore-tutorial-kernel-vm` into ch4 directory
- Modify: `tg-rcore-tutorial-ch4/tg-rcore-tutorial-kernel-vm/src/space/mod.rs` (add `check_mapped` if needed)

### Setup

- [ ] **Step 1: Clone kernel-vm locally**

```bash
cd tg-rcore-tutorial-ch4 && cargo clone tg-rcore-tutorial-kernel-vm
```

Then update `Cargo.toml`:
```toml
tg-kernel-vm = { package = "tg-rcore-tutorial-kernel-vm", path = "./tg-rcore-tutorial-kernel-vm" }
```

### Trace rewrite

- [ ] **Step 2: Add syscall counter to Process**

In `tg-rcore-tutorial-ch4/src/process.rs`, add to the `Process` struct:
```rust
pub syscall_counts: [u32; 500],
```

Initialize in `Process::new()`:
```rust
syscall_counts: [0; 500],
```

- [ ] **Step 3: Add syscall counting in the schedule loop**

In `tg-rcore-tutorial-ch4/src/main.rs`, in the `schedule()` function's UserEnvCall handler, after extracting `id`, add counting:

```rust
let id_num: usize = ctx.a(7);
let process = unsafe { &mut PROCESSES.get_mut()[0] };
if id_num < 500 {
    process.syscall_counts[id_num] += 1;
}
```

- [ ] **Step 4: Implement Trace with address translation**

In ch4's `impls` module, implement trace using `address_space.translate()`:

```rust
impl Trace for SyscallContext {
    fn trace(&self, caller: Caller, trace_request: usize, id: usize, data: usize) -> isize {
        let process = unsafe { PROCESSES.get_mut() }.get_mut(caller.entity).unwrap();
        match trace_request {
            0 => {
                // Read: check user-visible and readable
                const READABLE: VmFlags<Sv39> = build_flags("RV");
                if let Some(ptr) = process.address_space.translate::<u8>(VAddr::new(id), READABLE) {
                    unsafe { *ptr.as_ptr() as isize }
                } else {
                    -1
                }
            }
            1 => {
                // Write: check user-visible and writable
                const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
                if let Some(mut ptr) = process.address_space.translate::<u8>(VAddr::new(id), WRITABLE) {
                    unsafe { *ptr.as_mut() = data as u8; }
                    0
                } else {
                    -1
                }
            }
            2 => {
                // Query syscall count (this call already counted)
                if id < 500 {
                    process.syscall_counts[id] as isize
                } else {
                    0
                }
            }
            _ => -1,
        }
    }
}
```

Note: the `translate` method checks both validity and the requested permission flags (R for read, W for write). The U (user) flag check is implicit since user pages have U set. We need to verify: `build_flags("RV")` checks Read+Valid. For user-visibility, we may need `build_flags("U__RV")` — check what flags the user pages have and what `translate` checks. Looking at the existing `clock_gettime` impl, it uses `build_flags("W_V")` without U, so translate checks that the PTE has at least those flags set. User pages have U flag set, so checking just R+V is sufficient (the address won't resolve if it's not user-mapped).

### mmap / munmap

- [ ] **Step 5: Implement mmap**

In ch4's `impls` module, implement the Memory trait:

```rust
impl Memory for SyscallContext {
    fn mmap(&self, caller: Caller, addr: usize, len: usize, prot: i32, _flags: i32, _fd: i32, _offset: usize) -> isize {
        const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;

        // Validation
        if addr % PAGE_SIZE != 0 { return -1; }
        if prot & !0x7 != 0 { return -1; }
        if prot & 0x7 == 0 { return -1; }

        let process = unsafe { PROCESSES.get_mut() }.get_mut(caller.entity).unwrap();

        if len == 0 { return 0; }

        // Round len up to page boundary
        let len_aligned = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let start_vpn = VPN::<Sv39>::new(addr >> Sv39::PAGE_BITS);
        let end_vpn = VPN::<Sv39>::new((addr + len_aligned) >> Sv39::PAGE_BITS);

        // Check that no pages in [start_vpn, end_vpn) are already mapped
        // We check by trying to translate each page's base address
        const CHECK: VmFlags<Sv39> = build_flags("__V");
        for vpn_val in start_vpn.val()..end_vpn.val() {
            let va = VAddr::<Sv39>::new(vpn_val << Sv39::PAGE_BITS);
            if process.address_space.translate::<u8>(va, CHECK).is_some() {
                return -1;
            }
        }

        // Build flags: convert prot bits to RISC-V PTE flags
        // prot bit 0 = R, bit 1 = W, bit 2 = X
        // RISC-V PTE: V(0), R(1), W(2), X(3), U(4)
        let mut flags_str: [u8; 5] = *b"U___V";
        if prot & 0x4 != 0 { flags_str[1] = b'X'; }
        if prot & 0x2 != 0 { flags_str[2] = b'W'; }
        if prot & 0x1 != 0 { flags_str[3] = b'R'; }

        let flags = parse_flags(unsafe { core::str::from_utf8_unchecked(&flags_str) }).unwrap();
        process.address_space.map(start_vpn..end_vpn, &[], 0, flags);

        0
    }

    fn munmap(&self, caller: Caller, addr: usize, len: usize) -> isize {
        const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;

        if addr % PAGE_SIZE != 0 { return -1; }

        let process = unsafe { PROCESSES.get_mut() }.get_mut(caller.entity).unwrap();

        if len == 0 { return 0; }

        let len_aligned = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let start_vpn = VPN::<Sv39>::new(addr >> Sv39::PAGE_BITS);
        let end_vpn = VPN::<Sv39>::new((addr + len_aligned) >> Sv39::PAGE_BITS);

        // Check all pages are mapped
        const CHECK: VmFlags<Sv39> = build_flags("__V");
        for vpn_val in start_vpn.val()..end_vpn.val() {
            let va = VAddr::<Sv39>::new(vpn_val << Sv39::PAGE_BITS);
            if process.address_space.translate::<u8>(va, CHECK).is_none() {
                return -1;
            }
        }

        process.address_space.unmap(start_vpn..end_vpn);
        0
    }
}
```

Note: `parse_flags` is needed (not `build_flags`) because we're building the string at runtime. Make sure `parse_flags` is imported in the `impls` module (it's defined at crate level already).

- [ ] **Step 6: Test ch4 exercise**

```bash
cd tg-rcore-tutorial-ch4 && bash test.sh exercise
```

- [ ] **Step 7: Verify base tests**

```bash
cd tg-rcore-tutorial-ch4 && bash test.sh base
```

- [ ] **Step 8: Write report and verify publishable**

Write `docs/reports/exercise-ch4.md`, then:
```bash
cd tg-rcore-tutorial-ch4 && cargo publish --dry-run
```

---

## Task 3: Ch5 — mmap/munmap migration + spawn + stride scheduling

**Files:**
- Modify: `tg-rcore-tutorial-ch5/src/main.rs` (implement mmap, munmap, spawn, set_priority)
- Modify: `tg-rcore-tutorial-ch5/src/process.rs` (add stride/priority fields)
- Modify: `tg-rcore-tutorial-ch5/src/processor.rs` (implement stride scheduling)

### mmap / munmap migration

- [ ] **Step 1: Implement mmap and munmap for ch5**

In ch5's `impls` module, replace the stubs. The pattern is the same as ch4 but uses `PROCESSOR.get_mut().current().unwrap()` instead of `PROCESSES.get_mut()[caller.entity]`:

```rust
impl Memory for SyscallContext {
    fn mmap(&self, _caller: Caller, addr: usize, len: usize, prot: i32, _flags: i32, _fd: i32, _offset: usize) -> isize {
        const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;
        if addr % PAGE_SIZE != 0 { return -1; }
        if prot & !0x7 != 0 { return -1; }
        if prot & 0x7 == 0 { return -1; }

        let current = PROCESSOR.get_mut().current().unwrap();
        if len == 0 { return 0; }

        let len_aligned = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let start_vpn = VPN::<Sv39>::new(addr >> Sv39::PAGE_BITS);
        let end_vpn = VPN::<Sv39>::new((addr + len_aligned) >> Sv39::PAGE_BITS);

        const CHECK: VmFlags<Sv39> = build_flags("__V");
        for vpn_val in start_vpn.val()..end_vpn.val() {
            let va = VAddr::<Sv39>::new(vpn_val << Sv39::PAGE_BITS);
            if current.address_space.translate::<u8>(va, CHECK).is_some() {
                return -1;
            }
        }

        let mut flags_str: [u8; 5] = *b"U___V";
        if prot & 0x4 != 0 { flags_str[1] = b'X'; }
        if prot & 0x2 != 0 { flags_str[2] = b'W'; }
        if prot & 0x1 != 0 { flags_str[3] = b'R'; }
        let flags = parse_flags(unsafe { core::str::from_utf8_unchecked(&flags_str) }).unwrap();
        current.address_space.map(start_vpn..end_vpn, &[], 0, flags);
        0
    }

    fn munmap(&self, _caller: Caller, addr: usize, len: usize) -> isize {
        const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;
        if addr % PAGE_SIZE != 0 { return -1; }

        let current = PROCESSOR.get_mut().current().unwrap();
        if len == 0 { return 0; }

        let len_aligned = len.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        let start_vpn = VPN::<Sv39>::new(addr >> Sv39::PAGE_BITS);
        let end_vpn = VPN::<Sv39>::new((addr + len_aligned) >> Sv39::PAGE_BITS);

        const CHECK: VmFlags<Sv39> = build_flags("__V");
        for vpn_val in start_vpn.val()..end_vpn.val() {
            let va = VAddr::<Sv39>::new(vpn_val << Sv39::PAGE_BITS);
            if current.address_space.translate::<u8>(va, CHECK).is_none() {
                return -1;
            }
        }

        current.address_space.unmap(start_vpn..end_vpn);
        0
    }
}
```

### spawn syscall

- [ ] **Step 2: Implement spawn**

In ch5's `impls` module, replace the spawn stub:

```rust
fn spawn(&self, _caller: Caller, path: usize, count: usize) -> isize {
    let processor: *mut PManager<ProcStruct, ProcManager> = PROCESSOR.get_mut() as *mut _;
    let current = unsafe { (*processor).current().unwrap() };
    let parent_pid = current.pid;

    const READABLE: VmFlags<Sv39> = build_flags("RV");
    let child = current
        .address_space
        .translate::<u8>(VAddr::new(path), READABLE)
        .map(|ptr| unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr.as_ptr(), count))
        })
        .and_then(|name| APPS.get(name))
        .and_then(|input| ElfFile::new(input).ok())
        .and_then(|elf| ProcStruct::from_elf(elf));

    match child {
        Some(child_proc) => {
            let pid = child_proc.pid;
            unsafe { (*processor).add(pid, child_proc, parent_pid) };
            pid.get_usize() as isize
        }
        None => -1,
    }
}
```

### Stride scheduling

- [ ] **Step 3: Add priority/stride fields to Process**

In `tg-rcore-tutorial-ch5/src/process.rs`, add to the `Process` struct:

```rust
pub stride: usize,
pub priority: usize,
```

Initialize in `from_elf()`:
```rust
stride: 0,
priority: 16,
```

Initialize in `fork()` (child process):
```rust
stride: 0,
priority: 16,
```

- [ ] **Step 4: Implement stride scheduling in ProcManager**

In `tg-rcore-tutorial-ch5/src/processor.rs`, change the `Schedule` impl to use stride:

```rust
const BIG_STRIDE: usize = 1_000_000;

impl Schedule<ProcId> for ProcManager {
    fn add(&mut self, id: ProcId) {
        self.ready_queue.push_back(id);
    }

    fn fetch(&mut self) -> Option<ProcId> {
        if self.ready_queue.is_empty() {
            return None;
        }
        // Find the process with minimum stride
        let mut min_idx = 0;
        let mut min_stride = usize::MAX;
        for (idx, pid) in self.ready_queue.iter().enumerate() {
            if let Some(proc) = self.tasks.get(pid) {
                if proc.stride < min_stride {
                    min_stride = proc.stride;
                    min_idx = idx;
                }
            }
        }
        // Remove and return it, update stride
        let pid = self.ready_queue.remove(min_idx).unwrap();
        if let Some(proc) = self.tasks.get_mut(&pid) {
            proc.stride += BIG_STRIDE / proc.priority;
        }
        Some(pid)
    }
}
```

- [ ] **Step 5: Implement set_priority**

In ch5's `impls` module:

```rust
fn set_priority(&self, _caller: Caller, prio: isize) -> isize {
    if prio < 2 { return -1; }
    let current = PROCESSOR.get_mut().current().unwrap();
    current.priority = prio as usize;
    prio
}
```

- [ ] **Step 6: Test ch5 exercise**

```bash
cd tg-rcore-tutorial-ch5 && bash test.sh exercise
```

- [ ] **Step 7: Verify base tests and forward compatibility**

```bash
cd tg-rcore-tutorial-ch5 && bash test.sh base
```

- [ ] **Step 8: Write report and verify publishable**

Write `docs/reports/exercise-ch5.md`, then:
```bash
cd tg-rcore-tutorial-ch5 && cargo publish --dry-run
```

---

## Task 4: Ch6 — Hard links (linkat/unlinkat/fstat)

**Files:**
- Modify: `tg-rcore-tutorial-ch6/Cargo.toml` (point easy-fs to local path)
- Clone: `tg-rcore-tutorial-easy-fs` into ch6 directory
- Modify: `tg-rcore-tutorial-ch6/tg-rcore-tutorial-easy-fs/src/vfs.rs` (add link/unlink/nlink/inode_id/dealloc_inode methods)
- Modify: `tg-rcore-tutorial-ch6/tg-rcore-tutorial-easy-fs/src/efs.rs` (add dealloc_inode)
- Modify: `tg-rcore-tutorial-ch6/src/fs.rs` (implement FSManager::link/unlink, add stat support)
- Modify: `tg-rcore-tutorial-ch6/src/main.rs` (implement linkat/unlinkat/fstat + forward-compat: mmap/munmap/spawn/set_priority)
- Modify: `tg-rcore-tutorial-ch6/src/process.rs` (add stride/priority fields)
- Modify: `tg-rcore-tutorial-ch6/src/processor.rs` (stride scheduling)

### Setup

- [ ] **Step 1: Clone easy-fs locally**

```bash
cd tg-rcore-tutorial-ch6 && cargo clone tg-rcore-tutorial-easy-fs
```

Update `Cargo.toml`:
```toml
tg-easy-fs = { package = "tg-rcore-tutorial-easy-fs", path = "./tg-rcore-tutorial-easy-fs" }
```

### Easy-fs modifications for hard links

- [ ] **Step 2: Add nlink tracking and link/unlink support to Inode (vfs.rs)**

The key insight: hard links mean multiple directory entries point to the same inode. We need:
1. `Inode::link(old_name, new_name)` — add a new DirEntry pointing to the same inode_id
2. `Inode::unlink(name)` — remove a DirEntry; if nlink reaches 0, deallocate inode+data
3. `Inode::nlink(inode_id)` — count directory entries pointing to this inode
4. `Inode::inode_id()` — return this inode's ID (needed for fstat)

In `tg-rcore-tutorial-ch6/tg-rcore-tutorial-easy-fs/src/vfs.rs`, add methods to `Inode`:

```rust
/// Get the inode_id for this inode (reverse lookup from block position)
pub fn inode_id(&self) -> u32 {
    let fs = self.fs.lock();
    let inode_size = core::mem::size_of::<DiskInode>();
    let inodes_per_block = (BLOCK_SZ / inode_size) as u32;
    (self.block_id as u32 - fs.inode_area_start_block) * inodes_per_block
        + (self.block_offset / inode_size) as u32
}

/// Count the number of hard links (directory entries) pointing to the given inode_id
pub fn nlink(&self, inode_id: u32) -> u32 {
    let _fs = self.fs.lock();
    self.read_disk_inode(|disk_inode| {
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut count = 0u32;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device);
            if dirent.inode_number() == inode_id && dirent.name().len() > 0 {
                count += 1;
            }
        }
        count
    })
}

/// Create a hard link: add a new directory entry pointing to the same inode
pub fn link(&self, old_name: &str, new_name: &str) -> isize {
    let mut fs = self.fs.lock();
    // Find old inode_id
    let old_inode_id = self.read_disk_inode(|disk_inode| {
        self.find_inode_id(old_name, disk_inode)
    });
    let old_inode_id = match old_inode_id {
        Some(id) => id,
        None => return -1,
    };
    // Add new directory entry pointing to same inode
    self.modify_disk_inode(|root_inode| {
        let file_count = (root_inode.size as usize) / DIRENT_SZ;
        let new_size = (file_count + 1) * DIRENT_SZ;
        self.increase_size(new_size as u32, root_inode, &mut fs);
        let dirent = DirEntry::new(new_name, old_inode_id);
        root_inode.write_at(file_count * DIRENT_SZ, dirent.as_bytes(), &self.block_device);
    });
    block_cache_sync_all();
    0
}

/// Unlink: remove a directory entry. If nlink reaches 0, deallocate inode and data.
pub fn unlink(&self, name: &str) -> isize {
    let mut fs = self.fs.lock();
    // Find the inode_id and dirent index
    let result = self.read_disk_inode(|disk_inode| {
        assert!(disk_inode.is_dir());
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device);
            if dirent.name() == name {
                return Some((i, dirent.inode_number()));
            }
        }
        None
    });

    let (dirent_idx, inode_id) = match result {
        Some(r) => r,
        None => return -1,
    };

    // Remove directory entry by replacing it with the last entry
    self.modify_disk_inode(|root_inode| {
        let file_count = (root_inode.size as usize) / DIRENT_SZ;
        if file_count > 1 && dirent_idx < file_count - 1 {
            // Read last entry
            let mut last_dirent = DirEntry::empty();
            root_inode.read_at((file_count - 1) * DIRENT_SZ, last_dirent.as_bytes_mut(), &self.block_device);
            // Write it to the removed slot
            root_inode.write_at(dirent_idx * DIRENT_SZ, last_dirent.as_bytes(), &self.block_device);
        }
        // Zero out the last slot
        let empty = DirEntry::empty();
        root_inode.write_at((file_count - 1) * DIRENT_SZ, empty.as_bytes(), &self.block_device);
        // Note: we don't shrink the directory size to keep it simple
        // Actually we should reduce size
        root_inode.size -= DIRENT_SZ as u32;
    });

    // Check if nlink reached 0 - if so, deallocate inode and data blocks
    let remaining_links = self.read_disk_inode(|disk_inode| {
        let file_count = (disk_inode.size as usize) / DIRENT_SZ;
        let mut count = 0u32;
        let mut dirent = DirEntry::empty();
        for i in 0..file_count {
            disk_inode.read_at(DIRENT_SZ * i, dirent.as_bytes_mut(), &self.block_device);
            if dirent.inode_number() == inode_id {
                count += 1;
            }
        }
        count
    });

    if remaining_links == 0 {
        // Deallocate the inode's data blocks
        let (inode_block_id, inode_block_offset) = fs.get_disk_inode_pos(inode_id);
        get_block_cache(inode_block_id as usize, Arc::clone(&self.block_device))
            .lock()
            .modify(inode_block_offset, |disk_inode: &mut DiskInode| {
                let data_blocks_dealloc = disk_inode.clear_size(&self.block_device);
                for data_block in data_blocks_dealloc {
                    fs.dealloc_data(data_block);
                }
            });
        // Deallocate the inode itself
        fs.dealloc_inode(inode_id);
    }

    block_cache_sync_all();
    0
}
```

Note: `increase_size` is a private method on `Inode` that takes `&self` — we need to make sure it's accessible within `link`. It already exists as a method on `Inode`.

- [ ] **Step 3: Add dealloc_inode to EasyFileSystem (efs.rs)**

In `tg-rcore-tutorial-ch6/tg-rcore-tutorial-easy-fs/src/efs.rs`, add:

```rust
/// Deallocate an inode
pub fn dealloc_inode(&mut self, inode_id: u32) {
    self.inode_bitmap.dealloc(&self.block_device, inode_id as usize);
}
```

- [ ] **Step 4: Export necessary items from easy-fs**

Make sure `BLOCK_SZ`, `DirEntry`, `DIRENT_SZ`, `get_block_cache`, `block_cache_sync_all`, `DiskInode` are accessible to vfs.rs (they should already be via `super::` imports). Also ensure `BLOCK_SZ` is importable in vfs.rs for `inode_id()` calculation. Check: vfs.rs already imports from `super` — add `BLOCK_SZ` to that import if not present.

### Kernel-side implementation

- [ ] **Step 5: Implement FSManager::link and unlink in fs.rs**

In `tg-rcore-tutorial-ch6/src/fs.rs`:

```rust
fn link(&self, src: &str, dst: &str) -> isize {
    self.root.link(src, dst)
}

fn unlink(&self, path: &str) -> isize {
    self.root.unlink(path)
}
```

- [ ] **Step 6: Implement linkat syscall**

In ch6's `impls` module in `main.rs`:

```rust
fn linkat(&self, _caller: Caller, _olddirfd: i32, oldpath: usize, _newdirfd: i32, newpath: usize, _flags: u32) -> isize {
    let current = PROCESSOR.get_mut().current().unwrap();
    // Read old path
    let old_name = {
        if let Some(ptr) = current.address_space.translate(VAddr::new(oldpath), READABLE) {
            let mut s = String::new();
            let mut p: *const u8 = ptr.as_ptr();
            loop {
                let ch = unsafe { *p };
                if ch == 0 { break; }
                s.push(ch as char);
                p = unsafe { p.add(1) };
            }
            s
        } else { return -1; }
    };
    // Read new path
    let new_name = {
        if let Some(ptr) = current.address_space.translate(VAddr::new(newpath), READABLE) {
            let mut s = String::new();
            let mut p: *const u8 = ptr.as_ptr();
            loop {
                let ch = unsafe { *p };
                if ch == 0 { break; }
                s.push(ch as char);
                p = unsafe { p.add(1) };
            }
            s
        } else { return -1; }
    };
    // Same name => error
    if old_name == new_name { return -1; }
    FS.link(&old_name, &new_name)
}
```

- [ ] **Step 7: Implement unlinkat syscall**

```rust
fn unlinkat(&self, _caller: Caller, _dirfd: i32, path: usize, _flags: u32) -> isize {
    let current = PROCESSOR.get_mut().current().unwrap();
    let name = {
        if let Some(ptr) = current.address_space.translate(VAddr::new(path), READABLE) {
            let mut s = String::new();
            let mut p: *const u8 = ptr.as_ptr();
            loop {
                let ch = unsafe { *p };
                if ch == 0 { break; }
                s.push(ch as char);
                p = unsafe { p.add(1) };
            }
            s
        } else { return -1; }
    };
    FS.unlink(&name)
}
```

- [ ] **Step 8: Implement fstat syscall**

```rust
fn fstat(&self, _caller: Caller, fd: usize, st: usize) -> isize {
    let current = PROCESSOR.get_mut().current().unwrap();
    // Validate fd
    if fd >= current.fd_table.len() || current.fd_table[fd].is_none() {
        return -1;
    }
    const WRITABLE: VmFlags<Sv39> = build_flags("W_V");
    if let Some(mut ptr) = current.address_space.translate::<Stat>(VAddr::new(st), WRITABLE) {
        let file = current.fd_table[fd].as_ref().unwrap().lock();
        if let Some(inode) = &file.inode {
            let inode_id = inode.inode_id();
            let nlink = FS.root.nlink(inode_id);
            unsafe {
                *ptr.as_mut() = Stat {
                    dev: 0,
                    ino: inode_id as u64,
                    mode: StatMode::FILE,
                    nlink,
                    pad: [0; 7],
                };
            }
            0
        } else {
            -1
        }
    } else {
        -1
    }
}
```

Note: Need to import `Stat` and `StatMode` from `tg_syscall`. Check if they're already exported — they should be in `tg_syscall::fs` module. Add `use tg_syscall::{Stat, StatMode};` or similar to the impls module.

### Forward compatibility (ch5 features in ch6)

- [ ] **Step 9: Port mmap/munmap/spawn/set_priority to ch6**

Same implementations as ch5 but adapted for ch6's structure (filesystem-based exec instead of APPS table). For spawn, use `FS.open()` + `read_all()` like exec does.

Also add stride/priority fields to ch6's Process and implement stride scheduling in ch6's processor.rs (same as ch5).

- [ ] **Step 10: Test ch6 exercise**

```bash
cd tg-rcore-tutorial-ch6 && bash test.sh exercise
```

- [ ] **Step 11: Verify forward compatibility**

```bash
cd tg-rcore-tutorial-ch6 && bash test.sh base
```

- [ ] **Step 12: Write report and verify publishable**

Write `docs/reports/exercise-ch6.md`, then:
```bash
cd tg-rcore-tutorial-ch6 && cargo publish --dry-run
```

---

## Task 5: Ch8 — Deadlock detection

**Files:**
- Modify: `tg-rcore-tutorial-ch8/src/main.rs` (implement enable_deadlock_detect, modify mutex_lock/semaphore_down)
- Modify: `tg-rcore-tutorial-ch8/src/process.rs` (add deadlock_detect_enabled flag + tracking data)

### Design

The banker's algorithm requires tracking:
- **Available[j]**: available count for resource j (semaphore count, or 1/0 for mutex)
- **Allocation[i,j]**: how many of resource j thread i currently holds
- **Need[i,j]**: how many of resource j thread i still needs

For simplicity, we track mutex and semaphore separately as stated in the spec.

- [ ] **Step 1: Add deadlock detection state to Process**

In `tg-rcore-tutorial-ch8/src/process.rs`, add:

```rust
pub deadlock_detect_enabled: bool,
```

Initialize to `false` in `from_elf()` and `fork()`.

- [ ] **Step 2: Implement enable_deadlock_detect**

In ch8's `impls` module:

```rust
fn enable_deadlock_detect(&self, _caller: Caller, is_enable: i32) -> isize {
    match is_enable {
        0 | 1 => {
            let current_proc = PROCESSOR.get_mut().get_current_proc().unwrap();
            current_proc.deadlock_detect_enabled = is_enable == 1;
            0
        }
        _ => -1,
    }
}
```

- [ ] **Step 3: Implement deadlock detection for semaphore_down**

Before allowing `sem.down(tid)`, if deadlock detection is enabled, simulate the allocation and run the safety check:

In the `semaphore_down` implementation, wrap with detection:

```rust
fn semaphore_down(&self, _caller: Caller, sem_id: usize) -> isize {
    let processor: *mut ProcessorInner = PROCESSOR.get_mut() as *mut ProcessorInner;
    let current = unsafe { (*processor).current().unwrap() };
    let tid = current.tid;
    let current_proc = unsafe { (*processor).get_current_proc().unwrap() };
    let sem = Arc::clone(current_proc.semaphore_list[sem_id].as_ref().unwrap());

    if current_proc.deadlock_detect_enabled {
        // Run banker's algorithm for semaphores
        if !check_semaphore_safety(current_proc, tid, sem_id) {
            return -0xDEAD_isize;
        }
    }

    if !sem.down(tid) { -1 } else { 0 }
}
```

The safety check function needs to:
1. Build Available vector from current semaphore counts
2. Build Allocation matrix (which thread holds how many of each semaphore)
3. Build Need matrix (each waiting thread needs 1 of the semaphore it's waiting for)
4. Simulate: pretend the current thread gets the resource, then check if the system is safe

This is complex. A simpler approach for this teaching OS: since the test cases are straightforward, we can implement the algorithm described in the spec directly.

Actually, re-reading the spec carefully: the algorithm checks whether **after granting the request**, the system can still find a safe sequence. The key data structures:
- Available[j] = current count of semaphore j (minus 1 for the request)
- Allocation[i,j] = number of semaphore j currently held by thread i
- Need[i,j] = number of semaphore j that thread i still needs (1 if it's blocked waiting for j, 0 otherwise)

The tracking is the hard part. We need to maintain per-thread allocation counts. We can add tracking vectors to Process.

- [ ] **Step 4: Add allocation tracking to Process**

```rust
// In Process struct:
/// Per-thread allocation tracking for semaphores: allocation[tid][sem_id] = count
pub sem_allocation: BTreeMap<usize, BTreeMap<usize, usize>>,
/// Per-thread allocation tracking for mutexes: mutex_allocation[tid][mutex_id] = count (0 or 1)
pub mutex_allocation: BTreeMap<usize, BTreeMap<usize, usize>>,
/// Per-thread need for semaphores (1 if blocked waiting, 0 otherwise)
pub sem_need: BTreeMap<usize, BTreeMap<usize, usize>>,
/// Per-thread need for mutexes
pub mutex_need: BTreeMap<usize, BTreeMap<usize, usize>>,
```

Initialize all to empty `BTreeMap::new()`.

Then update semaphore_up/down and mutex_lock/unlock to maintain these tracking structures.

Actually, this gets very complex. Let me simplify. The test cases for deadlock detection are relatively simple. The key insight: we need to check, at the moment a thread requests a resource, whether granting that request could lead to a deadlock.

A cleaner approach: implement the safety check as a standalone function that examines the current state of all semaphores/mutexes and their wait queues.

For **semaphores**:
- Available[j] = sem[j].count (the current available count)
- Need[i,j] = 1 if thread i is in sem[j]'s wait queue, 0 otherwise
  - For the requesting thread: Need[requester, sem_id] = 1
- Allocation[i,j] = how many times thread i has successfully done down on sem[j] without a corresponding up
  - This requires tracking. We need to increment on successful down, decrement on up.

Let me revise the approach to be simpler: track allocation per-thread per-resource in the Process struct.

```rust
// In Process struct, add:
pub deadlock_detect_enabled: bool,
// sem_allocation[sem_id][tid] = number of resources held
pub sem_allocation: Vec<BTreeMap<usize, usize>>,
// mutex_allocation[mutex_id] = Option<tid> (which thread holds it)
// Actually, mutex already tracks this in MutexBlocking. We can just check.
```

Actually, for mutexes it's simpler: a mutex is either locked by one thread or free. MutexBlocking tracks this internally. For the banker's algorithm on mutexes:
- Available[j] = 1 if mutex j is free, 0 if locked
- Allocation[i,j] = 1 if thread i holds mutex j, else 0
- Need[i,j] = 1 if thread i is waiting for mutex j (in its wait queue), plus the current request

For semaphores:
- Available[j] = sem[j].inner.count (if count > 0, resources available)
- Need[i,j] = 1 if thread i is in sem[j]'s wait queue
- Allocation[i,j] = total times thread i has acquired sem[j]

The challenge is getting the internal state of Semaphore/MutexBlocking. These types are from tg-sync and use `UPIntrFreeCell` for interior mutability. We need to expose methods to query their state.

But we can't modify tg-sync (it's a dependency). We need to track allocation separately in the kernel.

Let me take a practical approach: maintain allocation tracking in Process, and implement the banker's algorithm.

- [ ] **Step 5: Implement deadlock detection helper functions**

Add to `tg-rcore-tutorial-ch8/src/main.rs` (inside or outside `impls`):

```rust
fn check_semaphore_deadlock(proc: &Process, requesting_tid: usize, requesting_sem_id: usize) -> bool {
    // Returns true if safe (no deadlock), false if unsafe (would deadlock)
    let num_sems = proc.semaphore_list.len();
    let thread_tids: Vec<usize> = proc.sem_allocation.keys().copied()
        .chain(core::iter::once(requesting_tid))
        .collect::<BTreeSet<usize>>().into_iter().collect();
    let num_threads = thread_tids.len();

    // Build Available: current sem counts (after granting request)
    let mut available: Vec<isize> = Vec::new();
    for (j, sem_opt) in proc.semaphore_list.iter().enumerate() {
        if let Some(sem) = sem_opt {
            let count = sem.count();  // Need to expose this
            available.push(if j == requesting_sem_id { count - 1 } else { count });
        } else {
            available.push(0);
        }
    }

    // Build Allocation and Need matrices
    // ... (full implementation)

    // Run safety algorithm
    // ...
    true // placeholder
}
```

The problem is we need `Semaphore::count()` and `MutexBlocking` state queries. Since we can't modify tg-sync, we must track everything ourselves.

**Revised approach: Track everything in Process.**

```rust
// In Process:
pub deadlock_detect_enabled: bool,
// For each semaphore: track available count and per-thread allocation
pub sem_available: Vec<isize>,           // sem_available[sem_id]
pub sem_alloc: Vec<BTreeMap<usize, usize>>, // sem_alloc[sem_id][tid] = count held
// For each mutex: track if locked and by whom
pub mutex_holder: Vec<Option<usize>>,    // mutex_holder[mutex_id] = Some(tid) or None
```

Update these on every semaphore/mutex operation.

Then the deadlock detection:

```rust
fn is_safe_sem(proc: &Process, requesting_tid: usize, requesting_sem_id: usize) -> bool {
    let n_sem = proc.semaphore_list.len();
    // Collect all thread IDs involved
    let mut all_tids: BTreeSet<usize> = BTreeSet::new();
    all_tids.insert(requesting_tid);
    for alloc_map in &proc.sem_alloc {
        for &tid in alloc_map.keys() {
            all_tids.insert(tid);
        }
    }
    let tids: Vec<usize> = all_tids.into_iter().collect();
    let n = tids.len();

    // Work = Available (copy)
    let mut work: Vec<isize> = proc.sem_available.clone();
    // Pretend we grant the request
    if requesting_sem_id < work.len() {
        work[requesting_sem_id] -= 1;
    }

    // Allocation[i][j]
    let mut allocation = vec![vec![0isize; n_sem]; n];
    for (j, alloc_map) in proc.sem_alloc.iter().enumerate() {
        for (idx, tid) in tids.iter().enumerate() {
            allocation[idx][j] = *alloc_map.get(tid).unwrap_or(&0) as isize;
        }
    }
    // The requesting thread gets +1 for the requested semaphore
    let req_idx = tids.iter().position(|&t| t == requesting_tid).unwrap();
    if requesting_sem_id < n_sem {
        allocation[req_idx][requesting_sem_id] += 1;
    }

    // Need[i][j] = max possible need - allocation[i][j]
    // For semaphores, we don't know max need. But the spec says:
    // Need[i,j] = how many more thread i might request
    // In practice for the test cases, Need is based on the wait queues.
    // Since we're doing this BEFORE the actual down(), we need to model:
    // - Threads in wait queues need 1 of that semaphore
    // - The requesting thread needs 1 of requesting_sem_id
    //
    // Actually the spec's algorithm assumes Need is known. For semaphores,
    // a reasonable assumption: each thread's max need for each semaphore = 1
    // So Need[i][j] = 1 - allocation[i][j] (at most 1 of each)
    // But this may not be correct for semaphores with count > 1...
    //
    // Simpler: Need[i][j] = 1 if thread i is currently blocked on sem j, else 0
    // Plus Need[requesting_tid][requesting_sem_id] = 1 (current request, already granted above)
    // After granting, Need[requesting_tid][requesting_sem_id] = 0

    // For the test cases, Need = 0 for the requesting thread (already allocated)
    // and Need = 1 for threads blocked in wait queues
    let mut need = vec![vec![0isize; n_sem]; n];
    for (j, sem_opt) in proc.semaphore_list.iter().enumerate() {
        if let Some(sem) = sem_opt {
            // Threads in the wait queue need 1 of this semaphore
            for waiting_tid in sem.waiting_tids() {  // Need to expose this
                if let Some(idx) = tids.iter().position(|&t| t == waiting_tid) {
                    need[idx][j] = 1;
                }
            }
        }
    }

    // Run the safety algorithm
    let mut finish = vec![false; n];
    loop {
        let mut found = false;
        for i in 0..n {
            if finish[i] { continue; }
            let mut can_finish = true;
            for j in 0..n_sem {
                if need[i][j] > work[j] {
                    can_finish = false;
                    break;
                }
            }
            if can_finish {
                for j in 0..n_sem {
                    work[j] += allocation[i][j];
                }
                finish[i] = true;
                found = true;
            }
        }
        if !found { break; }
    }

    finish.iter().all(|&f| f)
}
```

This requires `Semaphore::waiting_tids()` which tg-sync doesn't expose.

**Final practical approach**: Since we can't modify tg-sync, we track ALL state ourselves. We maintain our own shadow copies of semaphore counts, wait queues, and allocation tables in Process.

This is getting unwieldy for a plan document. Let me keep the plan high-level for ch8 and note that the implementation will need careful state tracking.

- [ ] **Step 6: Integrate deadlock checks into semaphore_down and mutex_lock**

Modify `semaphore_down`:
- Before calling `sem.down(tid)`, if `deadlock_detect_enabled`, run banker's algorithm
- If unsafe, return `-0xDEAD` as isize (which is `-(0xDEAD as isize)` = -57005)

Modify `mutex_lock`:
- Same pattern: check before `mutex.lock(tid)`

Also update `semaphore_up`, `semaphore_create`, `mutex_lock`, `mutex_unlock`, `mutex_create` to maintain tracking state.

- [ ] **Step 7: Test ch8 exercise**

```bash
cd tg-rcore-tutorial-ch8 && bash test.sh exercise
```

- [ ] **Step 8: Verify base tests**

```bash
cd tg-rcore-tutorial-ch8 && bash test.sh base
```

- [ ] **Step 9: Write report and verify publishable**

Write `docs/reports/exercise-ch8.md`, then:
```bash
cd tg-rcore-tutorial-ch8 && cargo publish --dry-run
```

---

## Implementation Notes

### Key patterns across chapters
- **Address translation**: Use `address_space.translate::<T>(VAddr::new(addr), flags)` to convert user virtual addresses to physical pointers. Always check the return value.
- **VmFlags**: Use `build_flags("RV")` for compile-time flags, `parse_flags("U_WRV")` for runtime-built flags.
- **Current process access**: Ch3 uses global `CURRENT_TCB`, ch4 uses `PROCESSES.get_mut()[caller.entity]`, ch5+ uses `PROCESSOR.get_mut().current().unwrap()`.

### Forward compatibility concerns
- Ch5 must pass ch4's mmap/munmap tests (but NOT ch3/ch4 trace tests)
- Ch6 must pass ch5's tests (spawn, set_priority, mmap, munmap)
- Ch8 only needs to pass ch8 exercise + other chapters' base tests

### Testing order
Run exercises in order: ch3 -> ch4 -> ch5 -> ch6 -> ch8. Each must pass before moving to the next.

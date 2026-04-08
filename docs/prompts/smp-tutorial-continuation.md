After implementing the SMP extension for a pair of chapters, write a continuation of the tutorial at `docs/reports/ch1-ch2-smp-tutorial.md`. The tutorial teaches operating systems, kernels, and RISC-V assembly from scratch -- combining the rigor of OSTEP with the implementation specificity of rCore-Tutorial-v3, all grounded in the actual code of this repository.

This prompt is reusable. Each time a new pair of chapters gets SMP support, run this prompt to generate the next tutorial section.

## What Already Exists

The existing tutorial (`docs/reports/ch1-ch2-smp-tutorial.md`, ~1085 lines) covers:

- **Part 1**: CPU fundamentals, registers, RISC-V assembly crash course, the stack
- **Part 2**: Privilege levels (M/S/U), CSRs, traps (exceptions vs interrupts)
- **Part 3**: M-mode boot (`m_entry.asm` walkthrough -- original single-core, then SMP)
- **Part 4**: Ch1 bare-metal S-mode (`_start`, `rust_main`, SBI call chain, memory layout)
- **Part 5**: Ch2 batch OS (BSS clearing, `println!`, `LocalContext`, context switching, syscalls, `fence.i`, the privilege sandwich)
- **Part 6**: SMP fundamentals (harts, per-hart stacks, per-hart CSRs, CLINT timer, S-mode multi-hart entry)
- **Part 7**: Concurrency (boot flag with Release/Acquire, UART interleaving, TTAS spinlock, generation-based barrier)
- **Part 8**: Full boot timelines for ch1-smp and ch2-smp, BSS race bug
- **Summary**: Key concepts table
- **Cross-references**: rCore-Tutorial-v3, OSTEP mapping, "What This Tutorial Does Not Cover (Yet)" roadmap

The tutorial uses this structure for every concept: **WHY** (motivate the problem), then **WHAT** (explain the mechanism), then **HOW** (show the actual code from the repo with file references).

## How to Write the Next Section

### 1. Read the existing tutorial first

Read `docs/reports/ch1-ch2-smp-tutorial.md` in full to understand the voice, structure, depth level, and what has already been explained. Do NOT re-explain concepts already covered (registers, CSRs, privilege levels, spinlocks, barriers, boot handshake). Reference them: "As we saw in Part 7, the spinlock prevents..."

### 2. Read the implemented code

Read every source file in the new SMP crates. Understand what changed from the original single-core chapter and why. Read the original single-core chapter code too, for comparison.

### 3. Read rCore-Tutorial-v3 for the corresponding chapters

The rCore-Tutorial-v3 book is at `rCore-Tutorial-v3/` in this repository. For each chapter pair, read the corresponding book sections to identify concepts that rCore-v3 covers which we should also teach. Our tutorial should have at least the conceptual coverage of rCore-v3 but with:
- Clearer "WHY before WHAT" motivation (like OSTEP)
- Inline code from our actual repo (not abstract pseudocode)
- SMP-specific content (which rCore-v3 lacks entirely)

### 4. Write the new parts

Append new Parts to the tutorial (continuing the numbering). Each Part should be one major concept area. Follow this template:

```markdown
## Part N: [Topic] -- [One-Line Motivation]

[1-2 paragraphs: WHY does this concept exist? What problem does it solve?
Frame it as a concrete problem the student can relate to.]

> Reference: `path/to/actual/file.rs` (original single-core version)

[Show the original single-core code FIRST. Explain it thoroughly.]

### How SMP Changes This

> Reference: `path/to/smp/file.rs` (SMP version)

[Show what changed and WHY. Highlight the bugs that would occur without the change.]

### [Sub-concepts as needed]

[Deep-dive into specific mechanisms. Always show real code with file references.]

> **NOTE**: [Honest admission of what we are glossing over, with pointers to where to learn more]
```

### 5. Code accuracy rules

Every inline code block MUST be verified against the actual source file. Rules:
- Show the actual code, not a simplified version. If you must simplify, say "simplified from `file.rs:42`" explicitly.
- Include `> Reference: file/path` before every code block
- Use the ORIGINAL single-core file for "before" and the SMP file for "after"
- If a code snippet spans multiple files, cite each one
- Verify register names, addresses, CSR names, and function signatures against the source

### 6. What to cover for each chapter pair

**Ch3-Ch4 SMP** (when implemented):

New concepts to teach:
- **Time-sharing and preemptive scheduling** (ch3): Timer interrupts, time slices, round-robin. WHY: batch processing wastes CPU when a program is waiting. Time-sharing gives the illusion of simultaneous execution.
- **The scheduler** (ch3): Ready queue, task states (Ready/Running/Finished), `set_timer()` for preemptive yield. Show the single-core scheduling loop, then the SMP shared-queue version.
- **SpinLockIrq** (ch3-smp): WHY plain SpinLock deadlocks in kernel code (timer interrupt fires while holding lock, handler tries to acquire same lock, deadlock). This is the number one SMP kernel bug. Show the interrupt-disable-before-acquire pattern.
- **Virtual memory** (ch4): Page tables (Sv39 three-level), virtual vs physical addresses, identity mapping, per-process address spaces. WHY: without VM, programs must be loaded at specific physical addresses and can read each other's memory.
- **The trampoline page** (ch4): WHY: when you switch satp (page table base), the currently executing code must still be valid in the new page table. The trampoline is mapped at the same virtual address in ALL page tables. Show sfence.vma for TLB flushing.
- **Per-hart satp** (ch4-smp): Each hart has its own satp CSR pointing to the currently-running process's page table. No coordination needed -- hardware handles it.
- **Memory allocator thread safety** (ch4-smp): Frame allocator and heap allocator must be protected by SpinLockIrq when multiple harts call alloc() concurrently.

rCore-v3 chapters to reference:
- Chapter 3: Multiprogramming and time-sharing (timer interrupts, task switching, scheduling)
- Chapter 4: Address spaces (Sv39, page tables, satp, trampoline)

OSTEP concepts to map:
- Ch3 maps to OSTEP Ch. 4-6 (Processes, Process API, Limited Direct Execution)
- Ch4 maps to OSTEP Ch. 13-16 (Address Spaces, Memory API, Address Translation, Paging)
- SpinLockIrq maps to OSTEP Ch. 28 (Locks) -- specifically the "interrupt disable" approach and why it is insufficient for multi-core alone

**Ch5-Ch6 SMP** (when implemented):

New concepts to teach:
- **Process management** (ch5): fork(), exec(), wait(), exit(). WHY: batch processing runs fixed programs. Process management lets programs create other programs dynamically -- the foundation of a shell.
- **Address space deep-copy** (ch5): fork() copies the entire page table tree and all mapped pages. Show the cost and why copy-on-write (COW) would be better (but is beyond scope).
- **Process tree** (ch5): Parent-child relationships, zombie processes, orphan cleanup. The ProcRel structure.
- **Stride scheduling** (ch5): Priority-based scheduling where each process has a stride that increases by BIG_STRIDE / priority after each time slice. Lower stride means higher effective priority.
- **Concurrent fork/wait/exit** (ch5-smp): The classic race: child calls exit() on hart 1 while parent calls wait() on hart 0. Both modify the dead_children map. Without a lock, updates are lost.
- **Per-hart current process** (ch5-smp): The global `current: Option<ProcId>` must become per-hart. Show what happens if two harts share a single current.
- **Coarse-grained vs fine-grained locking** (ch5-smp): One big lock around everything (simple, correct, but serializes) vs separate locks for queue/process-table/relationships (concurrent, but deadlock-prone). Teach lock ordering.
- **File system** (ch6): VirtIO block device, easy-fs, inodes, file descriptors. WHY: programs need persistent storage. Without a file system, all data is lost on shutdown.

rCore-v3 chapters to reference:
- Chapter 5: Process management
- Chapter 6: File system and I/O

OSTEP concepts to map:
- Ch5 maps to OSTEP Ch. 5 (Process API: fork, exec, wait)
- Ch5-SMP maps to OSTEP Ch. 28-31 (Locks, Lock-based Data Structures, Condition Variables, Semaphores)
- Ch6 maps to OSTEP Ch. 36-42 (I/O Devices, Hard Disk Drives, File System Implementation)

**Ch7-Ch8 SMP** (when implemented):

New concepts to teach:
- **IPC: Pipes and signals** (ch7)
- **Threads vs processes** (ch8): Shared address space, thread-local storage, why threads are cheaper than processes
- **Kernel synchronization primitives** (ch8): Mutexes, semaphores, condition variables -- built on top of spinlocks + sleep queues
- **Deadlock** (ch8-smp): The full theory -- necessary conditions (mutual exclusion, hold-and-wait, no preemption, circular wait), detection, prevention, avoidance

### 7. Update the "What This Tutorial Does Not Cover" section

After writing new parts, update the roadmap table at the end of the tutorial. Move topics from "not covered" to "covered in Part N". Add any new gaps discovered.

### 8. Update cross-references

Add new OSTEP chapter mappings and rCore-v3 comparisons at the end. Note where our approach differs from rCore-v3 (especially around SMP, which they do not cover).

### 9. Style rules

- **Voice**: Direct, technical, no filler. "Here is the problem. Here is the mechanism. Here is the code."
- **Length**: Each Part should be 150-300 lines. Do not pad, but do not skip important details.
- **Code**: Always real code from the repo with file references. Never fabricated "simplified" versions without explicit labeling.
- **NOTEs**: Be honest about what you are glossing over. A NOTE saying "this is a simplification; the full story involves X" is better than silence.
- **Diagrams**: ASCII art for memory layouts, privilege transitions, data flow. Keep them simple.
- **No emojis** unless the user explicitly requests them.
- **Chinese/English**: Write in English (matching the existing tutorial). Reference rCore-v3's Chinese source material but translate concepts.

## Output

Append the new Parts directly to `docs/reports/ch1-ch2-smp-tutorial.md`. Do not create a separate file. The tutorial is one continuous document that grows with each chapter pair.

After appending, update:
1. The "What This Tutorial Does Not Cover (Yet)" table -- move covered topics
2. The "Cross-References to Other OS Textbooks" section -- add new OSTEP/rCore-v3 mappings
3. The "Summary: Key Concepts" table -- add new concepts introduced

## Checklist

Before submitting the tutorial continuation:
- [ ] Every code snippet references an actual file in the repo
- [ ] Every code snippet matches the actual file content (verified by reading the file)
- [ ] Every register name, address, and CSR name is correct for RISC-V
- [ ] New concepts are motivated with WHY before WHAT
- [ ] SMP-specific content shows "before (single-core)" and "after (SMP)" with the specific change highlighted
- [ ] Bugs encountered during implementation are described with root cause analysis
- [ ] The "What This Tutorial Does Not Cover" section is updated
- [ ] Cross-references to OSTEP and rCore-v3 are added for new topics
- [ ] No fabricated code -- every snippet is real or explicitly labeled as simplified

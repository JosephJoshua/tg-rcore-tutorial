# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

TanGram rCore Tutorial — a modular RISC-V teaching OS kernel in Rust (Tsinghua AI4OSE Lab1). 23 crates organized in 4 dependency layers, with 8 progressive chapter kernels (ch1–ch8) that build from bare metal to concurrency/sync primitives. All crates share version 0.4.8, Rust edition 2024, GPL-3.0.

## Build & Run

Each chapter is an independent crate targeting `riscv64gc-unknown-none-elf`. Requires Rust stable (1.85.0+), QEMU >= 7.0, and `cargo-binutils`.

```bash
# Run a chapter kernel in QEMU
cd tg-rcore-tutorial-ch3
cargo run

# Run with exercise feature
cargo run --features exercise

# Check a component compiles (no QEMU needed)
cd tg-rcore-tutorial-syscall
cargo check
```

## Testing

Each chapter has a `test.sh` that pipes kernel output through `tg-rcore-tutorial-checker`:

```bash
cd tg-rcore-tutorial-ch3
bash test.sh          # base tests only
bash test.sh base     # same as above
bash test.sh exercise # exercise tests (--features exercise)
bash test.sh all      # both base and exercise
```

The checker is at `tg-rcore-tutorial-checker/` — install it or run it directly. CI runs all 8 chapters in parallel via `.github/workflows/ci.yml`.

## Architecture

### 4-Layer Crate Dependency Model

- **Layer 0 (base):** sbi, linker, console, kernel-context, kernel-alloc, kernel-vm, easy-fs, signal-defs, task-manage, checker — no internal dependencies
- **Layer 1:** syscall (→ signal-defs), signal (→ kernel-context, signal-defs), sync (→ task-manage)
- **Layer 2:** signal-impl (→ kernel-context, signal), user (→ console, syscall)
- **Layer 3 (chapters):** ch1→ch8, each adding components progressively (ch1: sbi only → ch8: all 12 components)

### Chapter Progression

| Chapter | Topic | Key additions |
|---------|-------|---------------|
| ch1 | Bare metal | SBI, panic handler |
| ch2 | Batch OS | Trap handling, syscalls |
| ch3 | Multiprogramming | Task scheduling, timer interrupts |
| ch4 | Virtual memory | Address spaces, page tables |
| ch5 | Process management | Fork/exec/wait |
| ch6 | File system | VirtIO block, inodes |
| ch7 | IPC + signals | Pipes, signal handlers |
| ch8 | Concurrency | Threads, mutexes, semaphores, condvars |

Chapters with exercises (exercise.md): ch3, ch4, ch5, ch6, ch8.

### Build Process (per chapter)

`build.rs` generates a linker script via `tg-linker`, cross-compiles user programs from `tg-rcore-tutorial-user/`, converts ELF→binary with `objcopy`, and embeds them into the kernel via generated assembly.

### Key Traits & Types

- `PageManager` (kernel-vm): page table management abstraction
- `LocalContext` (kernel-context): 31 regs + sepc, `execute()` for context switching
- Syscall traits in `tg-rcore-tutorial-syscall`: IO, FS, Signal, Time — kernel implements these, user calls them
- `tg-rcore-tutorial-syscall` uses `build.rs` to generate syscall numbers

## Git Subrepo Structure

Each `tg-rcore-tutorial-*` directory is a git subrepo with its own upstream. Use `git subrepo push/pull` for sync (requires git-subrepo tool installed via `scripts/install-git-subrepo.sh`).

## Publishing

All 23 crates publish to crates.io in dependency order:
```bash
bash scripts/publish-all.sh    # publish all
bash scripts/bump-version.sh   # unified version bump
```

The root crate bundles all subrepos as `bundle/submodules.tar.gz` for offline distribution.

## Container Environment

Pre-configured Docker images are available:
- `ghcr.io/chyyuu/tangram-crates:latest` (CI)
- `.devcontainer/` for VS Code with rust-analyzer configured for riscv64gc

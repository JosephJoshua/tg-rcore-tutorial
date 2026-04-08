# Operating Systems from Scratch: Ch1, Ch2, and SMP

A ground-up explanation of operating systems, kernels, and RISC-V assembly — taught through the ch1, ch2, and SMP implementations in this codebase.

---

## Part 1: What a Computer Actually Does

A CPU is a machine that does exactly one thing: **reads an instruction from memory, executes it, reads the next instruction, executes it**, forever. That's it. Everything else — windows, files, the internet — is built on top of this loop.

A RISC-V 64-bit CPU has:

- **Registers**: 32 tiny, ultra-fast storage slots named `x0` through `x31`. Each holds a 64-bit number. Registers have aliases: `x0` is always zero, `x1` is `ra` (return address), `x2` is `sp` (stack pointer), `x10`-`x17` are `a0`-`a7` (function arguments), `x5`-`x7` and `x28`-`x31` are `t0`-`t6` (temporaries). These are the only things the CPU can directly compute with.

- **Memory**: A big array of bytes. The CPU reads instructions and data from here. Each byte has an address (a number). On our system, memory starts at address `0x80000000` — this is where QEMU's simulated RAM lives.

- **Program Counter (PC)**: A special register holding the address of the next instruction to execute. After executing an instruction, PC usually advances by 4 bytes (each standard RISC-V instruction is 4 bytes).

> **NOTE**: Our target `riscv64gc` includes the "C" (compressed) extension, which allows 2-byte instructions for common operations. However, `ecall` is always 4 bytes, so the `mepc += 4` / `sepc += 4` logic in our trap handlers is correct for the specific case of advancing past `ecall`.

- **Control and Status Registers (CSRs)**: Special-purpose registers that control the CPU's behavior — privilege level, interrupt handling, memory protection. These are crucial for OS work. You read/write them with `csrr`/`csrw` instructions, not normal load/store.

### Registers Reference

| Register | ABI Name | Purpose |
|----------|----------|---------|
| x0 | zero | Hardwired to 0. Writes are ignored. |
| x1 | ra | Return address (where to go back after a function call) |
| x2 | sp | Stack pointer (top of the current function's scratch space) |
| x3 | gp | Global pointer (usually unused in bare-metal) |
| x4 | tp | Thread pointer (we use this for hart ID in SMP) |
| x5-x7 | t0-t2 | Temporary registers (caller-saved — functions can trash them) |
| x8-x9 | s0-s1 | Saved registers (callee-saved — functions must preserve them) |
| x10-x17 | a0-a7 | Argument/return registers (a0-a1 hold return values) |
| x18-x27 | s2-s11 | More saved registers |
| x28-x31 | t3-t6 | More temporaries |

### RISC-V Assembly Crash Course

Assembly is the human-readable form of machine instructions. Each line is one instruction:

```asm
# Arithmetic and data movement
li    t0, 42          # Load Immediate: t0 = 42
add   t1, t0, t0     # Add: t1 = t0 + t0 = 84
addi  t0, t1, 5      # Add Immediate: t0 = t1 + 5
mul   t0, t1, t2     # Multiply: t0 = t1 * t2
la    t2, some_label  # Load Address: t2 = address of some_label
mv    t0, t1          # Move: t0 = t1 (actually: addi t0, t1, 0)

# Memory access
ld    t3, 0(t2)       # Load Doubleword: t3 = *(uint64_t*)(t2 + 0)
sd    t3, 8(t2)       # Store Doubleword: *(uint64_t*)(t2 + 8) = t3

# Branching (changing what executes next)
beqz  a0, somewhere   # Branch if a0 == 0: jump to 'somewhere'
bnez  a0, somewhere   # Branch if a0 != 0
bge   t0, t1, label   # Branch if t0 >= t1 (signed)
j     label           # Unconditional jump

# Function calls
call  my_function     # Jump to my_function, save return address in ra
ret                   # Jump back to address in ra

# Bitwise operations
not   t1, t1           # Bitwise NOT: t1 = ~t1
and   t0, t0, t1      # Bitwise AND: t0 = t0 & t1

# Privileged / special
csrr  t0, mhartid     # Read CSR: t0 = value of mhartid register
csrw  mstatus, t0     # Write CSR: mstatus = t0
csrc  mip, t0         # CSR Clear bits: mip &= ~t0
csrrw sp, mscratch, sp # Atomic swap: tmp=mscratch; mscratch=sp; sp=tmp
ecall                  # Environment Call: trap to higher privilege level
mret                   # Return from M-mode trap (jumps to mepc register)
sret                   # Return from S-mode trap (jumps to sepc register)
fence.i                # Flush instruction cache (needed after writing code to memory)
wfi                    # Wait For Interrupt: sleep until something happens
```

**Key insight**: Assembly has no types, no safety, no abstractions. `t0` is just a 64-bit number. The CPU doesn't know if it's a pointer, an integer, a character, or garbage. *You* decide what it means.

### The Stack

A **stack** is a region of memory used for function calls. When a function is called, it needs space for local variables. It gets this space by moving the **stack pointer** (`sp`) downward (stacks grow toward lower addresses on RISC-V):

```
High addresses    ┌─────────────────┐
                  │ Caller's locals │
                  ├─────────────────┤ ← sp before call
                  │ ra (saved)      │
                  │ Local var 1     │
                  │ Local var 2     │
Low addresses     └─────────────────┘ ← sp during callee
```

Without a stack pointer set up, *you cannot call any function*. This is why the very first thing every program does is set `sp` to point to some valid memory.

---

## Part 2: Privilege Levels — Why an OS Exists

Here's the fundamental problem: if every program could do anything — write to any memory address, control any hardware device, shut down the machine — one buggy program would crash everything. A web browser bug could wipe your disk.

RISC-V solves this with **privilege levels** (also called "modes"):

```
┌──────────────────────────────────────────┐
│  M-mode (Machine)    — Highest privilege │  ← Firmware/SBI
│  Can do ANYTHING: configure hardware,    │
│  set memory protection, control other    │
│  privilege levels                        │
├──────────────────────────────────────────┤
│  S-mode (Supervisor) — Middle privilege  │  ← OS Kernel
│  Can manage virtual memory, handle       │
│  traps, run user programs               │
├──────────────────────────────────────────┤
│  U-mode (User)       — Lowest privilege  │  ← Applications
│  Can only compute and make syscalls.     │
│  Cannot touch hardware directly.         │
└──────────────────────────────────────────┘
```

**The CPU enforces these boundaries in hardware.** If U-mode code tries to execute a privileged instruction (like `csrw mstatus`), the CPU immediately **traps** — it stops the program and transfers control to the higher-privilege handler.

This is what an operating system fundamentally is: **the S-mode code that manages U-mode programs, using hardware-enforced privilege boundaries**.

### CSRs (Control and Status Registers)

These are special registers that control CPU behavior. Unlike general registers (x0-x31), CSRs are accessed with special instructions (`csrr`/`csrw`). Many CSRs are only accessible from M-mode.

**M-mode CSRs** (only accessible from M-mode):

| CSR | Purpose |
|-----|---------|
| `mhartid` | Read-only: this hart's unique ID number (0, 1, 2, 3...) |
| `mstatus` | CPU status: what privilege to return to on `mret`, interrupt enable bits |
| `mepc` | Where to jump when `mret` executes (M-mode Exception PC) |
| `mtvec` | Where the CPU jumps when an M-mode trap occurs |
| `mideleg` | Which interrupts to delegate down to S-mode |
| `medeleg` | Which exceptions to delegate down to S-mode |
| `mscratch` | Scratch register — we use it to stash the M-mode stack pointer |
| `mcause` | Why did we trap into M-mode? (exception code) |
| `mcounteren` | Which performance counters S-mode can read |
| `pmpaddr0` / `pmpcfg0` | Physical Memory Protection: what memory S-mode can access |

**S-mode CSRs** (accessible from S-mode and above):

| CSR | Purpose |
|-----|---------|
| `sstatus` | Like mstatus but for S-mode (SPP field controls sret target) |
| `scause` | Why did we trap into S-mode? (exception code or interrupt number) |
| `stvec` | Where the CPU jumps when an S-mode trap occurs |
| `sepc` | Where to return after handling an S-mode trap |
| `sscratch` | Scratch register (used to swap stack pointers in trap handler, like `mscratch`) |
| `stval` | Trap value: faulting address for page faults, or instruction for illegal-instruction |

> **NOTE**: There's also `scounteren` (S-mode counter enable) which controls whether U-mode can read counters like `rdtime`. The full delegation chain is: M sets `mcounteren` to allow S-mode, S sets `scounteren` to allow U-mode. We set `mcounteren = -1` in M-mode but don't touch `scounteren` in ch1/ch2 (no U-mode counter access needed).

### Traps: How Privilege Transitions Work

A **trap** is when the CPU stops normal execution and jumps to a handler. Two kinds:

1. **Exception**: Synchronous — the current instruction caused it. Examples: `ecall` (deliberate system call), illegal instruction, page fault.
2. **Interrupt**: Asynchronous — external event. Examples: timer fires, device needs attention.

After handling a trap, the handler executes **`mret`** (M-mode) or **`sret`** (S-mode) to return — these are *not* traps themselves, but trap-return instructions that jump back to `mepc`/`sepc` and restore the previous privilege level.

The flow:

```
S-mode code executes ecall instruction
    │
    ├─ CPU saves current PC into mepc
    ├─ CPU sets mcause to the trap reason
    ├─ CPU jumps to the address in mtvec
    ├─ CPU switches to M-mode
    │
    └─ Trap handler runs, does its work, then executes mret
        │
        └─ CPU jumps back to mepc, restores previous privilege
```

---

## Part 3: The Boot Process — What Happens When You Power On

When QEMU starts with `-bios none -kernel our_binary`:

1. RAM is zeroed
2. All CPUs (harts) begin executing at address `0x80000000` in M-mode
3. That's it. No OS, no loader, no setup. Just raw hardware.

Our binary is loaded so that `_m_start` is at exactly `0x80000000`.

### Walking Through m_entry.asm (Original Single-Core Version)

This is the very first code that runs on the CPU. We'll first look at the **original single-core version**, then see how SMP changes it in Part 6.

> Reference: `tg-rcore-tutorial-sbi/src/m_entry.asm` (original single-core SBI)

```asm
    .section .text.m_entry     # Linker places this at 0x80000000
    .globl _m_start
_m_start:
```

**`.section .text.m_entry`** — The linker script places this section at `0x80000000`. This is the very first code that runs.

#### Setting up the stack

```asm
    la sp, m_stack_top        # sp = address of stack top (end of a 16 KiB region)
    csrw mscratch, sp         # Save stack top for trap handler
```

The CPU just powered on. There's no stack. We load `sp` with the address of a pre-allocated 16 KiB region (`m_stack_lower_bound` to `m_stack_top`, defined later in the `.bss.m_stack` section of the same file). `mscratch` is a scratch CSR — we stash the stack pointer there so the trap handler can find it later (more on this below).

This code assumes **a single hart**. If 4 harts ran this simultaneously, they'd all set `sp` to the same `m_stack_top` address and corrupt each other's stacks. Part 6 shows how the SMP version fixes this.

#### Configuring the return privilege

```asm
    li t0, (1 << 11) | (1 << 7)
    csrw mstatus, t0
```

`mstatus` controls the CPU's behavior. We're setting two bit fields:
- **MPP** (bits 12:11) = `01` → "when `mret` executes, drop to **S-mode**"
- **MPIE** (bit 7) = `1` → "enable interrupts after `mret`"

This is how privilege transitions work: you *configure* the target privilege level in `mstatus`, then execute `mret`, and the CPU jumps to the address in `mepc` at the configured privilege level.

#### Setting the destination

```asm
    la t0, _start
    csrw mepc, t0
```

`mepc` = "Machine Exception Program Counter". When `mret` executes, the CPU jumps to this address. `_start` is the S-mode kernel entry point at `0x80200000`.

#### Setting the trap handler

```asm
    la t0, m_trap_vector
    csrw mtvec, t0
```

If S-mode code later executes `ecall` (e.g., to print a character via SBI), the CPU traps to M-mode and jumps to `mtvec`.

#### Delegating exceptions to S-mode

```asm
    li t0, 0xffff
    csrw mideleg, t0          # All interrupts → S-mode
    li t0, 0xffff
    li t1, (1 << 9)           # Exception 9: ecall from S-mode
    not t1, t1                # t1 = ~(1 << 9) = all bits set except bit 9
    and t0, t0, t1            # Clear bit 9
    csrw medeleg, t0          # All exceptions except S-mode ecall → S-mode
```

By default, ALL traps go to M-mode. That would be inefficient — the OS kernel (S-mode) should handle most traps itself. So we **delegate** everything to S-mode, *except* ecall from S-mode (because that's how the kernel calls SBI services — those must stay in M-mode).

#### Physical Memory Protection

```asm
    li t0, -1                 # pmpaddr0 = all 1s = entire address space
    csrw pmpaddr0, t0
    li t0, 0x0f               # TOR mode + Read + Write + eXecute
    csrw pmpcfg0, t0
```

By default, S-mode can't access any physical memory. PMP allows us to grant access. We allow everything here (teaching simplification).

#### Counter access and the big jump

```asm
    li t0, -1
    csrw mcounteren, t0       # Let S-mode read hardware counters (rdtime)

    mret                      # Jump to _start in S-mode!
```

After `mret`, the CPU is now in S-mode, executing at `_start` (`0x80200000`). M-mode's job is done (until the next `ecall`).

### The M-mode Trap Handler

When S-mode executes `ecall` (to print a character, set a timer, or shut down), the CPU:
1. Saves the current PC into `mepc`
2. Sets `mcause = 9` (ecall from S-mode)
3. Jumps to `mtvec` (our `m_trap_vector`)
4. Switches to M-mode

```asm
m_trap_vector:
    csrrw sp, mscratch, sp     # Atomic swap: sp ↔ mscratch
```

This single instruction is elegant. Before it: `sp` = S-mode stack, `mscratch` = M-mode stack. After it: `sp` = M-mode stack (we can push things), `mscratch` = S-mode stack (saved for later).

```asm
    addi sp, sp, -128          # Allocate 128 bytes on M-mode stack
    sd ra, 0(sp)               # Save registers that Rust handler might use
    sd t0, 8(sp)
    ...
    sd a7, 88(sp)
    call m_trap_handler         # Call the Rust function in msbi.rs
```

After the Rust handler returns:

```asm
    csrr t0, mepc              # Read the trapped instruction's address
    addi t0, t0, 4             # Advance past the ecall (4 bytes per instruction)
    csrw mepc, t0              # So mret returns to the NEXT instruction
```

Without this, `mret` would jump back to the `ecall` instruction, which would trap again — infinite loop.

```asm
    ld ra, 0(sp)               # Restore registers
    ...                         # (but NOT a0/a1 — they hold the SBI return value)
    addi sp, sp, 128
    csrrw sp, mscratch, sp     # Swap back: restore S-mode stack
    mret                        # Return to S-mode
```

The SBI call is complete. S-mode code resumes right after its `ecall`, with the result in `a0`/`a1`.

---

## Part 4: Ch1 — The Simplest S-mode Program

Ch1 is the "Hello World" of OS development. No user programs, no traps from U-mode, no memory management. Just S-mode code that prints text and renders graphics.

We'll first look at the **original single-core ch1**, then the SMP version in Part 6.

> Reference: `tg-rcore-tutorial-ch1/src/main.rs` (original single-core ch1)

### _start: The First S-mode Instruction

```rust
#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
unsafe extern "C" fn _start() -> ! {
    const STACK_SIZE: usize = 64 * 1024;  // 64 KiB

    #[unsafe(link_section = ".bss.uninit")]
    static mut STACK: [u8; STACK_SIZE] = [0u8; STACK_SIZE];

    core::arch::naked_asm!(
        "la sp, {stack} + {stack_size}",   // sp = top of stack
        "j  {main}",                        // jump to rust_main
        stack_size = const STACK_SIZE,
        stack      =   sym STACK,
        main       =   sym rust_main,
    )
}
```

This is a **naked function** (`#[unsafe(naked)]`) — the compiler emits no prologue or epilogue, just the assembly you write. Why? Because when we arrive here from `mret`, there's no S-mode stack yet. We can't call any Rust function until `sp` points to valid memory.

`#[unsafe(link_section = ".text.entry")]` ensures the linker places this function at the very start of the S-mode region (`0x80200000`). `#[unsafe(no_mangle)]` preserves the symbol name `_start` so the M-mode code's `la t0, _start` can find it.

The two assembly instructions:
1. `la sp, STACK + 65536` — Set `sp` to the top of a 64 KiB array (stacks grow downward, so we start at the highest address)
2. `j rust_main` — Jump to the Rust main function, which now has a valid stack

### rust_main: What Ch1 Does

```rust
extern "C" fn rust_main() -> ! {
    for c in b"Hello, world!\n" {
        console_putchar(*c);     // Each character → SBI ecall → M-mode UART write
    }

    // VirtIO-GPU tangram rendering (omitted for brevity):
    // - Initialize VirtIO-GPU device (scan MMIO address space 0x10001000-0x10008000)
    // - Get framebuffer (1280x800 BGRA pixels)
    // - Render tangram "OS" shapes using scanline polygon fill
    // - Flush to display

    shutdown(false)
}
```

### The SBI Call Chain

`console_putchar` is where the privilege layers become visible. Let me trace the full path one character takes:

```
rust_main() calls console_putchar(b'H')
  → sbi_call(eid=0x01, fid=0, arg0='H')
    → inline assembly executes ecall    ← TRAP: S-mode → M-mode
      → m_trap_vector (assembly)
        → csrrw sp, mscratch, sp       (switch to M-mode stack)
        → save registers to stack
        → call m_trap_handler (Rust)
          → check mcause == 9 (ecall from S-mode)
          → dispatch on eid=0x01 → handle_console_putchar('H')
            → uart::putchar('H')
              → write_volatile(0x10000000, 'H')   ← actual UART hardware write
          → return SbiRet { error: 0, value: 0 }
        → mepc += 4 (skip past ecall)
        → restore registers (a0/a1 = return value)
        → csrrw sp, mscratch, sp       (switch back to S-mode stack)
        → mret                          ← RETURN: M-mode → S-mode
    → sbi_call returns the value
  → console_putchar returns
```

The UART at address `0x10000000` is a **memory-mapped I/O device** — you write a byte to that address, and instead of changing RAM, it sends a character out the serial port. QEMU's `-serial stdio` connects this virtual serial port to your terminal.

`write_volatile` is crucial — it tells the compiler "this write has a side effect, don't optimize it away." Without it, the compiler could see "we're writing to an address nobody reads from" and delete the write entirely.

#### How `sbi_call` Works: The `nobios` Dual-Return

The SBI crate has two versions of `sbi_call`, selected at compile time by the `nobios` feature:

```rust
// With nobios: our own M-mode handler returns SbiRet { error, value }
// a0 = error code (0 = success, negative = failure)
// a1 = return value
fn sbi_call(eid: usize, fid: usize, arg0: usize, ...) -> usize {
    let ret1: isize;  // error (a0)
    let ret2: usize;  // value (a1)
    unsafe {
        core::arch::asm!("ecall",
            inlateout("x10") arg0 => ret1,
            inlateout("x11") arg1 => ret2,
            in("x16") fid, in("x17") eid,
        );
    }
    if ret1 < 0 { panic!("SBI call failed: {}", ret1); }
    ret2  // Return the value, not the error code
}
```

> Reference: `tg-rcore-tutorial-sbi/src/lib.rs`, lines 97-119

The register convention follows the RISC-V SBI specification: `x17` (a7) = extension ID, `x16` (a6) = function ID, `x10` (a0) = first argument and error return, `x11` (a1) = value return.

### Memory Layout

The linker script arranges everything in physical memory:

```
0x80000000  ┌──────────────────┐
            │ .text.m_entry    │  M-mode boot code (_m_start)
            │ .text.m_trap     │  M-mode trap handler
            │ .bss.m_stack     │  M-mode stack (16 KiB)
            │                  │
0x80200000  ├──────────────────┤
            │ .text.entry      │  S-mode entry (_start) — MUST be first
            │ .text            │  S-mode kernel code
            │ .rodata          │  Read-only data (strings, constants)
            │ .data            │  Initialized globals
            │ .bss             │  Uninitialized globals (zeroed)
            │ (STACK / STACKS) │  64 KiB (single-core) or 4 × 64 KiB (SMP)
            │ (allocator pool) │  5 MiB for VirtIO GPU DMA
            └──────────────────┘
```

The 2 MiB gap (0x80000000 → 0x80200000) separates M-mode and S-mode code. This is a convention, not a hardware requirement.

---

## Part 5: Ch2 — A Batch Operating System

Ch2 adds the three things that make it a real OS:

1. **User programs** (U-mode code loaded into memory)
2. **Privilege transitions** (S-mode ↔ U-mode via `sret`/traps)
3. **System calls** (U-mode programs request services from the kernel via `ecall`)

> Reference: `tg-rcore-tutorial-ch2/src/main.rs` (original single-core ch2)

### What `rust_main` Does Before Running Programs

Before executing any user program, the kernel must initialize itself:

```rust
extern "C" fn rust_main() -> ! {
    // Step 1: Clear BSS
    unsafe { tg_linker::KernelLayout::locate().zero_bss() };
    // Step 2: Initialize console
    tg_console::init_console(&Console);
    // Step 3: Initialize syscall handlers
    tg_syscall::init_io(&SyscallContext);
    tg_syscall::init_process(&SyscallContext);
    // Step 4: Run batch processing loop...
}
```

**BSS clearing** (`zero_bss()`): The `.bss` section holds global variables that are initialized to zero (e.g., `static mut COUNT: u32 = 0`). The linker marks where BSS starts (`__sbss`) and ends (`__ebss`), but the actual memory isn't zeroed by the loader — QEMU happens to zero RAM, but a real machine might have garbage. `zero_bss()` writes zeros from `__sbss` to `__ebss`. This must happen before any code reads a global variable.

**Console initialization**: Sets up `println!` by registering a `Console` trait object. In our codebase, `println!` works through Rust's `core::fmt::Write` trait:

```rust
struct Console;
impl tg_console::Console for Console {
    fn put_char(&self, c: u8) {
        tg_sbi::console_putchar(c);  // → ecall → M-mode → UART
    }
}
```

When you write `println!("hello {}", 42)`, Rust's formatting machinery calls `put_char` for each byte of the formatted string. Each `put_char` calls SBI, which calls `ecall`, which traps to M-mode — so printing "hello 42\n" involves 9 individual `ecall` trap round-trips.

> **NOTE**: This is why multi-hart output interleaves at the character level, not the line level. Between any two `put_char` calls, another hart can execute its own `put_char`. This is the root cause of garbled output in SMP Demo 1.

### Loading User Programs

The build script (`build.rs`) cross-compiles user programs, converts them to raw binary, and embeds them into the kernel image:

```asm
apps:
    .quad 0x80400000       # base address for loading
    .quad 0x200000         # step between apps
    .quad 22               # number of apps
    .quad app_0_start
    ...
app_0_start:
    .incbin "hello_world.bin"    # Raw bytes of the compiled program
app_0_end:
```

At runtime, `AppMeta::locate()` reads this table to find each app's bytes.

### The Execution Cycle

```rust
for (i, app) in tg_linker::AppMeta::locate().iter().enumerate() {
    let mut ctx = LocalContext::user(app_base);    // Create U-mode context
    // ... set up user stack ...

    loop {
        unsafe { ctx.execute() };      // Drop to U-mode, run user code

        // We only get here when the user program traps back to S-mode
        match scause::read().cause() {
            Trap::Exception(Exception::UserEnvCall) => {
                // User did ecall → handle the syscall
                match handle_syscall(&mut ctx) {
                    Done => continue,         // Resume user program
                    Exit(code) => break,      // User wants to exit
                    Error(id) => break,       // Unknown syscall, kill app
                }
            }
            trap => break,  // Illegal instruction, page fault, etc. → kill app
        }
    }
}
```

### Context Switching: The Heart of the Kernel

`LocalContext` holds all 31 general-purpose registers plus `sepc` (the user program's PC). `ctx.execute()` does this:

```
S-mode kernel                        U-mode user program
─────────────                        ──────────────────
ctx.execute():
  save kernel registers to stack
  restore user registers from ctx
  csrw sepc, <user's PC>
  csrw sstatus, <SPP=User>
  sret  ──────────────────────────►  User program runs
                                     ...
                                     ecall (or exception)
  ◄──────────────────────────────    CPU traps:
  save user registers to ctx           saves PC to sepc
  restore kernel registers              sets scause
  return from execute()                 jumps to stvec

  kernel reads scause, handles it
```

The key insight: **`sret` and traps are symmetric.** `sret` goes kernel→user. Traps go user→kernel. The register save/restore around them makes it look like `ctx.execute()` is a normal function call that "blocks" while the user program runs.

#### What `LocalContext` Contains

`LocalContext` (from the `tg-kernel-context` crate) stores a complete snapshot of the CPU state needed to resume a user program:

- **31 general-purpose registers** (x1-x31; x0 is always zero, not stored)
- **`sepc`** — the user program's current instruction address

When `ctx.execute()` runs, the assembly inside it:
1. Saves all kernel (S-mode) registers to the kernel stack
2. Writes `sstatus` with `SPP=0` (User mode) so `sret` will drop to U-mode
3. Writes `sepc` from the `LocalContext` (the user program's PC)
4. Loads all 31 user registers from the `LocalContext`
5. Executes `sret` — jumps to `sepc` in U-mode

When the user traps back (via `ecall` or exception), the hardware saves the user's PC into `sepc` and jumps to `stvec`. The trap handler assembly reverses the process: saves user registers into `LocalContext`, restores kernel registers, and returns from `ctx.execute()`.

> **NOTE**: rCore-Tutorial-v3 calls this structure `TrapContext` and shows the full save/restore assembly in `trap.S` (saving all 32 registers + `sstatus` + `sepc`). Our `tg-kernel-context` crate encapsulates this in `execute()`, hiding the assembly. The mechanics are identical — the abstraction level differs.

> **NOTE (Ch4+ preview)**: In chapters with virtual memory, the context also stores `kernel_satp` (kernel page table pointer), `kernel_sp` (kernel stack), and `trap_handler` address. This is because switching `satp` (page tables) during a trap requires a **trampoline page** — code mapped at the same virtual address in both kernel and user page tables. This is not relevant for ch1-ch2 (no virtual memory) but becomes critical in ch4.

#### User Stack: Surprisingly Small

Each user program gets a **4 KiB stack** — just 512 `usize` values:

```rust
let mut user_stack: MaybeUninit<[usize; 512]> = MaybeUninit::uninit();
*ctx.sp_mut() = unsafe { user_stack_ptr.add(512) } as usize;
```

`MaybeUninit` avoids wasting time zeroing memory that the user program will overwrite. This stack is allocated on the *kernel's* stack as a local variable — so the kernel stack must be large enough to hold it (ch2 uses 32 KiB kernel stack for this reason).

> **NOTE**: rCore-Tutorial-v3 gives each task a separate 8 KiB user stack in dedicated memory, plus a separate 8 KiB kernel stack per task. Our ch2 takes a simpler approach: one shared kernel stack, with user stacks as local variables. This works because ch2 is single-core and runs one task at a time.

#### `fence.i`: Why the Instruction Cache Needs Flushing

After each user program finishes, before loading the next one:

```rust
unsafe { core::arch::asm!("fence.i") };
```

Ch2 loads all user programs to the same base address (`0x80400000`). When app 0 finishes and app 1 is loaded to the same address, the **instruction cache** might still hold app 0's instructions at that address. `fence.i` tells the CPU: "the contents of memory that I might execute as instructions have changed — discard your instruction cache." Without this, the CPU might execute stale instructions from the previous program.

### System Calls: How User Programs Talk to the Kernel

A user program can't directly print to the screen (UART is a hardware device requiring privilege). Instead, it asks the kernel:

```rust
// In user program (U-mode):
fn syscall(id: usize, a0: usize, a1: usize, a2: usize) -> isize {
    let ret;
    unsafe {
        asm!(
            "ecall",           // Trap to S-mode
            in("a7") id,      // Syscall number in a7
            inlateout("a0") a0 => ret,  // Arg 0 / return value in a0
            in("a1") a1,
            in("a2") a2,
        );
    }
    ret
}
```

The kernel reads these from the saved context:

```rust
fn handle_syscall(ctx: &mut LocalContext) -> SyscallResult {
    let id = ctx.a(7).into();       // a7 = syscall ID (e.g., 64 = write)
    let args = [ctx.a(0), ...];     // a0-a5 = arguments

    // ... dispatch to handler ...

    *ctx.a_mut(0) = ret as _;       // Write return value to a0
    ctx.move_next();                // sepc += 4 (skip past ecall)
}
```

`ctx.move_next()` is essential: `ecall` sets `sepc` to the address *of* the `ecall` instruction. If we didn't advance it by 4 bytes, `sret` would jump back to the same `ecall`, trapping again infinitely.

### The Full Privilege Sandwich

One character of user output crosses the privilege boundary **four times**:

```
    ┌─────────────────────────────────────┐
    │ User program (U-mode)               │
    │   write(1, "H", 1) → ecall         │
    ├─────────────────────────────────────┤
    │ Kernel (S-mode)                     │  ← scause=8 (ecall from U-mode)
    │   handle_syscall → IO::write        │
    │   console_putchar → ecall           │
    ├─────────────────────────────────────┤
    │ SBI (M-mode)                        │  ← mcause=9 (ecall from S-mode)
    │   UART write at 0x10000000          │
    │   → mret                            │
    ├─────────────────────────────────────┤
    │ Kernel (S-mode)                     │
    │   → sret                            │
    ├─────────────────────────────────────┤
    │ User program (U-mode)               │
    │   ecall returns, 'H' was printed    │
    └─────────────────────────────────────┘
```

---

## Part 6: SMP — Multiple Cores Running Simultaneously

Everything above assumed one CPU core. Now we add three more. This changes *everything*.

### What Is a Hart?

RISC-V calls a hardware thread a **hart** (Hardware Thread). With `-smp 4`, QEMU simulates 4 harts. Each hart has:
- Its own set of 32 general-purpose registers
- Its own PC
- Its own CSRs (`mstatus`, `mepc`, `mhartid`, `mscratch`, etc.)
- **Shared memory** — all harts see the same RAM

**CSRs are per-hart, but memory is shared.** When hart 0 writes `csrw mstatus, t0`, only hart 0's `mstatus` changes. But when hart 0 writes `sd t0, 0(t1)` (store to memory), all harts can see that write.

### The Fundamental Challenge

With `-bios none -smp 4`, QEMU starts **all 4 harts executing `_m_start` simultaneously**. Not one after another — simultaneously.

The original single-hart M-mode entry (in `tg-rcore-tutorial-sbi/src/m_entry.asm`) begins with:

```asm
la sp, m_stack_top      # Set sp to the top of a single 16 KiB stack
csrw mscratch, sp       # Save for trap handler
```

If 4 harts ran this simultaneously, they'd all set `sp` to the same `m_stack_top` address! When hart 0 pushes data, hart 1 overwrites it. Total corruption. Every function call on every hart writes to the same memory.

### SMP M-mode Boot

The very first instruction must identify which hart we are:

> Reference: `jsph-tg-rcore-tutorial-sbi-smp/src/m_entry.asm`

```asm
_m_start:
    csrr  t0, mhartid          # t0 = this hart's unique ID (0, 1, 2, or 3)
```

`mhartid` is a read-only CSR containing the hardware-assigned hart ID. This is the one thing that's different between harts at boot.

#### Overflow guard

```asm
    li    t1, NUM_HARTS         # t1 = 4
    bge   t0, t1, .park_hart   # if hartid >= 4, go to sleep forever

.park_hart:
    wfi                         # Wait For Interrupt (sleep)
    j     .park_hart            # loop forever
```

If QEMU starts more harts than we have stacks for (e.g., `-smp 8`), excess harts sleep forever.

#### Per-hart stack calculation

```asm
    la    t1, m_stacks_base     # t1 = base of stack array
    li    t2, M_STACK_SIZE      # t2 = 4096
    addi  t3, t0, 1             # t3 = hartid + 1
    mul   t2, t2, t3            # t2 = (hartid + 1) * 4096
    add   sp, t1, t2            # sp = base + offset
```

This gives each hart its own 4 KiB region:

```
m_stacks_base ──► ┌──────────┐
                  │ Hart 0   │ 4 KiB     sp(hart0) = base + 4096
                  ├──────────┤
                  │ Hart 1   │ 4 KiB     sp(hart1) = base + 8192
                  ├──────────┤
                  │ Hart 2   │ 4 KiB     sp(hart2) = base + 12288
                  ├──────────┤
                  │ Hart 3   │ 4 KiB     sp(hart3) = base + 16384
m_stacks_top  ──► └──────────┘
```

Why `hartid + 1`? Because stacks grow downward. Hart 0's stack starts at `base + 4096` and grows down toward `base`. Hart 1's starts at `base + 8192` and grows down toward `base + 4096`. No overlap.

#### Per-hart mscratch

```asm
    csrw  mscratch, sp          # Each hart saves ITS OWN stack top
```

Since `csrw` is per-hart, hart 0's `mscratch` gets hart 0's stack top, hart 1 gets its own, etc. When the M-mode trap handler fires, `csrrw sp, mscratch, sp` gives each hart its own trap stack.

#### CSR configuration and the jump to S-mode

```asm
    # All the same CSR setup as single-core (mstatus, mepc, mtvec, etc.)
    # Each hart does this independently — CSRs are per-hart, no locks needed

    csrr  a0, mhartid           # a0 = hart ID (passed to S-mode)
    mret                        # Jump to _start in S-mode
```

#### Per-hart CLINT timer

The CLINT (Core Local Interruptor) has separate timer registers for each hart:

```
Hart 0: mtimecmp at 0x2004000
Hart 1: mtimecmp at 0x2004008
Hart 2: mtimecmp at 0x2004010
Hart 3: mtimecmp at 0x2004018
```

The original code hardcoded `0x2004000`. The SMP version computes the correct address:

```rust
fn handle_timer(time: u64) -> SbiRet {
    let hartid: usize;
    unsafe { core::arch::asm!("csrr {}, mhartid", out(reg) hartid) };
    let mtimecmp_addr = 0x200_4000 + 8 * hartid;
    unsafe { (mtimecmp_addr as *mut u64).write_volatile(time) };
    // ...
}
```

### S-mode Multi-Hart Entry

Compare the original single-core `_start` from Part 4 (one stack, no branching) with the SMP version:

> Reference: `jsph-tg-rcore-tutorial-ch1-smp/src/main.rs` (SMP version)

All 4 harts arrive at `_start` nearly simultaneously, each with `a0 = their hartid`:

```rust
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!(
        "mv   tp, a0",              // Save hart ID to tp register IMMEDIATELY
        "la   t0, {stacks}",
        "li   t1, {stack_size}",
        "addi t2, a0, 1",
        "mul  t1, t1, t2",
        "add  sp, t0, t1",          // Per-hart S-mode stack (64 KiB each)
        "bnez a0, {secondary}",     // if hartid != 0 → secondary_main
        "j    {main}",              // hart 0 → rust_main
    )
}
```

**Why `mv tp, a0` is the very first instruction:** `a0` holds the hart ID from M-mode, and any subsequent instruction might overwrite it. The `tp` (thread pointer) register is unused in `no_std` Rust (no thread-local storage), so it's safe permanent storage.

**The branch:** Hart 0 goes to `rust_main` (initialization + demos). Harts 1-3 go to `secondary_main` (wait, then participate).

---

## Part 7: The Concurrency Problem

Multiple harts sharing memory creates problems that don't exist in single-core systems.

### Problem 1: The Boot Race

Hart 0 must initialize the system (clear BSS, set up console) before secondary harts use anything. But all harts arrive at `_start` at roughly the same time.

**Solution: An atomic boot flag with memory ordering.**

> Reference: `jsph-tg-rcore-tutorial-ch1-smp/src/smp.rs`

```rust
static BOOT_HART_DONE: AtomicBool = AtomicBool::new(false);

// Hart 0 (after initialization):
BOOT_HART_DONE.store(true, Ordering::Release);

// Secondary harts:
while !BOOT_HART_DONE.load(Ordering::Acquire) {
    core::hint::spin_loop();
}
```

The `Release`/`Acquire` pair is about **memory ordering**. Modern CPUs (and compilers) may reorder memory operations for performance. Without ordering constraints:

```
Hart 0: BOOT_HART_DONE = true    ← CPU might execute this BEFORE console_init!
Hart 0: console_init()           ← Too late, hart 1 already saw the flag
Hart 1: sees BOOT_HART_DONE == true
Hart 1: tries to print → crash (console not initialized yet)
```

`Release` ordering means: "all my writes before this store must be visible to anyone who does an `Acquire` load of this same variable." When hart 1 loads `true` with `Acquire`, it's guaranteed to see all of hart 0's prior writes.

On RISC-V, this compiles to a `fence` instruction — a hardware memory barrier.

### Problem 2: UART Character Interleaving

SBI's `console_putchar` writes one byte at a time. When two harts print "Hello" simultaneously:

```
Hart 0 sends: H-e-l-l-o
Hart 1 sends: H-e-l-l-o
UART receives: H-H-e-e-l-l-l-l-o-o  (or any random interleaving)
```

This is Demo 1 in ch1-smp — the garbled output.

### The SpinLock

A lock ensures only one hart enters a critical section at a time.

```rust
pub struct SpinLock {
    locked: AtomicBool,
}
```

`AtomicBool` is special — reads and writes to it are **atomic**, meaning the hardware guarantees no other hart can see a "half-written" value.

#### The TTAS (Test-and-Test-and-Set) algorithm

```rust
pub fn lock(&self) {
    loop {
        // INNER LOOP: just READ the flag (cheap — stays in local cache)
        while self.locked.load(Ordering::Relaxed) {
            core::hint::spin_loop();
        }
        // Lock LOOKS free. Try to grab it with an atomic compare-and-swap.
        if self.locked.compare_exchange_weak(
            false,              // Expected: currently unlocked
            true,               // Desired: set to locked
            Ordering::Acquire,  // On success: acquire ordering
            Ordering::Relaxed,  // On failure: relaxed
        ).is_ok() {
            return;  // We got the lock!
        }
        // Someone else grabbed it between our read and CAS. Try again.
    }
}

pub fn unlock(&self) {
    self.locked.store(false, Ordering::Release);
}
```

**`compare_exchange_weak`** (CAS) does this atomically — as one indivisible operation:
1. Read `locked`
2. If it equals `false` (expected), set it to `true` (desired) and return `Ok`
3. If it equals `true` (someone else holds it), don't change it, return `Err`

No two harts can both succeed — the hardware guarantees this. On RISC-V, this compiles to an LR/SC (Load-Reserved / Store-Conditional) instruction pair.

**Why the inner `load(Relaxed)` loop?** Without it, every iteration would execute the expensive CAS instruction, which requires exclusive cache line ownership (a bus transaction between cores). The inner loop just reads the cache line locally — no bus traffic — until the lock appears free. This **TTAS** pattern is the standard optimization for spinlocks.

### The Barrier

A barrier is a meeting point: all harts must arrive before any can proceed.

The naive approach — a simple counter — has an ABA problem:

```
First barrier (Demo 1 → Demo 2):
  Hart 3 (fast): increments count, sees count == 4 → continues
  Hart 3: enters SECOND barrier, increments count (now 1)
  Hart 0 (slow): still in FIRST barrier
  Hart 0: sees count = 1 (not 4), waits forever → DEADLOCK
```

The fix is a **generation counter**:

```rust
pub struct Barrier {
    count: AtomicUsize,
    generation: AtomicUsize,
}

pub fn wait(&self, num_harts: usize) {
    let cur_gen = self.generation.load(Ordering::Acquire);
    if self.count.fetch_add(1, Ordering::AcqRel) + 1 == num_harts {
        // I'm the last to arrive — reset count, advance generation
        self.count.store(0, Ordering::Relaxed);
        self.generation.store(cur_gen.wrapping_add(1), Ordering::Release);
    } else {
        // Wait for generation to CHANGE (not for count to reach target)
        while self.generation.load(Ordering::Acquire) == cur_gen {
            core::hint::spin_loop();
        }
    }
}
```

Each hart remembers the generation when it entered. It waits for the generation to *change*, not for the count to reach a target. A fast hart re-entering the barrier increments the count, but waits on a *new* generation.

---

## Part 8: Putting It All Together

### Ch1-SMP: The Full Boot Sequence

```
Time 0: QEMU starts, 4 harts at 0x80000000 (M-mode)
        All execute _m_start simultaneously

Time 1: Each hart reads mhartid (0, 1, 2, 3)
        Each computes per-hart M-mode stack
        Each configures CSRs independently
        Each executes mret → _start at 0x80200000 (S-mode)

Time 2: Each hart in _start:
        mv tp, a0  (save hartid to tp register)
        Compute per-hart S-mode stack
        Hart 0 → rust_main
        Harts 1,2,3 → secondary_main

Time 3: Hart 0: prints "Hello, world!"
        Hart 0: ACTIVE_HARTS++ (= 1), BOOT_HART_DONE = true
        Harts 1,2,3: see flag, ACTIVE_HARTS++ (= 2, 3, 4)

Time 4: Hart 0: waits 10ms, reads ACTIVE_HARTS = 4
        Hart 0: publishes DEMO_HART_COUNT = 4
        All harts enter demo_sequence(hartid, 4)

Time 5: Demo 1 — all 4 harts print without lock → garbled output
        Barrier synchronizes all 4 harts

Time 6: Demo 2 — all 4 harts print with spinlock → clean output
        Barrier synchronizes all 4 harts

Time 7: Demo 3 — hart 0 does 4 sequential workloads (baseline)
        Then all 4 harts compute in parallel
        Hart 0 reports: "Speedup: 3.8x"

Time 8: Hart 0 renders tangram on VirtIO-GPU, shuts down
        Harts 1,2,3 enter WFI loop
```

### Ch2-SMP: Multi-Core Batch OS

Ch2-SMP is simpler — secondary harts don't participate in demos. They just prove they exist and go to sleep:

```
Time 0-2: Same M-mode and _start sequence

Time 3: Hart 0: zero_bss(), init console, init syscalls
        Hart 0: prints "[Hart 0] primary, running batch processing"
        Hart 0: BOOT_HART_DONE = true

Time 4: Harts 1,2,3: see flag, print "[Hart N] online, entering idle loop"
        Harts 1,2,3: set stvec to panic trap, enter WFI

Time 5+: Hart 0 runs all 22 user programs in batch mode
         (same as original ch2: load → U-mode → syscalls → next app)

Final: Hart 0 calls shutdown()
```

### The BSS Race Bug (Ch2-SMP Specific)

In ch2-smp, `HART_STACKS` was originally placed in `.bss.uninit` (inside the `.bss` section). When hart 0 called `tg_linker::KernelLayout::locate().zero_bss()`, it wiped everything from `__sbss` to `__ebss` — which included all 4 harts' stacks, even while secondary harts were already using them. Fix: place `HART_STACKS` in the `.boot.stack` section, which the linker script puts *after* `__ebss`, outside the zeroed range.

Ch1-smp doesn't have this problem because ch1 never calls `zero_bss()` (it doesn't use `tg_linker`). QEMU zeroes all RAM at startup, so ch1's `.bss` is already zero. But ch2-ch5 all call `zero_bss()`, so any SMP version of these chapters must use `.boot.stack` for hart stacks.

---


## Part 9: Multiprogramming and Preemptive Scheduling -- Why Batch Is Not Enough

Ch2 ran user programs one at a time: load, execute, handle syscalls, exit, load next. This is **batch processing** -- simple, but hugely wasteful. When a program does a slow I/O operation (reading from a device, waiting for network), the CPU sits idle. No other program can run, even if there are dozens waiting.

**Multiprogramming** loads all programs into memory simultaneously. When one program is waiting, the CPU switches to another. This eliminates most idle time.

But multiprogramming alone has a problem: a compute-bound program (one that never does I/O) will hog the CPU forever. The only way another program gets to run is if the running program voluntarily yields. This is **cooperative scheduling**, and it breaks as soon as any program is buggy or selfish.

**Preemptive scheduling** solves this: the hardware timer fires after a fixed interval (a **time slice**), forcing the kernel to take control regardless of what the program is doing. The kernel then picks the next program to run. This gives every program a fair share of CPU time -- the illusion of simultaneous execution.

### The Timer Interrupt

RISC-V provides a timer via the CLINT (Core Local Interruptor). The kernel sets a deadline by writing to `mtimecmp` (through SBI's `set_timer`), and when the hardware counter `mtime` reaches that value, a timer interrupt fires.

In our kernel, each time a task starts running, we set the timer 12,500 ticks ahead (approximately 1 ms on QEMU's 12.5 MHz clock):

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs`

```rust
tg_sbi::set_timer(time::read64() + 12500);
```

When the timer fires, the M-mode SBI handles the hardware timer interrupt and injects a **supervisor timer interrupt** (by setting the STIP bit in `sip`). The S-mode kernel then sees this as `scause` = `Trap::Interrupt(Interrupt::SupervisorTimer)`. The kernel re-enqueues the current task and picks the next one.

After handling the timer, we set the deadline to `u64::MAX` (effectively infinity) to prevent the timer from firing again while we're in the kernel:

```rust
tg_sbi::set_timer(u64::MAX);
```

The timer only matters while a user program is running. Setting it before `execute()` and clearing it after the trap return keeps kernel code safe from spurious timer interrupts.

> **NOTE**: As we saw in Part 6, the CLINT has separate `mtimecmp` registers for each hart (hart 0 at `0x2004000`, hart 1 at `0x2004008`, etc.). The SBI's `set_timer` implementation reads `mhartid` to compute the correct address. Each hart's timer is independent -- setting hart 0's timer does not affect hart 1's.

### Task Control Block

Each user program needs its own saved state. In ch3, this is the **Task Control Block** (TCB):

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/task.rs`

```rust
pub struct TaskControlBlock {
    ctx: LocalContext,        // 31 registers + sepc
    pub finish: bool,         // Has this task exited?
    stack: [usize; 1024],     // Per-task user stack (8 KiB)
}
```

`LocalContext` is the same context structure from Part 5 (ch2). It stores all 31 general-purpose registers and the program counter (`sepc`). When `tcb.execute()` runs, it restores these registers, drops to U-mode via `sret`, and runs the user program. When the user traps back, the registers are saved back into the TCB.

### The Single-Core Scheduling Loop

In the original single-core ch3, scheduling is a simple round-robin loop over an array of TCBs:

> Reference: `tg-rcore-tutorial-ch3/src/main.rs` (simplified)

```rust
let mut remain = num_apps;
let mut i = 0;
while remain > 0 {
    let tcb = &mut tcbs[i];
    if !tcb.finish {
        tg_sbi::set_timer(time::read64() + 12500);   // 1 ms time slice
        unsafe { tcb.execute() };                      // Drop to U-mode
        // ... handle trap (timer -> continue, exit -> remain -= 1) ...
    }
    i = (i + 1) % num_apps;   // Round-robin to next task
}
```

No locks, no queues, no synchronization. One CPU, one task at a time. The index `i` walks through the array; each slot is either running or finished. When all slots are finished (`remain == 0`), the kernel shuts down.

### How SMP Changes This

With 4 harts, we can't iterate an array -- all 4 harts would pick the same task. We need a **shared data structure** that lets each hart atomically claim the next available task.

#### The Shared Ready Queue

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs`

```rust
struct QueueState {
    queue: [usize; APP_CAPACITY],   // Ring buffer of ready task indices
    head: usize,                     // Next to dequeue
    tail: usize,                     // Next slot to enqueue
    count: usize,                    // Elements in queue
    finished_count: usize,           // Tasks done
    total_tasks: usize,              // Total loaded
}

static QUEUE: smp::SpinLockIrq<QueueState> = smp::SpinLockIrq::new(QueueState::new());
```

A fixed-size ring buffer. Hart 0 pushes all task indices at boot; each hart pops from the front, runs the task, and pushes it back (or marks it finished).

Why a ring buffer instead of a `VecDeque`? Ch3 has no heap allocator. Everything must be stack-allocated or static. The fixed-size `[usize; APP_CAPACITY]` ring buffer avoids any dynamic allocation.

#### SpinLockIrq: The #1 SMP Kernel Bug

The queue is protected by `SpinLockIrq` -- not a plain `SpinLock` like in Part 7. This distinction is critical and prevents the **most common SMP kernel deadlock**.

The scenario: imagine kernel code holds a lock and an interrupt fires on the same hart. The interrupt handler (or code it calls) tries to acquire the same lock -- **deadlock**. The hart is waiting for itself to release a lock that it can never release because it's stuck in the handler.

```
Hart 0: kernel code
  |-- LOCK.lock()           <-- lock acquired
  |-- ... doing work ...
  |-- INTERRUPT FIRES       <-- handler runs on same hart
       |-- LOCK.lock()      <-- tries to acquire... DEADLOCK!
           (hart 0 holds it, but hart 0 IS the interrupt handler)
```

In our kernel, the scheduling loop acquires `QUEUE.lock()` briefly. While the timer interrupt returns to the `execute()` call site (not to a separate handler), disabling interrupts while holding the lock is **defensive practice** -- it guarantees no interrupt-driven code path can ever touch the lock, regardless of future changes to the trap handling logic.

The fix: **disable S-mode interrupts before acquiring the lock**. No interrupt can fire while the lock is held, so the interrupt handler can never run on the same hart.

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/smp.rs`

```rust
pub fn lock(&self) -> RawSpinLockIrqGuard<'_> {
    // Step 1: Save and disable S-mode interrupts (sstatus.SIE)
    let sie_was_enabled = Self::disable_interrupts();

    // Step 2: TTAS spin (same pattern as Part 7's spinlock)
    while self.locked
        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        while self.locked.load(Ordering::Relaxed) {
            core::hint::spin_loop();
        }
    }

    RawSpinLockIrqGuard { lock: self, sie_was_enabled }
}
```

The `disable_interrupts` function reads the `sstatus` CSR, checks if the SIE (Supervisor Interrupt Enable) bit is set, and clears it:

```rust
fn disable_interrupts() -> bool {
    let sstatus: usize;
    unsafe { core::arch::asm!("csrr {}, sstatus", out(reg) sstatus) };
    let sie_was_enabled = (sstatus & (1 << 1)) != 0;
    if sie_was_enabled {
        unsafe { core::arch::asm!("csrc sstatus, {}", in(reg) (1usize << 1)) };
    }
    sie_was_enabled
}
```

The guard's `Drop` is careful about ordering -- it releases the lock **before** restoring interrupts:

```rust
impl Drop for RawSpinLockIrqGuard<'_> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);   // Release lock FIRST
        RawSpinLockIrq::restore_interrupts(self.sie_was_enabled);  // THEN restore
    }
}
```

If we restored interrupts first, a timer interrupt could fire while we still hold the lock. The interrupt handler on the same hart would deadlock -- the exact bug we're trying to prevent.

> **NOTE**: This is OSTEP Ch. 28's "just disable interrupts" approach to mutual exclusion. OSTEP warns that it's insufficient for multi-core systems -- and it is, by itself. Our `SpinLockIrq` combines both: interrupt disable (prevents same-hart deadlock) + atomic spinlock (prevents cross-hart races). Neither alone is sufficient; together, they're correct.

#### Ownership-by-Dequeue: The Lock-Free Data Access Pattern

Once a hart pops a task index from the queue, no other hart can see that index (it's been removed from the shared data structure). The hart has **exclusive ownership** of that task's TCB and syscall counts -- without holding any lock.

This is the key performance insight: the lock is held only during the brief `pop()` and `push()` operations (a few microseconds). The actual task execution (milliseconds) happens entirely lock-free.

To express this in Rust's type system, we use `UnsyncCell`:

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs`

```rust
struct UnsyncCell<T>(core::cell::UnsafeCell<T>);
unsafe impl<T> Sync for UnsyncCell<T> {}

static TCBS: UnsyncCell<[TaskControlBlock; APP_CAPACITY]> = ...;
static SYSCALL_COUNTS: UnsyncCell<[[u32; 500]; APP_CAPACITY]> = ...;
```

`UnsyncCell` wraps data in `UnsafeCell` and manually implements `Sync`. This tells Rust: "I guarantee thread safety through an external protocol" (the dequeue ownership discipline), not through the type system. It's `unsafe` because the compiler can't verify the guarantee -- you must get the discipline right.

#### The SMP Scheduling Loop

Every hart runs the same loop. The pattern is always: **lock briefly, mutate shared state, unlock, do expensive work without lock**.

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs`

```rust
fn scheduling_loop() -> ! {
    loop {
        // Briefly lock, pop, unlock
        let task_idx = {
            let mut q = QUEUE.lock();
            q.pop()
        };  // lock released here (guard dropped)

        match task_idx {
            Some(idx) => run_task(idx),    // Run WITHOUT holding lock
            None => {
                let all_done = {
                    let q = QUEUE.lock();
                    q.finished_count >= q.total_tasks
                };
                if all_done {
                    if smp::hart_id() == 0 { tg_sbi::shutdown(false); }
                    else { loop { unsafe { core::arch::asm!("wfi") }; } }
                }
                core::hint::spin_loop();
            }
        }
    }
}
```

And `run_task` pops a task, executes it, and handles the result:

```rust
fn run_task(idx: usize) {
    let tcb = unsafe { tcb_mut(idx) };      // Exclusive access (dequeued)
    tg_sbi::set_timer(time::read64() + 12500);
    unsafe { tcb.execute() };                // U-mode until trap

    match scause::read().cause() {
        Trap::Interrupt(Interrupt::SupervisorTimer) => {
            tg_sbi::set_timer(u64::MAX);     // Clear timer
            let mut q = QUEUE.lock();        // Brief lock to re-enqueue
            q.push(idx);
        }
        Trap::Exception(Exception::UserEnvCall) => {
            let counts = unsafe { syscall_counts_mut(idx) };
            let event = tcb.handle_syscall(idx, counts);
            match event {
                Event::None => {
                    let mut q = QUEUE.lock();
                    q.push(idx);
                }
                Event::Yield => {
                    let mut q = QUEUE.lock();
                    q.push(idx);
                }
                Event::Exit(code) => {
                    let mut q = QUEUE.lock();
                    tcb.finish = true;
                    q.finished_count += 1;
                }
                // ...
            }
        }
        // ... other traps kill the task ...
    }
}
```

Notice how `QUEUE.lock()` is never held across `tcb.execute()`. The lock protects only the queue mutations. With 4 harts and 4 tasks, all 4 tasks can execute in parallel -- the lock is only contended when two harts happen to finish their time slice at the same instant.

#### PRINT_LOCK: Serializing Console Output

As we saw in Part 7's Demo 1, concurrent `println!` from multiple harts produces garbled output. Ch3-SMP wraps all console output in a separate lock:

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/smp.rs`

```rust
pub static PRINT_LOCK: RawSpinLockIrq = RawSpinLockIrq::new();
```

Every `print!` in the kernel acquires `PRINT_LOCK` first. User-mode `write` syscalls do the same:

> Reference: `jsph-tg-rcore-tutorial-ch3-smp/src/main.rs` (in IO::write)

```rust
fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
    match fd {
        STDOUT | STDDEBUG => {
            let _pl = crate::smp::PRINT_LOCK.lock();
            print!("{}", unsafe { /* buffer contents */ });
            count as _
        }
        // ...
    }
}
```

This is a separate lock from `QUEUE` -- holding `PRINT_LOCK` does not block scheduling, and holding `QUEUE` does not block printing. Since neither lock is ever held when acquiring the other, there is no deadlock risk between them.

---

## Part 10: Virtual Memory -- Giving Every Process Its Own World

In ch3, all tasks share physical memory. Task A can read task B's memory if it knows the address. There's no isolation -- a buffer overflow in one program can corrupt another. Worse, every program must be compiled to run at a specific physical address.

Ch4 solves this with **virtual memory**: each process gets its own **address space** -- a mapping from virtual addresses (what the program sees) to physical addresses (where data actually lives in RAM). Process A and process B can both use address `0x10000` without conflict, because each address maps to different physical pages.

### Sv39: Three-Level Page Tables

RISC-V's Sv39 ("Supervisor Virtual, 39-bit") scheme divides a 39-bit virtual address into four parts:

```
38        30 29        21 20        12 11         0
+----------+----------+----------+--------------+
|  VPN[2]  |  VPN[1]  |  VPN[0]  |   Offset     |
| (9 bits) | (9 bits) | (9 bits) |  (12 bits)   |
+----------+----------+----------+--------------+
```

Translation walks a three-level tree of **page tables**, each containing 512 entries (2^9):

```
satp register -> Root page table
  +-- VPN[2] -> Level-2 PTE -> Level-1 page table
                  +-- VPN[1] -> Level-1 PTE -> Level-0 page table
                                  +-- VPN[0] -> Level-0 PTE -> Physical Page Number (PPN)

Physical address = PPN << 12 | Offset
```

Each Page Table Entry (PTE) contains a PPN and flags (Read, Write, eXecute, Valid, User, etc.). If the Valid flag is not set, the CPU raises a **page fault** exception.

The `satp` CSR tells the CPU where the root page table lives:

```
63   60 59         44 43                        0
+------+------------+----------------------------+
| Mode |    ASID    |      PPN of root            |
|  8   |            |      page table             |
+------+------------+----------------------------+
```

Mode = `8` means Sv39 is active. When `satp.Mode == 0`, translation is off (physical addresses used directly).

> **NOTE**: Each page is 4 KiB (2^12 bytes). A full Sv39 address space can map 2^27 pages = 512 GiB. The three-level structure means most of that space is unmapped without allocating page tables for it. Only the paths actually used get page table entries.

### Kernel Identity Mapping

The kernel maps its own code and data so that **virtual address == physical address** (identity mapping). This means kernel pointers work both with and without translation enabled -- critical during the moment `satp` is first activated.

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs` (simplified from `kernel_space`; ch4-smp has an identical function that returns the `AddressSpace`)

```rust
fn kernel_space(layout: KernelLayout, memory: usize, portal: usize, portal_pages: usize) {
    let mut space = AddressSpace::new();
    for region in layout.iter() {
        let flags = match region.title {
            Text        => "X_RV",    // eXecute, Read, Valid
            Rodata      => "__RV",    // Read-only, Valid
            Data | Boot => "_WRV",    // Write, Read, Valid
        };
        // Identity map: VPN == PPN
        space.map_extern(start..end, PPN::new(start.val()), build_flags(flags));
    }
    // Map heap region (identity)
    space.map_extern(heap_start..heap_end, PPN::new(heap_start.val()), build_flags("_WRV"));
    // Activate Sv39 paging
    unsafe { satp::set(satp::Mode::Sv39, 0, space.root_ppn().val()) };
}
```

After `satp::set`, every memory access goes through the page table. But because we mapped VA == PA, all existing pointers still work.

### Per-Process Address Spaces and ELF Loading

Each user process gets its own page table tree, populated by parsing the ELF binary:

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/process.rs` (ch4-smp has the same ELF loading logic in a function named `new`; ch5 adds `pid`, `stride`, and `priority` fields)

```rust
pub fn from_elf(elf: ElfFile) -> Option<Self> {
    let mut address_space = AddressSpace::new();
    for program in elf.program_iter() {
        if !matches!(program.get_type(), Ok(Type::Load)) { continue; }
        // Derive R/W/X flags from ELF segment header
        let mut flags: [u8; 5] = *b"U___V";   // User, Valid
        if program.flags().is_execute() { flags[1] = b'X'; }
        if program.flags().is_write()   { flags[2] = b'W'; }
        if program.flags().is_read()    { flags[3] = b'R'; }

        address_space.map(
            VAddr::new(off_mem).floor()..VAddr::new(end_mem).ceil(),
            &elf.input[off_file..][..len_file],   // Copy segment data into new pages
            off_mem & PAGE_MASK,
            parse_flags(unsafe { core::str::from_utf8_unchecked(&flags) }).unwrap(),
        );
    }
    // Allocate user stack (2 pages = 8 KiB)
    // Create ForeignContext with satp pointing to this address space
    Some(Self { pid: ProcId::new(), context, address_space, ... })
}
```

`address_space.map()` allocates physical pages, copies the ELF data into them, and creates PTEs mapping the program's virtual addresses to those pages. The `U` flag in the PTE marks these pages as accessible from U-mode -- the kernel's identity-mapped pages don't have this flag, so user programs can't access kernel memory.

### The Portal Page: Cross-Address-Space Execution

When the kernel switches from its address space to a user process's address space (by writing `satp`), the currently executing code must remain valid. If the kernel code is mapped at `0x80200000` in the kernel page table but not in the user page table, the CPU will page-fault immediately after the `satp` switch.

The **portal** (called "trampoline" in rCore-Tutorial-v3) solves this: a page mapped at the **same virtual address** in every address space -- kernel and all user processes. The context-switching code lives on this page. After `satp` switches, the portal code is still at the same virtual address, so execution continues safely.

> Reference: `jsph-tg-rcore-tutorial-ch4-smp/src/main.rs`

```rust
const PROTAL_TRANSIT: VPN<Sv39> = VPN::MAX;   // Highest virtual page (sic: "PROTAL" is a typo for "PORTAL" in the codebase)
```

The portal is mapped at the very top of the virtual address space (`VPN::MAX`). The `ForeignContext::execute()` function runs on this portal page -- it saves kernel registers, switches `satp` to the user page table, restores user registers, and executes `sret` to drop to U-mode.

Each user address space copies the portal mapping from the kernel's root page table. In ch4-smp, this is done inline during process loading; ch5-smp extracts it into a helper:

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs` (ch4-smp does the same copy inline in `rust_main`)

```rust
fn map_portal(space: &AddressSpace<Sv39, Sv39Manager>) {
    let portal_idx = PROTAL_TRANSIT.index_in(Sv39::MAX_LEVEL);
    space.root()[portal_idx] = unsafe { KERNEL_SPACE.assume_init_ref() }.root()[portal_idx];
}
```

This copies the top-level PTE, so the portal pages resolve to the same physical memory in every address space. The `G` (Global) flag is set on portal PTEs (`"__G_XWRV"`), which tells the hardware TLB to keep this entry valid across `satp` switches.

### ForeignContext: Context That Switches Address Spaces

Ch2's `LocalContext` saved registers and switched between S-mode and U-mode within the same address space. Ch4 introduces `ForeignContext`, which additionally switches the `satp` register:

```rust
pub struct ForeignContext {
    pub context: LocalContext,   // 31 registers + sepc
    pub satp: usize,             // This process's page table root
}
```

`ForeignContext::execute(portal, key)` does: save kernel state on the portal page, write this process's `satp`, flush TLB with `sfence.vma`, restore user registers, then `sret`. On trap return, the reverse: save user registers, write kernel `satp`, `sfence.vma`, restore kernel state, return.

### How SMP Changes This

#### Per-Hart satp: Free Parallelism

Each hart has its own `satp` CSR. When hart 0 is running process A (satp points to A's page table) and hart 1 is running process B (satp points to B's page table), there's no conflict. CSR writes are per-hart -- as we learned in Part 6, no synchronization is needed for per-hart CSRs.

The one requirement: secondary harts must load the kernel's `satp` before they can use virtual addresses.

> Reference: `jsph-tg-rcore-tutorial-ch4-smp/src/main.rs`

```rust
extern "C" fn secondary_main() -> ! {
    while !smp::BOOT_HART_DONE.load(Acquire) { core::hint::spin_loop(); }
    let satp_val = KERNEL_SATP.load(Relaxed);
    unsafe {
        core::arch::asm!(
            "csrw satp, {0}",
            "sfence.vma",        // Flush this hart's TLB
            in(reg) satp_val,
        );
    }
    unsafe { sie::set_stimer() };
    scheduling_loop()
}
```

`sfence.vma` flushes the hart's **TLB** (Translation Lookaside Buffer) -- a cache of recent virtual-to-physical translations. After changing `satp`, stale TLB entries from the previous page table would cause wrong translations.

> **NOTE**: `sfence.vma` only flushes the TLB on the executing hart. RISC-V does not have a "flush all TLBs" instruction. If you need to invalidate TLB entries on another hart (e.g., after unmapping a shared page), you'd need an inter-processor interrupt (IPI). Our SMP design avoids this problem: each process only runs on one hart at a time (ownership-by-dequeue), so only that hart's TLB has entries for it.

#### MultislotPortal: Concurrent Address-Space Switching

In single-core ch4, the portal has one slot -- only one context switch can happen at a time:

```rust
MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), 1)   // 1 slot
ctx.execute(portal, ())   // Empty tuple key (only one slot)
```

In SMP, 4 harts may switch address spaces simultaneously. The portal needs 4 independent slots:

> Reference: `jsph-tg-rcore-tutorial-ch4-smp/src/main.rs`

```rust
let portal = unsafe {
    MultislotPortal::init_transit(PROTAL_TRANSIT.base().val(), smp::NUM_HARTS)  // 4 slots
};
// ...
unsafe { process.context.execute(portal, TpReg) };   // TpReg selects slot by hart ID
```

`TpReg` tells the portal to read the `tp` register (which holds the hart ID, as set in `_start`) to select which slot to use. Each hart gets its own slot, so 4 harts can context-switch concurrently without interference.

#### The Locked Allocator: Protecting the Heap

Ch4 introduces the heap allocator (`tg-kernel-alloc`). When a hart maps new pages (in `Sv39Manager::page_alloc`), it calls `alloc_zeroed()`, which calls the global allocator. If two harts call `alloc_zeroed()` simultaneously, the buddy allocator's internal state gets corrupted -- it was designed for single-core use.

We solve this with a dedicated crate: `jsph-tg-rcore-tutorial-kernel-alloc-smp`. This is a copy of the standard kernel allocator with an interrupt-disabling spinlock built into `GlobalAlloc`:

> Reference: `jsph-tg-rcore-tutorial-kernel-alloc-smp/src/lib.rs`

```rust
unsafe impl GlobalAlloc for SmpGlobalAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _guard = LOCK.lock();   // Interrupt-safe spinlock
        if let Ok((ptr, _)) = heap_mut().allocate_layout::<u8>(layout) {
            ptr.as_ptr()
        } else {
            handle_alloc_error(layout)
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let _guard = LOCK.lock();
        unsafe { heap_mut().deallocate_layout(NonNull::new(ptr).unwrap(), layout) }
    }
}
```

Every allocation and deallocation automatically acquires the lock. No call site needs to think about thread safety -- it's handled inside the allocator. This is much cleaner than wrapping every `alloc` call in a manual lock (an approach that was tried first and proved fragile -- it's easy to miss a call site, especially in code you don't own like `BTreeMap::insert`).

#### Per-Hart CURRENT_PROCESS

In ch4, syscall handlers need to translate user virtual addresses to physical addresses. They need access to the current process's `address_space`. With one hart, a global variable suffices. With 4 harts running different processes, we need per-hart tracking:

> Reference: `jsph-tg-rcore-tutorial-ch4-smp/src/main.rs`

```rust
static CURRENT_PROCESS: [AtomicUsize; smp::NUM_HARTS] = {
    const ZERO: AtomicUsize = AtomicUsize::new(0);
    [ZERO; smp::NUM_HARTS]
};

fn current_process() -> &'static mut Process {
    let ptr = CURRENT_PROCESS[smp::hart_id()].load(Relaxed);
    unsafe { &mut *(ptr as *mut Process) }
}
```

Before executing a process, each hart stores a pointer to it in `CURRENT_PROCESS[hart_id]`. Syscall handlers call `current_process()` to get the process whose address space they need for translation. `Relaxed` ordering is fine because only the same hart reads what it wrote -- no cross-hart communication needed.

For example, the `write` syscall translates the user's buffer pointer before printing:

> Reference: `jsph-tg-rcore-tutorial-ch4-smp/src/main.rs` (in IO::write, simplified)

```rust
fn write(&self, _caller: Caller, fd: usize, buf: usize, count: usize) -> isize {
    let process = super::current_process();
    if let Some(ptr) = process.address_space
        .translate::<u8>(VAddr::new(buf), READABLE)
    {
        let _pl = crate::smp::PRINT_LOCK.lock();
        print!("{}", unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr.as_ptr(), count))
        });
        count as _
    } else { -1 }
}
```

Without the `translate` call, the kernel would try to dereference a user virtual address that only makes sense in the user's page table. Since the kernel uses identity mapping, the user's virtual address `0x10000` would map to physical address `0x10000` -- likely unmapped or belonging to a completely different program. Address translation is the price of memory isolation.

---

## Part 11: Process Management -- Programs That Create Programs

Ch3 and ch4 loaded all programs at boot and ran them until they finished. No program could create another program, modify the program list, or interact with other programs' lifecycles. This is multiprogramming, not an operating system.

Ch5 adds the **Unix process model** -- four system calls that form the foundation of every Unix-like OS:

- **`fork()`**: Create a new process by duplicating the current one (address space, registers, everything). The child gets a copy; the parent and child then diverge.
- **`exec(path)`**: Replace the current process's program with a different one (load a new ELF, discard the old address space). `fork() + exec()` is how Unix launches new programs.
- **`wait(pid)`**: A parent blocks until a child exits, then collects the child's exit code.
- **`exit(code)`**: Terminate the current process with a status code.

Together, these four calls enable a **shell**: the init process forks, the child execs a user command, the parent waits for the child to finish, then prints the next prompt.

### Address Space Deep-Copy: What fork() Actually Does

`fork()` creates a **complete copy** of the parent's address space -- every page table and every physical page:

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/process.rs`

```rust
pub fn fork(&mut self) -> Option<Process> {
    let pid = ProcId::new();
    let mut address_space: AddressSpace<Sv39, Sv39Manager> = AddressSpace::new();
    self.address_space.cloneself(&mut address_space);  // Deep-copy ALL pages
    map_portal(&address_space);                         // Share portal mapping
    let context = self.context.context.clone();         // Copy all registers
    let satp = (8 << 60) | address_space.root_ppn().val();  // 8 = Sv39 mode
    Some(Self {
        pid, context: ForeignContext { context, satp }, address_space,
        heap_bottom: self.heap_bottom, program_brk: self.program_brk,
        stride: 0, priority: 16,
    })
}
```

`cloneself` walks the parent's page table tree and allocates fresh physical pages for every mapped page, copying the contents byte by byte. The child gets identical virtual-to-physical mappings but pointing to **new** physical pages. After `fork()`, parent and child can modify their memory independently.

> **NOTE**: This is expensive -- forking a process with 100 mapped pages allocates and copies 100 * 4 KiB = 400 KiB. Production kernels use **copy-on-write** (COW): both processes initially share the same physical pages (marked read-only), and a page fault triggers copying only when one process actually writes. Our implementation does a full deep-copy for simplicity.

The parent's `fork()` returns the child's PID. The child's `fork()` returns 0 -- this is how the user program knows whether it's the parent or the child:

```rust
// In the fork syscall handler:
*child_proc.context.context.a_mut(0) = 0;   // Child sees fork() return 0
// Parent sees fork() return child_pid (set by the syscall return value)
```

### Process Tree and Parent-Child Relationships

Each process has a parent and may have children. The `ProcRel` structure (from `tg-task-manage`) tracks this:

```rust
// From tg-task-manage
pub struct ProcRel {
    pub parent: ProcId,
    pub children: Vec<ProcId>,
    pub dead_children: Vec<(ProcId, isize)>,  // (pid, exit_code) pairs
}
```

When a child exits, its PID and exit code are pushed into the parent's `dead_children` list. When the parent calls `wait()`, it consumes an entry from this list.

When a parent exits before its children (creating **orphans**), the orphaned children are reparented to the init process (PID 0). This prevents resource leaks -- someone must always be able to `wait()` on a child to collect its exit status.

### Stride Scheduling: Priority-Based Fairness

Ch3 used round-robin: every task gets the same time slice in turn. Ch5 adds **stride scheduling**, which gives higher-priority processes more CPU time.

Each process has a `stride` counter (starts at 0) and a `priority` (default 16, minimum 2). The scheduler always picks the process with the **lowest stride**. After running, the process's stride increases by `BIG_STRIDE / priority`:

```
BIG_STRIDE = 1,000,000

Process A (priority 16): stride increases by   62,500 per turn
Process B (priority 4):  stride increases by  250,000 per turn

A runs ~4x more often than B (lower stride increments mean more selections).
```

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs` (in `SmpProcManager::fetch_next`)

```rust
fn fetch_next(&mut self) -> Option<(ProcId, *mut Process)> {
    if self.ready_queue.is_empty() { return None; }
    // Find process with minimum stride
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
    let pid = self.ready_queue.remove(min_idx).unwrap();
    if let Some(proc) = self.tasks.get_mut(&pid) {
        proc.stride += BIG_STRIDE / proc.priority;
        let ptr = proc as *mut Process;
        Some((pid, ptr))
    } else {
        None
    }
}
```

This is an O(n) scan of the ready queue. A production scheduler would use a priority queue (min-heap), but the linear scan is correct and simple for our teaching OS.

### How SMP Changes This

#### The PManager Problem

The original single-core ch5 uses `PManager` from `tg-task-manage`. `PManager` has a critical design assumption: there is exactly one "current" process at any time. Its API exposes methods like `current()` (get the running process), `make_current_suspend()` (re-enqueue it), and `make_current_exited()` (terminate it).

With 4 harts, there are 4 "current" processes simultaneously. `PManager`'s single-current model is **fundamentally incompatible** with SMP. We can't just wrap `PManager` in a lock -- the problem is the API design itself, not thread safety.

#### SmpProcManager: A Custom Replacement

We replaced `PManager` entirely with `SmpProcManager`, which has no `current` field:

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs`

```rust
struct SmpProcManager {
    tasks: BTreeMap<ProcId, Process>,        // All living processes
    ready_queue: VecDeque<ProcId>,           // Ready to run
    rel_map: BTreeMap<ProcId, ProcRel>,      // Parent-child relationships
    blocked_on_wait: BTreeSet<ProcId>,       // Parents blocked in wait()
}

static PROC_MANAGER: smp::SpinLockIrq<SmpProcManager> =
    smp::SpinLockIrq::new(SmpProcManager::new());
```

Instead of a single `current`, per-hart atomics track what each hart is running:

```rust
static CURRENT_PROCESS: [AtomicUsize; NUM_HARTS] = ...;  // Raw pointer to Process
static CURRENT_PID: [AtomicUsize; NUM_HARTS] = ...;      // ProcId as usize
static WAIT_BLOCKED: [AtomicBool; NUM_HARTS] = ...;      // Was wait() blocked?
```

The scheduling loop is the same lock-pop-unlock-execute pattern from ch3, but with `fetch_next` doing stride scheduling instead of simple FIFO:

```rust
fn scheduling_loop() -> ! {
    loop {
        let task_info = {
            let mut pm = PROC_MANAGER.lock();
            pm.fetch_next()   // Returns (ProcId, *mut Process) or None
        };
        match task_info {
            Some((pid, proc_ptr)) => run_process(pid, proc_ptr),
            None => { /* idle handling */ }
        }
    }
}
```

#### Concurrent fork/exec/wait/exit

The classic SMP race: child calls `exit()` on hart 1 while parent calls `wait()` on hart 0. Both need to modify `ProcRel`'s `dead_children` list. Without synchronization, updates are lost.

Our coarse-grained lock (`SpinLockIrq<SmpProcManager>`) serializes all process manager operations. Every `fork`, `wait`, `exit`, and scheduling decision acquires the same lock. This is correct and simple, though it serializes these operations.

#### Blocking wait(): Avoiding the Busy-Retry Storm

The naive `wait()` implementation: if no child has exited yet, return a "try again" sentinel (-2). The user-space `waitpid()` loop calls `wait()` again immediately. On single-core, this is fine -- the child will eventually get a time slice and exit. On SMP, this creates a **thundering herd**: the parent repeatedly acquires the process manager lock hundreds of times per millisecond, preventing the child from making progress on its own hart.

Our fix: **blocking wait**. If a parent calls `wait()` and children exist but none have exited, the parent is removed from the ready queue and placed in `blocked_on_wait`:

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs`

```rust
// In SmpProcManager::wait():
// Children exist but none dead -- block the parent
self.blocked_on_wait.insert(parent_pid);
WAIT_BLOCKED[smp::hart_id()].store(true, Relaxed);
```

When a child exits, `make_exited()` checks if the parent was blocked and wakes it:

```rust
fn make_exited(&mut self, pid: ProcId, exit_code: isize) {
    self.tasks.remove(&pid);
    ACTIVE_PROCESSES.fetch_sub(1, Relaxed);
    self.ready_queue.retain(|p| *p != pid);   // Remove stale queue entries

    if let Some(current_rel) = self.rel_map.remove(&pid) {
        let parent_pid = current_rel.parent;
        let children = current_rel.children;
        // Notify parent of child death
        if let Some(parent_rel) = self.rel_map.get_mut(&parent_pid) {
            parent_rel.del_child(pid, exit_code);
        }
        // Wake parent if it was blocked in wait()
        if self.blocked_on_wait.remove(&parent_pid) {
            self.ready_queue.push_back(parent_pid);
        }
        // Reparent orphaned children to init (PID 0)
        let init_pid = ProcId::from_usize(0);
        for child_id in children {
            if let Some(child_rel) = self.rel_map.get_mut(&child_id) {
                child_rel.parent = init_pid;
            }
            // ... add to init's children ...
        }
    }
}
```

The caller (`run_process`) checks the per-hart flag and skips re-enqueuing:

```rust
if WAIT_BLOCKED[smp::hart_id()].swap(false, Relaxed) {
    // Process is blocked in wait() -- do NOT re-enqueue
} else {
    let mut pm = PROC_MANAGER.lock();
    pm.make_ready(pid);
}
```

This eliminates the busy-retry storm entirely. A parent blocked in `wait()` consumes zero CPU cycles until a child actually exits.

#### Lock-Free Idle: ACTIVE_PROCESSES

When there are fewer runnable processes than harts, idle harts have nothing to do. Without optimization, they'd repeatedly acquire the process manager lock just to find an empty queue -- wasting cycles and blocking other harts that are trying to do real work (fork, exit, etc.).

`ACTIVE_PROCESSES` is an atomic counter tracking the total number of living processes. Idle harts check it **without any lock**:

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/main.rs`

```rust
None => {
    if ACTIVE_PROCESSES.load(Relaxed) == 0 {
        if smp::hart_id() == 0 { tg_sbi::shutdown(false); }
        else { loop { unsafe { core::arch::asm!("wfi") }; } }
    }
    // Not all done, but nothing to run -- sleep until interrupt
    unsafe { core::arch::asm!("wfi") };
}
```

`wfi` (Wait For Interrupt) puts the hart to sleep until an interrupt arrives -- typically a timer interrupt. This eliminates wasted cycles entirely. The hart wakes up, checks if there's work, and either picks up a task or goes back to sleep.

`ACTIVE_PROCESSES` is incremented in `add()` (when a process is created) and decremented in `make_exited()` (when a process exits). Both happen under the `PROC_MANAGER` lock, but the counter itself is an atomic -- reads don't need the lock.

> **NOTE**: The combination of blocking wait + lock-free idle + locked allocator was the result of iterative debugging. The initial SMP implementation timed out on `forktest` (14 processes forking and waiting with 4 harts). The root causes were: (1) parents busy-retrying `wait()` hundreds of times per ms, (2) idle harts monopolizing the lock, and (3) concurrent heap allocation without synchronization. Each fix addressed one specific bottleneck, and all three were necessary.

#### The Process Struct

For completeness, here is the full process control block in ch5:

> Reference: `jsph-tg-rcore-tutorial-ch5-smp/src/process.rs`

```rust
pub struct Process {
    pub pid: ProcId,
    pub context: ForeignContext,                          // Registers + satp
    pub address_space: AddressSpace<Sv39, Sv39Manager>,   // Page table tree
    pub heap_bottom: usize,                                // Heap start address
    pub program_brk: usize,                                // Current heap end (sbrk)
    pub stride: usize,                                     // Stride scheduling counter
    pub priority: usize,                                   // Stride scheduling priority
}

unsafe impl Send for Process {}
```

The `unsafe impl Send` is required because `AddressSpace` contains `NonNull` pointers (which don't auto-implement `Send`). In our design, exclusive access is guaranteed by the scheduling protocol -- only the hart that dequeued a process may access it -- so `Send` is safe.

---

## Summary: Key Concepts

| Concept | What It Means | Where You See It |
|---------|--------------|-----------------|
| Privilege levels | Hardware-enforced permission boundaries | M->S (`mret`), S->U (`sret`), U->S (`ecall`) |
| CSRs | Per-hart configuration registers | `mstatus`, `mepc`, `mhartid`, `satp` |
| Trap | CPU stops current code, jumps to handler | `ecall`, illegal instruction, timer, page fault |
| Stack | Memory region for function calls (grows down) | Must be set up before any function call |
| Naked function | No compiler-generated prologue/epilogue | `_start` -- runs before stack exists |
| SBI | M-mode runtime providing services to S-mode | `console_putchar`, `set_timer`, `shutdown` |
| Context switching | Saving/restoring all registers on privilege change | `LocalContext` (ch2-ch3), `ForeignContext` (ch4-ch5) |
| Atomic operations | Hardware-guaranteed indivisible memory ops | CAS (`compare_exchange_weak`), `fetch_add` |
| Memory ordering | Rules about when writes become visible | Release/Acquire pairs, `fence` instructions |
| Spinlock (TTAS) | Mutual exclusion via atomic spin loop | Read cache first, then attempt CAS |
| Barrier | Wait until N threads arrive | Generation counter prevents ABA race |
| Preemptive scheduling | Timer interrupt forces context switch | `set_timer(time + 12500)`, `scause::SupervisorTimer` |
| SpinLockIrq | Spinlock that disables interrupts first | Prevents same-hart deadlock in kernel |
| Ownership-by-dequeue | Exclusive access without holding lock | Pop task from queue, own it until re-enqueue |
| Sv39 page tables | Three-level virtual-to-physical translation | `satp` CSR, VPN[2]/VPN[1]/VPN[0]/offset |
| Identity mapping | VA == PA for kernel memory | Kernel code works before and after `satp` activation |
| Portal / trampoline | Page mapped at same VA in all address spaces | Safe `satp` switching during context switch |
| MultislotPortal | Per-hart portal slots for concurrent switching | `TpReg` key selects slot by hart ID |
| Locked allocator | Heap allocator with built-in spinlock | `GlobalAlloc` wrapper in `kernel-alloc-smp` |
| fork() | Deep-copy address space to create child | `cloneself` + new ProcId |
| exec() | Replace address space with new ELF | Discards old pages, loads new program |
| wait() / exit() | Parent collects child's exit code | `ProcRel::dead_children` list |
| Stride scheduling | Priority-based: lower stride runs next | `stride += BIG_STRIDE / priority` after each run |
| Blocking wait | Remove parent from queue until child exits | `blocked_on_wait` set + `make_exited` wake-up |
| Lock-free idle | Atomic counter avoids lock for "all done?" | `ACTIVE_PROCESSES.load(Relaxed)` + `wfi` |

The progression ch1 -> ch2 -> ch3 -> ch4 -> ch5 mirrors the history of operating systems: **bare metal -> batch processing -> time-sharing -> virtual memory -> process management**. The SMP extensions at each stage show that concurrency is never free -- every shared data structure needs a synchronization strategy, and the right strategy depends on the access pattern.

---

## What This Tutorial Does Not Cover (Yet)

| Topic | Chapter | What It Adds |
|-------|---------|-------------|
| File system and I/O | Ch6 | VirtIO block device, easy-fs, inodes, file descriptors. Persistent storage. |
| IPC: Pipes and signals | Ch7 | Inter-process communication. Signal handlers. |
| Threads vs processes | Ch8 | Shared address space, thread-local storage, cheaper than fork. |
| Kernel sync primitives | Ch8 | Mutexes, semaphores, condition variables -- built on spinlocks + sleep queues. |
| Copy-on-write fork | -- | Share pages until write, then copy. Mentioned in Part 11 but not implemented. |
| Fine-grained locking | -- | Separate locks for scheduler, allocator, process table. Our ch5 uses coarse-grained (one lock for all process operations). |
| TLB shootdown / IPI | -- | Invalidating remote hart TLBs after page table changes. Avoided by our ownership-by-dequeue design. |
| Deadlock theory | Ch8-SMP | Necessary conditions (mutual exclusion, hold-and-wait, no preemption, circular wait), detection, prevention. |

### Cross-References to Other OS Textbooks

**rCore-Tutorial-v3 (Ch3)**: rCore-v3's ch3 covers multiprogramming and time-sharing with a `TaskManager` that has `ready_queue` and `current_task`. Our ch3-SMP replaces this with `SpinLockIrq<QueueState>` + ownership-by-dequeue -- the fundamental difference is that we separate the "who's ready" question (locked queue) from the "run this task" action (lock-free execution).

**rCore-Tutorial-v3 (Ch4)**: rCore-v3's ch4 introduces `MemorySet` for address spaces and uses a trampoline page at the highest virtual address. Our implementation uses the same concept (`PROTAL_TRANSIT = VPN::MAX`) but with `MultislotPortal` providing per-hart slots -- a feature rCore-v3 doesn't need because it's single-core.

**rCore-Tutorial-v3 (Ch5)**: rCore-v3's ch5 implements `fork`/`exec`/`waitpid` with a `TaskControlBlock` that uses `Arc<Mutex<...>>` for shared state. Our SMP version uses `SmpProcManager` with explicit ownership tracking -- a lower-level approach that avoids the overhead of reference counting.

**rCore-Tutorial-v3**: Uses a dedicated `trap.S` assembly file with `__alltraps` and `__restore` labels that explicitly save/restore all 32 registers into a `TrapContext` struct. Our `tg-kernel-context` crate wraps this in `LocalContext::execute()` / `ForeignContext::execute()`, hiding the assembly. The mechanics are identical -- the abstraction level differs.

**OSTEP Ch. 4-6 (Processes)**: Ch3 maps directly. OSTEP's "limited direct execution" protocol is our `set_timer` -> `execute` -> handle trap cycle. The timer interrupt implements OSTEP's "regaining control" mechanism (Ch. 6.3).

**OSTEP Ch. 13-16 (Virtual Memory)**: Ch4 maps to these chapters. Sv39's three-level page table is a concrete instance of OSTEP's "multi-level page table" (Ch. 20). Our identity mapping is OSTEP's "direct mapping" approach. The portal page implements the "carefully placed" trampoline code that OSTEP Ch. 15 describes for safe address space switching.

**OSTEP Ch. 5 (Process API)**: Ch5's fork/exec/wait map directly. OSTEP emphasizes the elegance of fork's "copy and diverge" model -- our code shows the cost (full page-table deep-copy) that OSTEP glosses over.

**OSTEP Ch. 28 (Locks)**: SpinLockIrq combines two of OSTEP's approaches: "disable interrupts" (insufficient for multi-core alone) and "test-and-set" (insufficient for single-core interrupt handlers alone). Together, they're correct for SMP kernels. This is the key insight OSTEP builds toward across the chapter.

**OSTEP Ch. 9 (Proportional Share Scheduling)**: Stride scheduling is covered in OSTEP as a deterministic version of lottery scheduling. Our `BIG_STRIDE / priority` formula is the standard stride algorithm.

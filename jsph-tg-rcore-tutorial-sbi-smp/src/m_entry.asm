# Multi-hart M-mode entry for SMP boot (-bios none)
# All harts start executing here simultaneously at 0x80000000.
# Each hart identifies itself, gets its own stack, configures CSRs, and jumps to S-mode.

    .equ NUM_HARTS,     4
    .equ M_STACK_SIZE,  4096        # 4 KiB per-hart M-mode stack

    .section .text.m_entry
    .globl _m_start
_m_start:
    # === Step 1: Identify this hart ===
    csrr  t0, mhartid              # t0 = hart ID

    # === Step 2: Guard against overflow ===
    li    t1, NUM_HARTS
    bge   t0, t1, .park_hart       # If hartid >= NUM_HARTS, park forever

    # === Step 3: Per-hart M-mode stack ===
    la    t1, m_stacks_base
    li    t2, M_STACK_SIZE
    addi  t3, t0, 1                # (hartid + 1)
    mul   t2, t2, t3               # offset = (hartid + 1) * stack_size
    add   sp, t1, t2               # sp = base + offset (top of this hart's stack)
    csrw  mscratch, sp             # Save for M-mode trap entry

    # === Step 4: M-mode configuration (each hart does its own) ===

    # mstatus: MPP=01 (S-mode), MPIE=1
    li    t1, (1 << 11) | (1 << 7)
    csrw  mstatus, t1

    # mepc: S-mode entry point
    la    t1, _start
    csrw  mepc, t1

    # mtvec: M-mode trap handler
    la    t1, m_trap_vector
    csrw  mtvec, t1

    # Interrupt/exception delegation to S-mode
    # (except ecall from S-mode, which stays in M-mode for SBI calls)
    li    t1, 0xffff
    csrw  mideleg, t1
    li    t1, 0xffff
    li    t2, (1 << 9)             # Exception 9: ecall from S-mode
    not   t2, t2
    and   t1, t1, t2
    csrw  medeleg, t1

    # PMP: allow S-mode full access (teaching simplification)
    li    t1, -1
    csrw  pmpaddr0, t1
    li    t1, 0x0f                 # TOR + RWX
    csrw  pmpcfg0, t1

    # Allow S-mode to read counters (rdtime, rdcycle)
    li    t1, -1
    csrw  mcounteren, t1

    # === Step 5: Pass hart ID to S-mode and jump ===
    csrr  a0, mhartid              # a0 = hart ID (S-mode receives this)
    mret                           # Jump to _start in S-mode

.park_hart:
    wfi
    j     .park_hart

    # === M-mode trap vector ===
    .section .text.m_trap
    .globl m_trap_vector
    .align 4
m_trap_vector:
    # Minimal M-mode trap entry: handle S-mode ecall (SBI calls)
    # Switch to per-hart M-mode stack via mscratch
    csrrw sp, mscratch, sp
    addi sp, sp, -128

    # Save registers used by Rust handler
    sd ra, 0(sp)
    sd t0, 8(sp)
    sd t1, 16(sp)
    sd t2, 24(sp)
    sd a0, 32(sp)
    sd a1, 40(sp)
    sd a2, 48(sp)
    sd a3, 56(sp)
    sd a4, 64(sp)
    sd a5, 72(sp)
    sd a6, 80(sp)
    sd a7, 88(sp)

    # Call Rust dispatch function (msbi.rs::m_trap_handler)
    call m_trap_handler

    # Advance past the ecall instruction
    csrr t0, mepc
    addi t0, t0, 4
    csrw mepc, t0

    # Restore registers (a0/a1 hold return values from handler)
    ld ra, 0(sp)
    ld t0, 8(sp)
    ld t1, 16(sp)
    ld t2, 24(sp)
    ld a2, 48(sp)
    ld a3, 56(sp)
    ld a4, 64(sp)
    ld a5, 72(sp)
    ld a6, 80(sp)
    ld a7, 88(sp)

    addi sp, sp, 128
    # Switch back to S-mode stack
    csrrw sp, mscratch, sp
    mret

    # === Per-hart M-mode stacks ===
    .section .bss.m_stack
    .globl m_stacks_base
m_stacks_base:
    .space M_STACK_SIZE * NUM_HARTS    # 4 KiB x 4 = 16 KiB total
    .globl m_stacks_top
m_stacks_top:

    .section .bss.m_data
    # Reserved M-mode data area
    .space 64

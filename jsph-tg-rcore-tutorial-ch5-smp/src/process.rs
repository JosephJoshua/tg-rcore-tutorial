//! Process management module for SMP.
//!
//! Defines the `Process` struct with fork, exec, and ELF loading.

use crate::{build_flags, map_portal, parse_flags, Sv39, Sv39Manager};
use alloc::alloc::alloc_zeroed;
use core::alloc::Layout;
use tg_kernel_context::{foreign::ForeignContext, LocalContext};
use tg_kernel_vm::{
    page_table::{MmuMeta, VAddr, PPN, VPN},
    AddressSpace,
};
use tg_task_manage::ProcId;
use xmas_elf::{
    header::{self, HeaderPt2, Machine},
    program, ElfFile,
};

/// Process control block.
///
/// Each process has an independent address space and execution context.
///
/// # Safety
/// `Send` is implemented manually because `AddressSpace` contains `NonNull`
/// which doesn't auto-implement `Send`. In our SMP design, exclusive access
/// is guaranteed by the scheduling protocol (only the hart that dequeued a
/// process may access it).
pub struct Process {
    /// Process identifier (immutable after creation).
    pub pid: ProcId,
    /// User-mode context including satp and general-purpose registers.
    pub context: ForeignContext,
    /// Independent Sv39 address space (page table).
    pub address_space: AddressSpace<Sv39, Sv39Manager>,
    /// Heap bottom address (next page after highest ELF segment).
    pub heap_bottom: usize,
    /// Current program break (heap top), adjusted by sbrk.
    pub program_brk: usize,
    /// Stride scheduling: current stride value.
    pub stride: usize,
    /// Stride scheduling: process priority.
    pub priority: usize,
}

// Safety: Process is moved between harts via the SpinLockIrq-protected
// SmpProcManager. Exclusive access is guaranteed by the scheduling protocol.
unsafe impl Send for Process {}

impl Process {
    /// Replace this process's address space with a new ELF program (exec syscall).
    pub fn exec(&mut self, elf: ElfFile) {
        let proc = Process::from_elf(elf).unwrap();
        self.address_space = proc.address_space;
        self.context = proc.context;
        self.heap_bottom = proc.heap_bottom;
        self.program_brk = proc.program_brk;
    }

    /// Deep-copy this process to create a child (fork syscall).
    pub fn fork(&mut self) -> Option<Process> {
        let pid = ProcId::new();
        // Deep-copy address space (all page tables and physical pages)
        let parent_addr_space = &self.address_space;
        let mut address_space: AddressSpace<Sv39, Sv39Manager> = AddressSpace::new();
        parent_addr_space.cloneself(&mut address_space);
        // Map portal in child address space
        map_portal(&address_space);
        // Copy parent's user-mode context (registers)
        let context = self.context.context.clone();
        // Build child satp: Mode=Sv39 | root page table PPN
        let satp = (8 << 60) | address_space.root_ppn().val();
        let foreign_ctx = ForeignContext { context, satp };
        Some(Self {
            pid,
            context: foreign_ctx,
            address_space,
            heap_bottom: self.heap_bottom,
            program_brk: self.program_brk,
            stride: 0,
            priority: 16,
        })
    }

    /// Create a new process from an ELF file.
    pub fn from_elf(elf: ElfFile) -> Option<Self> {
        // Validate ELF header: must be RISC-V 64-bit executable
        let entry = match elf.header.pt2 {
            HeaderPt2::Header64(pt2)
                if pt2.type_.as_type() == header::Type::Executable
                    && pt2.machine.as_machine() == Machine::RISC_V =>
            {
                pt2.entry_point as usize
            }
            _ => None?,
        };

        const PAGE_SIZE: usize = 1 << Sv39::PAGE_BITS;
        const PAGE_MASK: usize = PAGE_SIZE - 1;

        let mut address_space = AddressSpace::new();
        let mut max_end_va: usize = 0;

        // Map all LOAD segments
        for program in elf.program_iter() {
            if !matches!(program.get_type(), Ok(program::Type::Load)) {
                continue;
            }

            let off_file = program.offset() as usize;
            let len_file = program.file_size() as usize;
            let off_mem = program.virtual_addr() as usize;
            let end_mem = off_mem + program.mem_size() as usize;
            assert_eq!(off_file & PAGE_MASK, off_mem & PAGE_MASK);

            if end_mem > max_end_va {
                max_end_va = end_mem;
            }

            let mut flags: [u8; 5] = *b"U___V";
            if program.flags().is_execute() {
                flags[1] = b'X';
            }
            if program.flags().is_write() {
                flags[2] = b'W';
            }
            if program.flags().is_read() {
                flags[3] = b'R';
            }
            address_space.map(
                VAddr::new(off_mem).floor()..VAddr::new(end_mem).ceil(),
                &elf.input[off_file..][..len_file],
                off_mem & PAGE_MASK,
                parse_flags(unsafe { core::str::from_utf8_unchecked(&flags) }).unwrap(),
            );
        }

        let heap_bottom = VAddr::<Sv39>::new(max_end_va).ceil().base().val();

        // Allocate user stack (2 pages = 8 KiB)
        let stack = unsafe {
            alloc_zeroed(Layout::from_size_align_unchecked(
                2 << Sv39::PAGE_BITS,
                1 << Sv39::PAGE_BITS,
            ))
        };
        address_space.map_extern(
            VPN::new((1 << 26) - 2)..VPN::new(1 << 26),
            PPN::new(stack as usize >> Sv39::PAGE_BITS),
            build_flags("U_WRV"),
        );

        // Map portal page
        map_portal(&address_space);

        // Create user-mode context
        let mut context = LocalContext::user(entry);
        let satp = (8 << 60) | address_space.root_ppn().val();
        *context.sp_mut() = 1 << 38;

        Some(Self {
            pid: ProcId::new(),
            context: ForeignContext { context, satp },
            address_space,
            heap_bottom,
            program_brk: heap_bottom,
            stride: 0,
            priority: 16,
        })
    }

    /// Adjust the program break (sbrk syscall).
    pub fn change_program_brk(&mut self, size: isize) -> Option<usize> {
        let old_brk = self.program_brk;
        let new_brk = self.program_brk as isize + size;
        if new_brk < self.heap_bottom as isize {
            return None;
        }
        let new_brk = new_brk as usize;

        let old_brk_ceil = VAddr::<Sv39>::new(old_brk).ceil();
        let new_brk_ceil = VAddr::<Sv39>::new(new_brk).ceil();

        if size > 0 {
            if new_brk_ceil.val() > old_brk_ceil.val() {
                self.address_space
                    .map(old_brk_ceil..new_brk_ceil, &[], 0, build_flags("U_WRV"));
            }
        } else if size < 0 {
            if old_brk_ceil.val() > new_brk_ceil.val() {
                self.address_space.unmap(new_brk_ceil..old_brk_ceil);
            }
        }

        self.program_brk = new_brk;
        Some(old_brk)
    }
}

//! Task management: TCB, scheduling events, and syscall dispatch.

use tg_kernel_context::LocalContext;
use tg_syscall::{Caller, SyscallId};

/// Task Control Block.
pub struct TaskControlBlock {
    /// User context (registers + sepc + sstatus).
    ctx: LocalContext,
    /// Whether task has finished.
    pub finish: bool,
    /// User stack: 8 KiB per task.
    stack: [usize; 1024],
}

/// Scheduling event returned by handle_syscall.
pub enum SchedulingEvent {
    /// Continue current task.
    None,
    /// Task yielded.
    Yield,
    /// Task exited with code.
    Exit(usize),
    /// Unsupported syscall.
    UnsupportedSyscall(SyscallId),
}

impl TaskControlBlock {
    /// Zero-value for array init.
    pub const ZERO: Self = Self {
        ctx: LocalContext::empty(),
        finish: false,
        stack: [0; 1024],
    };

    /// Initialize task with entry point.
    pub fn init(&mut self, entry: usize) {
        self.stack.fill(0);
        self.finish = false;
        self.ctx = LocalContext::user(entry);
        *self.ctx.sp_mut() = self.stack.as_ptr() as usize + core::mem::size_of_val(&self.stack);
    }

    /// Execute this task (sret to U-mode).
    #[inline]
    pub unsafe fn execute(&mut self) {
        unsafe { self.ctx.execute() };
    }

    /// Handle syscall, return scheduling event.
    pub fn handle_syscall(&mut self, task_idx: usize) -> SchedulingEvent {
        use tg_syscall::{SyscallId as Id, SyscallResult as Ret};
        use SchedulingEvent as Event;

        let id_raw: usize = self.ctx.a(7);
        let args = [
            self.ctx.a(0),
            self.ctx.a(1),
            self.ctx.a(2),
            self.ctx.a(3),
            self.ctx.a(4),
            self.ctx.a(5),
        ];

        // Intercept custom FB syscalls before standard dispatch
        #[cfg(target_arch = "riscv64")]
        match id_raw {
            crate::SYSCALL_FB_INFO => {
                let ret = crate::handle_fb_info();
                *self.ctx.a_mut(0) = ret;
                self.ctx.move_next();
                return Event::None;
            }
            crate::SYSCALL_FB_WRITE => {
                let ret = crate::handle_fb_write(args[0], args[1], args[2], args[3], args[4]);
                *self.ctx.a_mut(0) = ret;
                self.ctx.move_next();
                return Event::None;
            }
            _ => {}
        }

        // Count standard syscalls
        if id_raw < 500 {
            unsafe { crate::SYSCALL_COUNTS[task_idx][id_raw] += 1; }
        }

        let id = id_raw.into();
        match tg_syscall::handle(Caller { entity: task_idx, flow: 0 }, id, args) {
            Ret::Done(ret) => match id {
                Id::EXIT => Event::Exit(self.ctx.a(0)),
                Id::SCHED_YIELD => {
                    *self.ctx.a_mut(0) = ret as _;
                    self.ctx.move_next();
                    Event::Yield
                }
                _ => {
                    *self.ctx.a_mut(0) = ret as _;
                    self.ctx.move_next();
                    Event::None
                }
            },
            Ret::Unsupported(_) => Event::UnsupportedSyscall(id),
        }
    }
}

//! Doom game port — doomgeneric platform layer + libc shim.

mod libc_shim;
mod platform;
mod keymap;

unsafe extern "C" {
    fn doomgeneric_Create(argc: i32, argv: *mut *mut i8);
    fn doomgeneric_Tick();
}

/// Run the Doom game.
pub fn run_game() {
    tg_syscall::write(tg_syscall::STDOUT, b"[doom] initializing...\n");
    // Initialize the Doom heap (12 MiB via sbrk)
    libc_shim::init_heap();

    // Test basic libc functions
    tg_syscall::write(tg_syscall::STDOUT, b"[doom] testing malloc\n");
    let p = libc_shim::malloc(64);
    if p.is_null() {
        tg_syscall::write(tg_syscall::STDOUT, b"[doom] malloc FAILED\n");
    } else {
        tg_syscall::write(tg_syscall::STDOUT, b"[doom] malloc OK\n");
    }
    tg_syscall::write(tg_syscall::STDOUT, b"[doom] testing strlen\n");
    let len = libc_shim::strlen(b"hello\0".as_ptr() as *const core::ffi::c_char);
    if len == 5 {
        tg_syscall::write(tg_syscall::STDOUT, b"[doom] strlen OK\n");
    } else {
        tg_syscall::write(tg_syscall::STDOUT, b"[doom] strlen WRONG\n");
    }

    unsafe {
        let mut arg0 = *b"doom\0";
        let mut arg1 = *b"-iwad\0";
        let mut arg2 = *b"doom1.wad\0";
        let mut argv: [*mut i8; 3] = [
            arg0.as_mut_ptr() as *mut i8,
            arg1.as_mut_ptr() as *mut i8,
            arg2.as_mut_ptr() as *mut i8,
        ];
        doomgeneric_Create(3, argv.as_mut_ptr());
        loop {
            doomgeneric_Tick();
        }
    }
}

#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    #[cfg(feature = "tangram")]
    {
        let idx: usize = 2;
        println!("[tangram] rendering piece {}: {}", idx, user_lib::tangram::PIECE_NAMES[idx]);
        user_lib::tangram::render_piece(idx);
        user_lib::tangram::spin_wait_ms(300);
    }
    0
}

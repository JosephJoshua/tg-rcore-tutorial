#![no_std]
#![no_main]

extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    #[cfg(feature = "tetris")]
    user_lib::tetris::run_game();
    0
}

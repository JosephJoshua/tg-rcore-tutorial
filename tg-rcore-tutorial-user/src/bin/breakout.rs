#![no_std]
#![no_main]

extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    #[cfg(feature = "breakout")]
    user_lib::breakout::run_game();
    0
}

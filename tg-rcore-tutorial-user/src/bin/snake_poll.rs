#![no_std]
#![no_main]

#[macro_use]
extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    user_lib::snake::run_game(user_lib::STDIN);
    0
}

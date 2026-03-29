#![no_std]
#![no_main]

extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    #[cfg(feature = "pingpong")]
    {
        user_lib::pingpong::run();
    }
    0
}

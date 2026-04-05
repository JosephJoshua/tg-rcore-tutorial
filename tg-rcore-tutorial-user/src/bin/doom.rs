#![no_std]
#![no_main]

extern crate user_lib;

#[unsafe(no_mangle)]
extern "C" fn main() -> i32 {
    user_lib::write(user_lib::STDOUT, b"[doom] main() entered\n");
    #[cfg(feature = "doom")]
    {
        user_lib::doom::run_game();
    }
    0
}

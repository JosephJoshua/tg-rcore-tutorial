//! DG_ platform function implementations for rCore.
//!
//! Implements the 6 doomgeneric platform functions as #[no_mangle] extern "C".

use super::keymap;
use crate::{fb_flush, fb_info, fb_write, get_time, read, sleep, STDIN};

// Doom screen dimensions (from doomgeneric.h defaults)
const DOOM_RESX: usize = 640;
const DOOM_RESY: usize = 400;

// Key event ring buffer — use volatile reads/writes to prevent optimizer
// from removing the queue logic (release mode optimizes away static mut accesses)
const KEY_QUEUE_SIZE: usize = 64;
static mut KEY_QUEUE: [(i32, u8); KEY_QUEUE_SIZE] = [(0, 0); KEY_QUEUE_SIZE];
static mut KEY_HEAD: usize = 0;
static mut KEY_TAIL: usize = 0;

fn volatile_load_head() -> usize {
    unsafe { core::ptr::read_volatile(&raw const KEY_HEAD) }
}
fn volatile_load_tail() -> usize {
    unsafe { core::ptr::read_volatile(&raw const KEY_TAIL) }
}
fn volatile_store_head(v: usize) {
    unsafe { core::ptr::write_volatile(&raw mut KEY_HEAD, v) }
}
fn volatile_store_tail(v: usize) {
    unsafe { core::ptr::write_volatile(&raw mut KEY_TAIL, v) }
}

// Screen info cached from fb_info()
static mut FB_WIDTH: u32 = 0;
static mut FB_HEIGHT: u32 = 0;

unsafe extern "C" {
    static mut DG_ScreenBuffer: *mut u32;
}

/// DG_Init — called once at startup by doomgeneric_Create().
#[unsafe(no_mangle)]
pub extern "C" fn DG_Init() {
    let (w, h) = fb_info();
    unsafe {
        FB_WIDTH = w;
        FB_HEIGHT = h;
    }
}

/// DG_DrawFrame — blit DG_ScreenBuffer to GPU framebuffer, centered.
#[unsafe(no_mangle)]
pub extern "C" fn DG_DrawFrame() {
    unsafe {
        let buf = DG_ScreenBuffer;
        if buf.is_null() {
            return;
        }

        let fb_w = FB_WIDTH as usize;
        let fb_h = FB_HEIGHT as usize;

        // Center the 640x400 Doom screen on the display
        let offset_x = fb_w.saturating_sub(DOOM_RESX) / 2;
        let offset_y = fb_h.saturating_sub(DOOM_RESY) / 2;

        // Set alpha byte to 0xFF for all pixels (XRGB -> ARGB with opaque alpha)
        let pixel_count = DOOM_RESX * DOOM_RESY;
        let pixels = core::slice::from_raw_parts_mut(buf, pixel_count);
        for p in pixels.iter_mut() {
            *p |= 0xFF00_0000;
        }

        // Write row by row to framebuffer
        for row in 0..DOOM_RESY {
            let row_ptr = buf.add(row * DOOM_RESX) as *const u8;
            fb_write(
                offset_x as u32,
                (offset_y + row) as u32,
                DOOM_RESX as u32,
                1,
                row_ptr,
            );
        }

        fb_flush();
    }

    // Poll keyboard while we're here
    poll_keyboard();
}

/// DG_SleepMs — sleep for N milliseconds.
#[unsafe(no_mangle)]
pub extern "C" fn DG_SleepMs(ms: u32) {
    sleep(ms as usize);
}

/// DG_GetTicksMs — milliseconds since program start.
#[unsafe(no_mangle)]
pub extern "C" fn DG_GetTicksMs() -> u32 {
    get_time() as u32
}

/// DG_GetKey — return 1 if key event available, 0 if not.
#[unsafe(no_mangle)]
pub extern "C" fn DG_GetKey(pressed: *mut i32, doom_key: *mut u8) -> i32 {
    // Poll keyboard before checking queue (I_GetEvent calls this before DG_DrawFrame)
    poll_keyboard();
    let head = volatile_load_head();
    let tail = volatile_load_tail();
    if head == tail {
        return 0;
    }
    unsafe {
        let idx = tail % KEY_QUEUE_SIZE;
        let entry = core::ptr::read_volatile(&raw const KEY_QUEUE[idx]);
        volatile_store_tail(tail + 1);
        *pressed = entry.0;
        *doom_key = entry.1;
        1
    }
}

/// DG_SetWindowTitle — no-op on VNC framebuffer.
#[unsafe(no_mangle)]
pub extern "C" fn DG_SetWindowTitle(_title: *const u8) {}

/// Poll VirtIO keyboard and enqueue translated key events.
fn poll_keyboard() {
    let mut buf = [0u8; 1];
    loop {
        let n = read(STDIN, &mut buf);
        if n <= 0 || buf[0] == 0 {
            break;
        }
        let raw = buf[0];
        let pressed = (raw & 0x80) == 0;
        let scancode = raw & 0x7F;

        // Debug: log raw keyboard input
        tg_syscall::write(tg_syscall::STDOUT, b"[K]");
        tg_syscall::write(tg_syscall::STDOUT, &[b'0' + scancode / 100, b'0' + (scancode/10)%10, b'0' + scancode%10]);
        if pressed { tg_syscall::write(tg_syscall::STDOUT, b"v\n"); }
        else { tg_syscall::write(tg_syscall::STDOUT, b"^\n"); }

        let doom_key = keymap::translate(scancode);
        if doom_key != 0 {
            let head = volatile_load_head();
            unsafe {
                let idx = head % KEY_QUEUE_SIZE;
                let entry = (if pressed { 1i32 } else { 0i32 }, doom_key);
                core::ptr::write_volatile(&raw mut KEY_QUEUE[idx], entry);
            }
            volatile_store_head(head + 1);
        }
    }
}

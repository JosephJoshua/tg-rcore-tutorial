//! Minimal libc implementation for Doom.
//!
//! Provides malloc/free, string ops, file I/O (with in-memory WAD), character
//! classification, qsort, and misc stubs. Printf family is implemented in C
//! (doom_libc.c) to avoid va_list FFI issues.

use core::ffi::{c_char, c_int, c_long, c_uint, c_ulong, c_void};

// ============================================================
// Memory allocator — bump allocator over sbrk()
// ============================================================

static mut HEAP_START: usize = 0;
static mut HEAP_END: usize = 0;
static mut HEAP_POS: usize = 0;

const DOOM_HEAP_SIZE: usize = 16 * 1024 * 1024; // 16 MiB (zone=8M + WAD=5M + overhead)

pub fn init_heap() {
    tg_syscall::write(tg_syscall::STDOUT, b"[doom] init_heap: calling sbrk\n");
    unsafe {
        HEAP_START = tg_syscall::sbrk(DOOM_HEAP_SIZE as i32) as usize;
        HEAP_POS = HEAP_START;
        HEAP_END = HEAP_START + DOOM_HEAP_SIZE;
    }
    tg_syscall::write(tg_syscall::STDOUT, b"[doom] init_heap: done\n");
}

#[unsafe(no_mangle)]
pub extern "C" fn malloc(size: usize) -> *mut c_void {
    unsafe {
        let aligned = (HEAP_POS + 15) & !15;
        let new_pos = aligned + size;
        if new_pos > HEAP_END {
            tg_syscall::write(tg_syscall::STDOUT, b"[doom] MALLOC FAILED!\n");
            return core::ptr::null_mut();
        }
        HEAP_POS = new_pos;
        aligned as *mut c_void
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn free(_ptr: *mut c_void) {
    // Bump allocator: no-op
}

#[unsafe(no_mangle)]
pub extern "C" fn realloc(ptr: *mut c_void, size: usize) -> *mut c_void {
    if ptr.is_null() {
        return malloc(size);
    }
    if size == 0 {
        return core::ptr::null_mut();
    }
    let new = malloc(size);
    if !new.is_null() {
        unsafe { memcpy(new, ptr, size) };
    }
    new
}

#[unsafe(no_mangle)]
pub extern "C" fn calloc(nmemb: usize, size: usize) -> *mut c_void {
    let total = nmemb * size;
    let ptr = malloc(total);
    if !ptr.is_null() {
        unsafe { memset(ptr, 0, total) };
    }
    ptr
}

// ============================================================
// Memory operations
// ============================================================

// memcpy, memset, memmove, memcmp are implemented in C (doom_stubs.c)
// to avoid Rust intrinsic recursion and ensure correct calling convention.
unsafe extern "C" {
    fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
    fn memset(s: *mut c_void, c: c_int, n: usize) -> *mut c_void;
}

// ============================================================
// String operations
// ============================================================

#[unsafe(no_mangle)]
pub extern "C" fn strlen(s: *const c_char) -> usize {
    let mut len = 0;
    unsafe {
        while *s.add(len) != 0 {
            len += 1;
        }
    }
    len
}

#[unsafe(no_mangle)]
pub extern "C" fn strcpy(dest: *mut c_char, src: *const c_char) -> *mut c_char {
    let mut i = 0;
    unsafe {
        loop {
            let c = *src.add(i);
            *dest.add(i) = c;
            if c == 0 {
                break;
            }
            i += 1;
        }
    }
    dest
}

#[unsafe(no_mangle)]
pub extern "C" fn strncpy(dest: *mut c_char, src: *const c_char, n: usize) -> *mut c_char {
    let mut i = 0;
    unsafe {
        while i < n {
            let c = *src.add(i);
            *dest.add(i) = c;
            if c == 0 {
                break;
            }
            i += 1;
        }
        while i < n {
            *dest.add(i) = 0;
            i += 1;
        }
    }
    dest
}

#[unsafe(no_mangle)]
pub extern "C" fn strcmp(s1: *const c_char, s2: *const c_char) -> c_int {
    let mut i = 0;
    unsafe {
        loop {
            let a = *s1.add(i) as u8;
            let b = *s2.add(i) as u8;
            if a != b || a == 0 {
                return a as c_int - b as c_int;
            }
            i += 1;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strncmp(s1: *const c_char, s2: *const c_char, n: usize) -> c_int {
    for i in 0..n {
        unsafe {
            let a = *s1.add(i) as u8;
            let b = *s2.add(i) as u8;
            if a != b || a == 0 {
                return a as c_int - b as c_int;
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn strncasecmp(s1: *const c_char, s2: *const c_char, n: usize) -> c_int {
    for i in 0..n {
        unsafe {
            let a = (*s1.add(i) as u8).to_ascii_lowercase();
            let b = (*s2.add(i) as u8).to_ascii_lowercase();
            if a != b || a == 0 {
                return a as c_int - b as c_int;
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn strcasecmp(s1: *const c_char, s2: *const c_char) -> c_int {
    let mut i = 0;
    unsafe {
        loop {
            let a = (*s1.add(i) as u8).to_ascii_lowercase();
            let b = (*s2.add(i) as u8).to_ascii_lowercase();
            if a != b || a == 0 {
                return a as c_int - b as c_int;
            }
            i += 1;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strcat(dest: *mut c_char, src: *const c_char) -> *mut c_char {
    unsafe {
        let dest_len = strlen(dest);
        strcpy(dest.add(dest_len), src);
    }
    dest
}

#[unsafe(no_mangle)]
pub extern "C" fn strncat(dest: *mut c_char, src: *const c_char, n: usize) -> *mut c_char {
    unsafe {
        let dest_len = strlen(dest);
        let mut i = 0;
        while i < n {
            let c = *src.add(i);
            if c == 0 {
                break;
            }
            *dest.add(dest_len + i) = c;
            i += 1;
        }
        *dest.add(dest_len + i) = 0;
    }
    dest
}

#[unsafe(no_mangle)]
pub extern "C" fn strchr(s: *const c_char, c: c_int) -> *mut c_char {
    let c = c as u8;
    let mut i = 0;
    unsafe {
        loop {
            let ch = *s.add(i) as u8;
            if ch == c {
                return s.add(i) as *mut c_char;
            }
            if ch == 0 {
                return core::ptr::null_mut();
            }
            i += 1;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strrchr(s: *const c_char, c: c_int) -> *mut c_char {
    let c = c as u8;
    let mut last: *mut c_char = core::ptr::null_mut();
    let mut i = 0;
    unsafe {
        loop {
            let ch = *s.add(i) as u8;
            if ch == c {
                last = s.add(i) as *mut c_char;
            }
            if ch == 0 {
                return last;
            }
            i += 1;
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strstr(haystack: *const c_char, needle: *const c_char) -> *mut c_char {
    unsafe {
        let needle_len = strlen(needle);
        if needle_len == 0 {
            return haystack as *mut c_char;
        }
        let haystack_len = strlen(haystack);
        if needle_len > haystack_len {
            return core::ptr::null_mut();
        }
        for i in 0..=(haystack_len - needle_len) {
            if strncmp(haystack.add(i), needle, needle_len) == 0 {
                return haystack.add(i) as *mut c_char;
            }
        }
        core::ptr::null_mut()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strdup(s: *const c_char) -> *mut c_char {
    unsafe {
        let len = strlen(s) + 1;
        let p = malloc(len) as *mut c_char;
        if !p.is_null() {
            core::ptr::copy_nonoverlapping(s as *const u8, p as *mut u8, len);
        }
        p
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strerror(_errnum: c_int) -> *mut c_char {
    b"error\0".as_ptr() as *mut c_char
}

// ============================================================
// Character classification
// ============================================================

#[unsafe(no_mangle)]
pub extern "C" fn isdigit(c: c_int) -> c_int {
    (c >= b'0' as c_int && c <= b'9' as c_int) as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn isspace(c: c_int) -> c_int {
    (c == b' ' as c_int || c == b'\t' as c_int || c == b'\n' as c_int
        || c == b'\r' as c_int || c == 0x0b || c == 0x0c) as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn isalpha(c: c_int) -> c_int {
    ((c >= b'a' as c_int && c <= b'z' as c_int) || (c >= b'A' as c_int && c <= b'Z' as c_int))
        as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn isalnum(c: c_int) -> c_int {
    (isalpha(c) != 0 || isdigit(c) != 0) as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn isprint(c: c_int) -> c_int {
    (c >= 0x20 && c <= 0x7e) as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn isupper(c: c_int) -> c_int {
    (c >= b'A' as c_int && c <= b'Z' as c_int) as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn islower(c: c_int) -> c_int {
    (c >= b'a' as c_int && c <= b'z' as c_int) as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn isxdigit(c: c_int) -> c_int {
    (isdigit(c) != 0
        || (c >= b'a' as c_int && c <= b'f' as c_int)
        || (c >= b'A' as c_int && c <= b'F' as c_int)) as c_int
}

#[unsafe(no_mangle)]
pub extern "C" fn toupper(c: c_int) -> c_int {
    if islower(c) != 0 {
        c - 32
    } else {
        c
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn tolower(c: c_int) -> c_int {
    if isupper(c) != 0 {
        c + 32
    } else {
        c
    }
}

// ============================================================
// Numeric conversion
// ============================================================

#[unsafe(no_mangle)]
pub extern "C" fn atoi(nptr: *const c_char) -> c_int {
    unsafe {
        let mut i = 0;
        // Skip whitespace
        while isspace(*nptr.add(i) as c_int) != 0 {
            i += 1;
        }
        let mut neg = false;
        if *nptr.add(i) == b'-' as c_char {
            neg = true;
            i += 1;
        } else if *nptr.add(i) == b'+' as c_char {
            i += 1;
        }
        let mut val: c_int = 0;
        while isdigit(*nptr.add(i) as c_int) != 0 {
            val = val * 10 + (*nptr.add(i) as u8 - b'0') as c_int;
            i += 1;
        }
        if neg { -val } else { val }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn strtol(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
) -> c_long {
    unsafe {
        let mut i = 0;
        while isspace(*nptr.add(i) as c_int) != 0 {
            i += 1;
        }
        let mut neg = false;
        if *nptr.add(i) == b'-' as c_char {
            neg = true;
            i += 1;
        } else if *nptr.add(i) == b'+' as c_char {
            i += 1;
        }

        let actual_base = if base == 0 {
            if *nptr.add(i) == b'0' as c_char {
                i += 1;
                if *nptr.add(i) == b'x' as c_char || *nptr.add(i) == b'X' as c_char {
                    i += 1;
                    16
                } else {
                    8
                }
            } else {
                10
            }
        } else {
            if base == 16 && *nptr.add(i) == b'0' as c_char
                && (*nptr.add(i + 1) == b'x' as c_char || *nptr.add(i + 1) == b'X' as c_char)
            {
                i += 2;
            }
            base
        };

        let mut val: c_long = 0;
        loop {
            let c = *nptr.add(i) as u8;
            let digit = if c >= b'0' && c <= b'9' {
                (c - b'0') as c_long
            } else if c >= b'a' && c <= b'z' {
                (c - b'a' + 10) as c_long
            } else if c >= b'A' && c <= b'Z' {
                (c - b'A' + 10) as c_long
            } else {
                break;
            };
            if digit >= actual_base as c_long {
                break;
            }
            val = val * actual_base as c_long + digit;
            i += 1;
        }

        if !endptr.is_null() {
            *endptr = nptr.add(i) as *mut c_char;
        }
        if neg { -val } else { val }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn atol(nptr: *const c_char) -> c_long {
    strtol(nptr, core::ptr::null_mut(), 10)
}

#[unsafe(no_mangle)]
pub extern "C" fn strtoul(
    nptr: *const c_char,
    endptr: *mut *mut c_char,
    base: c_int,
) -> c_ulong {
    // Reuse strtol logic — Doom doesn't use values near overflow boundaries
    strtol(nptr, endptr, base) as c_ulong
}

#[unsafe(no_mangle)]
pub extern "C" fn strtod(nptr: *const c_char, endptr: *mut *mut c_char) -> f64 {
    unsafe {
        let mut i = 0;
        while isspace(*nptr.add(i) as c_int) != 0 {
            i += 1;
        }
        let mut neg = false;
        if *nptr.add(i) == b'-' as c_char {
            neg = true;
            i += 1;
        } else if *nptr.add(i) == b'+' as c_char {
            i += 1;
        }

        // Integer part
        let mut val: f64 = 0.0;
        while isdigit(*nptr.add(i) as c_int) != 0 {
            val = val * 10.0 + (*nptr.add(i) as u8 - b'0') as f64;
            i += 1;
        }

        // Fractional part
        if *nptr.add(i) == b'.' as c_char {
            i += 1;
            let mut frac: f64 = 0.1;
            while isdigit(*nptr.add(i) as c_int) != 0 {
                val += (*nptr.add(i) as u8 - b'0') as f64 * frac;
                frac *= 0.1;
                i += 1;
            }
        }

        if !endptr.is_null() {
            *endptr = nptr.add(i) as *mut c_char;
        }
        if neg { -val } else { val }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn atof(nptr: *const c_char) -> f64 {
    strtod(nptr, core::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "C" fn abs(j: c_int) -> c_int {
    if j < 0 { -j } else { j }
}

// ============================================================
// Sorting
// ============================================================

type CmpFn = extern "C" fn(*const c_void, *const c_void) -> c_int;

#[unsafe(no_mangle)]
pub extern "C" fn qsort(
    base: *mut c_void,
    nmemb: usize,
    size: usize,
    compar: CmpFn,
) {
    // Simple insertion sort — sufficient for Doom's usage
    let base = base as *mut u8;
    // Use a small stack buffer for swaps (Doom's qsort calls are on small elements)
    let mut tmp = [0u8; 256];
    let swap_buf = if size <= 256 {
        tmp.as_mut_ptr()
    } else {
        malloc(size) as *mut u8
    };

    for i in 1..nmemb {
        for j in (1..=i).rev() {
            unsafe {
                let a = base.add(j * size);
                let b = base.add((j - 1) * size);
                if compar(a as *const c_void, b as *const c_void) < 0 {
                    core::ptr::copy_nonoverlapping(a, swap_buf, size);
                    core::ptr::copy_nonoverlapping(b, a, size);
                    core::ptr::copy_nonoverlapping(swap_buf, b, size);
                } else {
                    break;
                }
            }
        }
    }
}

// ============================================================
// File I/O — in-memory WAD + fd-based save files
// ============================================================

const MAX_FILES: usize = 16;
const SEEK_SET: c_int = 0;
const SEEK_CUR: c_int = 1;
const SEEK_END: c_int = 2;

#[repr(C)]
struct DoomFile {
    in_use: bool,
    // Memory-backed file (WAD loaded into heap)
    mem_data: *const u8,
    mem_len: usize,
    mem_pos: usize,
    // Fd-backed file (save games)
    fd: isize,
    is_write: bool,
    // Write buffer for fd-backed files
    write_buf: *mut u8,
    write_len: usize,
    write_cap: usize,
}

static mut FILES: [DoomFile; MAX_FILES] = {
    const EMPTY: DoomFile = DoomFile {
        in_use: false,
        mem_data: core::ptr::null(),
        mem_len: 0,
        mem_pos: 0,
        fd: -1,
        is_write: false,
        write_buf: core::ptr::null_mut(),
        write_len: 0,
        write_cap: 0,
    };
    [EMPTY; MAX_FILES]
};

// Stdio FILE pointers (dummy, never read/written through normally)
#[unsafe(no_mangle)]
pub static mut stdin: *mut DoomFile = core::ptr::null_mut();
#[unsafe(no_mangle)]
pub static mut stdout: *mut DoomFile = core::ptr::null_mut();
#[unsafe(no_mangle)]
pub static mut stderr: *mut DoomFile = core::ptr::null_mut();

// WAD cache — once loaded, we never free it
static mut WAD_DATA: *const u8 = core::ptr::null();
static mut WAD_LEN: usize = 0;

fn alloc_file_slot() -> Option<usize> {
    unsafe {
        for i in 0..MAX_FILES {
            if !FILES[i].in_use {
                FILES[i] = DoomFile {
                    in_use: true,
                    mem_data: core::ptr::null(),
                    mem_len: 0,
                    mem_pos: 0,
                    fd: -1,
                    is_write: false,
                    write_buf: core::ptr::null_mut(),
                    write_len: 0,
                    write_cap: 0,
                };
                return Some(i);
            }
        }
        None
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fopen(path: *const c_char, mode: *const c_char) -> *mut DoomFile {
    let slot = match alloc_file_slot() {
        Some(s) => s,
        None => return core::ptr::null_mut(),
    };

    let path_str = unsafe {
        let len = strlen(path);
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(path as *const u8, len))
    };
    let mode_str = unsafe {
        let len = strlen(mode);
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(mode as *const u8, len))
    };

    let is_write = mode_str.contains('w') || mode_str.contains('a');

    if is_write {
        // Write mode: open via syscall
        use tg_syscall::OpenFlags;
        let flags = OpenFlags::CREATE | OpenFlags::WRONLY | OpenFlags::TRUNC;
        let fd = tg_syscall::open(path_str, flags);
        if fd < 0 {
            unsafe { FILES[slot].in_use = false };
            return core::ptr::null_mut();
        }
        unsafe {
            FILES[slot].fd = fd;
            FILES[slot].is_write = true;
            // Allocate write buffer
            let cap = 256 * 1024; // 256 KiB for save games
            FILES[slot].write_buf = malloc(cap) as *mut u8;
            FILES[slot].write_cap = cap;
            FILES[slot].write_len = 0;
        }
    } else {
        // Read mode: load entire file into memory
        let is_wad = path_str.contains("doom1.wad") || path_str.contains("DOOM1.WAD");

        // Check WAD cache first
        if is_wad {
            unsafe {
                if !WAD_DATA.is_null() && WAD_LEN > 0 {
                    tg_syscall::write(tg_syscall::STDOUT, b"[doom] fopen: reusing cached WAD\n");
                    FILES[slot].mem_data = WAD_DATA;
                    FILES[slot].mem_len = WAD_LEN;
                    FILES[slot].mem_pos = 0;
                    return &raw mut FILES[slot];
                }
            }
        }

        use tg_syscall::OpenFlags;
        let fd = tg_syscall::open(path_str, OpenFlags::RDONLY);
        if fd < 0 {
            unsafe { FILES[slot].in_use = false };
            return core::ptr::null_mut();
        }

        // Read file in page-sized chunks (kernel translate is per-page)
        let chunk_size = 4096;
        let mut total = 0usize;
        let buf_cap = if is_wad { 5 * 1024 * 1024 } else { 256 * 1024 };
        let buf = malloc(buf_cap) as *mut u8;
        if buf.is_null() {
            tg_syscall::close(fd as usize);
            unsafe { FILES[slot].in_use = false };
            return core::ptr::null_mut();
        }

        loop {
            if total >= buf_cap { break; }
            let to_read = core::cmp::min(chunk_size, buf_cap - total);
            let slice = unsafe { core::slice::from_raw_parts_mut(buf.add(total), to_read) };
            let n = tg_syscall::read(fd as usize, slice);
            if n <= 0 { break; }
            total += n as usize;
        }

        tg_syscall::close(fd as usize);
        tg_syscall::write(tg_syscall::STDOUT, b"[doom] fopen: loaded file\n");

        // Cache WAD data for future reopens
        if is_wad {
            unsafe {
                WAD_DATA = buf;
                WAD_LEN = total;
            }
        }

        unsafe {
            FILES[slot].mem_data = buf;
            FILES[slot].mem_len = total;
            FILES[slot].mem_pos = 0;
        }
    }

    unsafe { &raw mut FILES[slot] }
}

#[unsafe(no_mangle)]
pub extern "C" fn fclose(f: *mut DoomFile) -> c_int {
    if f.is_null() {
        return -1;
    }
    unsafe {
        let file = &mut *f;
        if file.is_write && file.fd >= 0 {
            // Flush write buffer
            if file.write_len > 0 {
                let data = core::slice::from_raw_parts(file.write_buf, file.write_len);
                tg_syscall::write(file.fd as usize, data);
            }
            tg_syscall::close(file.fd as usize);
        }
        file.in_use = false;
    }
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn fread(
    ptr: *mut c_void,
    size: usize,
    nmemb: usize,
    f: *mut DoomFile,
) -> usize {
    if f.is_null() {
        tg_syscall::write(tg_syscall::STDOUT, b"[fread] NULL file!\n");
        return 0;
    }
    let total = size * nmemb;
    unsafe {
        let file = &mut *f;
        if !file.mem_data.is_null() {
            // Memory-backed read
            let avail = file.mem_len.saturating_sub(file.mem_pos);
            let to_copy = core::cmp::min(total, avail);
            memcpy(ptr, file.mem_data.add(file.mem_pos) as *mut c_void as *const c_void, to_copy);
            file.mem_pos += to_copy;
            to_copy / size
        } else if file.fd >= 0 && !file.is_write {
            let slice = core::slice::from_raw_parts_mut(ptr as *mut u8, total);
            let n = tg_syscall::read(file.fd as usize, slice);
            if n < 0 { 0 } else { n as usize / size }
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fwrite(
    ptr: *const c_void,
    size: usize,
    nmemb: usize,
    f: *mut DoomFile,
) -> usize {
    if f.is_null() {
        return 0;
    }
    let total = size * nmemb;
    unsafe {
        let file = &mut *f;
        if file.is_write && file.fd >= 0 {
            // Buffer writes
            let data = core::slice::from_raw_parts(ptr as *const u8, total);
            if file.write_len + total > file.write_cap {
                // Flush current buffer first
                if file.write_len > 0 {
                    let buf = core::slice::from_raw_parts(file.write_buf, file.write_len);
                    tg_syscall::write(file.fd as usize, buf);
                    file.write_len = 0;
                }
                // If still too large, write directly
                if total > file.write_cap {
                    tg_syscall::write(file.fd as usize, data);
                    return nmemb;
                }
            }
            core::ptr::copy_nonoverlapping(
                data.as_ptr(),
                file.write_buf.add(file.write_len),
                total,
            );
            file.write_len += total;
            nmemb
        } else {
            0
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn fseek(f: *mut DoomFile, offset: c_long, whence: c_int) -> c_int {
    if f.is_null() {
        return -1;
    }
    unsafe {
        let file = &mut *f;
        if !file.mem_data.is_null() {
            let new_pos = match whence {
                SEEK_SET => offset as usize,
                SEEK_CUR => (file.mem_pos as c_long + offset) as usize,
                SEEK_END => (file.mem_len as c_long + offset) as usize,
                _ => return -1,
            };
            if new_pos <= file.mem_len {
                file.mem_pos = new_pos;
                0
            } else {
                -1
            }
        } else {
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ftell(f: *mut DoomFile) -> c_long {
    if f.is_null() {
        return -1;
    }
    unsafe {
        let file = &*f;
        if !file.mem_data.is_null() {
            file.mem_pos as c_long
        } else {
            -1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn remove(_path: *const c_char) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn rename(_oldpath: *const c_char, _newpath: *const c_char) -> c_int {
    -1
}

// ============================================================
// Process control
// ============================================================

#[unsafe(no_mangle)]
pub extern "C" fn exit(status: c_int) -> ! {
    tg_syscall::exit(status);
    unreachable!()
}

#[unsafe(no_mangle)]
pub extern "C" fn abort() -> ! {
    tg_syscall::exit(-1);
    unreachable!()
}

// ============================================================
// Stubs — non-critical functions
// ============================================================

#[unsafe(no_mangle)]
pub static mut errno: c_int = 0;

#[unsafe(no_mangle)]
pub extern "C" fn signal(_signum: c_int, _handler: usize) -> usize {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn atexit(_func: extern "C" fn()) -> c_int {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn getenv(_name: *const c_char) -> *mut c_char {
    core::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub extern "C" fn mkdir(_path: *const c_char, _mode: c_uint) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn stat(_path: *const c_char, _buf: *mut c_void) -> c_int {
    -1
}

#[unsafe(no_mangle)]
pub extern "C" fn access(path: *const c_char, _amode: c_int) -> c_int {
    // Check if file exists by trying to open it
    let path_str = unsafe {
        let len = strlen(path);
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(path as *const u8, len))
    };
    let fd = tg_syscall::open(path_str, tg_syscall::OpenFlags::RDONLY);
    if fd >= 0 {
        tg_syscall::close(fd as usize);
        0
    } else {
        -1
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn time(_t: *mut c_long) -> c_long {
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn localtime(_timep: *const c_long) -> *mut c_void {
    core::ptr::null_mut()
}

// ============================================================
// Callbacks from doom_libc.c (printf implementation in C)
// ============================================================

#[unsafe(no_mangle)]
pub extern "C" fn doom_putchar(c: c_char) {
    tg_syscall::write(tg_syscall::STDOUT, &[c as u8]);
}

#[unsafe(no_mangle)]
pub extern "C" fn doom_write_stdout(s: *const c_char, len: c_int) -> c_int {
    if s.is_null() || len <= 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts(s as *const u8, len as usize) };
    tg_syscall::write(tg_syscall::STDOUT, slice);
    len
}

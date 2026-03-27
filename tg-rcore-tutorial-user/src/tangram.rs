//! Tangram piece geometry, colors, polygon rasterization, and rendering helpers.
//!
//! Gated by the `tangram` feature. Each piece is rendered into a heap-allocated
//! buffer and pushed to the kernel framebuffer via the FB_WRITE syscall.

use alloc::vec;

// ---- Color and palette ----

/// A color in BGRA format (matching VirtIO-GPU's B8G8R8A8UNORM).
#[derive(Clone, Copy)]
pub struct Color {
    pub b: u8,
    pub g: u8,
    pub r: u8,
    pub a: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r, a: 0xFF }
    }
}

/// 7 distinct colors for tangram pieces.
pub const COLORS: [Color; 7] = [
    Color::new(0xE0, 0x40, 0x40), // 0: Red
    Color::new(0xFF, 0xA0, 0x20), // 1: Orange
    Color::new(0xFF, 0xE0, 0x20), // 2: Yellow
    Color::new(0x40, 0xC0, 0x40), // 3: Green
    Color::new(0x20, 0xC0, 0xE0), // 4: Cyan
    Color::new(0x40, 0x60, 0xE0), // 5: Blue
    Color::new(0xC0, 0x40, 0xC0), // 6: Magenta
];

// ---- Piece geometry ----

pub struct Piece {
    pub vertices: &'static [(i32, i32)],
    pub color_idx: usize,
}

// "O" pieces — rectangular frame with hole
const O1: [(i32, i32); 3] = [(140, 184), (440, 184), (140, 274)];
const O2: [(i32, i32); 3] = [(440, 184), (440, 274), (140, 274)];
const O3: [(i32, i32); 4] = [(140, 274), (220, 274), (220, 494), (140, 494)];
const O4: [(i32, i32); 4] = [(360, 274), (440, 274), (440, 494), (360, 494)];
const O5: [(i32, i32); 3] = [(140, 494), (440, 494), (290, 584)];
const O6: [(i32, i32); 3] = [(140, 494), (290, 584), (140, 584)];
const O7: [(i32, i32); 3] = [(440, 494), (440, 584), (290, 584)];

// "S" pieces — two offset blocks forming Z/S shape
const S1: [(i32, i32); 3] = [(580, 184), (840, 184), (710, 384)];
const S2: [(i32, i32); 3] = [(580, 184), (710, 384), (580, 384)];
const S3: [(i32, i32); 3] = [(840, 184), (840, 384), (710, 384)];
const S4: [(i32, i32); 3] = [(540, 384), (800, 384), (670, 484)];
const S5: [(i32, i32); 3] = [(540, 384), (670, 484), (540, 584)];
const S6: [(i32, i32); 3] = [(800, 384), (800, 584), (670, 484)];
const S7: [(i32, i32); 3] = [(540, 584), (670, 484), (800, 584)];

/// All 14 tangram pieces for the "OS" display.
pub static PIECES: [Piece; 14] = [
    Piece { vertices: &O1, color_idx: 0 }, // Red
    Piece { vertices: &O2, color_idx: 1 }, // Orange
    Piece { vertices: &O3, color_idx: 2 }, // Yellow
    Piece { vertices: &O4, color_idx: 3 }, // Green
    Piece { vertices: &O5, color_idx: 4 }, // Cyan
    Piece { vertices: &O6, color_idx: 5 }, // Blue
    Piece { vertices: &O7, color_idx: 6 }, // Magenta
    Piece { vertices: &S1, color_idx: 4 }, // Cyan
    Piece { vertices: &S2, color_idx: 5 }, // Blue
    Piece { vertices: &S3, color_idx: 0 }, // Red
    Piece { vertices: &S4, color_idx: 6 }, // Magenta
    Piece { vertices: &S5, color_idx: 1 }, // Orange
    Piece { vertices: &S6, color_idx: 3 }, // Green
    Piece { vertices: &S7, color_idx: 2 }, // Yellow
];

/// Piece names for serial output.
pub const PIECE_NAMES: [&str; 14] = [
    "O1 (top-left tri)",
    "O2 (top-right tri)",
    "O3 (left bar)",
    "O4 (right bar)",
    "O5 (bottom-center tri)",
    "O6 (bottom-left tri)",
    "O7 (bottom-right tri)",
    "S1 (top-center tri)",
    "S2 (top-left tri)",
    "S3 (top-right tri)",
    "S4 (mid-center tri)",
    "S5 (bottom-left tri)",
    "S6 (bottom-right tri)",
    "S7 (bottom-center tri)",
];

// ---- Polygon rasterization ----

/// Fill a convex polygon into a BGRA pixel buffer.
/// Uses scanline rasterization with integer-only math.
pub fn fill_polygon(fb: &mut [u8], width: u32, height: u32, vertices: &[(i32, i32)], color: Color) {
    if vertices.len() < 3 {
        return;
    }

    let mut min_y = vertices[0].1;
    let mut max_y = vertices[0].1;
    for &(_, y) in vertices {
        if y < min_y { min_y = y; }
        if y > max_y { max_y = y; }
    }

    let min_y = if min_y < 0 { 0 } else { min_y };
    let max_y = if max_y >= height as i32 { height as i32 - 1 } else { max_y };
    let n = vertices.len();

    for y in min_y..=max_y {
        let mut x_intersections = [0i32; 16];
        let mut count = 0;

        for i in 0..n {
            let (x0, y0) = vertices[i];
            let (x1, y1) = vertices[(i + 1) % n];

            if y0 == y1 { continue; }

            let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
            if y < lo || y >= hi { continue; }

            let x = x0 + ((y - y0) as i64 * (x1 - x0) as i64 / (y1 - y0) as i64) as i32;
            if count < 16 {
                x_intersections[count] = x;
                count += 1;
            }
        }

        for i in 1..count {
            let key = x_intersections[i];
            let mut j = i;
            while j > 0 && x_intersections[j - 1] > key {
                x_intersections[j] = x_intersections[j - 1];
                j -= 1;
            }
            x_intersections[j] = key;
        }

        let mut i = 0;
        while i + 1 < count {
            let x_start = if x_intersections[i] < 0 { 0 } else { x_intersections[i] };
            let x_end = if x_intersections[i + 1] >= width as i32 {
                width as i32 - 1
            } else {
                x_intersections[i + 1]
            };

            for x in x_start..=x_end {
                let offset = ((y as u32 * width + x as u32) * 4) as usize;
                if offset + 3 < fb.len() {
                    fb[offset] = color.b;
                    fb[offset + 1] = color.g;
                    fb[offset + 2] = color.r;
                    fb[offset + 3] = color.a;
                }
            }
            i += 2;
        }
    }
}

// ---- Rendering helpers ----

/// Render a single tangram piece to the kernel framebuffer via FB_WRITE.
pub fn render_piece(piece_idx: usize) {
    let piece = &PIECES[piece_idx];
    let color = COLORS[piece.color_idx];

    // Compute bounding box
    let mut min_x = piece.vertices[0].0;
    let mut min_y = piece.vertices[0].1;
    let mut max_x = min_x;
    let mut max_y = min_y;
    for &(x, y) in piece.vertices {
        if x < min_x { min_x = x; }
        if x > max_x { max_x = x; }
        if y < min_y { min_y = y; }
        if y > max_y { max_y = y; }
    }

    let bbox_w = (max_x - min_x + 1) as usize;
    let bbox_h = (max_y - min_y + 1) as usize;

    // Allocate local buffer, filled with white (BGRA 0xFF)
    let buf_size = bbox_w * bbox_h * 4;
    let mut buf = vec![0xFFu8; buf_size];

    // Offset vertices to local buffer coordinates
    let mut local_verts = [(0i32, 0i32); 8];
    for (i, &(x, y)) in piece.vertices.iter().enumerate() {
        local_verts[i] = (x - min_x, y - min_y);
    }
    let local_verts = &local_verts[..piece.vertices.len()];

    // Rasterize into local buffer
    fill_polygon(&mut buf, bbox_w as u32, bbox_h as u32, local_verts, color);

    // Push to kernel framebuffer
    crate::fb_write(min_x as u32, min_y as u32, bbox_w as u32, bbox_h as u32, buf.as_ptr());
}

/// Spin-wait for `ms` milliseconds using rdtime (QEMU virt: 10 MHz timer).
pub fn spin_wait_ms(ms: usize) {
    let ticks = ms * 10_000;
    let start: usize;
    unsafe { core::arch::asm!("rdtime {}", out(reg) start) };
    loop {
        let now: usize;
        unsafe { core::arch::asm!("rdtime {}", out(reg) now) };
        if now.wrapping_sub(start) >= ticks {
            break;
        }
    }
}

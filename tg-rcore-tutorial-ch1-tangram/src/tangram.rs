//! Tangram piece geometry, colors, and polygon rasterization.
//!
//! Each tangram piece is a convex polygon defined by vertex coordinates.
//! Rasterization uses integer-only scanline fill.

/// A color in BGRA format (matching VirtIO-GPU's B8G8R8A8UNORM).
#[derive(Clone, Copy)]
pub struct Color {
    /// Blue channel.
    pub b: u8,
    /// Green channel.
    pub g: u8,
    /// Red channel.
    pub r: u8,
    /// Alpha channel.
    pub a: u8,
}

impl Color {
    /// Create a new opaque color from RGB values.
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { b, g, r, a: 0xFF }
    }
}

/// 7 distinct colors for tangram pieces.
pub const COLORS: [Color; 7] = [
    Color::new(0xE0, 0x40, 0x40), // Red
    Color::new(0xFF, 0xA0, 0x20), // Orange
    Color::new(0xFF, 0xE0, 0x20), // Yellow
    Color::new(0x40, 0xC0, 0x40), // Green
    Color::new(0x20, 0xC0, 0xE0), // Cyan
    Color::new(0x40, 0x60, 0xE0), // Blue
    Color::new(0xC0, 0x40, 0xC0), // Magenta
];

/// White background.
pub const WHITE: Color = Color::new(0xFF, 0xFF, 0xFF);

/// A tangram piece: a polygon (vertices) + color index.
pub struct Piece {
    /// Polygon vertices as (x, y) coordinate pairs.
    pub vertices: &'static [(i32, i32)],
    /// Index into the COLORS array.
    pub color_idx: usize,
}

/// Fill a convex polygon into a BGRA framebuffer.
///
/// Uses scanline rasterization with integer-only math.
/// Assumes the polygon is convex (all tangram pieces are).
pub fn fill_polygon(fb: &mut [u8], width: u32, height: u32, vertices: &[(i32, i32)], color: Color) {
    if vertices.len() < 3 {
        return;
    }

    // Find bounding box.
    let mut min_y = vertices[0].1;
    let mut max_y = vertices[0].1;
    for &(_, y) in vertices {
        if y < min_y {
            min_y = y;
        }
        if y > max_y {
            max_y = y;
        }
    }

    // Clamp to screen.
    let min_y = if min_y < 0 { 0 } else { min_y };
    let max_y = if max_y >= height as i32 {
        height as i32 - 1
    } else {
        max_y
    };

    let n = vertices.len();

    // For each scanline in the bounding box:
    for y in min_y..=max_y {
        // Find X intersections with all edges.
        let mut x_intersections = [0i32; 16]; // Max 16 intersections (more than enough)
        let mut count = 0;

        for i in 0..n {
            let (x0, y0) = vertices[i];
            let (x1, y1) = vertices[(i + 1) % n];

            // Skip horizontal edges.
            if y0 == y1 {
                continue;
            }

            // Check if scanline crosses this edge.
            let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
            if y < lo || y >= hi {
                continue;
            }

            // Linear interpolation using integer math:
            // x = x0 + (y - y0) * (x1 - x0) / (y1 - y0)
            let x = x0 + ((y - y0) as i64 * (x1 - x0) as i64 / (y1 - y0) as i64) as i32;
            if count < 16 {
                x_intersections[count] = x;
                count += 1;
            }
        }

        // Sort intersections (simple insertion sort — at most ~8 elements).
        for i in 1..count {
            let key = x_intersections[i];
            let mut j = i;
            while j > 0 && x_intersections[j - 1] > key {
                x_intersections[j] = x_intersections[j - 1];
                j -= 1;
            }
            x_intersections[j] = key;
        }

        // Fill between pairs of intersections.
        let mut i = 0;
        while i + 1 < count {
            let x_start = if x_intersections[i] < 0 {
                0
            } else {
                x_intersections[i]
            };
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

// ---- Tangram "O" pieces ----
// O is a rectangular frame with a hole in the center.
// Outer: (140,184)→(440,584), Inner hole: (220,274)→(360,494)
//
// Decomposition: top bar (2 tri), left bar (rect), right bar (rect),
//                bottom bar (3 tri) = 7 pieces.

// Top bar: split diagonally
const O1: [(i32, i32); 3] = [(140, 184), (440, 184), (140, 274)];
const O2: [(i32, i32); 3] = [(440, 184), (440, 274), (140, 274)];
// Left bar
const O3: [(i32, i32); 4] = [(140, 274), (220, 274), (220, 494), (140, 494)];
// Right bar
const O4: [(i32, i32); 4] = [(360, 274), (440, 274), (440, 494), (360, 494)];
// Bottom bar: split into 3 triangles meeting at bottom-center
const O5: [(i32, i32); 3] = [(140, 494), (440, 494), (290, 584)];
const O6: [(i32, i32); 3] = [(140, 494), (290, 584), (140, 584)];
const O7: [(i32, i32); 3] = [(440, 494), (440, 584), (290, 584)];

// ---- Tangram "S" pieces ----
// S is two offset rectangular blocks forming a Z/S shape.
// Top block (right-aligned): (580,184)→(840,384), 260×200
// Bottom block (left-aligned): (540,384)→(800,584), 260×200
// Both blocks are 260px wide for symmetry.
//
// Top block: 3 triangles meeting at bottom-center (710,384)
// Bottom block: 4 triangles meeting at center (670,484)

// Top block
const S1: [(i32, i32); 3] = [(580, 184), (840, 184), (710, 384)];
const S2: [(i32, i32); 3] = [(580, 184), (710, 384), (580, 384)];
const S3: [(i32, i32); 3] = [(840, 184), (840, 384), (710, 384)];
// Bottom block
const S4: [(i32, i32); 3] = [(540, 384), (800, 384), (670, 484)];
const S5: [(i32, i32); 3] = [(540, 384), (670, 484), (540, 584)];
const S6: [(i32, i32); 3] = [(800, 384), (800, 584), (670, 484)];
const S7: [(i32, i32); 3] = [(540, 584), (670, 484), (800, 584)];

/// All 14 tangram pieces for the "OS" display.
pub static PIECES: [Piece; 14] = [
    // "O" — 7 pieces (frame with hole)
    Piece { vertices: &O1, color_idx: 0 }, // Red
    Piece { vertices: &O2, color_idx: 1 }, // Orange
    Piece { vertices: &O3, color_idx: 2 }, // Yellow
    Piece { vertices: &O4, color_idx: 3 }, // Green
    Piece { vertices: &O5, color_idx: 4 }, // Cyan
    Piece { vertices: &O6, color_idx: 5 }, // Blue
    Piece { vertices: &O7, color_idx: 6 }, // Magenta
    // "S" — 7 pieces (two offset blocks)
    Piece { vertices: &S1, color_idx: 4 }, // Cyan
    Piece { vertices: &S2, color_idx: 5 }, // Blue
    Piece { vertices: &S3, color_idx: 0 }, // Red
    Piece { vertices: &S4, color_idx: 6 }, // Magenta
    Piece { vertices: &S5, color_idx: 1 }, // Orange
    Piece { vertices: &S6, color_idx: 3 }, // Green
    Piece { vertices: &S7, color_idx: 2 }, // Yellow
];

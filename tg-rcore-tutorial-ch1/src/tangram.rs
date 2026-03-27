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
#[allow(dead_code)]
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

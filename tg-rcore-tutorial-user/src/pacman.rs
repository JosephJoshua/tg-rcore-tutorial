//! Pac-Man game with multi-process ghost AI via pipes and signals.
//!
//! Architecture: parent process renders and handles input; 4 child processes
//! each run one ghost's AI, communicating via pipe pairs and receiving
//! SIGUSR1/SIGUSR2 for frightened/eaten transitions.

use crate::{
    close, exit, fb_flush, fb_info, fb_write, fork, get_time, kill, open, pipe, pipe_read,
    pipe_write, read, sched_yield, sigaction, sigreturn, sleep, wait, write, OpenFlags,
    SignalAction, SignalNo, STDIN,
};

// ============================================================
// Constants
// ============================================================

// Maze dimensions
const MAZE_W: usize = 21;
const MAZE_H: usize = 21;
const CELL_SIZE: usize = 28;

// Framebuffer
const FB_W: usize = 1280;
const FB_H: usize = 800;

// Maze pixel offset (left-aligned with some padding)
const MAZE_PX_X: usize = 40;
const MAZE_PX_Y: usize = (FB_H - MAZE_H * CELL_SIZE) / 2;

// HUD position (right of maze)
const HUD_X: usize = MAZE_PX_X + MAZE_W * CELL_SIZE + 40;
const HUD_Y: usize = MAZE_PX_Y;

// Character size within cell (centered with padding)
const CHAR_SIZE: usize = 20;
const CHAR_PAD: usize = (CELL_SIZE - CHAR_SIZE) / 2;

// Tile buffer for fill_rect (16x16 = 1024 bytes max)
const TILE_SIZE: usize = 16;

// Timing (ticks at ~8 Hz game logic)
const FRAME_MS: usize = 125; // 8 ticks per second
const SCATTER_TICKS: u32 = 56; // 7 seconds
const CHASE_TICKS: u32 = 160; // 20 seconds
const FRIGHT_TICKS: u32 = 48; // 6 seconds
const READY_TICKS: u32 = 16; // 2 seconds
const DYING_TICKS: u32 = 8; // 1 second
const FLASH_TICKS: u32 = 16; // 2 seconds

// Directions
const DIR_UP: u8 = 0;
const DIR_RIGHT: u8 = 1;
const DIR_DOWN: u8 = 2;
const DIR_LEFT: u8 = 3;
const DIR_NONE: u8 = 255;

// Direction deltas: [dy, dx] for UP, RIGHT, DOWN, LEFT
const DX: [i8; 4] = [0, 1, 0, -1];
const DY: [i8; 4] = [-1, 0, 1, 0];

// Ghost indices
const BLINKY: usize = 0;
const PINKY: usize = 1;
const INKY: usize = 2;
const CLYDE: usize = 3;

// Ghost scatter corners (grid coords)
const SCATTER_TARGETS: [[u8; 2]; 4] = [
    [0, 20],  // Blinky: top-right
    [0, 0],   // Pinky: top-left
    [20, 20], // Inky: bottom-right
    [20, 0],  // Clyde: bottom-left
];

// Ghost house position
const GHOST_HOUSE_X: u8 = 10;
const GHOST_HOUSE_Y: u8 = 10;

// Pac-Man start position
const PAC_START_X: u8 = 10;
const PAC_START_Y: u8 = 15;

// Ghost start positions
const GHOST_START: [[u8; 2]; 4] = [
    [8, 10],  // Blinky: above ghost house
    [10, 9],  // Pinky: inside ghost house
    [10, 10], // Inky: inside ghost house
    [10, 11], // Clyde: inside ghost house
];

// Keyboard scancodes
const KEY_W: u8 = 17;
const KEY_A: u8 = 30;
const KEY_S: u8 = 31;
const KEY_D: u8 = 32;
const KEY_UP: u8 = 72;
const KEY_LEFT: u8 = 75;
const KEY_DOWN: u8 = 80;
const KEY_RIGHT: u8 = 77;
const KEY_SPACE: u8 = 57;
const KEY_Q: u8 = 16;
const KEY_ESC: u8 = 1;

// Scoring
const DOT_SCORE: u32 = 10;
const PELLET_SCORE: u32 = 50;
const GHOST_SCORES: [u32; 4] = [200, 400, 800, 1600];

// Initial lives
const INIT_LIVES: u32 = 3;

// Pipe protocol sizes
const PIPE_TO_GHOST: usize = 7;
const _PIPE_FROM_GHOST: usize = 1;

// ============================================================
// Colors (BGRA byte order)
// ============================================================

const BG_VOID: [u8; 4] = [0x00, 0x00, 0x00, 0xFF];
const PATH_COLOR: [u8; 4] = [0x08, 0x08, 0x08, 0xFF];
const WALL_FACE: [u8; 4] = [0xDE, 0x21, 0x21, 0xFF];
const WALL_EDGE: [u8; 4] = [0xFF, 0x52, 0x52, 0xFF];
const WALL_SHADOW: [u8; 4] = [0x80, 0x10, 0x10, 0xFF];
const DOT_COLOR: [u8; 4] = [0x97, 0xB8, 0xFF, 0xFF];
const PELLET_COLOR: [u8; 4] = [0xD0, 0xFF, 0xFF, 0xFF];
const PAC_BODY: [u8; 4] = [0x00, 0xD7, 0xFF, 0xFF];
const PAC_MOUTH: [u8; 4] = [0x08, 0x08, 0x08, 0xFF]; // same as path
const GHOST_COLORS: [[u8; 4]; 4] = [
    [0x00, 0x00, 0xFF, 0xFF], // Blinky: red
    [0xC0, 0x80, 0xFF, 0xFF], // Pinky: pink
    [0xDE, 0xFF, 0x00, 0xFF], // Inky: cyan
    [0x52, 0xB8, 0xFF, 0xFF], // Clyde: orange
];
const FRIGHT_BODY: [u8; 4] = [0xB8, 0x18, 0x18, 0xFF];
const GHOST_EYE_WHITE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const GHOST_EYE_PUPIL: [u8; 4] = [0x40, 0x10, 0x10, 0xFF];
const FRIGHT_EYE: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const TEXT_COLOR: [u8; 4] = [0xEE, 0xF0, 0xFF, 0xFF];
const TEXT_LABEL: [u8; 4] = [0x60, 0x50, 0x80, 0xFF];
const TEXT_DIM: [u8; 4] = [0x40, 0x30, 0x50, 0xFF];
const READY_COLOR: [u8; 4] = [0x00, 0xD7, 0xFF, 0xFF];
const GAMEOVER_COLOR: [u8; 4] = [0x30, 0x30, 0xFF, 0xFF];

// ============================================================
// Maze layout
// ============================================================
// W=wall, D=dot, P=power pellet, E=empty, G=ghost house, T=tunnel

const CELL_WALL: u8 = 0;
const CELL_DOT: u8 = 1;
const CELL_PELLET: u8 = 2;
const CELL_EMPTY: u8 = 3;
const _CELL_GHOST: u8 = 4;
const _CELL_TUNNEL: u8 = 5;

#[rustfmt::skip]
static INITIAL_MAZE: [u8; MAZE_W * MAZE_H] = [
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
    0,1,1,1,1,1,1,1,1,1,0,1,1,1,1,1,1,1,1,1,0,
    0,2,0,0,1,0,0,0,0,1,0,1,0,0,0,0,1,0,0,2,0,
    0,1,0,0,1,0,0,0,0,1,0,1,0,0,0,0,1,0,0,1,0,
    0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0,
    0,1,0,0,1,0,1,0,0,0,0,0,0,0,1,0,1,0,0,1,0,
    0,1,1,1,1,0,1,1,1,0,0,0,1,1,1,0,1,1,1,1,0,
    0,0,0,0,1,0,0,0,3,0,0,0,3,0,0,0,1,0,0,0,0,
    0,0,0,0,1,0,3,3,3,4,4,4,3,3,3,0,1,0,0,0,0,
    5,3,3,3,1,0,3,0,0,4,4,4,0,0,3,0,1,3,3,3,5,
    0,0,0,0,1,0,3,0,0,4,4,4,0,0,3,0,1,0,0,0,0,
    0,0,0,0,1,0,3,0,0,0,0,0,0,0,3,0,1,0,0,0,0,
    0,0,0,0,1,0,3,3,3,3,3,3,3,3,3,0,1,0,0,0,0,
    0,1,1,1,1,1,1,1,1,0,0,0,1,1,1,1,1,1,1,1,0,
    0,1,0,0,1,0,0,0,1,0,0,0,1,0,0,0,1,0,0,1,0,
    0,2,1,0,1,1,1,1,1,1,3,1,1,1,1,1,1,0,1,2,0,
    0,0,1,0,1,0,1,0,0,0,0,0,0,0,1,0,1,0,1,0,0,
    0,1,1,1,1,0,1,1,1,0,0,0,1,1,1,0,1,1,1,1,0,
    0,1,0,0,0,0,0,0,1,0,0,0,1,0,0,0,0,0,0,1,0,
    0,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,0,
    0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,
];

fn total_dots() -> u32 {
    let mut count = 0u32;
    let mut i = 0;
    while i < MAZE_W * MAZE_H {
        if INITIAL_MAZE[i] == CELL_DOT || INITIAL_MAZE[i] == CELL_PELLET {
            count += 1;
        }
        i += 1;
    }
    count
}

// ============================================================
// Font (5x7 bitmap, scale 2)
// ============================================================

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const FONT_SCALE: usize = 2;
const CHAR_PX_W: usize = GLYPH_W * FONT_SCALE + 2;

#[rustfmt::skip]
const GLYPHS: [(u8, [u8; 7]); 40] = [
    (b'0', [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
    (b'1', [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    (b'2', [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111]),
    (b'3', [0b01110, 0b10001, 0b00001, 0b00110, 0b00001, 0b10001, 0b01110]),
    (b'4', [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
    (b'5', [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
    (b'6', [0b01110, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b01110]),
    (b'7', [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000]),
    (b'8', [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
    (b'9', [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b10001, 0b01110]),
    (b'A', [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    (b'B', [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110]),
    (b'C', [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110]),
    (b'D', [0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110]),
    (b'E', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111]),
    (b'F', [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000]),
    (b'G', [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110]),
    (b'H', [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
    (b'I', [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
    (b'K', [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001]),
    (b'L', [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111]),
    (b'M', [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001]),
    (b'N', [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001]),
    (b'O', [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
    (b'P', [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000]),
    (b'R', [0b11110, 0b10001, 0b10001, 0b11110, 0b10010, 0b10001, 0b10001]),
    (b'S', [0b01110, 0b10001, 0b10000, 0b01110, 0b00001, 0b10001, 0b01110]),
    (b'T', [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
    (b'U', [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
    (b'V', [0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b01010, 0b00100]),
    (b'W', [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001]),
    (b'X', [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001]),
    (b'Y', [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100]),
    (b' ', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
    (b'-', [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000]),
    (b'!', [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100]),
    (b':', [0b00000, 0b00100, 0b00000, 0b00000, 0b00000, 0b00100, 0b00000]),
    (b'.', [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00100]),
    (b'Q', [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101]),
    (b'J', [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100]),
];

// ============================================================
// PRNG (xorshift32)
// ============================================================

static mut RNG_STATE: u32 = 12345;

fn xorshift32() -> u32 {
    unsafe {
        let mut x = RNG_STATE;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        RNG_STATE = x;
        x
    }
}

fn seed_rng(s: u32) {
    unsafe {
        RNG_STATE = if s == 0 { 1 } else { s };
    }
}

// ============================================================
// Rendering helpers
// ============================================================

/// Fill a rectangle with a solid color using 16x16 tile writes.
fn fill_rect(x: usize, y: usize, w: usize, h: usize, color: [u8; 4]) {
    if w == 0 || h == 0 {
        return;
    }
    // Build a row of TILE_SIZE pixels
    let mut tile = [0u8; TILE_SIZE * TILE_SIZE * 4];
    for i in 0..TILE_SIZE * TILE_SIZE {
        tile[i * 4] = color[0];
        tile[i * 4 + 1] = color[1];
        tile[i * 4 + 2] = color[2];
        tile[i * 4 + 3] = color[3];
    }

    let mut ry = 0;
    while ry < h {
        let th = if ry + TILE_SIZE <= h {
            TILE_SIZE
        } else {
            h - ry
        };
        let mut rx = 0;
        while rx < w {
            let tw = if rx + TILE_SIZE <= w {
                TILE_SIZE
            } else {
                w - rx
            };
            fb_write(
                (x + rx) as u32,
                (y + ry) as u32,
                tw as u32,
                th as u32,
                tile.as_ptr(),
            );
            rx += TILE_SIZE;
        }
        ry += TILE_SIZE;
    }
}

/// Draw a single glyph character at pixel position.
fn draw_char(x: usize, y: usize, ch: u8, color: [u8; 4]) {
    let glyph = match GLYPHS.iter().find(|(c, _)| *c == ch.to_ascii_uppercase()) {
        Some((_, rows)) => rows,
        None => return,
    };
    let pw = GLYPH_W * FONT_SCALE;
    let ph = GLYPH_H * FONT_SCALE;
    // Buffer for one character: 10x14 = 140 pixels * 4 = 560 bytes
    let mut buf = [0u8; 10 * 14 * 4];
    for gr in 0..GLYPH_H {
        for gc in 0..GLYPH_W {
            if (glyph[gr] >> (GLYPH_W - 1 - gc)) & 1 == 1 {
                for sr in 0..FONT_SCALE {
                    for sc in 0..FONT_SCALE {
                        let idx = ((gr * FONT_SCALE + sr) * pw + gc * FONT_SCALE + sc) * 4;
                        buf[idx] = color[0];
                        buf[idx + 1] = color[1];
                        buf[idx + 2] = color[2];
                        buf[idx + 3] = color[3];
                    }
                }
            }
        }
    }
    fb_write(x as u32, y as u32, pw as u32, ph as u32, buf.as_ptr());
}

fn draw_string(x: usize, y: usize, s: &[u8], color: [u8; 4]) {
    for (i, &ch) in s.iter().enumerate() {
        draw_char(x + i * CHAR_PX_W, y, ch, color);
    }
}

fn draw_number(x: usize, y: usize, mut n: u32, color: [u8; 4]) {
    let mut d = [0u8; 10];
    let mut len = 0;
    if n == 0 {
        d[0] = b'0';
        len = 1;
    } else {
        while n > 0 {
            d[len] = b'0' + (n % 10) as u8;
            n /= 10;
            len += 1;
        }
        // Reverse
        let mut i = 0;
        while i < len / 2 {
            d.swap(i, len - 1 - i);
            i += 1;
        }
    }
    draw_string(x, y, &d[..len], color);
}

// ============================================================
// Maze helpers
// ============================================================

fn maze_idx(gx: u8, gy: u8) -> usize {
    gy as usize * MAZE_W + gx as usize
}

fn is_walkable(maze: &[u8], gx: i32, gy: i32) -> bool {
    // Handle tunnel wrapping
    if gy < 0 || gy >= MAZE_H as i32 {
        return false;
    }
    if gx < 0 || gx >= MAZE_W as i32 {
        // Tunnel row check
        return gy == 9; // row 9 has tunnels
    }
    let cell = maze[gy as usize * MAZE_W + gx as usize];
    cell != CELL_WALL
}

fn is_walkable_for_ghost(maze: &[u8], gx: i32, gy: i32) -> bool {
    if gy < 0 || gy >= MAZE_H as i32 {
        return false;
    }
    if gx < 0 || gx >= MAZE_W as i32 {
        return gy == 9;
    }
    let cell = maze[gy as usize * MAZE_W + gx as usize];
    cell != CELL_WALL
}

fn tunnel_wrap_x(gx: i32) -> u8 {
    if gx < 0 {
        (MAZE_W as i32 - 1) as u8
    } else if gx >= MAZE_W as i32 {
        0
    } else {
        gx as u8
    }
}

fn manhattan(x1: u8, y1: u8, x2: u8, y2: u8) -> u32 {
    let dx = if x1 > x2 { x1 - x2 } else { x2 - x1 } as u32;
    let dy = if y1 > y2 { y1 - y2 } else { y2 - y1 } as u32;
    dx + dy
}

fn opposite_dir(d: u8) -> u8 {
    match d {
        DIR_UP => DIR_DOWN,
        DIR_DOWN => DIR_UP,
        DIR_LEFT => DIR_RIGHT,
        DIR_RIGHT => DIR_LEFT,
        _ => DIR_NONE,
    }
}

// ============================================================
// Maze rendering
// ============================================================

/// Check if a neighbor cell in a given direction is not a wall.
fn neighbor_is_path(maze: &[u8], gx: usize, gy: usize, dir: u8) -> bool {
    let nx = gx as i32 + DX[dir as usize] as i32;
    let ny = gy as i32 + DY[dir as usize] as i32;
    if nx < 0 || nx >= MAZE_W as i32 || ny < 0 || ny >= MAZE_H as i32 {
        return true; // edges facing outside treated as corridor-facing for tunnel entrances
    }
    maze[ny as usize * MAZE_W + nx as usize] != CELL_WALL
}

fn draw_wall_cell(maze: &[u8], gx: usize, gy: usize) {
    let px = MAZE_PX_X + gx * CELL_SIZE;
    let py = MAZE_PX_Y + gy * CELL_SIZE;
    let cs = CELL_SIZE;
    let bevel: usize = 2;

    // Fill with shadow base
    fill_rect(px, py, cs, cs, WALL_SHADOW);

    // Add bright edge strips on sides facing corridors
    if neighbor_is_path(maze, gx, gy, DIR_UP) {
        fill_rect(px, py, cs, bevel, WALL_EDGE);
    }
    if neighbor_is_path(maze, gx, gy, DIR_DOWN) {
        fill_rect(px, py + cs - bevel, cs, bevel, WALL_EDGE);
    }
    if neighbor_is_path(maze, gx, gy, DIR_LEFT) {
        fill_rect(px, py, bevel, cs, WALL_EDGE);
    }
    if neighbor_is_path(maze, gx, gy, DIR_RIGHT) {
        fill_rect(px + cs - bevel, py, bevel, cs, WALL_EDGE);
    }

    // Interior face
    fill_rect(
        px + bevel,
        py + bevel,
        cs - bevel * 2,
        cs - bevel * 2,
        WALL_FACE,
    );
}

fn draw_path_cell(px: usize, py: usize) {
    fill_rect(px, py, CELL_SIZE, CELL_SIZE, PATH_COLOR);
}

fn draw_dot(gx: usize, gy: usize) {
    let px = MAZE_PX_X + gx * CELL_SIZE + CELL_SIZE / 2 - 2;
    let py = MAZE_PX_Y + gy * CELL_SIZE + CELL_SIZE / 2 - 2;
    fill_rect(px, py, 4, 4, DOT_COLOR);
}

fn draw_pellet(gx: usize, gy: usize, visible: bool) {
    let px = MAZE_PX_X + gx * CELL_SIZE + CELL_SIZE / 2 - 5;
    let py = MAZE_PX_Y + gy * CELL_SIZE + CELL_SIZE / 2 - 5;
    if visible {
        fill_rect(px, py, 10, 10, PELLET_COLOR);
    } else {
        fill_rect(px, py, 10, 10, PATH_COLOR);
    }
}

fn draw_maze(maze: &[u8]) {
    for gy in 0..MAZE_H {
        for gx in 0..MAZE_W {
            let cell = maze[gy * MAZE_W + gx];
            let px = MAZE_PX_X + gx * CELL_SIZE;
            let py = MAZE_PX_Y + gy * CELL_SIZE;
            match cell {
                CELL_WALL => draw_wall_cell(maze, gx, gy),
                CELL_DOT => {
                    draw_path_cell(px, py);
                    draw_dot(gx, gy);
                }
                CELL_PELLET => {
                    draw_path_cell(px, py);
                    draw_pellet(gx, gy, true);
                }
                _ => {
                    // EMPTY, GHOST, TUNNEL
                    draw_path_cell(px, py);
                }
            }
        }
    }
}

// ============================================================
// Character rendering
// ============================================================

fn cell_px(gx: u8, gy: u8) -> (usize, usize) {
    (
        MAZE_PX_X + gx as usize * CELL_SIZE + CHAR_PAD,
        MAZE_PX_Y + gy as usize * CELL_SIZE + CHAR_PAD,
    )
}

fn erase_cell(gx: u8, gy: u8) {
    let px = MAZE_PX_X + gx as usize * CELL_SIZE;
    let py = MAZE_PX_Y + gy as usize * CELL_SIZE;
    fill_rect(px, py, CELL_SIZE, CELL_SIZE, PATH_COLOR);
}

/// Draw Pac-Man as a filled circle with a mouth wedge.
fn draw_pacman(gx: u8, gy: u8, dir: u8, mouth_open: bool) {
    let (px, py) = cell_px(gx, gy);
    let cs = CHAR_SIZE;
    let half = cs / 2;
    let r = half as i32;
    let r2 = r * r;

    // 20x20 pixel buffer = 1600 bytes
    let mut buf = [0u8; 20 * 20 * 4];
    // Fill with path color first (transparent background)
    for i in 0..(cs * cs) {
        buf[i * 4] = PATH_COLOR[0];
        buf[i * 4 + 1] = PATH_COLOR[1];
        buf[i * 4 + 2] = PATH_COLOR[2];
        buf[i * 4 + 3] = PATH_COLOR[3];
    }

    for row in 0..cs {
        for col in 0..cs {
            let dy = row as i32 - r;
            let dx = col as i32 - r;
            if dx * dx + dy * dy <= r2 {
                // Check if in mouth wedge
                let in_mouth = if mouth_open {
                    match dir {
                        DIR_RIGHT => dx > 0 && dy.unsigned_abs() as i32 * 3 < dx * 2,
                        DIR_LEFT => dx < 0 && dy.unsigned_abs() as i32 * 3 < (-dx) * 2,
                        DIR_UP => dy < 0 && dx.unsigned_abs() as i32 * 3 < (-dy) * 2,
                        DIR_DOWN => dy > 0 && dx.unsigned_abs() as i32 * 3 < dy * 2,
                        _ => false,
                    }
                } else {
                    false
                };
                let idx = (row * cs + col) * 4;
                if in_mouth {
                    buf[idx] = PAC_MOUTH[0];
                    buf[idx + 1] = PAC_MOUTH[1];
                    buf[idx + 2] = PAC_MOUTH[2];
                    buf[idx + 3] = PAC_MOUTH[3];
                } else {
                    buf[idx] = PAC_BODY[0];
                    buf[idx + 1] = PAC_BODY[1];
                    buf[idx + 2] = PAC_BODY[2];
                    buf[idx + 3] = PAC_BODY[3];
                }
            }
        }
    }

    fb_write(px as u32, py as u32, cs as u32, cs as u32, buf.as_ptr());
}

/// Draw a ghost sprite (body + eyes + wavy skirt).
fn draw_ghost(gx: u8, gy: u8, ghost_idx: usize, dir: u8, frightened: bool, eaten: bool) {
    let (_px, _py) = cell_px(gx, gy);
    let cs = CHAR_SIZE;
    let half = cs / 2;

    // 20x20 pixel buffer = 1600 bytes
    let mut buf = [0u8; 20 * 20 * 4];
    // Fill with path color
    for i in 0..(cs * cs) {
        buf[i * 4] = PATH_COLOR[0];
        buf[i * 4 + 1] = PATH_COLOR[1];
        buf[i * 4 + 2] = PATH_COLOR[2];
        buf[i * 4 + 3] = PATH_COLOR[3];
    }

    if !eaten {
        let body_color = if frightened {
            FRIGHT_BODY
        } else {
            GHOST_COLORS[ghost_idx]
        };

        // Draw rounded-top body shape
        for row in 0..cs {
            for col in 0..cs {
                let draw = if row < half {
                    // Top half: circular dome
                    let dy = row as i32 - half as i32;
                    let dx = col as i32 - half as i32;
                    let r = half as i32;
                    dx * dx + dy * dy <= r * r
                } else if row >= cs - 3 {
                    // Bottom: wavy skirt
                    let wave = ((col / 4) + 1) % 2 == 0;
                    if row == cs - 1 {
                        wave
                    } else if row == cs - 2 {
                        true
                    } else {
                        true
                    }
                } else {
                    // Middle: full width rectangular body
                    col >= 1 && col < cs - 1
                };

                if draw {
                    let idx = (row * cs + col) * 4;
                    buf[idx] = body_color[0];
                    buf[idx + 1] = body_color[1];
                    buf[idx + 2] = body_color[2];
                    buf[idx + 3] = body_color[3];
                }
            }
        }
    }

    // Draw eyes (always visible, even when eaten -- eyes-only for eaten ghosts)
    let eye_y = 6usize;
    let eye_left_x = 4usize;
    let eye_right_x = 12usize;
    let eye_w = 5usize;
    let eye_h = 5usize;

    let eye_color = if frightened && !eaten {
        FRIGHT_EYE
    } else {
        GHOST_EYE_WHITE
    };

    // Draw eye whites
    for ey in 0..eye_h {
        for ex in 0..eye_w {
            let li = ((eye_y + ey) * cs + eye_left_x + ex) * 4;
            let ri = ((eye_y + ey) * cs + eye_right_x + ex) * 4;
            buf[li] = eye_color[0];
            buf[li + 1] = eye_color[1];
            buf[li + 2] = eye_color[2];
            buf[li + 3] = eye_color[3];
            buf[ri] = eye_color[0];
            buf[ri + 1] = eye_color[1];
            buf[ri + 2] = eye_color[2];
            buf[ri + 3] = eye_color[3];
        }
    }

    // Draw pupils (directional) -- not drawn if frightened (unless eaten)
    if !frightened || eaten {
        let (pdx, pdy): (i32, i32) = match dir {
            DIR_UP => (0, -1),
            DIR_DOWN => (0, 1),
            DIR_LEFT => (-1, 0),
            DIR_RIGHT => (1, 0),
            _ => (0, 0),
        };
        let pupil_cx = 2i32 + pdx;
        let pupil_cy = 2i32 + pdy;
        for py2 in 0..3i32 {
            for px2 in 0..3i32 {
                let ry = pupil_cy - 1 + py2;
                let rx = pupil_cx - 1 + px2;
                if ry >= 0 && ry < eye_h as i32 && rx >= 0 && rx < eye_w as i32 {
                    let li = ((eye_y + ry as usize) * cs + eye_left_x + rx as usize) * 4;
                    let ri = ((eye_y + ry as usize) * cs + eye_right_x + rx as usize) * 4;
                    buf[li] = GHOST_EYE_PUPIL[0];
                    buf[li + 1] = GHOST_EYE_PUPIL[1];
                    buf[li + 2] = GHOST_EYE_PUPIL[2];
                    buf[li + 3] = GHOST_EYE_PUPIL[3];
                    buf[ri] = GHOST_EYE_PUPIL[0];
                    buf[ri + 1] = GHOST_EYE_PUPIL[1];
                    buf[ri + 2] = GHOST_EYE_PUPIL[2];
                    buf[ri + 3] = GHOST_EYE_PUPIL[3];
                }
            }
        }
    }

    // Frightened mouth (wavy white line)
    if frightened && !eaten {
        let mouth_y = 14usize;
        for mx in 2..18 {
            let my_off: usize = if (mx / 2) % 2 == 0 { 0 } else { 1 };
            let idx = ((mouth_y + my_off) * cs + mx) * 4;
            buf[idx] = FRIGHT_EYE[0];
            buf[idx + 1] = FRIGHT_EYE[1];
            buf[idx + 2] = FRIGHT_EYE[2];
            buf[idx + 3] = FRIGHT_EYE[3];
        }
    }

    let cpx = MAZE_PX_X + gx as usize * CELL_SIZE + CHAR_PAD;
    let cpy = MAZE_PX_Y + gy as usize * CELL_SIZE + CHAR_PAD;
    fb_write(cpx as u32, cpy as u32, cs as u32, cs as u32, buf.as_ptr());
}

// ============================================================
// HUD rendering
// ============================================================

fn draw_hud_static() {
    // Labels
    draw_string(HUD_X, HUD_Y, b"SCORE", TEXT_LABEL);
    draw_string(HUD_X, HUD_Y + 60, b"HIGH", TEXT_LABEL);
    draw_string(HUD_X, HUD_Y + 120, b"LIVES", TEXT_LABEL);
    draw_string(HUD_X, HUD_Y + 180, b"LEVEL", TEXT_LABEL);

    // Controls
    let cy = HUD_Y + 260;
    draw_string(HUD_X, cy, b"CONTROLS", TEXT_DIM);
    let dy: usize = 22;
    draw_string(HUD_X, cy + dy, b"WASD  MOVE", TEXT_DIM);
    draw_string(HUD_X, cy + dy * 2, b"SPC   PAUSE", TEXT_DIM);
    draw_string(HUD_X, cy + dy * 3, b"Q     QUIT", TEXT_DIM);
}

fn draw_hud_values(score: u32, high_score: u32, lives: u32, level: u32) {
    // Clear value areas
    fill_rect(HUD_X, HUD_Y + 25, 120, 18, BG_VOID);
    fill_rect(HUD_X, HUD_Y + 85, 120, 18, BG_VOID);
    fill_rect(HUD_X, HUD_Y + 145, 120, 18, BG_VOID);
    fill_rect(HUD_X, HUD_Y + 205, 120, 18, BG_VOID);

    draw_number(HUD_X, HUD_Y + 25, score, TEXT_COLOR);
    draw_number(HUD_X, HUD_Y + 85, high_score, TEXT_COLOR);
    draw_number(HUD_X, HUD_Y + 145, lives, TEXT_COLOR);
    draw_number(HUD_X, HUD_Y + 205, level, TEXT_COLOR);
}

// ============================================================
// High score persistence
// ============================================================

fn load_high_score() -> u32 {
    let fd = open("pacman_hi\0", OpenFlags::RDONLY);
    if fd < 0 {
        return 0;
    }
    let buf = [0u8; 4];
    let n = read(fd as usize, &buf);
    close(fd as usize);
    if n == 4 {
        u32::from_le_bytes(buf)
    } else {
        0
    }
}

fn save_high_score(score: u32) {
    let fd = open(
        "pacman_hi\0",
        OpenFlags::CREATE | OpenFlags::WRONLY | OpenFlags::TRUNC,
    );
    if fd >= 0 {
        let buf = score.to_le_bytes();
        write(fd as usize, &buf);
        close(fd as usize);
    }
}

// ============================================================
// Ghost AI (child process)
// ============================================================

// Statics for signal handlers in child processes
static mut FRIGHTENED_TICKS_LEFT: u32 = 0;
static mut RESPAWNING_FLAG: bool = false;

fn handle_power_pellet() {
    unsafe {
        FRIGHTENED_TICKS_LEFT = FRIGHT_TICKS;
    }
    sigreturn();
}

fn handle_eaten() {
    unsafe {
        RESPAWNING_FLAG = true;
    }
    sigreturn();
}

/// Pick the best direction toward a target, avoiding reverse and walls.
fn pick_best_direction(
    maze: &[u8],
    gx: u8,
    gy: u8,
    current_dir: u8,
    target_x: u8,
    target_y: u8,
) -> u8 {
    let rev = opposite_dir(current_dir);
    let mut best_dir = current_dir;
    let mut best_dist = u32::MAX;
    // Priority order: UP, LEFT, DOWN, RIGHT (classic Pac-Man tie-break)
    let order = [DIR_UP, DIR_LEFT, DIR_DOWN, DIR_RIGHT];

    for &d in order.iter() {
        if d == rev && current_dir != DIR_NONE {
            continue;
        }
        let nx = gx as i32 + DX[d as usize] as i32;
        let ny = gy as i32 + DY[d as usize] as i32;
        if is_walkable_for_ghost(maze, nx, ny) {
            let wx = tunnel_wrap_x(nx);
            let wy = if ny < 0 {
                0u8
            } else if ny >= MAZE_H as i32 {
                (MAZE_H - 1) as u8
            } else {
                ny as u8
            };
            let dist = manhattan(wx, wy, target_x, target_y);
            if dist < best_dist {
                best_dist = dist;
                best_dir = d;
            }
        }
    }
    best_dir
}

fn pick_random_direction(maze: &[u8], gx: u8, gy: u8, current_dir: u8) -> u8 {
    let rev = opposite_dir(current_dir);
    let mut dirs = [DIR_NONE; 4];
    let mut count = 0u8;

    for d in 0..4u8 {
        if d == rev && current_dir != DIR_NONE {
            continue;
        }
        let nx = gx as i32 + DX[d as usize] as i32;
        let ny = gy as i32 + DY[d as usize] as i32;
        if is_walkable_for_ghost(maze, nx, ny) {
            dirs[count as usize] = d;
            count += 1;
        }
    }

    if count == 0 {
        // Fallback: try reverse
        let nx = gx as i32 + DX[rev as usize] as i32;
        let ny = gy as i32 + DY[rev as usize] as i32;
        if is_walkable_for_ghost(maze, nx, ny) {
            return rev;
        }
        return current_dir;
    }
    dirs[(xorshift32() % count as u32) as usize]
}

/// Ghost child process main loop.
/// ghost_id: 0=Blinky, 1=Pinky, 2=Inky, 3=Clyde
/// read_fd: pipe to read state from parent
/// write_fd: pipe to write direction back to parent
fn ghost_main(ghost_id: usize, read_fd: usize, write_fd: usize) -> ! {
    // Seed RNG differently per ghost
    seed_rng((get_time() as u32).wrapping_add(ghost_id as u32 * 7919));

    // Register signal handlers
    let mut power_action = SignalAction::default();
    power_action.handler = handle_power_pellet as *const () as usize;
    let old = SignalAction::default();
    sigaction(SignalNo::SIGUSR1, &power_action, &old);

    let mut eaten_action = SignalAction::default();
    eaten_action.handler = handle_eaten as *const () as usize;
    sigaction(SignalNo::SIGUSR2, &eaten_action, &old);

    let mut current_dir: u8 = DIR_UP;
    let mut mode_tick: u32 = 0;
    let mut scatter_mode = true; // Start in scatter

    loop {
        // Read 7 bytes: pac_x, pac_y, pac_dir, ghost_x, ghost_y, blinky_x, blinky_y
        let mut state = [0u8; PIPE_TO_GHOST];
        let n = pipe_read(read_fd, &mut state);
        if n <= 0 {
            // Parent closed pipe -- exit
            exit(0);
            unreachable!();
        }

        let pac_x = state[0];
        let pac_y = state[1];
        let pac_dir = state[2];
        let gx = state[3];
        let gy = state[4];
        let blinky_x = state[5];
        let blinky_y = state[6];

        // Check respawning flag
        let respawning = unsafe { RESPAWNING_FLAG };
        if respawning {
            // Move toward ghost house
            let dir =
                pick_best_direction(&INITIAL_MAZE, gx, gy, current_dir, GHOST_HOUSE_X, GHOST_HOUSE_Y);
            current_dir = dir;
            // Check if arrived at ghost house
            if gx == GHOST_HOUSE_X && gy == GHOST_HOUSE_Y {
                unsafe {
                    RESPAWNING_FLAG = false;
                }
            }
            let out = [dir; 1];
            pipe_write(write_fd, &out);
            continue;
        }

        // Check frightened mode
        let frightened_ticks = unsafe { FRIGHTENED_TICKS_LEFT };
        if frightened_ticks > 0 {
            unsafe {
                FRIGHTENED_TICKS_LEFT = frightened_ticks - 1;
            }
            // Random movement when frightened
            let dir = pick_random_direction(&INITIAL_MAZE, gx, gy, current_dir);
            current_dir = dir;
            let out = [dir; 1];
            pipe_write(write_fd, &out);
            continue;
        }

        // Normal mode: scatter/chase alternation
        mode_tick += 1;
        if scatter_mode && mode_tick >= SCATTER_TICKS {
            scatter_mode = false;
            mode_tick = 0;
        } else if !scatter_mode && mode_tick >= CHASE_TICKS {
            scatter_mode = true;
            mode_tick = 0;
        }

        let (target_x, target_y) = if scatter_mode {
            (
                SCATTER_TARGETS[ghost_id][1],
                SCATTER_TARGETS[ghost_id][0],
            )
        } else {
            // Chase mode: each ghost has unique targeting
            match ghost_id {
                BLINKY => {
                    // Directly chase Pac-Man
                    (pac_x, pac_y)
                }
                PINKY => {
                    // Target 4 cells ahead of Pac-Man
                    let tx = pac_x as i32 + DX[pac_dir as usize % 4] as i32 * 4;
                    let ty = pac_y as i32 + DY[pac_dir as usize % 4] as i32 * 4;
                    (
                        tx.clamp(0, MAZE_W as i32 - 1) as u8,
                        ty.clamp(0, MAZE_H as i32 - 1) as u8,
                    )
                }
                INKY => {
                    // Vector from Blinky to 2-ahead-of-pac, doubled
                    let ahead_x = pac_x as i32 + DX[pac_dir as usize % 4] as i32 * 2;
                    let ahead_y = pac_y as i32 + DY[pac_dir as usize % 4] as i32 * 2;
                    let tx = ahead_x * 2 - blinky_x as i32;
                    let ty = ahead_y * 2 - blinky_y as i32;
                    (
                        tx.clamp(0, MAZE_W as i32 - 1) as u8,
                        ty.clamp(0, MAZE_H as i32 - 1) as u8,
                    )
                }
                CLYDE => {
                    // Chase when far (>8), scatter when close
                    let dist = manhattan(gx, gy, pac_x, pac_y);
                    if dist > 8 {
                        (pac_x, pac_y)
                    } else {
                        (
                            SCATTER_TARGETS[CLYDE][1],
                            SCATTER_TARGETS[CLYDE][0],
                        )
                    }
                }
                _ => (pac_x, pac_y),
            }
        };

        let dir = pick_best_direction(&INITIAL_MAZE, gx, gy, current_dir, target_x, target_y);
        current_dir = dir;
        let out = [dir; 1];
        pipe_write(write_fd, &out);
    }
}

// ============================================================
// Game state
// ============================================================

#[derive(Clone, Copy, PartialEq)]
enum GameState {
    Ready,
    Playing,
    Dying,
    LevelComplete,
    GameOver,
    Paused,
}

struct GhostInfo {
    x: u8,
    y: u8,
    dir: u8,
    pid: isize,
    write_fd: usize, // parent writes state to ghost
    read_fd: usize,  // parent reads direction from ghost
    frightened_ticks: u32,
    eaten: bool,
    prev_x: u8,
    prev_y: u8,
}

struct Game {
    maze: [u8; MAZE_W * MAZE_H],
    pac_x: u8,
    pac_y: u8,
    pac_dir: u8,
    pac_next_dir: u8,
    ghosts: [GhostInfo; 4],
    state: GameState,
    state_timer: u32,
    score: u32,
    high_score: u32,
    lives: u32,
    level: u32,
    dots_eaten: u32,
    total_dots: u32,
    mouth_open: bool,
    mouth_timer: u32,
    ghosts_eaten_combo: u32,
    prev_pac_x: u8,
    prev_pac_y: u8,
    pellet_blink: bool,
    pellet_blink_timer: u32,
    prev_score: u32,
    prev_lives: u32,
    prev_level: u32,
    prev_high_score: u32,
}

impl Game {
    fn new() -> Self {
        Game {
            maze: INITIAL_MAZE,
            pac_x: PAC_START_X,
            pac_y: PAC_START_Y,
            pac_dir: DIR_LEFT,
            pac_next_dir: DIR_LEFT,
            ghosts: [
                GhostInfo {
                    x: GHOST_START[0][1],
                    y: GHOST_START[0][0],
                    dir: DIR_LEFT,
                    pid: 0,
                    write_fd: 0,
                    read_fd: 0,
                    frightened_ticks: 0,
                    eaten: false,
                    prev_x: GHOST_START[0][1],
                    prev_y: GHOST_START[0][0],
                },
                GhostInfo {
                    x: GHOST_START[1][1],
                    y: GHOST_START[1][0],
                    dir: DIR_UP,
                    pid: 0,
                    write_fd: 0,
                    read_fd: 0,
                    frightened_ticks: 0,
                    eaten: false,
                    prev_x: GHOST_START[1][1],
                    prev_y: GHOST_START[1][0],
                },
                GhostInfo {
                    x: GHOST_START[2][1],
                    y: GHOST_START[2][0],
                    dir: DIR_UP,
                    pid: 0,
                    write_fd: 0,
                    read_fd: 0,
                    frightened_ticks: 0,
                    eaten: false,
                    prev_x: GHOST_START[2][1],
                    prev_y: GHOST_START[2][0],
                },
                GhostInfo {
                    x: GHOST_START[3][1],
                    y: GHOST_START[3][0],
                    dir: DIR_UP,
                    pid: 0,
                    write_fd: 0,
                    read_fd: 0,
                    frightened_ticks: 0,
                    eaten: false,
                    prev_x: GHOST_START[3][1],
                    prev_y: GHOST_START[3][0],
                },
            ],
            state: GameState::Ready,
            state_timer: 0,
            score: 0,
            high_score: 0,
            lives: INIT_LIVES,
            level: 1,
            dots_eaten: 0,
            total_dots: total_dots(),
            mouth_open: true,
            mouth_timer: 0,
            ghosts_eaten_combo: 0,
            prev_pac_x: PAC_START_X,
            prev_pac_y: PAC_START_Y,
            pellet_blink: true,
            pellet_blink_timer: 0,
            prev_score: u32::MAX,
            prev_lives: u32::MAX,
            prev_level: u32::MAX,
            prev_high_score: u32::MAX,
        }
    }

    fn reset_positions(&mut self) {
        self.pac_x = PAC_START_X;
        self.pac_y = PAC_START_Y;
        self.pac_dir = DIR_LEFT;
        self.pac_next_dir = DIR_LEFT;
        self.prev_pac_x = PAC_START_X;
        self.prev_pac_y = PAC_START_Y;

        for i in 0..4 {
            self.ghosts[i].x = GHOST_START[i][1];
            self.ghosts[i].y = GHOST_START[i][0];
            self.ghosts[i].dir = if i == 0 { DIR_LEFT } else { DIR_UP };
            self.ghosts[i].frightened_ticks = 0;
            self.ghosts[i].eaten = false;
            self.ghosts[i].prev_x = GHOST_START[i][1];
            self.ghosts[i].prev_y = GHOST_START[i][0];
        }

        self.ghosts_eaten_combo = 0;
    }

    fn reset_level(&mut self) {
        self.maze = INITIAL_MAZE;
        self.dots_eaten = 0;
        self.total_dots = total_dots();
        self.reset_positions();
    }
}

// ============================================================
// Ghost process management
// ============================================================

fn spawn_ghosts(game: &mut Game) {
    for i in 0..4 {
        // Create pipes: parent_to_ghost and ghost_to_parent
        let mut p2g = [0usize; 2]; // [read_end, write_end]
        let mut g2p = [0usize; 2];
        pipe(&mut p2g);
        pipe(&mut g2p);

        let child_pid = fork();
        if child_pid == 0 {
            // Child process
            // Close unused ends
            close(p2g[1]); // close write end of parent-to-ghost
            close(g2p[0]); // close read end of ghost-to-parent

            ghost_main(i, p2g[0], g2p[1]);
            // ghost_main never returns
        } else {
            // Parent process
            close(p2g[0]); // close read end of parent-to-ghost
            close(g2p[1]); // close write end of ghost-to-parent

            game.ghosts[i].pid = child_pid;
            game.ghosts[i].write_fd = p2g[1]; // parent writes state here
            game.ghosts[i].read_fd = g2p[0]; // parent reads direction here
        }
    }
}

fn kill_ghosts(game: &mut Game) {
    for i in 0..4 {
        if game.ghosts[i].pid > 0 {
            // Close pipes first -- ghost will get EOF and exit
            close(game.ghosts[i].write_fd);
            close(game.ghosts[i].read_fd);
            // Wait for child to exit
            let mut exit_code: i32 = 0;
            wait(&mut exit_code as *mut i32);
            game.ghosts[i].pid = 0;
        }
    }
}

fn send_ghost_state(game: &Game, ghost_idx: usize) {
    let state = [
        game.pac_x,
        game.pac_y,
        game.pac_dir,
        game.ghosts[ghost_idx].x,
        game.ghosts[ghost_idx].y,
        game.ghosts[BLINKY].x,
        game.ghosts[BLINKY].y,
    ];
    pipe_write(game.ghosts[ghost_idx].write_fd, &state);
}

fn read_ghost_direction(game: &Game, ghost_idx: usize) -> u8 {
    let mut buf = [DIR_UP; 1];
    let n = pipe_read(game.ghosts[ghost_idx].read_fd, &mut buf);
    if n > 0 {
        buf[0]
    } else {
        game.ghosts[ghost_idx].dir
    }
}

// ============================================================
// Input handling
// ============================================================

fn poll_keyboard() -> u8 {
    let buf = [0u8; 1];
    let n = read(STDIN, &buf);
    if n > 0 && buf[0] != 0 {
        buf[0]
    } else {
        0
    }
}

fn keycode_to_dir(key: u8) -> u8 {
    match key {
        KEY_W | KEY_UP => DIR_UP,
        KEY_D | KEY_RIGHT => DIR_RIGHT,
        KEY_S | KEY_DOWN => DIR_DOWN,
        KEY_A | KEY_LEFT => DIR_LEFT,
        _ => DIR_NONE,
    }
}

// ============================================================
// Main game entry point
// ============================================================

/// Entry point for the Pac-Man game.
pub fn run_game() {
    crate::println!("Pac-Man starting...");

    // Seed RNG
    seed_rng(get_time() as u32);

    // Get framebuffer info
    let (_fb_w, _fb_h) = fb_info();

    // Load high score
    let high_score = load_high_score();

    // Initialize game state
    let mut game = Game::new();
    game.high_score = high_score;

    // Clear screen
    fill_rect(0, 0, FB_W, FB_H, BG_VOID);

    // Draw initial maze
    draw_maze(&game.maze);

    // Draw HUD
    draw_hud_static();
    draw_hud_values(game.score, game.high_score, game.lives, game.level);

    // Spawn ghost processes
    spawn_ghosts(&mut game);

    // Draw "READY!" text
    let ready_x = MAZE_PX_X + (MAZE_W * CELL_SIZE - 6 * CHAR_PX_W) / 2;
    let ready_y = MAZE_PX_Y + 13 * CELL_SIZE + CELL_SIZE / 2 - 7;
    draw_string(ready_x, ready_y, b"READY!", READY_COLOR);

    // Draw initial characters
    draw_pacman(game.pac_x, game.pac_y, game.pac_dir, true);
    for i in 0..4 {
        draw_ghost(
            game.ghosts[i].x,
            game.ghosts[i].y,
            i,
            game.ghosts[i].dir,
            false,
            false,
        );
    }
    fb_flush();

    game.state = GameState::Ready;
    game.state_timer = 0;

    let mut quit = false;
    let mut prev_state = game.state;
    let mut flash_count: u32 = 0;

    // Main game loop
    loop {
        let frame_start = get_time();

        // --- Input ---
        // Drain all pending key events
        loop {
            let key = poll_keyboard();
            if key == 0 {
                break;
            }
            // Only process key press (not release: bit 7 set means release)
            if key & 0x80 != 0 {
                continue;
            }

            match key {
                KEY_Q | KEY_ESC => {
                    quit = true;
                }
                KEY_SPACE => {
                    if game.state == GameState::Playing {
                        prev_state = GameState::Playing;
                        game.state = GameState::Paused;
                        // Draw PAUSED text
                        let px = MAZE_PX_X + (MAZE_W * CELL_SIZE - 6 * CHAR_PX_W) / 2;
                        let py = MAZE_PX_Y + 10 * CELL_SIZE;
                        draw_string(px, py, b"PAUSED", READY_COLOR);
                        fb_flush();
                    } else if game.state == GameState::Paused {
                        game.state = prev_state;
                        // Erase PAUSED text by redrawing those cells
                        for gx in 8..13 {
                            let cell = game.maze[10 * MAZE_W + gx];
                            let cpx = MAZE_PX_X + gx * CELL_SIZE;
                            let cpy = MAZE_PX_Y + 10 * CELL_SIZE;
                            if cell == CELL_WALL {
                                draw_wall_cell(&game.maze, gx, 10);
                            } else {
                                draw_path_cell(cpx, cpy);
                            }
                        }
                    } else if game.state == GameState::GameOver {
                        // Restart game
                        game = Game::new();
                        game.high_score = load_high_score();
                        game.state = GameState::Ready;
                        game.state_timer = 0;
                        // Respawn ghosts
                        kill_ghosts(&mut game);
                        spawn_ghosts(&mut game);
                        // Redraw everything
                        fill_rect(0, 0, FB_W, FB_H, BG_VOID);
                        draw_maze(&game.maze);
                        draw_hud_static();
                        draw_hud_values(game.score, game.high_score, game.lives, game.level);
                        draw_pacman(game.pac_x, game.pac_y, game.pac_dir, true);
                        for i in 0..4 {
                            draw_ghost(
                                game.ghosts[i].x,
                                game.ghosts[i].y,
                                i,
                                game.ghosts[i].dir,
                                false,
                                false,
                            );
                        }
                        let rx = MAZE_PX_X + (MAZE_W * CELL_SIZE - 6 * CHAR_PX_W) / 2;
                        let ry = MAZE_PX_Y + 13 * CELL_SIZE + CELL_SIZE / 2 - 7;
                        draw_string(rx, ry, b"READY!", READY_COLOR);
                        fb_flush();
                        continue;
                    }
                }
                _ => {
                    let d = keycode_to_dir(key);
                    if d != DIR_NONE {
                        game.pac_next_dir = d;
                    }
                }
            }
        }

        if quit {
            break;
        }

        // --- State machine ---
        match game.state {
            GameState::Ready => {
                game.state_timer += 1;
                if game.state_timer >= READY_TICKS {
                    game.state = GameState::Playing;
                    // Erase READY! text
                    for gx in 7..14 {
                        let cpx = MAZE_PX_X + gx * CELL_SIZE;
                        let cpy = MAZE_PX_Y + 13 * CELL_SIZE;
                        draw_path_cell(cpx, cpy);
                        // Redraw dot if present
                        let cell = game.maze[13 * MAZE_W + gx];
                        if cell == CELL_DOT {
                            draw_dot(gx, 13);
                        }
                    }
                }
            }
            GameState::Playing => {
                // --- Move Pac-Man ---
                // Try next direction first
                let nx = game.pac_x as i32 + DX[game.pac_next_dir as usize] as i32;
                let ny = game.pac_y as i32 + DY[game.pac_next_dir as usize] as i32;
                if is_walkable(&game.maze, nx, ny) {
                    game.pac_dir = game.pac_next_dir;
                }

                // Move in current direction
                let mx = game.pac_x as i32 + DX[game.pac_dir as usize] as i32;
                let my = game.pac_y as i32 + DY[game.pac_dir as usize] as i32;
                if is_walkable(&game.maze, mx, my) {
                    game.prev_pac_x = game.pac_x;
                    game.prev_pac_y = game.pac_y;
                    game.pac_x = tunnel_wrap_x(mx);
                    game.pac_y = if my < 0 {
                        0
                    } else if my >= MAZE_H as i32 {
                        (MAZE_H - 1) as u8
                    } else {
                        my as u8
                    };
                }

                // --- Eat dots/pellets ---
                let cell_idx = maze_idx(game.pac_x, game.pac_y);
                match game.maze[cell_idx] {
                    CELL_DOT => {
                        game.maze[cell_idx] = CELL_EMPTY;
                        game.score += DOT_SCORE;
                        game.dots_eaten += 1;
                    }
                    CELL_PELLET => {
                        game.maze[cell_idx] = CELL_EMPTY;
                        game.score += PELLET_SCORE;
                        game.dots_eaten += 1;
                        game.ghosts_eaten_combo = 0;

                        // Signal all ghosts: frightened mode
                        for i in 0..4 {
                            game.ghosts[i].frightened_ticks = FRIGHT_TICKS;
                            if game.ghosts[i].pid > 0 && !game.ghosts[i].eaten {
                                kill(game.ghosts[i].pid, SignalNo::SIGUSR1);
                            }
                        }
                    }
                    _ => {}
                }

                // --- Update ghost positions via pipes ---
                for i in 0..4 {
                    send_ghost_state(&game, i);
                }
                for i in 0..4 {
                    let new_dir = read_ghost_direction(&game, i);
                    game.ghosts[i].prev_x = game.ghosts[i].x;
                    game.ghosts[i].prev_y = game.ghosts[i].y;
                    game.ghosts[i].dir = new_dir;

                    let ngx = game.ghosts[i].x as i32 + DX[new_dir as usize] as i32;
                    let ngy = game.ghosts[i].y as i32 + DY[new_dir as usize] as i32;
                    if is_walkable_for_ghost(&INITIAL_MAZE, ngx, ngy) {
                        game.ghosts[i].x = tunnel_wrap_x(ngx);
                        game.ghosts[i].y = if ngy < 0 {
                            0
                        } else if ngy >= MAZE_H as i32 {
                            (MAZE_H - 1) as u8
                        } else {
                            ngy as u8
                        };
                    }
                }

                // Decrement ghost frightened ticks (parent tracking)
                for i in 0..4 {
                    if game.ghosts[i].frightened_ticks > 0 {
                        game.ghosts[i].frightened_ticks -= 1;
                    }
                }

                // --- Collision detection ---
                for i in 0..4 {
                    if game.ghosts[i].x == game.pac_x && game.ghosts[i].y == game.pac_y {
                        if game.ghosts[i].eaten {
                            // Already eaten, no interaction
                            continue;
                        }
                        if game.ghosts[i].frightened_ticks > 0 {
                            // Eat the ghost
                            game.ghosts[i].eaten = true;
                            let bonus = if (game.ghosts_eaten_combo as usize) < GHOST_SCORES.len()
                            {
                                GHOST_SCORES[game.ghosts_eaten_combo as usize]
                            } else {
                                1600
                            };
                            game.score += bonus;
                            game.ghosts_eaten_combo += 1;
                            // Signal ghost to respawn
                            if game.ghosts[i].pid > 0 {
                                kill(game.ghosts[i].pid, SignalNo::SIGUSR2);
                            }
                        } else {
                            // Pac-Man dies
                            game.state = GameState::Dying;
                            game.state_timer = 0;
                        }
                    }
                }

                // Check if ghost returned to ghost house after being eaten
                for i in 0..4 {
                    if game.ghosts[i].eaten
                        && game.ghosts[i].x == GHOST_HOUSE_X
                        && game.ghosts[i].y == GHOST_HOUSE_Y
                    {
                        game.ghosts[i].eaten = false;
                        game.ghosts[i].frightened_ticks = 0;
                    }
                }

                // --- Level complete ---
                if game.dots_eaten >= game.total_dots {
                    game.state = GameState::LevelComplete;
                    game.state_timer = 0;
                    flash_count = 0;
                }

                // Update high score
                if game.score > game.high_score {
                    game.high_score = game.score;
                }

                // Mouth animation
                game.mouth_timer += 1;
                if game.mouth_timer >= 2 {
                    game.mouth_open = !game.mouth_open;
                    game.mouth_timer = 0;
                }

                // Pellet blink
                game.pellet_blink_timer += 1;
                if game.pellet_blink_timer >= 3 {
                    game.pellet_blink = !game.pellet_blink;
                    game.pellet_blink_timer = 0;
                }
            }
            GameState::Dying => {
                game.state_timer += 1;
                if game.state_timer >= DYING_TICKS {
                    game.lives = game.lives.saturating_sub(1);
                    if game.lives == 0 {
                        game.state = GameState::GameOver;
                        // Save high score
                        if game.score > load_high_score() {
                            save_high_score(game.score);
                        }
                    } else {
                        game.reset_positions();
                        game.state = GameState::Ready;
                        game.state_timer = 0;

                        // Redraw maze (clear old positions)
                        draw_maze(&game.maze);
                        // Draw READY!
                        let rx = MAZE_PX_X + (MAZE_W * CELL_SIZE - 6 * CHAR_PX_W) / 2;
                        let ry = MAZE_PX_Y + 13 * CELL_SIZE + CELL_SIZE / 2 - 7;
                        draw_string(rx, ry, b"READY!", READY_COLOR);
                        // Draw characters at starting positions
                        draw_pacman(game.pac_x, game.pac_y, game.pac_dir, true);
                        for i in 0..4 {
                            draw_ghost(
                                game.ghosts[i].x,
                                game.ghosts[i].y,
                                i,
                                game.ghosts[i].dir,
                                false,
                                false,
                            );
                        }
                    }
                }
            }
            GameState::LevelComplete => {
                game.state_timer += 1;
                flash_count += 1;

                // Flash maze 3 times over FLASH_TICKS
                if flash_count % 4 < 2 {
                    // Draw walls in white
                    for gy in 0..MAZE_H {
                        for gx in 0..MAZE_W {
                            if game.maze[gy * MAZE_W + gx] == CELL_WALL {
                                let px = MAZE_PX_X + gx * CELL_SIZE;
                                let py = MAZE_PX_Y + gy * CELL_SIZE;
                                fill_rect(px, py, CELL_SIZE, CELL_SIZE, TEXT_COLOR);
                            }
                        }
                    }
                } else {
                    draw_maze(&game.maze);
                }

                if game.state_timer >= FLASH_TICKS {
                    game.level += 1;
                    game.reset_level();
                    game.state = GameState::Ready;
                    game.state_timer = 0;

                    // Redraw everything
                    draw_maze(&game.maze);
                    draw_pacman(game.pac_x, game.pac_y, game.pac_dir, true);
                    for i in 0..4 {
                        draw_ghost(
                            game.ghosts[i].x,
                            game.ghosts[i].y,
                            i,
                            game.ghosts[i].dir,
                            false,
                            false,
                        );
                    }
                    let rx = MAZE_PX_X + (MAZE_W * CELL_SIZE - 6 * CHAR_PX_W) / 2;
                    let ry = MAZE_PX_Y + 13 * CELL_SIZE + CELL_SIZE / 2 - 7;
                    draw_string(rx, ry, b"READY!", READY_COLOR);
                }
            }
            GameState::GameOver => {
                // Draw GAME OVER overlay (once)
                if game.state_timer == 0 {
                    let ow = 12 * CHAR_PX_W;
                    let ox = MAZE_PX_X + (MAZE_W * CELL_SIZE - ow) / 2;
                    let oy = MAZE_PX_Y + 9 * CELL_SIZE;

                    fill_rect(ox - 8, oy - 8, ow + 16, 80, BG_VOID);
                    draw_string(ox, oy, b"GAME OVER", GAMEOVER_COLOR);
                    draw_string(ox, oy + 30, b"SPC RESTART", TEXT_DIM);
                }
                game.state_timer += 1;
            }
            GameState::Paused => {
                // Do nothing, wait for space
            }
        }

        // --- Render ---
        if game.state == GameState::Playing || game.state == GameState::Dying {
            // Erase previous positions
            erase_cell(game.prev_pac_x, game.prev_pac_y);
            // Redraw dot/pellet if still present at previous position
            let prev_idx = maze_idx(game.prev_pac_x, game.prev_pac_y);
            if game.maze[prev_idx] == CELL_DOT {
                draw_dot(game.prev_pac_x as usize, game.prev_pac_y as usize);
            } else if game.maze[prev_idx] == CELL_PELLET {
                draw_pellet(
                    game.prev_pac_x as usize,
                    game.prev_pac_y as usize,
                    game.pellet_blink,
                );
            }

            for i in 0..4 {
                erase_cell(game.ghosts[i].prev_x, game.ghosts[i].prev_y);
                // Redraw dot/pellet at ghost's previous position
                let gidx = maze_idx(game.ghosts[i].prev_x, game.ghosts[i].prev_y);
                if game.maze[gidx] == CELL_DOT {
                    draw_dot(game.ghosts[i].prev_x as usize, game.ghosts[i].prev_y as usize);
                } else if game.maze[gidx] == CELL_PELLET {
                    draw_pellet(
                        game.ghosts[i].prev_x as usize,
                        game.ghosts[i].prev_y as usize,
                        game.pellet_blink,
                    );
                }
            }

            // Draw characters at new positions
            draw_pacman(game.pac_x, game.pac_y, game.pac_dir, game.mouth_open);
            for i in 0..4 {
                draw_ghost(
                    game.ghosts[i].x,
                    game.ghosts[i].y,
                    i,
                    game.ghosts[i].dir,
                    game.ghosts[i].frightened_ticks > 0,
                    game.ghosts[i].eaten,
                );
            }

            // Blink pellets
            for gy in 0..MAZE_H {
                for gx in 0..MAZE_W {
                    if game.maze[gy * MAZE_W + gx] == CELL_PELLET {
                        draw_pellet(gx, gy, game.pellet_blink);
                    }
                }
            }
        }

        // Update HUD only when values change
        if game.score != game.prev_score
            || game.high_score != game.prev_high_score
            || game.lives != game.prev_lives
            || game.level != game.prev_level
        {
            draw_hud_values(game.score, game.high_score, game.lives, game.level);
            game.prev_score = game.score;
            game.prev_high_score = game.high_score;
            game.prev_lives = game.lives;
            game.prev_level = game.level;
        }

        fb_flush();

        // --- Frame pacing ---
        let frame_end = get_time();
        let elapsed = (frame_end - frame_start) as usize;
        if elapsed < FRAME_MS {
            sleep(FRAME_MS - elapsed);
        } else {
            sched_yield();
        }
    }

    // Cleanup
    kill_ghosts(&mut game);

    // Save high score on exit
    if game.score > load_high_score() {
        save_high_score(game.score);
    }

    crate::println!("Pac-Man exiting. Final score: {}", game.score);
}

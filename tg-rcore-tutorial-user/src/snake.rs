//! Snake game module -- pixel-art GUI rendered via VirtIO-GPU framebuffer.
//!
//! Provides `run_game(input_fd)` as the entry point. The `input_fd` parameter
//! selects polling (STDIN=0) or interrupt-buffered (STDIN_BUFFERED=3) input.

extern crate alloc;
use alloc::vec;
use crate::{fb_info, fb_write, read, sched_yield, sleep};

// ========== Constants ==========

const GRID_W: u32 = 20;
const GRID_H: u32 = 20;
const CELL_SIZE: u32 = 32;

const BOARD_X: u32 = 160;
const BOARD_Y: u32 = 80;
const BOARD_PX_W: u32 = GRID_W * CELL_SIZE;
const BOARD_PX_H: u32 = GRID_H * CELL_SIZE;

const PANEL_X: u32 = 840;
const PANEL_Y: u32 = 100;
const SCORE_VALUE_Y: u32 = PANEL_Y + 100;

const TICK_MS: usize = 150;

const CHAR_W: u32 = 5;
const CHAR_H: u32 = 7;
const CHAR_SCALE: u32 = 4;
const CHAR_PX_W: u32 = CHAR_W * CHAR_SCALE;
const CHAR_PX_H: u32 = CHAR_H * CHAR_SCALE;
const CHAR_SPACING: u32 = CHAR_PX_W + 4;

// ========== Colors (BGRA byte order) ==========

// Deep space
const BG_COLOR: [u8; 4] = [0x1A, 0x0D, 0x0D, 0xFF];       // #0D0D1A

// Board checkerboard — two very similar dark tones
const BOARD_A: [u8; 4] = [0x28, 0x14, 0x14, 0xFF];          // #141428
const BOARD_B: [u8; 4] = [0x30, 0x18, 0x18, 0xFF];          // #181830

// Snake
const HEAD_COLOR: [u8; 4] = [0x14, 0xFF, 0x39, 0xFF];       // #39FF14 neon green
const HEAD_BORDER: [u8; 4] = [0x0A, 0x8A, 0x1A, 0xFF];      // #1A8A0A dark green
const HEAD_HIGHLIGHT: [u8; 4] = [0x80, 0xFF, 0x80, 0xFF];    // #80FF80 bright
const BODY_COLOR: [u8; 4] = [0x33, 0xCC, 0x00, 0xFF];       // #00CC33
const BODY_BORDER: [u8; 4] = [0x1A, 0x66, 0x00, 0xFF];      // #00661A
const BODY_HIGHLIGHT: [u8; 4] = [0x66, 0xEE, 0x33, 0xFF];   // #33EE66
const EYE_DARK: [u8; 4] = [0x11, 0x08, 0x08, 0xFF];         // #080811
const EYE_BRIGHT: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];       // #FFFFFF

// Food — hot pink orb
const FOOD_COLOR: [u8; 4] = [0x55, 0x22, 0xFF, 0xFF];       // #FF2255
const FOOD_BORDER: [u8; 4] = [0x33, 0x11, 0xBB, 0xFF];      // #BB1133
const FOOD_HIGHLIGHT: [u8; 4] = [0xAA, 0x77, 0xFF, 0xFF];   // #FF77AA

// UI
const ACCENT_COLOR: [u8; 4] = [0xFF, 0x44, 0x44, 0xFF];     // #4444FF electric blue
const BORDER_GLOW: [u8; 4] = [0x88, 0x33, 0x33, 0xFF];      // #333388 subtle glow
const BORDER_MAIN: [u8; 4] = [0xDD, 0x55, 0x55, 0xFF];      // #5555DD
const TEXT_COLOR: [u8; 4] = [0xEE, 0xDD, 0xDD, 0xFF];       // #DDDDEE cool white
const TEXT_DIM: [u8; 4] = [0x88, 0x77, 0x77, 0xFF];         // #777788
const OVERLAY_COLOR: [u8; 4] = [0x18, 0x0A, 0x0A, 0xFF];    // #0A0A18

// ========== Bitmap font (5x7 glyphs) ==========

fn glyph(ch: u8) -> Option<[u8; 7]> {
    Some(match ch {
        b'0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        b'1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        b'2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        b'3' => [0x0E, 0x11, 0x01, 0x06, 0x01, 0x11, 0x0E],
        b'4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        b'5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        b'6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        b'7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        b'8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        b'9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        b'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        b'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        b'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        b'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        b'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],
        b'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        b'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        b'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        b'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        b'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        b'S' => [0x0E, 0x11, 0x10, 0x0E, 0x01, 0x11, 0x0E],
        b'V' => [0x11, 0x11, 0x11, 0x11, 0x0A, 0x0A, 0x04],
        b'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        b' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        b':' => [0x00, 0x04, 0x04, 0x00, 0x04, 0x04, 0x00],
        _ => return None,
    })
}

// ========== Direction ==========

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Up,
    Down,
    Left,
    Right,
}

impl Direction {
    fn opposite(self) -> Self {
        match self {
            Direction::Up => Direction::Down,
            Direction::Down => Direction::Up,
            Direction::Left => Direction::Right,
            Direction::Right => Direction::Left,
        }
    }
}

// ========== Input parser ==========

enum InputState {
    Normal,
    EscSeen,
    BracketSeen,
}

struct InputParser {
    state: InputState,
}

impl InputParser {
    fn new() -> Self {
        InputParser { state: InputState::Normal }
    }

    fn feed(&mut self, byte: u8) -> Option<Direction> {
        match self.state {
            InputState::Normal => match byte {
                b'w' | b'W' => Some(Direction::Up),
                b'a' | b'A' => Some(Direction::Left),
                b's' | b'S' => Some(Direction::Down),
                b'd' | b'D' => Some(Direction::Right),
                0x1B => {
                    self.state = InputState::EscSeen;
                    None
                }
                _ => None,
            },
            InputState::EscSeen => {
                self.state = if byte == b'[' {
                    InputState::BracketSeen
                } else {
                    InputState::Normal
                };
                None
            }
            InputState::BracketSeen => {
                self.state = InputState::Normal;
                match byte {
                    b'A' => Some(Direction::Up),
                    b'B' => Some(Direction::Down),
                    b'C' => Some(Direction::Right),
                    b'D' => Some(Direction::Left),
                    _ => None,
                }
            }
        }
    }
}

// ========== Snake ==========

struct Snake {
    body: [(u8, u8); 400],
    head: usize,
    len: usize,
    dir: Direction,
    next_dir: Direction,
}

impl Snake {
    fn new() -> Self {
        let mut s = Snake {
            body: [(0, 0); 400],
            head: 2,
            len: 3,
            dir: Direction::Right,
            next_dir: Direction::Right,
        };
        s.body[0] = (8, 10);
        s.body[1] = (9, 10);
        s.body[2] = (10, 10);
        s
    }

    fn head_pos(&self) -> (u8, u8) {
        self.body[self.head]
    }

    fn tail_pos(&self) -> (u8, u8) {
        let idx = (self.head + 400 - self.len + 1) % 400;
        self.body[idx]
    }

    fn next_head_signed(&self) -> (i8, i8) {
        let (hx, hy) = self.body[self.head];
        match self.dir {
            Direction::Up => (hx as i8, hy as i8 - 1),
            Direction::Down => (hx as i8, hy as i8 + 1),
            Direction::Left => (hx as i8 - 1, hy as i8),
            Direction::Right => (hx as i8 + 1, hy as i8),
        }
    }

    fn occupies(&self, x: u8, y: u8, exclude_tail: bool) -> bool {
        let check_len = if exclude_tail { self.len - 1 } else { self.len };
        for i in 0..check_len {
            let idx = (self.head + 400 - i) % 400;
            if self.body[idx] == (x, y) {
                return true;
            }
        }
        false
    }

    fn advance(&mut self, grow: bool) -> ((u8, u8), Option<(u8, u8)>) {
        let old_tail = if grow { None } else { Some(self.tail_pos()) };
        let (nx, ny) = self.next_head_signed();
        let new_head = (nx as u8, ny as u8);
        self.head = (self.head + 1) % 400;
        self.body[self.head] = new_head;
        if grow {
            self.len += 1;
        }
        (new_head, old_tail)
    }

    fn set_direction(&mut self, dir: Direction) {
        if dir != self.dir.opposite() {
            self.next_dir = dir;
        }
    }
}

// ========== Game state ==========

#[derive(PartialEq)]
enum GameState {
    Playing,
    GameOver,
}

struct TickResult {
    new_head: (u8, u8),
    prev_head: (u8, u8),
    old_tail: Option<(u8, u8)>,
    food_changed: bool,
}

struct Game {
    snake: Snake,
    food: (u8, u8),
    score: u32,
    state: GameState,
    rng: u64,
}

impl Game {
    fn new() -> Self {
        let seed: u64;
        unsafe { core::arch::asm!("rdtime {}", out(reg) seed) };
        let mut game = Game {
            snake: Snake::new(),
            food: (0, 0),
            score: 0,
            state: GameState::Playing,
            rng: if seed == 0 { 1 } else { seed },
        };
        game.spawn_food();
        game
    }

    fn next_rand(&mut self) -> u64 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng = x;
        x
    }

    fn spawn_food(&mut self) {
        loop {
            let x = (self.next_rand() % GRID_W as u64) as u8;
            let y = (self.next_rand() % GRID_H as u64) as u8;
            if !self.snake.occupies(x, y, false) {
                self.food = (x, y);
                return;
            }
        }
    }

    fn update(&mut self) -> Option<TickResult> {
        if self.state != GameState::Playing {
            return None;
        }

        self.snake.dir = self.snake.next_dir;

        let (nx, ny) = self.snake.next_head_signed();
        if nx < 0 || nx >= GRID_W as i8 || ny < 0 || ny >= GRID_H as i8 {
            self.state = GameState::GameOver;
            return None;
        }

        let new_pos = (nx as u8, ny as u8);
        let will_grow = new_pos == self.food;

        if self.snake.occupies(new_pos.0, new_pos.1, !will_grow) {
            self.state = GameState::GameOver;
            return None;
        }

        let prev_head = self.snake.head_pos();
        let (new_head, old_tail) = self.snake.advance(will_grow);

        let food_changed = if will_grow {
            self.score += 1;
            self.spawn_food();
            true
        } else {
            false
        };

        Some(TickResult { new_head, prev_head, old_tail, food_changed })
    }
}

// ========== Rendering ==========

const RENDER_BUF_SIZE: usize = (CELL_SIZE * CELL_SIZE * 4) as usize;
static mut RENDER_BUF: [u8; RENDER_BUF_SIZE] = [0; RENDER_BUF_SIZE];

fn draw_rect(x: u32, y: u32, w: u32, h: u32, color: [u8; 4]) {
    let pixels = w as usize * h as usize;
    let mut buf = vec![0u8; pixels * 4];
    for i in 0..pixels {
        let off = i * 4;
        buf[off] = color[0];
        buf[off + 1] = color[1];
        buf[off + 2] = color[2];
        buf[off + 3] = color[3];
    }
    fb_write(x, y, w, h, buf.as_ptr());
}

fn draw_board_cell(gx: u32, gy: u32) {
    let buf = unsafe { &mut *(&raw mut RENDER_BUF) };
    let color = if (gx + gy) % 2 == 0 { &BOARD_A } else { &BOARD_B };
    let cs = CELL_SIZE as usize;
    for py in 0..cs {
        for px in 0..cs {
            let off = (py * cs + px) * 4;
            buf[off..off + 4].copy_from_slice(color);
        }
    }
    fb_write(BOARD_X + gx * CELL_SIZE, BOARD_Y + gy * CELL_SIZE, CELL_SIZE, CELL_SIZE, buf.as_ptr());
}

/// Linearly interpolate between two colors by `t` (0..256 range, 0=a, 256=b).
fn lerp_color(a: &[u8; 4], b: &[u8; 4], t: u32) -> [u8; 4] {
    [
        ((a[0] as u32 * (256 - t) + b[0] as u32 * t) / 256) as u8,
        ((a[1] as u32 * (256 - t) + b[1] as u32 * t) / 256) as u8,
        ((a[2] as u32 * (256 - t) + b[2] as u32 * t) / 256) as u8,
        0xFF,
    ]
}

fn draw_snake_head(pos: (u8, u8), dir: Direction) {
    let buf = unsafe { &mut *(&raw mut RENDER_BUF) };
    let cs = CELL_SIZE as usize;

    // Eye positions based on direction (top-left of 3x3 eye patch)
    let (eye1, eye2) = match dir {
        Direction::Right => ((23usize, 9usize), (23usize, 20usize)),
        Direction::Left  => ((6usize, 9usize),  (6usize, 20usize)),
        Direction::Up    => ((9usize, 6usize),  (20usize, 6usize)),
        Direction::Down  => ((9usize, 23usize), (20usize, 23usize)),
    };

    for py in 0..cs {
        for px in 0..cs {
            let off = (py * cs + px) * 4;
            let edge_dist = px.min(31 - px).min(py).min(31 - py);

            // Check if pixel is part of an eye
            let in_eye1 = px >= eye1.0 && px < eye1.0 + 3 && py >= eye1.1 && py < eye1.1 + 3;
            let in_eye2 = px >= eye2.0 && px < eye2.0 + 3 && py >= eye2.1 && py < eye2.1 + 3;
            let is_pupil1 = px == eye1.0 + 1 && py == eye1.1 + 1;
            let is_pupil2 = px == eye2.0 + 1 && py == eye2.1 + 1;

            let c = if is_pupil1 || is_pupil2 {
                EYE_BRIGHT
            } else if in_eye1 || in_eye2 {
                EYE_DARK
            } else if px >= 10 && px < 12 && py >= 7 && py < 9 {
                // 2x2 specular highlight
                HEAD_HIGHLIGHT
            } else if edge_dist <= 1 {
                HEAD_BORDER
            } else if edge_dist <= 3 {
                // Lerp from border to main: edge_dist 2 -> t=128, edge_dist 3 -> t=256
                let t = (edge_dist - 1) * 128;
                lerp_color(&HEAD_BORDER, &HEAD_COLOR, t as u32)
            } else {
                HEAD_COLOR
            };

            buf[off..off + 4].copy_from_slice(&c);
        }
    }
    fb_write(
        BOARD_X + pos.0 as u32 * CELL_SIZE,
        BOARD_Y + pos.1 as u32 * CELL_SIZE,
        CELL_SIZE, CELL_SIZE, buf.as_ptr(),
    );
}

fn draw_snake_body(pos: (u8, u8)) {
    let buf = unsafe { &mut *(&raw mut RENDER_BUF) };
    let cs = CELL_SIZE as usize;
    for py in 0..cs {
        for px in 0..cs {
            let off = (py * cs + px) * 4;
            let edge_dist = px.min(31 - px).min(py).min(31 - py);

            let c = if px >= 10 && px < 12 && py >= 7 && py < 9 {
                // 2x2 specular highlight
                BODY_HIGHLIGHT
            } else if edge_dist <= 1 {
                BODY_BORDER
            } else if edge_dist <= 3 {
                let t = (edge_dist - 1) * 128;
                lerp_color(&BODY_BORDER, &BODY_COLOR, t as u32)
            } else {
                BODY_COLOR
            };

            buf[off..off + 4].copy_from_slice(&c);
        }
    }
    fb_write(
        BOARD_X + pos.0 as u32 * CELL_SIZE,
        BOARD_Y + pos.1 as u32 * CELL_SIZE,
        CELL_SIZE, CELL_SIZE, buf.as_ptr(),
    );
}

fn draw_food_cell(gx: u32, gy: u32) {
    let buf = unsafe { &mut *(&raw mut RENDER_BUF) };
    let board_bg = if (gx + gy) % 2 == 0 { &BOARD_A } else { &BOARD_B };
    let cs = CELL_SIZE as usize;
    for py in 0..cs {
        for px in 0..cs {
            let off = (py * cs + px) * 4;
            let cx = px as i32 - 15;
            let cy = py as i32 - 15;
            let dist_sq = (cx * cx + cy * cy) as u32;

            let c: &[u8; 4] = if dist_sq > 169 {
                board_bg
            } else if dist_sq > 144 {
                &FOOD_BORDER
            } else if dist_sq > 25 {
                &FOOD_COLOR
            } else {
                &FOOD_HIGHLIGHT
            };

            buf[off..off + 4].copy_from_slice(c);
        }
    }
    fb_write(BOARD_X + gx * CELL_SIZE, BOARD_Y + gy * CELL_SIZE, CELL_SIZE, CELL_SIZE, buf.as_ptr());
}

fn draw_char(x: u32, y: u32, g: &[u8; 7], fg: [u8; 4], bg: [u8; 4]) {
    let buf = unsafe { &mut *(&raw mut RENDER_BUF) };
    let pw = CHAR_PX_W as usize;
    let ph = CHAR_PX_H as usize;
    for py in 0..ph {
        for px in 0..pw {
            let off = (py * pw + px) * 4;
            let gy = py / CHAR_SCALE as usize;
            let gx = px / CHAR_SCALE as usize;
            let bit = (g[gy] >> (CHAR_W as usize - 1 - gx)) & 1;
            let c = if bit == 1 { &fg } else { &bg };
            buf[off] = c[0];
            buf[off + 1] = c[1];
            buf[off + 2] = c[2];
            buf[off + 3] = c[3];
        }
    }
    fb_write(x, y, CHAR_PX_W, CHAR_PX_H, buf.as_ptr());
}

fn draw_text(x: u32, y: u32, text: &[u8], fg: [u8; 4], bg: [u8; 4]) {
    let mut cx = x;
    for &ch in text {
        if let Some(g) = glyph(ch) {
            draw_char(cx, y, &g, fg, bg);
            cx += CHAR_SPACING;
        }
    }
}

fn draw_number(x: u32, y: u32, n: u32, fg: [u8; 4], bg: [u8; 4]) {
    if n == 0 {
        if let Some(g) = glyph(b'0') {
            draw_char(x, y, &g, fg, bg);
        }
        return;
    }
    let mut digits = [0u8; 10];
    let mut count = 0;
    let mut val = n;
    while val > 0 {
        digits[count] = (val % 10) as u8;
        val /= 10;
        count += 1;
    }
    let mut cx = x;
    for i in (0..count).rev() {
        if let Some(g) = glyph(b'0' + digits[i]) {
            draw_char(cx, y, &g, fg, bg);
            cx += CHAR_SPACING;
        }
    }
}

fn count_digits(n: u32) -> u32 {
    if n == 0 { return 1; }
    let mut count = 0u32;
    let mut val = n;
    while val > 0 { val /= 10; count += 1; }
    count
}

fn draw_score(score: u32) {
    draw_rect(PANEL_X, SCORE_VALUE_Y, 6 * CHAR_SPACING, CHAR_PX_H, BG_COLOR);
    draw_number(PANEL_X, SCORE_VALUE_Y, score, TEXT_COLOR, BG_COLOR);
}

/// Font glyph data for lowercase letters used in hint text.
fn glyph_lower(ch: u8) -> Option<[u8; 7]> {
    Some(match ch {
        b'a' => [0x00, 0x00, 0x0E, 0x01, 0x0F, 0x11, 0x0F],
        b'e' => [0x00, 0x00, 0x0E, 0x11, 0x1F, 0x10, 0x0E],
        b'k' => [0x10, 0x10, 0x12, 0x14, 0x18, 0x14, 0x12],
        b'n' => [0x00, 0x00, 0x16, 0x19, 0x11, 0x11, 0x11],
        b'o' => [0x00, 0x00, 0x0E, 0x11, 0x11, 0x11, 0x0E],
        b'r' => [0x00, 0x00, 0x16, 0x19, 0x10, 0x10, 0x10],
        b's' => [0x00, 0x00, 0x0E, 0x10, 0x0E, 0x01, 0x1E],
        b't' => [0x04, 0x04, 0x0E, 0x04, 0x04, 0x04, 0x02],
        b'y' => [0x00, 0x00, 0x11, 0x11, 0x0F, 0x01, 0x0E],
        _ => return glyph(ch),
    })
}

/// Draw text with mixed case support (uses glyph_lower for lowercase).
fn draw_text_mixed(x: u32, y: u32, text: &[u8], fg: [u8; 4], bg: [u8; 4]) {
    let mut cx = x;
    for &ch in text {
        let g = if ch >= b'a' && ch <= b'z' {
            glyph_lower(ch)
        } else {
            glyph(ch)
        };
        if let Some(g) = g {
            draw_char(cx, y, &g, fg, bg);
            cx += CHAR_SPACING;
        }
    }
}

fn draw_initial_screen(game: &Game) {
    // Background is filled by kernel (BG_COLOR). Only draw game elements.

    // Draw border — 3px: 1px BORDER_GLOW outer, 1px gap (BG_COLOR), 1px BORDER_MAIN inner
    // Outer glow (1px)
    draw_rect(BOARD_X - 3, BOARD_Y - 3, BOARD_PX_W + 6, 1, BORDER_GLOW);
    draw_rect(BOARD_X - 3, BOARD_Y + BOARD_PX_H + 2, BOARD_PX_W + 6, 1, BORDER_GLOW);
    draw_rect(BOARD_X - 3, BOARD_Y - 2, 1, BOARD_PX_H + 4, BORDER_GLOW);
    draw_rect(BOARD_X + BOARD_PX_W + 2, BOARD_Y - 2, 1, BOARD_PX_H + 4, BORDER_GLOW);
    // Gap (1px) — use BG_COLOR
    draw_rect(BOARD_X - 2, BOARD_Y - 2, BOARD_PX_W + 4, 1, BG_COLOR);
    draw_rect(BOARD_X - 2, BOARD_Y + BOARD_PX_H + 1, BOARD_PX_W + 4, 1, BG_COLOR);
    draw_rect(BOARD_X - 2, BOARD_Y - 1, 1, BOARD_PX_H + 2, BG_COLOR);
    draw_rect(BOARD_X + BOARD_PX_W + 1, BOARD_Y - 1, 1, BOARD_PX_H + 2, BG_COLOR);
    // Inner main (1px)
    draw_rect(BOARD_X - 1, BOARD_Y - 1, BOARD_PX_W + 2, 1, BORDER_MAIN);
    draw_rect(BOARD_X - 1, BOARD_Y + BOARD_PX_H, BOARD_PX_W + 2, 1, BORDER_MAIN);
    draw_rect(BOARD_X - 1, BOARD_Y, 1, BOARD_PX_H, BORDER_MAIN);
    draw_rect(BOARD_X + BOARD_PX_W, BOARD_Y, 1, BOARD_PX_H, BORDER_MAIN);

    // Draw all grid cells (checkerboard)
    for gy in 0..GRID_H {
        for gx in 0..GRID_W {
            draw_board_cell(gx, gy);
        }
    }

    // Draw snake
    for i in 0..game.snake.len {
        let idx = (game.snake.head + 400 - i) % 400;
        let (x, y) = game.snake.body[idx];
        if i == 0 {
            draw_snake_head((x, y), game.snake.dir);
        } else {
            draw_snake_body((x, y));
        }
    }

    // Draw food
    draw_food_cell(game.food.0 as u32, game.food.1 as u32);

    // Draw score panel
    draw_text(PANEL_X, PANEL_Y, b"SNAKE", ACCENT_COLOR, BG_COLOR);
    // Decorative line below title
    draw_rect(PANEL_X, PANEL_Y + CHAR_PX_H + 8, 5 * CHAR_SPACING, 1, BORDER_MAIN);

    draw_text(PANEL_X, PANEL_Y + 60, b"SCORE", TEXT_DIM, BG_COLOR);
    draw_score(game.score);

    // Decorative line below score
    draw_rect(PANEL_X, SCORE_VALUE_Y + CHAR_PX_H + 12, 5 * CHAR_SPACING, 1, BORDER_MAIN);

    // Controls hint
    draw_text(PANEL_X, PANEL_Y + 200, b"WASD", TEXT_DIM, BG_COLOR);
}

fn render_tick(game: &Game, result: &TickResult) {
    draw_snake_head(result.new_head, game.snake.dir);
    draw_snake_body(result.prev_head);
    if let Some((tx, ty)) = result.old_tail {
        draw_board_cell(tx as u32, ty as u32);
    }
    if result.food_changed {
        draw_food_cell(game.food.0 as u32, game.food.1 as u32);
        draw_score(game.score);
    }
}

fn draw_game_over(score: u32) {
    let ow = 440u32;
    let oh = 240u32;
    let ox = BOARD_X + (BOARD_PX_W - ow) / 2;
    let oy = BOARD_Y + (BOARD_PX_H - oh) / 2;

    // Overlay background in strips
    let strip_h = 10u32;
    let mut y = 0u32;
    while y < oh {
        let h = core::cmp::min(strip_h, oh - y);
        draw_rect(ox, oy + y, ow, h, OVERLAY_COLOR);
        y += strip_h;
    }

    // Overlay border (2px ACCENT_COLOR)
    draw_rect(ox, oy, ow, 2, ACCENT_COLOR);
    draw_rect(ox, oy + oh - 2, ow, 2, ACCENT_COLOR);
    draw_rect(ox, oy, 2, oh, ACCENT_COLOR);
    draw_rect(ox + ow - 2, oy, 2, oh, ACCENT_COLOR);

    // "GAME" / "OVER" in ACCENT_COLOR
    let text_x = ox + (ow - 4 * CHAR_SPACING) / 2;
    draw_text(text_x, oy + 30, b"GAME", ACCENT_COLOR, OVERLAY_COLOR);
    draw_text(text_x, oy + 70, b"OVER", ACCENT_COLOR, OVERLAY_COLOR);

    // "SCORE" label in TEXT_DIM
    let label_x = ox + (ow - 5 * CHAR_SPACING) / 2;
    draw_text(label_x, oy + 120, b"SCORE", TEXT_DIM, OVERLAY_COLOR);

    // Score digits in TEXT_COLOR
    let digits = count_digits(score);
    let score_x = ox + (ow - digits * CHAR_SPACING) / 2;
    draw_number(score_x, oy + 158, score, TEXT_COLOR, OVERLAY_COLOR);

    // Hint text: "any key to restart"
    draw_text_mixed(ox + 60, oy + oh - 30, b"any key to restart", TEXT_DIM, OVERLAY_COLOR);
}

// ========== Input helpers ==========

fn drain_input(input_fd: usize, parser: &mut InputParser) -> Option<Direction> {
    let mut last_dir = None;
    let buf = [0u8; 1];
    loop {
        let ret = read(input_fd, &buf);
        if ret <= 0 {
            break;
        }
        if let Some(d) = parser.feed(buf[0]) {
            last_dir = Some(d);
        }
    }
    last_dir
}

fn wait_for_key(input_fd: usize) {
    let buf = [0u8; 1];
    loop {
        if read(input_fd, &buf) > 0 {
            return;
        }
        sched_yield();
    }
}

// ========== Game loop ==========

/// Main entry point. Call with `STDIN` (0) for polling or `STDIN_BUFFERED` (3) for interrupt.
pub fn run_game(input_fd: usize) {
    let (sw, sh) = fb_info();
    assert!(sw >= 1280 && sh >= 800, "screen too small: {}x{}", sw, sh);

    loop {
        let mut game = Game::new();
        let mut parser = InputParser::new();

        draw_initial_screen(&game);

        loop {
            if let Some(d) = drain_input(input_fd, &mut parser) {
                game.snake.set_direction(d);
            }

            match game.update() {
                Some(result) => render_tick(&game, &result),
                None => {
                    if game.state == GameState::GameOver {
                        draw_game_over(game.score);
                        wait_for_key(input_fd);
                        break;
                    }
                }
            }

            sleep(TICK_MS);
        }
    }
}

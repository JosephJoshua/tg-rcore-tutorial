//! Breakout (brick-breaker) game for ch6: VirtIO GPU rendering with file-based save/load.

use crate::{close, fb_flush, fb_info, fb_write, get_time, open, read, sched_yield, sleep, write, OpenFlags, STDIN};

// ========== Layout ==========

const PLAY_X: usize = 180;
const PLAY_Y: usize = 100;
const PLAY_W: usize = 920;
const PLAY_H: usize = 600;

const BRICK_COLS: usize = 10;
const BRICK_ROWS: usize = 5;
const BRICK_W: usize = 90;
const BRICK_H: usize = 25;
const BRICK_GAP: usize = 2;
const BRICK_OFFSET_X: usize = PLAY_X + (PLAY_W - BRICK_COLS * (BRICK_W + BRICK_GAP)) / 2;
const BRICK_OFFSET_Y: usize = PLAY_Y + 40;

const PADDLE_W: usize = 120;
const PADDLE_H: usize = 14;
const PADDLE_Y: usize = PLAY_Y + PLAY_H - 40;
const PADDLE_SPEED: i32 = 8;

const BALL_SIZE: usize = 10;
const FP: i32 = 256;
const INIT_BALL_SPEED: i32 = 3 * FP;

// ========== Colors (BGRA) ==========

const BG: [u8; 4] = [0x12, 0x06, 0x08, 0xFF];       // deep void
const PLAY_BG: [u8; 4] = [0x18, 0x0A, 0x0C, 0xFF];   // slightly lighter void
const PADDLE_COLOR: [u8; 4] = [0xFF, 0xCC, 0x40, 0xFF]; // electric amber
const BALL_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const BALL_GLOW: [u8; 4] = [0x80, 0xD0, 0xFF, 0x60];   // soft cyan halo
const TEXT_VALUE: [u8; 4] = [0xEE, 0xF0, 0xFF, 0xFF];   // cool white
const TEXT_LABEL: [u8; 4] = [0x60, 0x50, 0x80, 0xFF];   // muted lavender
const TEXT_TITLE: [u8; 4] = [0xFF, 0x80, 0x20, 0xFF];   // hot orange (BGRA)
const TEXT_TITLE_GLOW: [u8; 4] = [0x40, 0x20, 0x08, 0xFF]; // dim orange shadow
const TEXT_KEY: [u8; 4] = [0x40, 0xF0, 0xFF, 0xFF];     // neon cyan
const TEXT_DIM: [u8; 4] = [0x40, 0x30, 0x50, 0xFF];     // dusty purple
const GAMEOVER_BG: [u8; 4] = [0x08, 0x02, 0x02, 0xE8];
const GAMEOVER_TEXT: [u8; 4] = [0x30, 0x30, 0xFF, 0xFF]; // neon red
const GLOW_OUTER: [u8; 4] = [0x40, 0x10, 0x08, 0xFF];   // dim orange glow
const GLOW_MID: [u8; 4] = [0x60, 0x20, 0x10, 0xFF];     // mid orange glow
const GLOW_INNER: [u8; 4] = [0xA0, 0x40, 0x18, 0xFF];   // hot orange glow

// Row colors for bricks (top to bottom) — vivid neon BGRA
const BRICK_COLORS: [[u8; 4]; 5] = [
    [0x20, 0x20, 0xF8, 0xFF], // Row 0: hot red
    [0x10, 0x60, 0xF8, 0xFF], // Row 1: deep orange
    [0x00, 0xD0, 0xF0, 0xFF], // Row 2: golden yellow
    [0x20, 0xE8, 0x30, 0xFF], // Row 3: neon green
    [0xF0, 0xC0, 0x10, 0xFF], // Row 4: electric cyan
];

// Points per row (top = most valuable)
const ROW_SCORES: [u32; 5] = [50, 40, 30, 20, 10];

// ========== Font ==========

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const FONT_SCALE: usize = 3;
const CHAR_PX_W: usize = GLYPH_W * FONT_SCALE + 2;

const GLYPHS: [(u8, [u8; 7]); 35] = [
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
];

// ========== SaveData ==========

#[repr(C)]
struct SaveData {
    magic: u32,
    ball_x: i32,
    ball_y: i32,
    ball_vx: i32,
    ball_vy: i32,
    paddle_x: i32,
    score: u32,
    lives: u32,
    level: u32,
    bricks: [u8; 50],
    ball_attached: u8,
    _pad: [u8; 3],
    bricks_destroyed: u32,
}

fn save_game(game: &Game) {
    let data = SaveData {
        magic: 0x42524B4F,
        ball_x: game.ball_x,
        ball_y: game.ball_y,
        ball_vx: game.ball_vx,
        ball_vy: game.ball_vy,
        paddle_x: game.paddle_x,
        score: game.score,
        lives: game.lives,
        level: game.level,
        bricks: game.bricks,
        ball_attached: if game.ball_attached { 1 } else { 0 },
        _pad: [0; 3],
        bricks_destroyed: game.bricks_destroyed,
    };
    let bytes = unsafe {
        core::slice::from_raw_parts(
            &data as *const SaveData as *const u8,
            core::mem::size_of::<SaveData>(),
        )
    };
    let fd = open("breakout_save\0", OpenFlags::CREATE | OpenFlags::WRONLY | OpenFlags::TRUNC);
    if fd >= 0 {
        write(fd as usize, bytes);
        close(fd as usize);
    }
}

fn load_game(game: &mut Game) -> bool {
    let fd = open("breakout_save\0", OpenFlags::RDONLY);
    if fd < 0 {
        return false;
    }
    let mut data = SaveData {
        magic: 0,
        ball_x: 0,
        ball_y: 0,
        ball_vx: 0,
        ball_vy: 0,
        paddle_x: 0,
        score: 0,
        lives: 0,
        level: 0,
        bricks: [0; 50],
        ball_attached: 0,
        _pad: [0; 3],
        bricks_destroyed: 0,
    };
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(
            &mut data as *mut SaveData as *mut u8,
            core::mem::size_of::<SaveData>(),
        )
    };
    let n = read(fd as usize, bytes);
    close(fd as usize);
    if n as usize != core::mem::size_of::<SaveData>() || data.magic != 0x42524B4F {
        return false;
    }
    game.ball_x = data.ball_x;
    game.ball_y = data.ball_y;
    game.ball_vx = data.ball_vx;
    game.ball_vy = data.ball_vy;
    game.paddle_x = data.paddle_x;
    game.score = data.score;
    game.lives = data.lives;
    game.level = data.level;
    game.bricks = data.bricks;
    game.ball_attached = data.ball_attached != 0;
    game.bricks_destroyed = data.bricks_destroyed;
    true
}

// ========== Game State ==========

struct Game {
    ball_x: i32,
    ball_y: i32,
    ball_vx: i32,
    ball_vy: i32,
    paddle_x: i32,
    score: u32,
    lives: u32,
    level: u32,
    bricks: [u8; 50],
    ball_attached: bool,
    bricks_destroyed: u32,
    flash_timer: i32,
    flash_msg: u8, // 0=none, 1=SAVED, 2=LOADED
    prev_score: u32,
    prev_lives: u32,
    prev_level: u32,
}

impl Game {
    fn new() -> Self {
        Self {
            ball_x: 0,
            ball_y: 0,
            ball_vx: 0,
            ball_vy: 0,
            paddle_x: (PLAY_X + PLAY_W / 2 - PADDLE_W / 2) as i32,
            score: 0,
            lives: 3,
            level: 1,
            bricks: [1; 50],
            ball_attached: true,
            bricks_destroyed: 0,
            flash_timer: 0,
            flash_msg: 0,
            prev_score: u32::MAX,
            prev_lives: u32::MAX,
            prev_level: u32::MAX,
        }
    }

    fn ball_px_x(&self) -> i32 {
        self.ball_x / FP
    }

    fn ball_px_y(&self) -> i32 {
        self.ball_y / FP
    }

    fn attached_ball_x(&self) -> i32 {
        (self.paddle_x + PADDLE_W as i32 / 2 - BALL_SIZE as i32 / 2) * FP
    }

    fn attached_ball_y(&self) -> i32 {
        (PADDLE_Y as i32 - BALL_SIZE as i32) * FP
    }
}

// ========== Rendering ==========

fn draw_rect(x: usize, y: usize, w: usize, h: usize, color: [u8; 4]) {
    let mut row_buf = [0u8; 1280 * 4];
    for i in 0..w {
        row_buf[i * 4] = color[0];
        row_buf[i * 4 + 1] = color[1];
        row_buf[i * 4 + 2] = color[2];
        row_buf[i * 4 + 3] = color[3];
    }
    for r in 0..h {
        fb_write(x as u32, (y + r) as u32, w as u32, 1, row_buf.as_ptr());
    }
}

fn draw_char(x: usize, y: usize, ch: u8, color: [u8; 4]) {
    let glyph = match GLYPHS.iter().find(|(c, _)| *c == ch) {
        Some((_, rows)) => rows,
        None => return,
    };
    let w = GLYPH_W * FONT_SCALE;
    let h = GLYPH_H * FONT_SCALE;
    let mut buf = [0u8; (5 * 3) * (7 * 3) * 4];
    for gr in 0..GLYPH_H {
        for gc in 0..GLYPH_W {
            if (glyph[gr] >> (GLYPH_W - 1 - gc)) & 1 == 1 {
                for sr in 0..FONT_SCALE {
                    for sc in 0..FONT_SCALE {
                        let idx = ((gr * FONT_SCALE + sr) * w + gc * FONT_SCALE + sc) * 4;
                        buf[idx] = color[0];
                        buf[idx + 1] = color[1];
                        buf[idx + 2] = color[2];
                        buf[idx + 3] = color[3];
                    }
                }
            }
        }
    }
    fb_write(x as u32, y as u32, w as u32, h as u32, buf.as_ptr());
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
        for i in 0..len / 2 {
            d.swap(i, len - 1 - i);
        }
    }
    draw_string(x, y, &d[..len], color);
}

// ========== Visual helpers ==========

fn highlight(c: [u8; 4]) -> [u8; 4] {
    [c[0].saturating_add(50), c[1].saturating_add(50), c[2].saturating_add(50), c[3]]
}

fn shadow(c: [u8; 4]) -> [u8; 4] {
    [(c[0] as u16 * 40 / 100) as u8, (c[1] as u16 * 40 / 100) as u8,
     (c[2] as u16 * 40 / 100) as u8, c[3]]
}

// ========== Game-specific rendering ==========

fn draw_brick(col: usize, row: usize) {
    let bx = BRICK_OFFSET_X + col * (BRICK_W + BRICK_GAP);
    let by = BRICK_OFFSET_Y + row * (BRICK_H + BRICK_GAP);
    let base = BRICK_COLORS[row];
    let hi = highlight(base);
    let sh = shadow(base);
    // 3D beveled brick: bright top-left edge, dark bottom-right edge
    let bevel: usize = 2;
    // Top highlight strip
    draw_rect(bx, by, BRICK_W, bevel, hi);
    // Bottom shadow strip
    draw_rect(bx, by + BRICK_H - bevel, BRICK_W, bevel, sh);
    // Left highlight strip
    draw_rect(bx, by + bevel, bevel, BRICK_H - bevel * 2, hi);
    // Right shadow strip
    draw_rect(bx + BRICK_W - bevel, by + bevel, bevel, BRICK_H - bevel * 2, sh);
    // Inner face
    draw_rect(bx + bevel, by + bevel, BRICK_W - bevel * 2, BRICK_H - bevel * 2, base);
}

fn erase_brick(col: usize, row: usize) {
    let bx = BRICK_OFFSET_X + col * (BRICK_W + BRICK_GAP);
    let by = BRICK_OFFSET_Y + row * (BRICK_H + BRICK_GAP);
    draw_rect(bx, by, BRICK_W, BRICK_H, PLAY_BG);
}

fn draw_paddle(game: &Game) {
    let px = game.paddle_x as usize;
    let hi = highlight(PADDLE_COLOR);
    let sh = shadow(PADDLE_COLOR);
    // Beveled paddle: bright top, body, dark bottom
    draw_rect(px, PADDLE_Y, PADDLE_W, 2, hi);
    draw_rect(px, PADDLE_Y + 2, PADDLE_W, PADDLE_H - 4, PADDLE_COLOR);
    draw_rect(px, PADDLE_Y + PADDLE_H - 2, PADDLE_W, 2, sh);
}

fn erase_paddle(game: &Game) {
    draw_rect(game.paddle_x as usize, PADDLE_Y, PADDLE_W, PADDLE_H, PLAY_BG);
}

const GLOW_PAD: usize = 4; // glow extends this many pixels around ball

fn ball_pos(game: &Game) -> (i32, i32) {
    if game.ball_attached {
        (game.attached_ball_x() / FP, game.attached_ball_y() / FP)
    } else {
        (game.ball_px_x(), game.ball_px_y())
    }
}

fn draw_ball(game: &Game) {
    let (bx, by) = ball_pos(game);
    // Glow halo behind ball
    let gx = (bx as usize).saturating_sub(GLOW_PAD);
    let gy = (by as usize).saturating_sub(GLOW_PAD);
    draw_rect(gx, gy, BALL_SIZE + GLOW_PAD * 2, BALL_SIZE + GLOW_PAD * 2, BALL_GLOW);
    // Bright core
    draw_rect(bx as usize, by as usize, BALL_SIZE, BALL_SIZE, BALL_COLOR);
}

fn erase_ball(game: &Game) {
    let (bx, by) = ball_pos(game);
    let gx = (bx as usize).saturating_sub(GLOW_PAD);
    let gy = (by as usize).saturating_sub(GLOW_PAD);
    draw_rect(gx, gy, BALL_SIZE + GLOW_PAD * 2, BALL_SIZE + GLOW_PAD * 2, PLAY_BG);
}

fn draw_all_bricks(game: &Game) {
    for row in 0..BRICK_ROWS {
        for col in 0..BRICK_COLS {
            if game.bricks[row * BRICK_COLS + col] != 0 {
                draw_brick(col, row);
            } else {
                erase_brick(col, row);
            }
        }
    }
}

fn draw_hud_values(game: &mut Game) {
    if game.score == game.prev_score
        && game.lives == game.prev_lives
        && game.level == game.prev_level
    {
        return;
    }
    let hx = PLAY_X + PLAY_W + 30;
    let hy = PLAY_Y + 60;
    // Clear value areas
    draw_rect(hx, hy + 25, 130, 22, BG);
    draw_rect(hx, hy + 85, 130, 22, BG);
    draw_rect(hx, hy + 145, 130, 22, BG);
    // Draw values
    draw_number(hx, hy + 25, game.score, TEXT_VALUE);
    draw_number(hx, hy + 85, game.lives, TEXT_VALUE);
    draw_number(hx, hy + 145, game.level, TEXT_VALUE);
    game.prev_score = game.score;
    game.prev_lives = game.lives;
    game.prev_level = game.level;
}

fn draw_hud(game: &mut Game) {
    let hx = PLAY_X + PLAY_W + 30;
    let hy = PLAY_Y + 60;

    // Labels
    draw_string(hx, hy, b"SCORE", TEXT_LABEL);
    draw_string(hx, hy + 60, b"LIVES", TEXT_LABEL);
    draw_string(hx, hy + 120, b"LEVEL", TEXT_LABEL);

    draw_hud_values(game);

    // Controls
    let cy = hy + 200;
    draw_string(hx, cy, b"CONTROLS", TEXT_DIM);
    let dy = 26;
    draw_string(hx, cy + dy, b"A D  MOVE", TEXT_DIM);
    draw_string(hx, cy + dy, b"A D", TEXT_KEY);
    draw_string(hx, cy + dy * 2, b"SPC  LAUNCH", TEXT_DIM);
    draw_string(hx, cy + dy * 2, b"SPC", TEXT_KEY);
    draw_string(hx, cy + dy * 3, b"F5   SAVE", TEXT_DIM);
    draw_string(hx, cy + dy * 3, b"F5", TEXT_KEY);
    draw_string(hx, cy + dy * 4, b"F9   LOAD", TEXT_DIM);
    draw_string(hx, cy + dy * 4, b"F9", TEXT_KEY);
}

const FLASH_COLOR: [u8; 4] = [0x40, 0xF0, 0xFF, 0xFF]; // neon cyan
const FLASH_GLOW: [u8; 4] = [0x10, 0x40, 0x50, 0xFF];  // dim cyan shadow

fn draw_flash_msg(game: &Game) {
    let msg: &[u8] = if game.flash_msg == 1 { b"SAVED" } else { b"LOADED" };
    let tw = msg.len() * CHAR_PX_W;
    let fx = PLAY_X + (PLAY_W - tw) / 2;
    let fy = PLAY_Y + PLAY_H / 2 - 10;
    // Glow shadow then bright text
    draw_string(fx + 2, fy + 2, msg, FLASH_GLOW);
    draw_string(fx, fy, msg, FLASH_COLOR);
}

fn erase_flash_msg(game: &Game) {
    let msg: &[u8] = if game.flash_msg == 1 { b"SAVED" } else { b"LOADED" };
    let tw = msg.len() * CHAR_PX_W;
    let fx = PLAY_X + (PLAY_W - tw) / 2;
    let fy = PLAY_Y + PLAY_H / 2 - 10;
    // Erase both text and shadow
    draw_rect(fx, fy, tw + 2, GLYPH_H * FONT_SCALE + 2, PLAY_BG);
}

fn draw_glow_border() {
    let bx = PLAY_X;
    let by = PLAY_Y;
    let bw = PLAY_W;
    let bh = PLAY_H;
    // 3-layer neon glow around play area
    draw_rect(bx - 6, by - 6, bw + 12, bh + 12, GLOW_OUTER);
    draw_rect(bx - 4, by - 4, bw + 8, bh + 8, GLOW_MID);
    draw_rect(bx - 2, by - 2, bw + 4, bh + 4, GLOW_INNER);
}

fn draw_initial_screen(game: &mut Game) {
    // Clear entire screen
    draw_rect(0, 0, 1280, 800, BG);

    // Title with glow: draw dim shadow offset, then bright on top
    let title = b"BREAKOUT";
    let tw = title.len() * CHAR_PX_W;
    let tx = PLAY_X + (PLAY_W - tw) / 2;
    let ty = PLAY_Y - 45;
    draw_string(tx + 2, ty + 2, title, TEXT_TITLE_GLOW);
    draw_string(tx, ty, title, TEXT_TITLE);

    // Neon glow border
    draw_glow_border();

    // Play area background
    draw_rect(PLAY_X, PLAY_Y, PLAY_W, PLAY_H, PLAY_BG);

    // All bricks
    draw_all_bricks(game);

    // Paddle
    draw_paddle(game);

    // Ball
    draw_ball(game);

    // HUD
    draw_hud(game);
    fb_flush();
}

fn draw_game_over(game: &Game) {
    let ow = PLAY_W - 60;
    let oh = 160;
    let ox = PLAY_X + 30;
    let oy = PLAY_Y + (PLAY_H - oh) / 2;
    // Dark overlay with glowing border
    draw_rect(ox - 4, oy - 4, ow + 8, oh + 8, GLOW_OUTER);
    draw_rect(ox - 2, oy - 2, ow + 4, oh + 4, GLOW_MID);
    draw_rect(ox, oy, ow, oh, GAMEOVER_BG);

    // Centered "GAME OVER" title
    let go_text = b"GAME OVER";
    let go_w = go_text.len() * CHAR_PX_W;
    let go_x = ox + (ow - go_w) / 2;
    draw_string(go_x + 2, oy + 22, go_text, [0x10, 0x10, 0x80, 0xFF]);
    draw_string(go_x, oy + 20, go_text, GAMEOVER_TEXT);

    // Score display centered
    let score_label = b"SCORE";
    let sl_w = score_label.len() * CHAR_PX_W;
    draw_string(ox + (ow - sl_w) / 2 - 40, oy + 70, score_label, TEXT_LABEL);
    draw_number(ox + (ow - sl_w) / 2 + sl_w - 20, oy + 70, game.score, TEXT_VALUE);

    // Prompt
    let prompt = b"PRESS ENTER";
    let pw = prompt.len() * CHAR_PX_W;
    draw_string(ox + (ow - pw) / 2, oy + 115, prompt, TEXT_KEY);
}

// ========== Physics ==========

fn update_physics(game: &mut Game) {
    if game.ball_attached {
        // Ball follows paddle
        game.ball_x = game.attached_ball_x();
        game.ball_y = game.attached_ball_y();
        return;
    }

    // Move ball
    game.ball_x += game.ball_vx;
    game.ball_y += game.ball_vy;

    let bx = game.ball_px_x();
    let by = game.ball_px_y();

    // Wall collisions
    // Left wall
    if bx < PLAY_X as i32 {
        game.ball_x = PLAY_X as i32 * FP;
        game.ball_vx = -game.ball_vx;
    }
    // Right wall
    if bx + BALL_SIZE as i32 > (PLAY_X + PLAY_W) as i32 {
        game.ball_x = (PLAY_X + PLAY_W - BALL_SIZE) as i32 * FP;
        game.ball_vx = -game.ball_vx;
    }
    // Top wall
    if by < PLAY_Y as i32 {
        game.ball_y = PLAY_Y as i32 * FP;
        game.ball_vy = -game.ball_vy;
    }

    // Recompute pixel coords after wall bounce
    let bx = game.ball_px_x();
    let by = game.ball_px_y();

    // Paddle collision
    if game.ball_vy > 0
        && by + BALL_SIZE as i32 >= PADDLE_Y as i32
        && by + BALL_SIZE as i32 <= PADDLE_Y as i32 + PADDLE_H as i32
        && bx + BALL_SIZE as i32 > game.paddle_x
        && bx < game.paddle_x + PADDLE_W as i32
    {
        game.ball_y = (PADDLE_Y as i32 - BALL_SIZE as i32) * FP;
        game.ball_vy = -game.ball_vy;

        // Adjust vx based on hit position
        let ball_center = bx + BALL_SIZE as i32 / 2;
        let paddle_center = game.paddle_x + PADDLE_W as i32 / 2;
        let offset = ball_center - paddle_center;
        // Normalize offset to [-FP, FP] range
        let half_paddle = PADDLE_W as i32 / 2;
        let normalized = if half_paddle > 0 {
            offset * FP / half_paddle
        } else {
            0
        };
        game.ball_vx = normalized * 3;
    }

    // Recompute after paddle bounce
    let bx = game.ball_px_x();
    let by = game.ball_px_y();

    // Brick collision
    let mut hit = false;
    for row in 0..BRICK_ROWS {
        for col in 0..BRICK_COLS {
            let idx = row * BRICK_COLS + col;
            if game.bricks[idx] == 0 {
                continue;
            }

            let brick_x = (BRICK_OFFSET_X + col * (BRICK_W + BRICK_GAP)) as i32;
            let brick_y = (BRICK_OFFSET_Y + row * (BRICK_H + BRICK_GAP)) as i32;

            // AABB overlap test
            if bx + BALL_SIZE as i32 > brick_x
                && bx < brick_x + BRICK_W as i32
                && by + BALL_SIZE as i32 > brick_y
                && by < brick_y + BRICK_H as i32
            {
                // Destroy brick
                game.bricks[idx] = 0;
                game.score += ROW_SCORES[row];
                game.bricks_destroyed += 1;
                erase_brick(col, row);

                if !hit {
                    // Bounce direction: compare overlaps
                    let ball_cx = bx + BALL_SIZE as i32 / 2;
                    let ball_cy = by + BALL_SIZE as i32 / 2;
                    let brick_cx = brick_x + BRICK_W as i32 / 2;
                    let brick_cy = brick_y + BRICK_H as i32 / 2;

                    let dx = (ball_cx - brick_cx).abs();
                    let dy = (ball_cy - brick_cy).abs();

                    // Compare overlap ratios to decide bounce axis
                    let overlap_x = (BALL_SIZE as i32 + BRICK_W as i32) / 2 - dx;
                    let overlap_y = (BALL_SIZE as i32 + BRICK_H as i32) / 2 - dy;

                    if overlap_x < overlap_y {
                        game.ball_vx = -game.ball_vx;
                    } else {
                        game.ball_vy = -game.ball_vy;
                    }
                    hit = true;
                }

                // Speed increase every 5 bricks
                if game.bricks_destroyed % 5 == 0 {
                    game.ball_vx = game.ball_vx * 11 / 10;
                    game.ball_vy = game.ball_vy * 11 / 10;
                }
            }
        }
    }

    // Ball falls below play area
    let by = game.ball_px_y();
    if by > (PLAY_Y + PLAY_H) as i32 {
        if game.lives > 0 {
            game.lives -= 1;
        }
        if game.lives > 0 {
            // Reset ball to attached
            game.ball_attached = true;
            game.ball_vx = 0;
            game.ball_vy = 0;
            game.ball_x = game.attached_ball_x();
            game.ball_y = game.attached_ball_y();
        }
    }

    // Level progression: all bricks destroyed
    if game.bricks_destroyed >= (BRICK_ROWS * BRICK_COLS) as u32 {
        game.level += 1;
        game.bricks = [1; 50];
        game.bricks_destroyed = 0;
        game.ball_attached = true;
        game.ball_vx = 0;
        game.ball_vy = 0;
        game.ball_x = game.attached_ball_x();
        game.ball_y = game.attached_ball_y();
        // Full redraw for new level
        game.prev_score = u32::MAX;
        game.prev_lives = u32::MAX;
        game.prev_level = u32::MAX;
        draw_rect(PLAY_X, PLAY_Y, PLAY_W, PLAY_H, PLAY_BG);
        draw_all_bricks(game);
        draw_paddle(game);
        draw_ball(game);
        draw_hud_values(game);
        fb_flush();
    }
}

// ========== Input ==========

fn poll_key() -> Option<u8> {
    let mut buf = [0u8; 1];
    if read(STDIN, &mut buf) > 0 && buf[0] != 0 {
        Some(buf[0])
    } else {
        None
    }
}

fn drain_input(game: &mut Game) {
    loop {
        match poll_key() {
            Some(b'a') => {
                erase_paddle(game);
                erase_ball(game);
                game.paddle_x -= PADDLE_SPEED;
                if game.paddle_x < PLAY_X as i32 {
                    game.paddle_x = PLAY_X as i32;
                }
                draw_paddle(game);
                if game.ball_attached {
                    game.ball_x = game.attached_ball_x();
                    game.ball_y = game.attached_ball_y();
                }
                draw_ball(game);
            }
            Some(b'd') => {
                erase_paddle(game);
                erase_ball(game);
                game.paddle_x += PADDLE_SPEED;
                if game.paddle_x > (PLAY_X + PLAY_W - PADDLE_W) as i32 {
                    game.paddle_x = (PLAY_X + PLAY_W - PADDLE_W) as i32;
                }
                draw_paddle(game);
                if game.ball_attached {
                    game.ball_x = game.attached_ball_x();
                    game.ball_y = game.attached_ball_y();
                }
                draw_ball(game);
            }
            Some(b' ') => {
                if game.ball_attached {
                    game.ball_attached = false;
                    game.ball_x = game.attached_ball_x();
                    game.ball_y = game.attached_ball_y();
                    game.ball_vx = FP; // slight right angle
                    // Speed increases with level
                    game.ball_vy = -INIT_BALL_SPEED - (game.level as i32 - 1) * FP / 2;
                }
            }
            Some(b'\x05') => {
                // F5 = save
                save_game(game);
                game.flash_msg = 1;
                game.flash_timer = 30;
            }
            Some(b'\x09') => {
                // F9 = load
                if load_game(game) {
                    game.prev_score = u32::MAX;
                    game.prev_lives = u32::MAX;
                    game.prev_level = u32::MAX;
                    draw_initial_screen(game);
                    game.flash_msg = 2;
                    game.flash_timer = 30;
                }
            }
            Some(b'\r') => {
                // Enter key: used for restart after game over (handled in main loop)
            }
            Some(_) => {}
            None => break,
        }
    }
}

// ========== Main Game Loop ==========

/// Entry point for the Breakout game.
pub fn run_game() {
    let _ = fb_info();
    loop {
        let mut game = Game::new();
        draw_initial_screen(&mut game);
        let mut last_tick = get_time();
        let mut game_over = false;

        loop {
            if !game_over {
                drain_input(&mut game);
                // Re-check game_over after load could change lives
                game_over = game.lives == 0;
            }

            let now = get_time();
            if (now - last_tick) as usize >= 16 {
                // ~60fps
                last_tick = now;

                if !game_over {
                    // Erase ball at old position
                    erase_ball(&game);

                    // Physics update
                    update_physics(&mut game);

                    // Draw ball at new position
                    draw_ball(&game);

                    // Update HUD if score changed
                    draw_hud_values(&mut game);

                    // Flash message countdown
                    if game.flash_timer > 0 {
                        game.flash_timer -= 1;
                        if game.flash_timer == 29 {
                            draw_flash_msg(&game);
                        } else if game.flash_timer == 0 {
                            erase_flash_msg(&game);
                        }
                    }

                    // Check game over
                    if game.lives == 0 {
                        game_over = true;
                        draw_game_over(&game);
                    }

                    fb_flush();
                } else {
                    // Wait for Enter to restart
                    if poll_key() == Some(b'\r') {
                        break;
                    }
                }
            }

            sched_yield();
            sleep(8);
        }
    }
}

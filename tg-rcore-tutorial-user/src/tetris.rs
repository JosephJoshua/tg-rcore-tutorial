//! Tetris game for ch4: VirtIO GPU rendering with non-blocking keyboard input.

use crate::{fb_info, fb_write, get_time, read, sched_yield, sleep, STDIN};

// ========== Layout ==========

const BOARD_W: usize = 10;
const BOARD_H: usize = 20;
const CELL: usize = 28;

const BOARD_X: usize = 160;
const BOARD_Y: usize = 130;
const BOARD_PX_W: usize = BOARD_W * CELL;
const BOARD_PX_H: usize = BOARD_H * CELL;

const PANEL_X: usize = BOARD_X + BOARD_PX_W + 50;
const PANEL_Y: usize = BOARD_Y;

// ========== Colors (BGRA) ==========

const BG: [u8; 4] = [0x1E, 0x0A, 0x0A, 0xFF]; // deep navy-black
const BOARD_BG1: [u8; 4] = [0x2A, 0x14, 0x14, 0xFF]; // dark cell
const BOARD_BG2: [u8; 4] = [0x30, 0x18, 0x18, 0xFF]; // slightly lighter cell
const GLOW_DIM: [u8; 4] = [0x60, 0x20, 0x10, 0xFF]; // outer glow
const GLOW_MID: [u8; 4] = [0x80, 0x30, 0x18, 0xFF]; // mid glow
const GLOW_HOT: [u8; 4] = [0xC0, 0x50, 0x20, 0xFF]; // inner glow
const TEXT_LABEL: [u8; 4] = [0x80, 0x60, 0x50, 0xFF]; // dim label
const TEXT_VALUE: [u8; 4] = [0xFF, 0xEE, 0xDD, 0xFF]; // bright value
const TEXT_TITLE: [u8; 4] = [0x40, 0xE0, 0xFF, 0xFF]; // neon cyan title
const TEXT_KEY: [u8; 4] = [0x30, 0xE0, 0x30, 0xFF]; // neon green keys
const TEXT_DIM: [u8; 4] = [0x55, 0x44, 0x40, 0xFF]; // very dim
const GAMEOVER_BG: [u8; 4] = [0x10, 0x05, 0x05, 0xE0];
const GAMEOVER_TEXT: [u8; 4] = [0x20, 0x30, 0xFF, 0xFF]; // neon red-ish
const GHOST_ALPHA: u8 = 0x40;

// Piece colors — vivid neon (BGRA)
const PIECE_COLORS: [[u8; 4]; 7] = [
    [0xFF, 0xE0, 0x20, 0xFF], // I: electric cyan
    [0x00, 0xE8, 0xF0, 0xFF], // O: golden yellow
    [0xE0, 0x30, 0xC0, 0xFF], // T: hot magenta
    [0x20, 0xF0, 0x40, 0xFF], // S: neon green
    [0x30, 0x30, 0xF0, 0xFF], // Z: vivid red
    [0xF0, 0x80, 0x20, 0xFF], // J: bright blue
    [0x20, 0x90, 0xF0, 0xFF], // L: neon orange
];

// Highlight/shadow for 3D bevel
fn highlight(c: [u8; 4]) -> [u8; 4] {
    let h = |v: u8| v.saturating_add(60);
    [h(c[0]), h(c[1]), h(c[2]), c[3]]
}
fn shadow(c: [u8; 4]) -> [u8; 4] {
    let s = |v: u8| (v as u16 * 40 / 100) as u8;
    [s(c[0]), s(c[1]), s(c[2]), c[3]]
}

// ========== Pieces ==========

const PIECES: [[[(i8, i8); 4]; 4]; 7] = [
    // I
    [[(0,-1),(0,0),(0,1),(0,2)],[(-1,0),(0,0),(1,0),(2,0)],
     [(0,-1),(0,0),(0,1),(0,2)],[(-1,0),(0,0),(1,0),(2,0)]],
    // O
    [[(0,0),(0,1),(1,0),(1,1)],[(0,0),(0,1),(1,0),(1,1)],
     [(0,0),(0,1),(1,0),(1,1)],[(0,0),(0,1),(1,0),(1,1)]],
    // T
    [[(0,-1),(0,0),(0,1),(1,0)],[(-1,0),(0,0),(1,0),(0,1)],
     [(-1,0),(0,-1),(0,0),(0,1)],[(0,-1),(-1,0),(0,0),(1,0)]],
    // S
    [[(0,0),(0,1),(1,-1),(1,0)],[(-1,0),(0,0),(0,1),(1,1)],
     [(0,0),(0,1),(1,-1),(1,0)],[(-1,0),(0,0),(0,1),(1,1)]],
    // Z
    [[(0,-1),(0,0),(1,0),(1,1)],[(0,0),(1,0),(-1,1),(0,1)],
     [(0,-1),(0,0),(1,0),(1,1)],[(0,0),(1,0),(-1,1),(0,1)]],
    // J
    [[(0,-1),(0,0),(0,1),(1,-1)],[(-1,0),(0,0),(1,0),(1,1)],
     [(-1,1),(0,-1),(0,0),(0,1)],[(-1,-1),(-1,0),(0,0),(1,0)]],
    // L
    [[(0,-1),(0,0),(0,1),(1,1)],[(-1,0),(0,0),(1,0),(-1,1)],
     [(-1,-1),(0,-1),(0,0),(0,1)],[(1,-1),(-1,0),(0,0),(1,0)]],
];

// ========== Font ==========

const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const FONT_SCALE: usize = 3;
const CHAR_PX_W: usize = GLYPH_W * FONT_SCALE + 2;

const GLYPHS: [(u8, [u8; 7]); 33] = [
    (b'0', [0b01110,0b10001,0b10011,0b10101,0b11001,0b10001,0b01110]),
    (b'1', [0b00100,0b01100,0b00100,0b00100,0b00100,0b00100,0b01110]),
    (b'2', [0b01110,0b10001,0b00001,0b00110,0b01000,0b10000,0b11111]),
    (b'3', [0b01110,0b10001,0b00001,0b00110,0b00001,0b10001,0b01110]),
    (b'4', [0b00010,0b00110,0b01010,0b10010,0b11111,0b00010,0b00010]),
    (b'5', [0b11111,0b10000,0b11110,0b00001,0b00001,0b10001,0b01110]),
    (b'6', [0b01110,0b10000,0b11110,0b10001,0b10001,0b10001,0b01110]),
    (b'7', [0b11111,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000]),
    (b'8', [0b01110,0b10001,0b10001,0b01110,0b10001,0b10001,0b01110]),
    (b'9', [0b01110,0b10001,0b10001,0b01111,0b00001,0b10001,0b01110]),
    (b'A', [0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001]),
    (b'C', [0b01110,0b10001,0b10000,0b10000,0b10000,0b10001,0b01110]),
    (b'D', [0b11110,0b10001,0b10001,0b10001,0b10001,0b10001,0b11110]),
    (b'E', [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b11111]),
    (b'F', [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000]),
    (b'G', [0b01110,0b10001,0b10000,0b10111,0b10001,0b10001,0b01110]),
    (b'H', [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001]),
    (b'I', [0b01110,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110]),
    (b'K', [0b10001,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001]),
    (b'L', [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111]),
    (b'M', [0b10001,0b11011,0b10101,0b10101,0b10001,0b10001,0b10001]),
    (b'N', [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001]),
    (b'O', [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110]),
    (b'P', [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000]),
    (b'R', [0b11110,0b10001,0b10001,0b11110,0b10010,0b10001,0b10001]),
    (b'S', [0b01110,0b10001,0b10000,0b01110,0b00001,0b10001,0b01110]),
    (b'T', [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100]),
    (b'V', [0b10001,0b10001,0b10001,0b10001,0b01010,0b01010,0b00100]),
    (b'W', [0b10001,0b10001,0b10001,0b10101,0b10101,0b11011,0b10001]),
    (b'X', [0b10001,0b10001,0b01010,0b00100,0b01010,0b10001,0b10001]),
    (b'Y', [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100]),
    (b' ', [0b00000,0b00000,0b00000,0b00000,0b00000,0b00000,0b00000]),
    (b'-', [0b00000,0b00000,0b00000,0b11111,0b00000,0b00000,0b00000]),
];

// ========== PRNG ==========

struct Rng { state: u64 }

impl Rng {
    fn new(seed: u64) -> Self { Self { state: if seed == 0 { 1 } else { seed } } }
    fn next(&mut self) -> u64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        self.state
    }
    fn next_piece(&mut self) -> u8 { (self.next() % 7) as u8 }
}

// ========== Game State ==========

#[derive(Clone, Copy, PartialEq)]
enum GameState { Playing, GameOver }

struct Piece { shape: u8, rotation: u8, row: i8, col: i8 }

impl Piece {
    fn cells(&self) -> [(i8, i8); 4] {
        let o = PIECES[self.shape as usize][self.rotation as usize];
        [(self.row+o[0].0, self.col+o[0].1), (self.row+o[1].0, self.col+o[1].1),
         (self.row+o[2].0, self.col+o[2].1), (self.row+o[3].0, self.col+o[3].1)]
    }
    fn moved(&self, dr: i8, dc: i8) -> Self {
        Self { shape: self.shape, rotation: self.rotation, row: self.row+dr, col: self.col+dc }
    }
    fn rotated(&self) -> Self {
        Self { shape: self.shape, rotation: (self.rotation+1)%4, row: self.row, col: self.col }
    }
}

struct Game {
    board: [u8; BOARD_W * BOARD_H],
    current: Piece,
    next_shape: u8,
    score: u32,
    level: u32,
    lines: u32,
    state: GameState,
    rng: Rng,
    tick_ms: usize,
    soft_drop: bool,
}

impl Game {
    fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let cs = rng.next_piece();
        let ns = rng.next_piece();
        Self {
            board: [0; BOARD_W * BOARD_H],
            current: Piece { shape: cs, rotation: 0, row: 0, col: 4 },
            next_shape: ns, score: 0, level: 1, lines: 0,
            state: GameState::Playing, rng, tick_ms: 800, soft_drop: false,
        }
    }

    fn fits(&self, piece: &Piece) -> bool {
        for (r, c) in piece.cells() {
            if r >= BOARD_H as i8 || c < 0 || c >= BOARD_W as i8 { return false; }
            // Allow r < 0 (above board) during movement — only checked on lock
            if r >= 0 && self.board[r as usize * BOARD_W + c as usize] != 0 { return false; }
        }
        true
    }

    fn ghost_row(&self) -> i8 {
        let mut dr: i8 = 0;
        while self.fits(&self.current.moved(dr + 1, 0)) { dr += 1; }
        self.current.row + dr
    }

    /// Lock piece onto board. Returns true if any cell was above the visible board (lock out).
    fn lock_piece(&mut self) -> bool {
        let color = self.current.shape + 1;
        let mut above = false;
        for (r, c) in self.current.cells() {
            if r < 0 {
                above = true;
            } else if r < BOARD_H as i8 && c >= 0 && c < BOARD_W as i8 {
                self.board[r as usize * BOARD_W + c as usize] = color;
            }
        }
        above
    }

    fn clear_lines(&mut self) -> u32 {
        let mut cleared = 0u32;
        let mut dst = BOARD_H as i32 - 1;
        for src in (0..BOARD_H).rev() {
            if (0..BOARD_W).all(|c| self.board[src * BOARD_W + c] != 0) {
                cleared += 1;
            } else {
                if dst as usize != src {
                    for c in 0..BOARD_W {
                        self.board[dst as usize * BOARD_W + c] = self.board[src * BOARD_W + c];
                    }
                }
                dst -= 1;
            }
        }
        for r in 0..=dst {
            for c in 0..BOARD_W { self.board[r as usize * BOARD_W + c] = 0; }
        }
        cleared
    }

    fn add_score(&mut self, lc: u32) {
        let pts = match lc { 1=>100, 2=>300, 3=>500, 4=>800, _=>0 };
        self.score += pts * self.level;
        self.lines += lc;
        self.level = 1 + self.lines / 10;
        self.tick_ms = if self.level >= 8 { 100 } else { 800 - (self.level as usize - 1) * 100 };
    }

    fn spawn_next(&mut self) -> bool {
        self.current = Piece { shape: self.next_shape, rotation: 0, row: 0, col: 4 };
        self.next_shape = self.rng.next_piece();
        self.fits(&self.current)
    }

    fn try_move(&mut self, dr: i8, dc: i8) -> bool {
        let m = self.current.moved(dr, dc);
        if self.fits(&m) { self.current = m; true } else { false }
    }

    fn try_rotate(&mut self) -> bool {
        let r = self.current.rotated();
        if self.fits(&r) { self.current = r; true } else { false }
    }

    fn hard_drop(&mut self) {
        while self.fits(&self.current.moved(1, 0)) { self.current.row += 1; }
    }
}

// ========== Rendering ==========

fn draw_rect(x: usize, y: usize, w: usize, h: usize, color: [u8; 4]) {
    let mut row_buf = [0u8; 1280 * 4];
    for i in 0..w {
        row_buf[i*4] = color[0]; row_buf[i*4+1] = color[1];
        row_buf[i*4+2] = color[2]; row_buf[i*4+3] = color[3];
    }
    for r in 0..h { fb_write(x as u32, (y+r) as u32, w as u32, 1, row_buf.as_ptr()); }
}

fn draw_cell_3d(x: usize, y: usize, base: [u8; 4]) {
    let hi = highlight(base);
    let sh = shadow(base);
    let mut buf = [0u8; CELL * CELL * 4];
    let bevel = 2;
    for r in 0..CELL {
        for c in 0..CELL {
            let idx = (r * CELL + c) * 4;
            let color = if r < bevel || c < bevel {
                hi // top-left highlight
            } else if r >= CELL - bevel || c >= CELL - bevel {
                sh // bottom-right shadow
            } else {
                base
            };
            buf[idx] = color[0]; buf[idx+1] = color[1];
            buf[idx+2] = color[2]; buf[idx+3] = color[3];
        }
    }
    fb_write(x as u32, y as u32, CELL as u32, CELL as u32, buf.as_ptr());
}

fn draw_ghost_cell(x: usize, y: usize, color: [u8; 4]) {
    let mut buf = [0u8; CELL * CELL * 4];
    for r in 0..CELL {
        for c in 0..CELL {
            let idx = (r * CELL + c) * 4;
            if r <= 1 || c <= 1 || r >= CELL - 2 || c >= CELL - 2 {
                buf[idx] = color[0]; buf[idx+1] = color[1];
                buf[idx+2] = color[2]; buf[idx+3] = GHOST_ALPHA;
            }
        }
    }
    fb_write(x as u32, y as u32, CELL as u32, CELL as u32, buf.as_ptr());
}

fn draw_board_cell(row: usize, col: usize) {
    let c = if (row + col) % 2 == 0 { BOARD_BG1 } else { BOARD_BG2 };
    draw_cell_3d(BOARD_X + col * CELL, BOARD_Y + row * CELL, c);
}

fn draw_piece_cell(row: usize, col: usize, shape: u8) {
    draw_cell_3d(BOARD_X + col * CELL, BOARD_Y + row * CELL, PIECE_COLORS[shape as usize]);
}

fn restore_cell(game: &Game, r: usize, c: usize) {
    let v = game.board[r * BOARD_W + c];
    if v == 0 { draw_board_cell(r, c); } else { draw_piece_cell(r, c, v - 1); }
}

fn draw_char(x: usize, y: usize, ch: u8, color: [u8; 4]) {
    let glyph = match GLYPHS.iter().find(|(c, _)| *c == ch) {
        Some((_, rows)) => rows, None => return,
    };
    let w = GLYPH_W * FONT_SCALE;
    let h = GLYPH_H * FONT_SCALE;
    let mut buf = [0u8; (5*3) * (7*3) * 4];
    for gr in 0..GLYPH_H {
        for gc in 0..GLYPH_W {
            if (glyph[gr] >> (GLYPH_W - 1 - gc)) & 1 == 1 {
                for sr in 0..FONT_SCALE { for sc in 0..FONT_SCALE {
                    let idx = ((gr*FONT_SCALE+sr) * w + gc*FONT_SCALE+sc) * 4;
                    buf[idx] = color[0]; buf[idx+1] = color[1];
                    buf[idx+2] = color[2]; buf[idx+3] = color[3];
                }}
            }
        }
    }
    fb_write(x as u32, y as u32, w as u32, h as u32, buf.as_ptr());
}

fn draw_string(x: usize, y: usize, s: &[u8], color: [u8; 4]) {
    for (i, &ch) in s.iter().enumerate() { draw_char(x + i * CHAR_PX_W, y, ch, color); }
}

fn draw_number(x: usize, y: usize, mut n: u32, color: [u8; 4]) {
    let mut d = [0u8; 10]; let mut len = 0;
    if n == 0 { d[0] = b'0'; len = 1; }
    else { while n > 0 { d[len] = b'0' + (n % 10) as u8; n /= 10; len += 1; }
           for i in 0..len/2 { d.swap(i, len-1-i); } }
    draw_string(x, y, &d[..len], color);
}

// ========== Screen Composition ==========

fn draw_glow_border() {
    let bx = BOARD_X; let by = BOARD_Y;
    let bw = BOARD_PX_W; let bh = BOARD_PX_H;
    // 3-layer neon glow
    draw_rect(bx-6, by-6, bw+12, bh+12, GLOW_DIM);
    draw_rect(bx-4, by-4, bw+8,  bh+8,  GLOW_MID);
    draw_rect(bx-2, by-2, bw+4,  bh+4,  GLOW_HOT);
}

fn draw_next_piece(shape: u8) {
    let preview_x = PANEL_X;
    let preview_y = PANEL_Y + 30;
    draw_rect(preview_x, preview_y, 4*CELL+10, 3*CELL+10, BG);
    let offsets = PIECES[shape as usize][0];
    for (dr, dc) in offsets {
        let px = preview_x + 5 + ((dc+1) as usize) * CELL;
        let py = preview_y + 5 + (dr as usize) * CELL;
        draw_cell_3d(px, py, PIECE_COLORS[shape as usize]);
    }
}

fn draw_panel_values(game: &Game) {
    let vx = PANEL_X; let vy = PANEL_Y;
    draw_rect(vx, vy+160, 130, 22, BG);
    draw_rect(vx, vy+220, 130, 22, BG);
    draw_rect(vx, vy+280, 130, 22, BG);
    draw_number(vx, vy+160, game.score, TEXT_VALUE);
    draw_number(vx, vy+220, game.level, TEXT_VALUE);
    draw_number(vx, vy+280, game.lines, TEXT_VALUE);
}

fn draw_controls() {
    let cx = PANEL_X;
    let cy = PANEL_Y + 340;
    draw_string(cx, cy, b"CONTROLS", TEXT_DIM);
    let dy = 28;
    draw_string(cx, cy+dy*1, b"W", TEXT_KEY);
    draw_string(cx+CHAR_PX_W*2, cy+dy*1, b"ROTATE", TEXT_DIM);
    draw_string(cx, cy+dy*2, b"A D", TEXT_KEY);
    draw_string(cx+CHAR_PX_W*4, cy+dy*2, b"MOVE", TEXT_DIM);
    draw_string(cx, cy+dy*3, b"S", TEXT_KEY);
    draw_string(cx+CHAR_PX_W*2, cy+dy*3, b"SOFT", TEXT_DIM);
    draw_string(cx, cy+dy*4, b"SPACE", TEXT_KEY);
    draw_string(cx+CHAR_PX_W*6, cy+dy*4, b"DROP", TEXT_DIM);
}

fn draw_initial_screen(game: &Game) {
    draw_rect(0, 0, 1280, 800, BG);

    // Title
    let title = b"TETRIS";
    let tw = title.len() * CHAR_PX_W;
    draw_string(BOARD_X + (BOARD_PX_W - tw) / 2, BOARD_Y - 40, title, TEXT_TITLE);

    // Board glow border + empty board
    draw_glow_border();
    for r in 0..BOARD_H { for c in 0..BOARD_W { draw_board_cell(r, c); } }

    // Panel labels
    draw_string(PANEL_X, PANEL_Y, b"NEXT", TEXT_LABEL);
    draw_string(PANEL_X, PANEL_Y+135, b"SCORE", TEXT_LABEL);
    draw_string(PANEL_X, PANEL_Y+195, b"LEVEL", TEXT_LABEL);
    draw_string(PANEL_X, PANEL_Y+255, b"LINES", TEXT_LABEL);

    draw_panel_values(game);
    draw_next_piece(game.next_shape);
    draw_controls();
    draw_falling_piece(game);
}

fn draw_falling_piece(game: &Game) {
    // Ghost
    let gr = game.ghost_row();
    let ghost = Piece { shape: game.current.shape, rotation: game.current.rotation,
                        row: gr, col: game.current.col };
    for (r, c) in ghost.cells() {
        if r >= 0 && r < BOARD_H as i8 && c >= 0 && c < BOARD_W as i8 {
            draw_ghost_cell(BOARD_X + c as usize * CELL, BOARD_Y + r as usize * CELL,
                           PIECE_COLORS[game.current.shape as usize]);
        }
    }
    // Current piece
    for (r, c) in game.current.cells() {
        if r >= 0 && r < BOARD_H as i8 && c >= 0 && c < BOARD_W as i8 {
            draw_piece_cell(r as usize, c as usize, game.current.shape);
        }
    }
}

fn erase_falling_piece(game: &Game) {
    let gr = game.ghost_row();
    let ghost = Piece { shape: game.current.shape, rotation: game.current.rotation,
                        row: gr, col: game.current.col };
    for (r, c) in ghost.cells() {
        if r >= 0 && r < BOARD_H as i8 && c >= 0 && c < BOARD_W as i8 {
            restore_cell(game, r as usize, c as usize);
        }
    }
    for (r, c) in game.current.cells() {
        if r >= 0 && r < BOARD_H as i8 && c >= 0 && c < BOARD_W as i8 {
            restore_cell(game, r as usize, c as usize);
        }
    }
}

fn redraw_board(game: &Game) {
    for r in 0..BOARD_H { for c in 0..BOARD_W {
        let v = game.board[r * BOARD_W + c];
        if v == 0 { draw_board_cell(r, c); } else { draw_piece_cell(r, c, v-1); }
    }}
}

fn draw_game_over(game: &Game) {
    let ow = BOARD_PX_W - 20;
    let oh = 120;
    let ox = BOARD_X + 10;
    let oy = BOARD_Y + (BOARD_PX_H - oh) / 2;
    draw_rect(ox, oy, ow, oh, GAMEOVER_BG);

    draw_string(ox+40, oy+15, b"GAME OVER", GAMEOVER_TEXT);

    // Show final score
    draw_string(ox+40, oy+50, b"SCORE", TEXT_LABEL);
    draw_number(ox+40 + 6*CHAR_PX_W, oy+50, game.score, TEXT_VALUE);

    draw_string(ox+40, oy+80, b"PRESS ANY KEY", TEXT_DIM);
}

// ========== Input ==========

fn poll_key() -> Option<u8> {
    let mut buf = [0u8; 1];
    if read(STDIN, &mut buf) > 0 && buf[0] != 0 { Some(buf[0]) } else { None }
}

fn drain_input(game: &mut Game) -> bool {
    loop {
        match poll_key() {
            Some(b'a') => { erase_falling_piece(game); game.try_move(0,-1); draw_falling_piece(game); }
            Some(b'd') => { erase_falling_piece(game); game.try_move(0,1);  draw_falling_piece(game); }
            Some(b'w') => { erase_falling_piece(game); game.try_rotate();   draw_falling_piece(game); }
            Some(b's') => { game.soft_drop = true; }
            Some(b' ') => {
                erase_falling_piece(game);
                game.hard_drop();
                draw_falling_piece(game);
                return true;
            }
            Some(_) => {}
            None => break,
        }
    }
    false
}

fn wait_for_key() {
    loop {
        if poll_key().is_some() { return; }
        sched_yield(); sleep(50);
    }
}

// ========== Main Game Loop ==========

/// Entry point for the Tetris game.
pub fn run_game() {
    let _ = fb_info();
    loop {
        let seed = get_time() as u64;
        let mut game = Game::new(seed);
        draw_initial_screen(&game);
        let mut last_tick = get_time();

        loop {
            let hard_dropped = drain_input(&mut game);
            let now = get_time();
            let interval = if game.soft_drop { 50 } else { game.tick_ms };
            let should_tick = hard_dropped || (now - last_tick) as usize >= interval;

            if should_tick && game.state == GameState::Playing {
                last_tick = now;
                game.soft_drop = false;
                erase_falling_piece(&game);

                if hard_dropped || !game.try_move(1, 0) {
                    // Save cells before lock overwrites current piece
                    let locked_cells = game.current.cells();
                    let locked_shape = game.current.shape;
                    let lock_out = game.lock_piece();
                    let cleared = game.clear_lines();

                    if cleared > 0 {
                        game.add_score(cleared);
                        redraw_board(&game);
                        draw_panel_values(&game);
                    } else {
                        // Draw locked piece immediately
                        for (r, c) in locked_cells {
                            if r >= 0 && r < BOARD_H as i8 && c >= 0 && c < BOARD_W as i8 {
                                draw_piece_cell(r as usize, c as usize, locked_shape);
                            }
                        }
                    }

                    // Game over: lock out (piece above board) or block out (can't spawn)
                    if lock_out || !game.spawn_next() {
                        game.state = GameState::GameOver;
                        redraw_board(&game);
                        draw_game_over(&game);
                    } else {
                        draw_next_piece(game.next_shape);
                    }
                }

                if game.state == GameState::Playing {
                    draw_falling_piece(&game);
                }
            }

            if game.state == GameState::GameOver {
                wait_for_key();
                break;
            }

            sched_yield();
            sleep(16);
        }
    }
}

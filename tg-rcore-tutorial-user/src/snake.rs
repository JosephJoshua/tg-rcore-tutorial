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

const BG_COLOR: [u8; 4] = [0x2E, 0x1A, 0x1A, 0xFF];
const BOARD_COLOR: [u8; 4] = [0x3E, 0x21, 0x16, 0xFF];
const GRID_COLOR: [u8; 4] = [0x44, 0x27, 0x1A, 0xFF];
const HEAD_COLOR: [u8; 4] = [0x41, 0xFF, 0x00, 0xFF];
const BODY_COLOR: [u8; 4] = [0x30, 0xB8, 0x00, 0xFF];
const FOOD_COLOR: [u8; 4] = [0x44, 0x44, 0xFF, 0xFF];
const BORDER_COLOR: [u8; 4] = [0x8E, 0x9B, 0x0F, 0xFF];
const TEXT_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const OVERLAY_COLOR: [u8; 4] = [0x3A, 0x1E, 0x0E, 0xFF];

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

fn draw_cell(gx: u32, gy: u32, color: [u8; 4]) {
    let buf = unsafe { &mut *(&raw mut RENDER_BUF) };
    let cs = CELL_SIZE as usize;
    for py in 0..cs {
        for px in 0..cs {
            let off = (py * cs + px) * 4;
            let c = if px == 0 || py == 0 { &GRID_COLOR } else { &color };
            buf[off] = c[0];
            buf[off + 1] = c[1];
            buf[off + 2] = c[2];
            buf[off + 3] = c[3];
        }
    }
    fb_write(
        BOARD_X + gx * CELL_SIZE,
        BOARD_Y + gy * CELL_SIZE,
        CELL_SIZE,
        CELL_SIZE,
        buf.as_ptr(),
    );
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

fn draw_initial_screen(game: &Game) {
    let (sw, sh) = fb_info();

    // Fill background in strips
    let strip_h = 20u32;
    let strip_pixels = sw as usize * strip_h as usize;
    let mut strip = vec![0u8; strip_pixels * 4];
    for i in 0..strip_pixels {
        let off = i * 4;
        strip[off] = BG_COLOR[0];
        strip[off + 1] = BG_COLOR[1];
        strip[off + 2] = BG_COLOR[2];
        strip[off + 3] = BG_COLOR[3];
    }
    let mut y = 0u32;
    while y < sh {
        let h = core::cmp::min(strip_h, sh - y);
        fb_write(0, y, sw, h, strip.as_ptr());
        y += strip_h;
    }
    drop(strip);

    // Draw border
    draw_rect(BOARD_X - 2, BOARD_Y - 2, BOARD_PX_W + 4, 2, BORDER_COLOR);
    draw_rect(BOARD_X - 2, BOARD_Y + BOARD_PX_H, BOARD_PX_W + 4, 2, BORDER_COLOR);
    draw_rect(BOARD_X - 2, BOARD_Y, 2, BOARD_PX_H, BORDER_COLOR);
    draw_rect(BOARD_X + BOARD_PX_W, BOARD_Y, 2, BOARD_PX_H, BORDER_COLOR);

    // Draw all grid cells
    for gy in 0..GRID_H {
        for gx in 0..GRID_W {
            draw_cell(gx, gy, BOARD_COLOR);
        }
    }

    // Draw snake
    for i in 0..game.snake.len {
        let idx = (game.snake.head + 400 - i) % 400;
        let (x, y) = game.snake.body[idx];
        let color = if i == 0 { HEAD_COLOR } else { BODY_COLOR };
        draw_cell(x as u32, y as u32, color);
    }

    // Draw food
    draw_cell(game.food.0 as u32, game.food.1 as u32, FOOD_COLOR);

    // Draw score panel
    draw_text(PANEL_X, PANEL_Y, b"SNAKE", TEXT_COLOR, BG_COLOR);
    draw_text(PANEL_X, PANEL_Y + 60, b"SCORE", TEXT_COLOR, BG_COLOR);
    draw_score(game.score);
    draw_text(PANEL_X, PANEL_Y + 200, b"WASD", TEXT_COLOR, BG_COLOR);
}

fn render_tick(game: &Game, result: &TickResult) {
    draw_cell(result.new_head.0 as u32, result.new_head.1 as u32, HEAD_COLOR);
    draw_cell(result.prev_head.0 as u32, result.prev_head.1 as u32, BODY_COLOR);
    if let Some((tx, ty)) = result.old_tail {
        draw_cell(tx as u32, ty as u32, BOARD_COLOR);
    }
    if result.food_changed {
        draw_cell(game.food.0 as u32, game.food.1 as u32, FOOD_COLOR);
        draw_score(game.score);
    }
}

fn draw_game_over(score: u32) {
    let ox = BOARD_X + 120;
    let oy = BOARD_Y + 200;
    let ow = 400u32;
    let oh = 200u32;

    // Overlay background in strips
    let strip_h = 10u32;
    let mut y = 0u32;
    while y < oh {
        let h = core::cmp::min(strip_h, oh - y);
        draw_rect(ox, oy + y, ow, h, OVERLAY_COLOR);
        y += strip_h;
    }

    // Overlay border
    draw_rect(ox, oy, ow, 2, BORDER_COLOR);
    draw_rect(ox, oy + oh - 2, ow, 2, BORDER_COLOR);
    draw_rect(ox, oy, 2, oh, BORDER_COLOR);
    draw_rect(ox + ow - 2, oy, 2, oh, BORDER_COLOR);

    // "GAME OVER" text
    let text_x = ox + (ow - 4 * CHAR_SPACING) / 2;
    draw_text(text_x, oy + 30, b"GAME", TEXT_COLOR, OVERLAY_COLOR);
    draw_text(text_x, oy + 70, b"OVER", TEXT_COLOR, OVERLAY_COLOR);

    // Score
    let label_x = ox + (ow - 5 * CHAR_SPACING) / 2;
    draw_text(label_x, oy + 120, b"SCORE", TEXT_COLOR, OVERLAY_COLOR);
    let digits = count_digits(score);
    let score_x = ox + (ow - digits * CHAR_SPACING) / 2;
    draw_number(score_x, oy + 155, score, TEXT_COLOR, OVERLAY_COLOR);
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

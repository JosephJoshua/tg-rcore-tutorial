//! Two-player Pong game using shared memory IPC between two processes.
//!
//! Parent process: player 1 (W/S keys), runs ball physics and rendering.
//! Child process: player 2 (Up/Down keys), writes input to shared memory.

use crate::{fb_flush, fb_info, fb_write, shm_create};

// ─── Constants ───

const PLAY_LEFT: i32 = 90;
const PLAY_RIGHT: i32 = 1190;
const PLAY_TOP: i32 = 50;
const PLAY_BOTTOM: i32 = 750;
const PLAY_WIDTH: i32 = PLAY_RIGHT - PLAY_LEFT;
const PLAY_HEIGHT: i32 = PLAY_BOTTOM - PLAY_TOP;

const PADDLE_WIDTH: i32 = 12;
const PADDLE_HEIGHT: i32 = 100;
const PADDLE1_X: i32 = 100;
const PADDLE2_X: i32 = 1168;
const PADDLE_SPEED: i32 = 6;

const BALL_SIZE: i32 = 12;
const BALL_INIT_SPEED: i32 = 4 * 256;
const BALL_SPEED_CAP: i32 = 10 * 256;

const FP_SHIFT: i32 = 8;
const FP_ONE: i32 = 1 << FP_SHIFT;

const WIN_SCORE: u32 = 5;

// ─── Neon Arcade palette (BGRA format) ───
const BG_COLOR: [u8; 4] = [0x12, 0x06, 0x04, 0xFF];       // #040612 deep void
const BG_FIELD: [u8; 4] = [0x18, 0x0A, 0x06, 0xFF];       // #060A18 slightly lighter field
const WALL_COLOR: [u8; 4] = [0x44, 0x22, 0x11, 0xFF];      // #112244 dim indigo border
const WALL_GLOW: [u8; 4] = [0x66, 0x33, 0x18, 0xFF];       // #183366 border glow
const P1_COLOR: [u8; 4] = [0xEE, 0xFF, 0x00, 0xFF];        // #00FFEE neon cyan
const P1_GLOW: [u8; 4] = [0x55, 0x66, 0x00, 0xFF];         // #006655 cyan glow
const P2_COLOR: [u8; 4] = [0x66, 0x00, 0xFF, 0xFF];        // #FF0066 neon magenta/pink
const P2_GLOW: [u8; 4] = [0x22, 0x00, 0x55, 0xFF];         // #550022 magenta glow
const BALL_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];      // #FFFFFF pure white
const BALL_GLOW: [u8; 4] = [0x55, 0x55, 0x55, 0xFF];       // #555555 white glow
const BALL_TRAIL1: [u8; 4] = [0x33, 0x33, 0x33, 0xFF];     // #333333 recent trail
const BALL_TRAIL2: [u8; 4] = [0x1A, 0x1A, 0x1A, 0xFF];     // #1A1A1A fading trail
const CENTER_LINE_COLOR: [u8; 4] = [0x28, 0x14, 0x0C, 0xFF]; // #0C1428 subtle center
const SCORE_COLON: [u8; 4] = [0x40, 0x30, 0x20, 0xFF];     // dim colon dots

// VirtIO keyboard scancodes
const KEY_W: u8 = 17;
const KEY_S: u8 = 31;
const KEY_UP: u8 = 103;
const KEY_DOWN: u8 = 108;
const KEY_SPACE: u8 = 57;
const KEY_ENTER: u8 = 28;

// Game states
const STATE_WAITING: u32 = 0;
const STATE_PLAYING: u32 = 1;
const STATE_POINT_SCORED: u32 = 2;
const STATE_GAME_OVER: u32 = 3;

// ─── Shared memory layout ───

#[repr(C)]
struct SharedState {
    ball_x: i32,
    ball_y: i32,
    ball_vx: i32,
    ball_vy: i32,
    paddle1_y: i32,
    paddle2_y: i32,
    score1: u32,
    score2: u32,
    tick: u32,
    game_state: u32,
    p1_up: u8,
    p1_down: u8,
    p2_up: u8,
    p2_down: u8,
}

// ─── Drawing helpers ───

fn fill_rect(x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    if w <= 0 || h <= 0 {
        return;
    }
    const TILE: i32 = 16;
    let mut buf = [0u8; (TILE * TILE * 4) as usize];

    let mut ty = 0;
    while ty < h {
        let th = if h - ty > TILE { TILE } else { h - ty };
        let mut tx = 0;
        while tx < w {
            let tw = if w - tx > TILE { TILE } else { w - tx };
            let pixels = (tw * th) as usize;
            for i in 0..pixels {
                let off = i * 4;
                buf[off] = color[0];
                buf[off + 1] = color[1];
                buf[off + 2] = color[2];
                buf[off + 3] = color[3];
            }
            fb_write(
                (x + tx) as u32,
                (y + ty) as u32,
                tw as u32,
                th as u32,
                buf.as_ptr(),
            );
            tx += TILE;
        }
        ty += TILE;
    }
}

// ─── 7-segment digit rendering ───

const DIGIT_W: i32 = 20;
const DIGIT_H: i32 = 30;
const SEG_T: i32 = 4;

const DIGIT_SEGS: [u8; 10] = [
    0b0111111, // 0
    0b0000110, // 1
    0b1011011, // 2
    0b1001111, // 3
    0b1100110, // 4
    0b1101101, // 5
    0b1111101, // 6
    0b0000111, // 7
    0b1111111, // 8
    0b1101111, // 9
];

fn draw_digit(x: i32, y: i32, digit: u32, color: [u8; 4]) {
    if digit > 9 {
        return;
    }
    let segs = DIGIT_SEGS[digit as usize];
    let w = DIGIT_W;
    let h = DIGIT_H;
    let half = h / 2;
    if segs & (1 << 0) != 0 {
        fill_rect(x, y, w, SEG_T, color);
    }
    if segs & (1 << 1) != 0 {
        fill_rect(x + w - SEG_T, y, SEG_T, half, color);
    }
    if segs & (1 << 2) != 0 {
        fill_rect(x + w - SEG_T, y + half, SEG_T, half, color);
    }
    if segs & (1 << 3) != 0 {
        fill_rect(x, y + h - SEG_T, w, SEG_T, color);
    }
    if segs & (1 << 4) != 0 {
        fill_rect(x, y + half, SEG_T, half, color);
    }
    if segs & (1 << 5) != 0 {
        fill_rect(x, y, SEG_T, half, color);
    }
    if segs & (1 << 6) != 0 {
        fill_rect(x, y + half - SEG_T / 2, w, SEG_T, color);
    }
}

fn draw_score(score1: u32, score2: u32) {
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let scale = 2;
    let dw = DIGIT_W * scale; // 40
    let dh = DIGIT_H * scale; // 60
    let sy = PLAY_TOP + 15;
    // Erase score area
    fill_rect(cx - dw - 30, sy - 2, dw * 2 + 60, dh + 4, BG_FIELD);
    // P1 score in cyan, left of center
    draw_big_digit(cx - dw - 20, sy, score1, P1_COLOR, scale);
    // Colon dots
    fill_rect(cx - 4, sy + dh / 3, 8, 8, SCORE_COLON);
    fill_rect(cx - 4, sy + dh * 2 / 3, 8, 8, SCORE_COLON);
    // P2 score in magenta, right of center
    draw_big_digit(cx + 20, sy, score2, P2_COLOR, scale);
}

fn draw_background() {
    // Fill entire play area with deep field color
    fill_rect(PLAY_LEFT, PLAY_TOP, PLAY_WIDTH, PLAY_HEIGHT, BG_FIELD);

    // Outer glow border (wider, dimmer)
    fill_rect(PLAY_LEFT - 2, PLAY_TOP - 2, PLAY_WIDTH + 4, 6, WALL_GLOW);
    fill_rect(PLAY_LEFT - 2, PLAY_BOTTOM - 4, PLAY_WIDTH + 4, 6, WALL_GLOW);
    fill_rect(PLAY_LEFT - 2, PLAY_TOP - 2, 6, PLAY_HEIGHT + 4, WALL_GLOW);
    fill_rect(PLAY_RIGHT - 4, PLAY_TOP - 2, 6, PLAY_HEIGHT + 4, WALL_GLOW);

    // Inner bright border
    fill_rect(PLAY_LEFT, PLAY_TOP, PLAY_WIDTH, 2, WALL_COLOR);
    fill_rect(PLAY_LEFT, PLAY_BOTTOM - 2, PLAY_WIDTH, 2, WALL_COLOR);
    fill_rect(PLAY_LEFT, PLAY_TOP, 2, PLAY_HEIGHT, WALL_COLOR);
    fill_rect(PLAY_RIGHT - 2, PLAY_TOP, 2, PLAY_HEIGHT, WALL_COLOR);

    // Center dashed line — alternating subtle dots
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2 - 1;
    let mut y = PLAY_TOP + 12;
    while y < PLAY_BOTTOM - 12 {
        fill_rect(cx, y, 2, 6, CENTER_LINE_COLOR);
        y += 18;
    }
}

const GLOW_PAD: i32 = 3; // glow extends 3px beyond element

fn draw_paddle(x: i32, y: i32, player: u8) {
    let (color, glow) = if player == 1 { (P1_COLOR, P1_GLOW) } else { (P2_COLOR, P2_GLOW) };
    // Glow halo
    fill_rect(x - GLOW_PAD, y - GLOW_PAD, PADDLE_WIDTH + GLOW_PAD * 2, PADDLE_HEIGHT + GLOW_PAD * 2, glow);
    // Bright paddle
    fill_rect(x, y, PADDLE_WIDTH, PADDLE_HEIGHT, color);
}

fn erase_paddle(x: i32, y: i32) {
    fill_rect(x - GLOW_PAD, y - GLOW_PAD, PADDLE_WIDTH + GLOW_PAD * 2, PADDLE_HEIGHT + GLOW_PAD * 2, BG_FIELD);
}

fn draw_ball(x: i32, y: i32) {
    // Glow halo
    fill_rect(x - GLOW_PAD, y - GLOW_PAD, BALL_SIZE + GLOW_PAD * 2, BALL_SIZE + GLOW_PAD * 2, BALL_GLOW);
    // Bright core
    fill_rect(x, y, BALL_SIZE, BALL_SIZE, BALL_COLOR);
}

fn draw_ball_trail(x: i32, y: i32, age: u8) {
    let color = if age == 1 { BALL_TRAIL1 } else { BALL_TRAIL2 };
    let shrink = age as i32 * 2;
    let s = BALL_SIZE - shrink;
    if s > 0 {
        fill_rect(x + shrink / 2, y + shrink / 2, s, s, color);
    }
}

fn erase_ball_area(x: i32, y: i32) {
    fill_rect(x - GLOW_PAD, y - GLOW_PAD, BALL_SIZE + GLOW_PAD * 2, BALL_SIZE + GLOW_PAD * 2, BG_FIELD);
}

/// Draw a large 7-segment digit (scaled by `scale` from base 20x30).
fn draw_big_digit(x: i32, y: i32, digit: u32, color: [u8; 4], scale: i32) {
    if digit > 9 {
        return;
    }
    let segs = DIGIT_SEGS[digit as usize];
    let w = DIGIT_W * scale;
    let h = DIGIT_H * scale;
    let half = h / 2;
    let t = SEG_T * scale;
    if segs & (1 << 0) != 0 { fill_rect(x, y, w, t, color); }
    if segs & (1 << 1) != 0 { fill_rect(x + w - t, y, t, half, color); }
    if segs & (1 << 2) != 0 { fill_rect(x + w - t, y + half, t, half, color); }
    if segs & (1 << 3) != 0 { fill_rect(x, y + h - t, w, t, color); }
    if segs & (1 << 4) != 0 { fill_rect(x, y + half, t, half, color); }
    if segs & (1 << 5) != 0 { fill_rect(x, y, t, half, color); }
    if segs & (1 << 6) != 0 { fill_rect(x, y + half - t / 2, w, t, color); }
}

const BANNER_W: i32 = 340;
const BANNER_H: i32 = 220;

/// Draw game over screen with neon banner.
fn draw_game_over(winner: u32) {
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let cy = (PLAY_TOP + PLAY_BOTTOM) / 2;
    let bx = cx - BANNER_W / 2;
    let by = cy - BANNER_H / 2;

    let (color, glow) = if winner == 1 { (P1_COLOR, P1_GLOW) } else { (P2_COLOR, P2_GLOW) };

    // Outer glow
    fill_rect(bx - 4, by - 4, BANNER_W + 8, BANNER_H + 8, glow);
    // Dark overlay banner
    fill_rect(bx, by, BANNER_W, BANNER_H, BG_COLOR);
    // Bright border
    fill_rect(bx, by, BANNER_W, 3, color);
    fill_rect(bx, by + BANNER_H - 3, BANNER_W, 3, color);
    fill_rect(bx, by, 3, BANNER_H, color);
    fill_rect(bx + BANNER_W - 3, by, 3, BANNER_H, color);

    // "P" letter rendered manually
    let scale = 3;
    let pw = DIGIT_W * scale;
    let ph = DIGIT_H * scale;
    let pt = SEG_T * scale;
    let phalf = ph / 2;
    let px = cx - pw - 15;
    let py = cy - phalf;
    fill_rect(px, py, pw, pt, color);                       // top
    fill_rect(px, py, pt, phalf, color);                    // top-left
    fill_rect(px + pw - pt, py, pt, phalf, color);          // top-right
    fill_rect(px, py + phalf - pt / 2, pw, pt, color);     // middle
    fill_rect(px, py + phalf, pt, phalf, color);            // bottom-left

    // Winner digit (1 or 2)
    draw_big_digit(cx + 15, cy - phalf, winner, color, scale);
}

/// Erase the game over banner area.
fn erase_game_over() {
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let cy = (PLAY_TOP + PLAY_BOTTOM) / 2;
    fill_rect(
        cx - BANNER_W / 2 - 4,
        cy - BANNER_H / 2 - 4,
        BANNER_W + 8,
        BANNER_H + 8,
        BG_FIELD,
    );
}

// ─── PRNG ───

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn range(&mut self, min: i32, max: i32) -> i32 {
        let r = self.next();
        let range = max - min;
        if range <= 0 {
            return min;
        }
        min + (r % range as u64) as i32
    }
}

// ─── Ball reset ───

fn reset_ball(state: &mut SharedState, rng: &mut Rng) {
    let center_x = ((PLAY_LEFT + PLAY_RIGHT) / 2) * FP_ONE;
    let center_y = ((PLAY_TOP + PLAY_BOTTOM) / 2) * FP_ONE;
    state.ball_x = center_x;
    state.ball_y = center_y;
    let dir_x = if rng.next() & 1 == 0 { 1 } else { -1 };
    let vy = rng.range(-2 * FP_ONE, 2 * FP_ONE);
    state.ball_vx = BALL_INIT_SPEED * dir_x;
    state.ball_vy = vy;
}

fn init_game(state: &mut SharedState, rng: &mut Rng) {
    let center_y = (PLAY_TOP + PLAY_BOTTOM) / 2 - PADDLE_HEIGHT / 2;
    state.paddle1_y = center_y;
    state.paddle2_y = center_y;
    state.score1 = 0;
    state.score2 = 0;
    state.tick = 0;
    state.game_state = STATE_WAITING;
    state.p1_up = 0;
    state.p1_down = 0;
    state.p2_up = 0;
    state.p2_down = 0;
    reset_ball(state, rng);
}

/// Held-key state for both players.
struct KeyState {
    w: bool,
    s: bool,
    up: bool,
    down: bool,
    any_pressed: bool, // true if any key was pressed this frame
}

impl KeyState {
    fn new() -> Self {
        Self { w: false, s: false, up: false, down: false, any_pressed: false }
    }

    /// Drain all pending keyboard events and update held state.
    /// Kernel returns: keycode for press, keycode|0x80 for release.
    fn update(&mut self) {
        self.any_pressed = false;
        loop {
            let mut buf = [0u8; 1];
            let ret = crate::read(crate::STDIN_BUFFERED, &mut buf);
            if ret <= 0 {
                break;
            }
            let byte = buf[0];
            let released = byte & 0x80 != 0;
            let code = byte & 0x7F;
            let held = !released;
            match code {
                KEY_W => self.w = held,
                KEY_S => self.s = held,
                KEY_UP => self.up = held,
                KEY_DOWN => self.down = held,
                _ => {}
            }
            if held {
                self.any_pressed = true;
            }
        }
    }
}

fn clamp(val: i32, min: i32, max: i32) -> i32 {
    if val < min { min } else if val > max { max } else { val }
}

fn update_physics(state: &mut SharedState, rng: &mut Rng) {
    state.ball_x += state.ball_vx;
    state.ball_y += state.ball_vy;

    let bx = state.ball_x / FP_ONE;
    let by = state.ball_y / FP_ONE;

    // Wall bounce (top/bottom)
    if by <= PLAY_TOP + 4 {
        state.ball_y = (PLAY_TOP + 5) * FP_ONE;
        state.ball_vy = state.ball_vy.abs();
    }
    if by + BALL_SIZE >= PLAY_BOTTOM - 4 {
        state.ball_y = (PLAY_BOTTOM - 5 - BALL_SIZE) * FP_ONE;
        state.ball_vy = -(state.ball_vy.abs());
    }

    // Paddle 1 collision (left)
    if state.ball_vx < 0
        && bx <= PADDLE1_X + PADDLE_WIDTH
        && bx + BALL_SIZE >= PADDLE1_X
        && by + BALL_SIZE >= state.paddle1_y
        && by <= state.paddle1_y + PADDLE_HEIGHT
    {
        state.ball_x = (PADDLE1_X + PADDLE_WIDTH + 1) * FP_ONE;
        state.ball_vx = -(state.ball_vx);
        let hit_pos = (by + BALL_SIZE / 2) - state.paddle1_y;
        let offset = hit_pos - PADDLE_HEIGHT / 2;
        state.ball_vy = offset * 4;
        if state.ball_vx.abs() < BALL_SPEED_CAP {
            state.ball_vx = state.ball_vx + state.ball_vx.signum() * 32;
        }
    }

    // Paddle 2 collision (right)
    if state.ball_vx > 0
        && bx + BALL_SIZE >= PADDLE2_X
        && bx <= PADDLE2_X + PADDLE_WIDTH
        && by + BALL_SIZE >= state.paddle2_y
        && by <= state.paddle2_y + PADDLE_HEIGHT
    {
        state.ball_x = (PADDLE2_X - BALL_SIZE - 1) * FP_ONE;
        state.ball_vx = -(state.ball_vx);
        let hit_pos = (by + BALL_SIZE / 2) - state.paddle2_y;
        let offset = hit_pos - PADDLE_HEIGHT / 2;
        state.ball_vy = offset * 4;
        if state.ball_vx.abs() < BALL_SPEED_CAP {
            state.ball_vx = state.ball_vx + state.ball_vx.signum() * 32;
        }
    }

    // Scoring
    let bx = state.ball_x / FP_ONE;
    if bx < PLAY_LEFT {
        state.score2 += 1;
        if state.score2 >= WIN_SCORE {
            state.game_state = STATE_GAME_OVER;
        } else {
            state.game_state = STATE_POINT_SCORED;
        }
        reset_ball(state, rng);
    } else if bx + BALL_SIZE > PLAY_RIGHT {
        state.score1 += 1;
        if state.score1 >= WIN_SCORE {
            state.game_state = STATE_GAME_OVER;
        } else {
            state.game_state = STATE_POINT_SCORED;
        }
        reset_ball(state, rng);
    }
}

/// Run the Pong game. Called from the pingpong binary.
pub fn run() {
    let (width, height) = fb_info();
    crate::println!(
        "[pingpong] PID={}, framebuffer {}x{}",
        crate::getpid(),
        width,
        height
    );

    let shm_addr = shm_create();
    if shm_addr == usize::MAX {
        crate::println!("[pingpong] ERROR: shm_create failed");
        return;
    }
    crate::println!("[pingpong] shared memory at {:#x}", shm_addr);

    let state = shm_addr as *mut SharedState;
    let state = unsafe { &mut *state };

    let seed = crate::get_time() as u64;
    let mut rng = Rng::new(seed);

    init_game(state, &mut rng);

    // Fork child 1 (player 1 paddle)
    let pid1 = crate::fork();
    if pid1 < 0 {
        crate::println!("[pingpong] ERROR: fork (player 1) failed");
        return;
    }
    if pid1 == 0 {
        player_loop(state, 1);
    }

    // Fork child 2 (player 2 paddle)
    let pid2 = crate::fork();
    if pid2 < 0 {
        crate::println!("[pingpong] ERROR: fork (player 2) failed");
        return;
    }
    if pid2 == 0 {
        player_loop(state, 2);
    }

    // Parent: coordinator (keyboard + physics + rendering)
    crate::println!(
        "[pingpong] parent PID={}, P1 PID={}, P2 PID={}",
        crate::getpid(),
        pid1,
        pid2
    );
    parent_loop(state, &mut rng);

    // Wait for both children
    let mut exit_code: i32 = 0;
    crate::waitpid(pid1 as isize, &mut exit_code);
    crate::waitpid(pid2 as isize, &mut exit_code);
}

/// Player subprocess: reads input flags from shared memory, moves paddle.
fn player_loop(state: &mut SharedState, player: u8) -> ! {
    crate::println!("[pingpong] player {} PID={}", player, crate::getpid());
    loop {
        if player == 1 {
            if state.p1_up != 0 {
                state.paddle1_y -= PADDLE_SPEED;
                state.p1_up = 0;
            }
            if state.p1_down != 0 {
                state.paddle1_y += PADDLE_SPEED;
                state.p1_down = 0;
            }
            state.paddle1_y = clamp(
                state.paddle1_y,
                PLAY_TOP + 4,
                PLAY_BOTTOM - 4 - PADDLE_HEIGHT,
            );
        } else {
            if state.p2_up != 0 {
                state.paddle2_y -= PADDLE_SPEED;
                state.p2_up = 0;
            }
            if state.p2_down != 0 {
                state.paddle2_y += PADDLE_SPEED;
                state.p2_down = 0;
            }
            state.paddle2_y = clamp(
                state.paddle2_y,
                PLAY_TOP + 4,
                PLAY_BOTTOM - 4 - PADDLE_HEIGHT,
            );
        }
        if state.game_state == STATE_GAME_OVER && state.tick == u32::MAX {
            crate::exit(0);
            unreachable!();
        }
        crate::sched_yield();
    }
}

fn parent_loop(state: &mut SharedState, rng: &mut Rng) {
    crate::println!("[pingpong] Press any key on VNC to start!");
    crate::println!("[pingpong] P1: W/S  |  P2: Up/Down arrows");
    // Fill screen outside play area with deep void
    fill_rect(0, 0, 1280, 800, BG_COLOR);
    draw_background();
    draw_score(state.score1, state.score2);
    draw_paddle(PADDLE1_X, state.paddle1_y, 1);
    draw_paddle(PADDLE2_X, state.paddle2_y, 2);
    fb_flush();

    let mut keys = KeyState::new();
    let mut game_over_drawn = false;
    let mut point_scored_drawn = false;
    let mut pause_end_ms: isize = 0;
    let mut last_flash_ms: isize = 0;
    let mut prev_ball_x = state.ball_x / FP_ONE;
    let mut prev_ball_y = state.ball_y / FP_ONE;
    // Ball trail: 2 previous positions
    let mut trail1_x = prev_ball_x;
    let mut trail1_y = prev_ball_y;
    let mut trail2_x = prev_ball_x;
    let mut trail2_y = prev_ball_y;
    let mut prev_paddle1_y = state.paddle1_y;
    let mut prev_paddle2_y = state.paddle2_y;
    let mut prev_score1 = state.score1;
    let mut prev_score2 = state.score2;

    loop {
        keys.update(); // drain all pending keyboard events

        match state.game_state {
            STATE_WAITING => {
                if keys.any_pressed {
                    state.game_state = STATE_PLAYING;
                }
            }
            STATE_PLAYING => {
                // Parent writes held-state to shared memory;
                // child processes read flags and move their paddles
                state.p1_up = keys.w as u8;
                state.p1_down = keys.s as u8;
                state.p2_up = keys.up as u8;
                state.p2_down = keys.down as u8;

                update_physics(state, rng);
            }
            STATE_POINT_SCORED => {
                let now = crate::get_time();
                if !point_scored_drawn {
                    draw_score(state.score1, state.score2);
                    prev_score1 = state.score1;
                    prev_score2 = state.score2;
                    erase_ball_area(prev_ball_x, prev_ball_y);
                    erase_ball_area(trail1_x, trail1_y);
                    erase_ball_area(trail2_x, trail2_y);
                    fb_flush();
                    point_scored_drawn = true;
                    pause_end_ms = now + 3000; // 3 second pause
                    last_flash_ms = now;
                }
                if now < pause_end_ms {
                    // Flash score every 400ms
                    if now - last_flash_ms >= 400 {
                        last_flash_ms = now;
                        // Toggle: erase score area
                        let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
                        let scale = 2;
                        let dw = DIGIT_W * scale;
                        let dh = DIGIT_H * scale;
                        let sy = PLAY_TOP + 15;
                        fill_rect(cx - dw - 30, sy - 2, dw * 2 + 60, dh + 4, BG_FIELD);
                        fb_flush();
                    } else if now - last_flash_ms >= 200 {
                        // Show score (second half of flash cycle)
                        draw_score(state.score1, state.score2);
                        fb_flush();
                    }
                } else {
                    point_scored_drawn = false;
                    state.game_state = STATE_PLAYING;
                    draw_score(state.score1, state.score2);
                    fb_flush();
                    prev_ball_x = state.ball_x / FP_ONE;
                    prev_ball_y = state.ball_y / FP_ONE;
                    trail1_x = prev_ball_x;
                    trail1_y = prev_ball_y;
                    trail2_x = prev_ball_x;
                    trail2_y = prev_ball_y;
                }
            }
            STATE_GAME_OVER => {
                if !game_over_drawn {
                    // Erase ball
                    erase_ball_area(prev_ball_x, prev_ball_y);
                    erase_ball_area(trail1_x, trail1_y);
                    erase_ball_area(trail2_x, trail2_y);
                    // Draw final score
                    draw_score(state.score1, state.score2);
                    let winner = if state.score1 >= WIN_SCORE { 1 } else { 2 };
                    draw_game_over(winner);
                    fb_flush();
                    crate::println!(
                        "[pingpong] Player {} wins! ({}-{}) Press any key to restart.",
                        winner, state.score1, state.score2
                    );
                    game_over_drawn = true;
                }
                if keys.any_pressed {
                    game_over_drawn = false;
                    erase_game_over();
                    init_game(state, rng);
                    draw_background();
                    draw_score(0, 0);
                    draw_paddle(PADDLE1_X, state.paddle1_y, 1);
                    draw_paddle(PADDLE2_X, state.paddle2_y, 2);
                    fb_flush();
                    prev_score1 = 0;
                    prev_score2 = 0;
                    prev_paddle1_y = state.paddle1_y;
                    prev_paddle2_y = state.paddle2_y;
                    prev_ball_x = state.ball_x / FP_ONE;
                    prev_ball_y = state.ball_y / FP_ONE;
                    trail1_x = prev_ball_x;
                    trail1_y = prev_ball_y;
                    trail2_x = prev_ball_x;
                    trail2_y = prev_ball_y;
                }
            }
            _ => {}
        }

        // ─── Rendering (skip during pause/game-over) ───
        if state.game_state == STATE_POINT_SCORED || state.game_state == STATE_GAME_OVER {
            state.tick = state.tick.wrapping_add(1);
            crate::sched_yield();
            continue;
        }
        let ball_x = state.ball_x / FP_ONE;
        let ball_y = state.ball_y / FP_ONE;
        let ball_moved = ball_x != prev_ball_x || ball_y != prev_ball_y;

        if ball_moved {
            // Erase oldest trail
            erase_ball_area(trail2_x, trail2_y);
            // Erase previous trail (now becomes trail2)
            erase_ball_area(trail1_x, trail1_y);
            // Erase current ball position
            erase_ball_area(prev_ball_x, prev_ball_y);

            // Shift trail history
            trail2_x = trail1_x;
            trail2_y = trail1_y;
            trail1_x = prev_ball_x;
            trail1_y = prev_ball_y;
            prev_ball_x = ball_x;
            prev_ball_y = ball_y;
        }

        // Redraw paddles if moved
        if state.paddle1_y != prev_paddle1_y {
            erase_paddle(PADDLE1_X, prev_paddle1_y);
            prev_paddle1_y = state.paddle1_y;
        }
        draw_paddle(PADDLE1_X, state.paddle1_y, 1);

        if state.paddle2_y != prev_paddle2_y {
            erase_paddle(PADDLE2_X, prev_paddle2_y);
            prev_paddle2_y = state.paddle2_y;
        }
        draw_paddle(PADDLE2_X, state.paddle2_y, 2);

        // Draw ball trail + ball on top
        if ball_moved {
            draw_ball_trail(trail2_x, trail2_y, 2);
            draw_ball_trail(trail1_x, trail1_y, 1);
        }
        draw_ball(ball_x, ball_y);

        if state.score1 != prev_score1 || state.score2 != prev_score2 {
            draw_score(state.score1, state.score2);
            prev_score1 = state.score1;
            prev_score2 = state.score2;
        }

        fb_flush();
        state.tick = state.tick.wrapping_add(1);
        crate::sched_yield();
    }
}

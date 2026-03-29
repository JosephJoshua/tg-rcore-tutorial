//! Two-player Pong game using shared memory IPC between two processes.
//!
//! Parent process: player 1 (W/S keys), runs ball physics and rendering.
//! Child process: player 2 (Up/Down keys), writes input to shared memory.

use crate::{fb_info, fb_write, shm_create};

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

// Colors (BGRA format)
const BG_COLOR: [u8; 4] = [0x1A, 0x0D, 0x0D, 0xFF];
const WALL_COLOR: [u8; 4] = [0x4E, 0x3E, 0x2E, 0xFF];
const PADDLE_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const BALL_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];
const CENTER_LINE_COLOR: [u8; 4] = [0x40, 0x40, 0x40, 0xFF];
const SCORE_COLOR_P1: [u8; 4] = [0x44, 0xCC, 0x44, 0xFF];
const SCORE_COLOR_P2: [u8; 4] = [0x44, 0x44, 0xCC, 0xFF];
const MSG_COLOR: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

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
    p2_up: u8,
    p2_down: u8,
}

// ─── Drawing helpers ───

fn fill_rect(x: i32, y: i32, w: i32, h: i32, color: [u8; 4]) {
    if w <= 0 || h <= 0 {
        return;
    }
    const TILE: i32 = 32;
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
    let sy = PLAY_TOP + 10;
    fill_rect(cx - 60, sy, 120, DIGIT_H + 4, BG_COLOR);
    draw_digit(cx - 50, sy, score1, SCORE_COLOR_P1);
    fill_rect(cx - 4, sy + 8, 8, 4, MSG_COLOR);
    fill_rect(cx - 4, sy + 18, 8, 4, MSG_COLOR);
    draw_digit(cx + 30, sy, score2, SCORE_COLOR_P2);
}

fn draw_background() {
    fill_rect(PLAY_LEFT, PLAY_TOP, PLAY_WIDTH, PLAY_HEIGHT, BG_COLOR);
    fill_rect(PLAY_LEFT, PLAY_TOP, PLAY_WIDTH, 4, WALL_COLOR);
    fill_rect(PLAY_LEFT, PLAY_BOTTOM - 4, PLAY_WIDTH, 4, WALL_COLOR);
    fill_rect(PLAY_LEFT, PLAY_TOP, 2, PLAY_HEIGHT, WALL_COLOR);
    fill_rect(PLAY_RIGHT - 2, PLAY_TOP, 2, PLAY_HEIGHT, WALL_COLOR);
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2 - 1;
    let mut y = PLAY_TOP + 10;
    while y < PLAY_BOTTOM - 10 {
        fill_rect(cx, y, 2, 10, CENTER_LINE_COLOR);
        y += 20;
    }
}

fn draw_paddle(x: i32, y: i32) {
    fill_rect(x, y, PADDLE_WIDTH, PADDLE_HEIGHT, PADDLE_COLOR);
}

fn erase_paddle(x: i32, y: i32) {
    fill_rect(x, y, PADDLE_WIDTH, PADDLE_HEIGHT, BG_COLOR);
}

fn draw_ball(x: i32, y: i32) {
    fill_rect(x, y, BALL_SIZE, BALL_SIZE, BALL_COLOR);
}

fn erase_ball(x: i32, y: i32) {
    fill_rect(x, y, BALL_SIZE, BALL_SIZE, BG_COLOR);
}

fn draw_message(msg_len: usize) {
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let cy = (PLAY_TOP + PLAY_BOTTOM) / 2;
    let total_w = msg_len as i32 * 8;
    let sx = cx - total_w / 2;
    fill_rect(sx - 4, cy - 4, total_w + 8, 16, BG_COLOR);
    for i in 0..msg_len {
        fill_rect(sx + i as i32 * 8, cy, 6, 8, MSG_COLOR);
    }
}

fn erase_message_area() {
    let cx = (PLAY_LEFT + PLAY_RIGHT) / 2;
    let cy = (PLAY_TOP + PLAY_BOTTOM) / 2;
    fill_rect(cx - 120, cy - 4, 240, 16, BG_COLOR);
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
    state.p2_up = 0;
    state.p2_down = 0;
    reset_ball(state, rng);
}

fn poll_key() -> u8 {
    let mut buf = [0u8; 1];
    let ret = crate::read(crate::STDIN_BUFFERED, &mut buf);
    if ret > 0 {
        buf[0]
    } else {
        0
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
            state.ball_vx = state.ball_vx - state.ball_vx.signum() * 32;
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

    let pid = crate::fork();
    if pid < 0 {
        crate::println!("[pingpong] ERROR: fork failed");
        return;
    }

    if pid == 0 {
        child_loop(state);
    } else {
        crate::println!(
            "[pingpong] parent PID={}, child PID={}",
            crate::getpid(),
            pid
        );
        parent_loop(state, &mut rng);
        let mut exit_code: i32 = 0;
        crate::waitpid(pid as isize, &mut exit_code);
    }
}

fn child_loop(state: &mut SharedState) -> ! {
    crate::println!("[pingpong] child PID={} (player 2)", crate::getpid());
    loop {
        let key = poll_key();
        match key {
            KEY_UP => {
                state.p2_up = 1;
            }
            KEY_DOWN => {
                state.p2_down = 1;
            }
            _ => {}
        }
        if state.game_state == STATE_GAME_OVER && state.tick == u32::MAX {
            crate::exit(0);
            unreachable!();
        }
        crate::sched_yield();
    }
}

fn parent_loop(state: &mut SharedState, rng: &mut Rng) {
    draw_background();
    draw_score(state.score1, state.score2);
    draw_paddle(PADDLE1_X, state.paddle1_y);
    draw_paddle(PADDLE2_X, state.paddle2_y);
    // "PRESS SPACE TO START" = 20 chars
    draw_message(20);

    let mut prev_ball_x = state.ball_x / FP_ONE;
    let mut prev_ball_y = state.ball_y / FP_ONE;
    let mut prev_paddle1_y = state.paddle1_y;
    let mut prev_paddle2_y = state.paddle2_y;
    let mut prev_score1 = state.score1;
    let mut prev_score2 = state.score2;

    loop {
        let key = poll_key();

        match state.game_state {
            STATE_WAITING => {
                if key == KEY_SPACE {
                    state.game_state = STATE_PLAYING;
                    erase_message_area();
                }
            }
            STATE_PLAYING => {
                match key {
                    KEY_W => {
                        state.paddle1_y -= PADDLE_SPEED;
                    }
                    KEY_S => {
                        state.paddle1_y += PADDLE_SPEED;
                    }
                    _ => {}
                }
                state.paddle1_y = clamp(
                    state.paddle1_y,
                    PLAY_TOP + 4,
                    PLAY_BOTTOM - 4 - PADDLE_HEIGHT,
                );

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

                update_physics(state, rng);
            }
            STATE_POINT_SCORED => {
                if key == KEY_SPACE {
                    state.game_state = STATE_PLAYING;
                    erase_message_area();
                } else {
                    // "POINT! PRESS SPACE" = 18 chars
                    draw_message(18);
                }
            }
            STATE_GAME_OVER => {
                if state.score1 >= WIN_SCORE {
                    // "PLAYER 1 WINS! ENTER" = 21 chars
                    draw_message(21);
                } else {
                    // "PLAYER 2 WINS! ENTER" = 21 chars
                    draw_message(21);
                }
                if key == KEY_ENTER {
                    erase_message_area();
                    init_game(state, rng);
                    draw_background();
                    prev_score1 = 0;
                    prev_score2 = 0;
                }
            }
            _ => {}
        }

        // Incremental rendering
        let ball_x = state.ball_x / FP_ONE;
        let ball_y = state.ball_y / FP_ONE;
        if ball_x != prev_ball_x || ball_y != prev_ball_y {
            erase_ball(prev_ball_x, prev_ball_y);
            draw_ball(ball_x, ball_y);
            prev_ball_x = ball_x;
            prev_ball_y = ball_y;
        }

        if state.paddle1_y != prev_paddle1_y {
            erase_paddle(PADDLE1_X, prev_paddle1_y);
            draw_paddle(PADDLE1_X, state.paddle1_y);
            prev_paddle1_y = state.paddle1_y;
        }

        if state.paddle2_y != prev_paddle2_y {
            erase_paddle(PADDLE2_X, prev_paddle2_y);
            draw_paddle(PADDLE2_X, state.paddle2_y);
            prev_paddle2_y = state.paddle2_y;
        }

        if state.score1 != prev_score1 || state.score2 != prev_score2 {
            draw_score(state.score1, state.score2);
            prev_score1 = state.score1;
            prev_score2 = state.score2;
        }

        state.tick = state.tick.wrapping_add(1);
        crate::sched_yield();
    }
}

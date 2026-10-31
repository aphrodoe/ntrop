//! Shard Stack app state: the current piece falls under gravity (the app's timer) while the player moves and
//! rotates it. Pieces come from a xorshift32 PRNG seeded once from the chip and are never credited.
//! Moves are only accepted where the shape is reachable by a straight fall from the top, so a lock always equals
//! the core's hard-drop landing for (rot, col): the core only models hard-drop placements.

use entropy_core::shard::{rotation_count, shape, spawn_col, SHAPES, W};

pub const BASE_MS: u32 = 700;
pub const STEP_MS: u32 = 40;
pub const FLOOR_MS: u32 = 300;

pub struct Shard {
    pub piece: u8,
    pub rot: u8,
    pub col: u8,
    pub row: u8,
    pub next: u8,
    pub lines: u32,
    /// Milliseconds since the piece last fell a row.
    pub acc: u32,
    rng: u32,
}

impl Shard {
    pub fn new(seed: u32) -> Self {
        let mut s = Self { piece: 0, rot: 0, col: 0, row: 0, next: 0, lines: 0, acc: 0, rng: seed.max(1) };
        s.next = s.draw();
        s.next_piece();
        s
    }

    fn draw(&mut self) -> u8 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        (x >> 8) as u8 % SHAPES.len() as u8
    }

    /// Promotes the preview piece to the current one, centred at the top.
    pub fn next_piece(&mut self) {
        self.piece = self.next;
        self.next = self.draw();
        self.rot = 0;
        self.col = spawn_col(self.piece, 0);
        self.row = 0;
        self.acc = 0;
    }

    /// If the centred spawn is blocked, moves to the nearest reachable (rot, col) at the top.
    pub fn settle_spawn(&mut self, fits: &dyn Fn(u8, u8, u8) -> bool) {
        if fits(self.rot, self.col, 0) { return; }
        let c0 = self.col as i32;
        let best = (0..rotation_count(self.piece)).flat_map(|r| (0..W).map(move |c| (r, c)))
            .filter(|&(r, c)| fits(r, c, 0)).min_by_key(|&(r, c)| ((c as i32 - c0).abs(), r));
        if let Some((r, c)) = best { self.rot = r; self.col = c; }
    }

    fn width(&self, rot: u8) -> u8 {
        shape(self.piece, rot).iter().map(|c| c.0).max().unwrap_or(0) + 1
    }

    /// Moves one column at a time, stopping at the first blocked step.
    pub fn shift(&mut self, d: i32, fits: &dyn Fn(u8, u8, u8) -> bool) {
        for _ in 0..d.abs() {
            let c = self.col as i32 + d.signum();
            if c < 0 || c > (W - self.width(self.rot)) as i32 || !fits(self.rot, c as u8, self.row) { break; }
            self.col = c as u8;
        }
    }

    pub fn rotate(&mut self, fits: &dyn Fn(u8, u8, u8) -> bool) {
        let rot = (self.rot + 1) % rotation_count(self.piece);
        let col = self.col.min(W - self.width(rot));
        if fits(rot, col, self.row) { self.rot = rot; self.col = col; }
    }

    /// Milliseconds per row at the current line count.
    pub fn interval_ms(&self) -> u32 {
        BASE_MS.saturating_sub(STEP_MS * self.lines).max(FLOOR_MS)
    }

    /// Cells (8x16 row-major) of the piece at its current position.
    pub fn cells(&self) -> [u8; 128] {
        let mut o = [0u8; 128];
        for (x, y) in shape(self.piece, self.rot) {
            let (c, r) = ((self.col + x) as usize, (self.row + y) as usize);
            if c < 8 && r < 16 { o[r * 8 + c] = 1; }
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn stays_in_bounds() {
        let open = |_: u8, _: u8, _: u8| true;
        let mut s = Shard::new(7);
        for i in 0..200 { s.shift(if i % 3 == 0 { -5 } else { 2 }, &open); s.rotate(&open); s.next_piece(); assert!(s.col + s.width(s.rot) <= W); }
    }
    #[test] fn blocked_moves_are_refused() {
        let wall = |_: u8, c: u8, _: u8| c < 3;
        let mut s = Shard::new(3); s.col = 0; s.shift(5, &wall); assert!(s.col < 3);
    }
    #[test] fn gravity_speeds_up_with_lines_to_a_floor() {
        let mut s = Shard::new(1); assert_eq!(s.interval_ms(), 700); s.lines = 3; assert_eq!(s.interval_ms(), 580); s.lines = 99; assert_eq!(s.interval_ms(), 300);
    }
}

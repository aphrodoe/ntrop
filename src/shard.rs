//! Shard Stack app state: the falling piece waits at the top (no gravity) while the player picks column and
//! rotation. Pieces come from a xorshift32 PRNG seeded once from the chip and are never credited.

use entropy_core::shard::{rotation_count, shape, SHAPES, W};

pub struct Shard {
    pub piece: u8,
    pub rot: u8,
    pub col: u8,
    rng: u32,
}

impl Shard {
    pub fn new(seed: u32) -> Self {
        let mut s = Self { piece: 0, rot: 0, col: 0, rng: seed.max(1) };
        s.next_piece();
        s
    }

    pub fn next_piece(&mut self) {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        self.piece = (x >> 8) as u8 % SHAPES.len() as u8;
        self.rot = 0;
        self.col = (W - self.width()) / 2;
    }

    fn width(&self) -> u8 {
        shape(self.piece, self.rot).iter().map(|c| c.0).max().unwrap_or(0) + 1
    }

    pub fn shift(&mut self, d: i32) {
        self.col = (self.col as i32 + d).clamp(0, (W - self.width()) as i32) as u8;
    }

    pub fn rotate(&mut self) {
        self.rot = (self.rot + 1) % rotation_count(self.piece);
        self.col = self.col.min(W - self.width());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn stays_in_bounds() { let mut s = Shard::new(7); for i in 0..200 { s.shift(if i % 3 == 0 { -5 } else { 2 }); s.rotate(); s.next_piece(); assert!(s.col + s.width() <= W); } }
}

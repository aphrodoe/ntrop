//! Glitch Glider flight physics and course. Nothing here is secret and nothing is random:
//! the course is fixed so timing is the only thing the player can vary (spec Goal 12).

pub const W: f32 = 480.0;
pub const H: f32 = 480.0;
pub const SPARK_X: f32 = 120.0;
pub const PILLAR_W: f32 = 56.0;
pub const SPACING: f32 = 240.0;
pub const GAP: f32 = 170.0;
pub const GAP_Y: f32 = H / 2.0;
const SPEED: f32 = 120.0; // px/s
const GRAVITY: f32 = 900.0; // px/s^2
const PULSE: f32 = -330.0; // px/s per tap
const R: f32 = 10.0;

#[derive(Default)]
pub struct Glide {
    pub y: f32,
    vy: f32,
    scroll: f32,
}

impl Glide {
    pub fn new() -> Self {
        Self { y: GAP_Y, vy: 0.0, scroll: 0.0 }
    }

    pub fn tap(&mut self) {
        self.vy = PULSE;
    }

    pub fn tick(&mut self, ms: u32) {
        let dt = ms as f32 / 1000.0;
        self.vy += GRAVITY * dt;
        self.y += self.vy * dt;
        self.scroll += SPEED * dt;
        let hit_wall = self.y < R || self.y > H - R;
        let px = self.pillars().into_iter().find(|&px| px < SPARK_X + R && px + PILLAR_W > SPARK_X - R);
        if hit_wall || (px.is_some() && (self.y - GAP_Y).abs() > GAP / 2.0 - R) {
            *self = Self { scroll: self.scroll, ..Self::new() }; // no game over, no bits lost
        }
    }

    /// Left edges of the pillars currently on screen.
    pub fn pillars(&self) -> Vec<f32> {
        let off = self.scroll % SPACING;
        (0..3).map(|k| k as f32 * SPACING - off + SPACING / 2.0).filter(|&x| x > -PILLAR_W && x < W).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn falls_and_resets_without_panic() { let mut g = Glide::new(); for _ in 0..2000 { g.tick(16); } assert!(g.y >= 0.0 && g.y <= H); g.tap(); g.tick(16); assert!(g.y < H); }
    #[test] fn course_is_deterministic() { let (mut a, mut b) = (Glide::new(), Glide::new()); for _ in 0..500 { a.tick(16); b.tick(16); } assert_eq!(a.pillars(), b.pillars()); }
}

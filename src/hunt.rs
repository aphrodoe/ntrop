//! Static Hunt: first-person raycast of a 16x16 room of animated static, with violet ghosts.
//! Holds only non-secret game state; miss offsets go straight to the session and are never kept.

use slint_keyos_platform::slint::{Rgb8Pixel, SharedPixelBuffer};

pub const W: u32 = 240;
pub const H: u32 = 288;
const FOV: f32 = 60.0 * std::f32::consts::PI / 180.0;
const DEG_PER_PX: f32 = 0.5;
const SPAWN_DIST: f32 = 8.0;
const MAX_GHOSTS: usize = 3;
const SPAWN_CD_MS: i32 = 800;
const BURST_MS: u32 = 150;
const TAU: f32 = std::f32::consts::TAU;
/// Game violet #8B5CFF, used for the ghost halo.
const GHOST_RGB: (u32, u32, u32) = (0x8B, 0x5C, 0xFF);

struct Ghost {
    angle: f32,
    dist: f32,
}

pub struct Hunt {
    angle: f32,
    ghosts: Vec<Ghost>,
    spawn_cd: i32,
    bursts: Vec<(i32, i32, u32)>,
    rng: u32,
}

fn wall(x: i32, y: i32) -> bool {
    !(1..15).contains(&x) || !(1..15).contains(&y) || matches!((x, y), (4, 4) | (11, 5) | (5, 11) | (11, 11))
}

impl Hunt {
    pub fn new(seed: u32) -> Self {
        let mut h = Self { angle: 0.0, ghosts: Vec::new(), spawn_cd: 0, bursts: Vec::new(), rng: seed.max(1) };
        for _ in 0..MAX_GHOSTS {
            h.spawn();
        }
        h
    }

    fn next(&mut self) -> u32 {
        let mut s = self.rng;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.rng = s;
        s
    }

    fn spawn(&mut self) {
        let angle = (self.next() >> 8) as f32 / (1u32 << 24) as f32 * TAU;
        self.ghosts.push(Ghost { angle, dist: SPAWN_DIST });
    }

    pub fn turn(&mut self, dx_px: i32) {
        self.angle = (self.angle + dx_px as f32 * DEG_PER_PX * TAU / 360.0).rem_euclid(TAU);
    }

    pub fn tick(&mut self, dt_ms: u32) {
        let dt = dt_ms as f32 / 1000.0;
        for g in &mut self.ghosts {
            g.dist -= dt; // 1 unit/s
        }
        self.ghosts.retain(|g| g.dist > 0.3); // reached you: fades out, no penalty
        for b in &mut self.bursts {
            b.2 = b.2.saturating_sub(dt_ms);
        }
        self.bursts.retain(|b| b.2 > 0);
        self.spawn_cd -= dt_ms as i32;
        if self.ghosts.len() < MAX_GHOSTS && self.spawn_cd <= 0 {
            self.spawn();
            self.spawn_cd = SPAWN_CD_MS;
        }
    }

    /// Screen centre and half-width of a ghost, or None when outside the view.
    fn project(&self, g: &Ghost) -> Option<(i32, i32, i32)> {
        let rel = (g.angle - self.angle + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
        if rel.abs() > FOV / 2.0 {
            return None;
        }
        let cx = ((rel / FOV + 0.5) * W as f32) as i32;
        let hw = ((H as f32 * 0.3 / g.dist) as i32).clamp(4, 110);
        Some((cx, H as i32 / 2, hw))
    }

    /// Miss offset from the nearest on-screen ghost centre (None: no ghost on screen). A shot inside the half-width bursts it.
    pub fn tap(&mut self, x: i32, y: i32) -> Option<(i32, i32)> {
        let (i, (cx, cy, hw)) = self
            .ghosts
            .iter()
            .enumerate()
            .filter_map(|(i, g)| self.project(g).map(|p| (i, p)))
            .min_by_key(|&(_, (cx, cy, _))| (x - cx).pow(2) + (y - cy).pow(2))?;
        let (dx, dy) = (x - cx, y - cy);
        if dx * dx + dy * dy <= hw * hw {
            self.ghosts.remove(i);
            self.bursts.push((cx, cy, BURST_MS));
        }
        Some((dx, dy))
    }

    pub fn render(&mut self, buf: &mut SharedPixelBuffer<Rgb8Pixel>) {
        let px = buf.make_mut_slice();
        let mut s = self.next() | 1;
        let mut noise = || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            s >> 24
        };
        let (w, h) = (W as usize, H as usize);
        let mid = h as i32 / 2;
        for x in 0..w {
            let ray = self.angle + ((x as f32 + 0.5) / W as f32 - 0.5) * FOV;
            let (dx, dy) = (ray.cos(), ray.sin());
            let (mut mx, mut my) = (8i32, 8i32);
            let (ddx, ddy) = (if dx == 0.0 { 1e9 } else { (1.0 / dx).abs() }, if dy == 0.0 { 1e9 } else { (1.0 / dy).abs() });
            let (sx, mut sdx) = if dx < 0.0 { (-1, (8.0 - mx as f32) * ddx) } else { (1, (mx as f32 + 1.0 - 8.0) * ddx) };
            let (sy, mut sdy) = if dy < 0.0 { (-1, (8.0 - my as f32) * ddy) } else { (1, (my as f32 + 1.0 - 8.0) * ddy) };
            let mut side = false;
            let mut dist = 1.0;
            for _ in 0..32 {
                if sdx < sdy {
                    dist = sdx;
                    sdx += ddx;
                    mx += sx;
                    side = false;
                } else {
                    dist = sdy;
                    sdy += ddy;
                    my += sy;
                    side = true;
                }
                if wall(mx, my) {
                    break;
                }
            }
            let perp = (dist * (ray - self.angle).cos()).max(0.1);
            let half = ((H as f32 * 0.5 / perp) as i32).min(mid);
            let shade = (256.0 / (1.0 + perp * 0.35)) as u32 * if side { 3 } else { 4 } / 4;
            for y in 0..h {
                let n = noise();
                let (r, g, b) = if (y as i32 - mid).abs() < half {
                    let v = n * shade >> 8;
                    (v * 6 / 10, v * 35 / 100, v)
                } else {
                    let v = n >> 4; // faint static floor and ceiling
                    (v / 2, v / 3, v)
                };
                px[y * w + x] = Rgb8Pixel { r: r as u8, g: g as u8, b: b as u8 };
            }
        }
        let mut order: Vec<&Ghost> = self.ghosts.iter().collect();
        order.sort_by(|a, b| b.dist.total_cmp(&a.dist)); // far first
        for g in order {
            let Some((cx, cy, hw)) = self.project(g) else { continue };
            let hh = hw * 14 / 10;
            let fade = (g.dist / 1.5).min(1.0);
            let (gw, gh) = (hw * 5 / 4, hh * 5 / 4); // halo margin
            for dy in -gh..=gh {
                for dx in -gw..=gw {
                    let (nx, ny) = (dx as f32 / hw as f32, dy as f32 / hh as f32);
                    let inside = if ny < 0.0 { nx * nx + ny * ny <= 1.0 } else { nx.abs() <= 1.0 && ny <= 0.9 + 0.1 * (nx * 9.0).sin() };
                    let (px_x, px_y) = (cx + dx, cy + dy);
                    if px_x < 0 || px_y < 0 || px_x >= W as i32 || px_y >= H as i32 {
                        continue;
                    }
                    if !inside {
                        if nx * nx + ny * ny <= 1.5625 {
                            let f = (fade * 90.0) as u32; // dim halo
                            px[px_y as usize * w + px_x as usize] = Rgb8Pixel { r: (GHOST_RGB.0 * f >> 8) as u8, g: (GHOST_RGB.1 * f >> 8) as u8, b: (GHOST_RGB.2 * f >> 8) as u8 };
                        }
                        continue;
                    }
                    let eye = ny > -0.5 && ny < -0.1 && (nx.abs() - 0.4).abs() < 0.14;
                    let n = noise();
                    let (r, g2, b) = if eye { (0, 0, 0) } else { (90 + n * 4 / 10, 40 + n / 5, 200 + n / 5) };
                    let f = (fade * 256.0) as u32;
                    px[px_y as usize * w + px_x as usize] = Rgb8Pixel { r: (r * f >> 8) as u8, g: (g2 * f >> 8) as u8, b: (b * f >> 8) as u8 };
                }
            }
        }
        for &(cx, cy, ttl) in &self.bursts {
            let rad = 8.0 + (BURST_MS - ttl) as f32 / BURST_MS as f32 * 50.0;
            let r = rad as i32 + 4;
            for dy in -r..=r {
                for dx in -r..=r {
                    let d = ((dx * dx + dy * dy) as f32).sqrt();
                    let (px_x, px_y) = (cx + dx, cy + dy);
                    if (d - rad).abs() < 4.0 && px_x >= 0 && px_y >= 0 && px_x < W as i32 && px_y < H as i32 {
                        px[px_y as usize * w + px_x as usize] = Rgb8Pixel { r: 255, g: 255, b: 255 };
                    }
                }
            }
        }
    }
}

//! Source health tests, simplified from NIST SP 800-90B (repetition count + adaptive proportion).
//! Unhealthy samples are still hashed into the pool; they just earn no credit.

/// Consecutive bad touch deltas before the touch source is flagged unhealthy.
pub const TOUCH_RCT_LIMIT: u32 = 8;
/// Adaptive-proportion window (touch deltas).
pub const TOUCH_APT_WINDOW: usize = 64;
/// A delta occurring more than this many times in the window flags the source.
pub const TOUCH_APT_MAX: usize = 32;
/// Consecutive stuck camera frames before the camera source is flagged unhealthy.
pub const CAM_STUCK_LIMIT: u32 = 5;
/// Minimum mean |Δ| of sampled low nibbles between frames for a frame to count as live.
pub const CAM_MIN_MEAN_DIFF: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Good,
    Bad,
    Unhealthy,
}

use std::collections::VecDeque;

use sha2::{Digest, Sha256};

#[derive(Default)]
pub struct TouchHealth {
    prev: Option<(i32, i32)>,
    bad_run: u32,
    window: VecDeque<(i32, i32)>,
}

impl TouchHealth {
    pub fn check(&mut self, dx: i32, dy: i32) -> Verdict {
        let d = (dx, dy);
        let bad = d == (0, 0) || self.prev == Some(d);
        self.prev = Some(d);

        if self.window.len() == TOUCH_APT_WINDOW {
            self.window.pop_front();
        }
        self.window.push_back(d);
        // ponytail: O(64) scan per sample; fine at touch rates.
        let dominant = self.window.iter().filter(|&&w| w == d).count() > TOUCH_APT_MAX;

        self.bad_run = if bad { self.bad_run + 1 } else { 0 };
        if dominant || self.bad_run >= TOUCH_RCT_LIMIT {
            Verdict::Unhealthy
        } else if bad {
            Verdict::Bad
        } else {
            Verdict::Good
        }
    }
}

#[derive(Default)]
pub struct CameraHealth {
    prev_hash: Option<[u8; 32]>,
    prev_nibbles: Vec<u8>,
    stuck_run: u32,
}

impl CameraHealth {
    pub fn check(&mut self, frame: &[u8]) -> Verdict {
        let hash: [u8; 32] = Sha256::digest(frame).into();
        let nibbles: Vec<u8> = frame.iter().step_by(16).map(|b| b & 0x0f).collect();
        let prev_hash = self.prev_hash.replace(hash);
        let prev_nibbles = std::mem::replace(&mut self.prev_nibbles, nibbles);
        if prev_hash.is_none() {
            return Verdict::Bad; // nothing to compare against yet
        }

        let n = prev_nibbles.len().min(self.prev_nibbles.len()).max(1);
        let diff: u32 = prev_nibbles
            .iter()
            .zip(&self.prev_nibbles)
            .map(|(a, b)| a.abs_diff(*b) as u32)
            .sum();
        let stuck = prev_hash == Some(hash) || (diff as f32 / n as f32) < CAM_MIN_MEAN_DIFF;

        self.stuck_run = if stuck { self.stuck_run + 1 } else { 0 };
        if self.stuck_run >= CAM_STUCK_LIMIT {
            Verdict::Unhealthy
        } else if stuck {
            Verdict::Bad
        } else {
            Verdict::Good
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn noisy_frame(seed: u32) -> Vec<u8> {
        let mut s = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..4096)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                (s >> 16) as u8
            })
            .collect()
    }

    #[test]
    fn touch_zero_delta_bad() {
        assert_eq!(TouchHealth::default().check(0, 0), Verdict::Bad);
    }

    #[test]
    fn touch_repeat_delta_bad() {
        let mut h = TouchHealth::default();
        assert_eq!(h.check(3, 4), Verdict::Good);
        assert_eq!(h.check(3, 4), Verdict::Bad);
    }

    #[test]
    fn touch_unhealthy_after_8_bad() {
        let mut h = TouchHealth::default();
        for _ in 0..TOUCH_RCT_LIMIT - 1 {
            assert_eq!(h.check(0, 0), Verdict::Bad);
        }
        assert_eq!(h.check(0, 0), Verdict::Unhealthy);
    }

    #[test]
    fn touch_recovers_after_unhealthy() {
        let mut h = TouchHealth::default();
        for _ in 0..TOUCH_RCT_LIMIT {
            h.check(0, 0);
        }
        assert_eq!(h.check(0, 0), Verdict::Unhealthy);
        assert_eq!(h.check(4, -2), Verdict::Good);
    }

    #[test]
    fn touch_apt_flags_dominant_delta() {
        // A,A,distinct repeated: never 8 bad in a row, but A fills > half the window.
        let mut h = TouchHealth::default();
        let mut flagged = false;
        for i in 0..30 {
            flagged |= h.check(5, 5) == Verdict::Unhealthy;
            flagged |= h.check(5, 5) == Verdict::Unhealthy;
            assert_ne!(h.check(100 + i, 1), Verdict::Unhealthy);
        }
        assert!(flagged);
    }

    #[test]
    fn camera_identical_frame_bad() {
        let mut h = CameraHealth::default();
        let f = noisy_frame(1);
        h.check(&noisy_frame(0));
        assert_eq!(h.check(&f), Verdict::Good);
        assert_eq!(h.check(&f), Verdict::Bad);
    }

    #[test]
    fn camera_unhealthy_after_5_stuck() {
        let mut h = CameraHealth::default();
        let f = noisy_frame(7);
        h.check(&f);
        for _ in 0..CAM_STUCK_LIMIT - 1 {
            assert_eq!(h.check(&f), Verdict::Bad);
        }
        assert_eq!(h.check(&f), Verdict::Unhealthy);
    }

    #[test]
    fn camera_flat_frames_bad() {
        // Different frames whose sampled low nibbles barely move: a frozen/static image with tiny drift.
        let mut h = CameraHealth::default();
        let mut f = vec![0x40u8; 4096];
        h.check(&f);
        f[1] = 0x41; // not a sampled byte: hash changes, sampled nibbles don't
        assert_eq!(h.check(&f), Verdict::Bad);
    }

    #[test]
    fn camera_noisy_frames_good() {
        let mut h = CameraHealth::default();
        assert_eq!(h.check(&noisy_frame(0)), Verdict::Bad); // no previous frame yet
        for i in 1..20 {
            assert_eq!(h.check(&noisy_frame(i)), Verdict::Good);
        }
    }
}

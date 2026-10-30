//! Entropy pool: absorbs every sample, credits conservative bits, mixes with hardware randomness.

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

use crate::health::{CameraHealth, TouchHealth, Verdict};
use crate::WordCount;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleResult {
    Credited(u32),
    Uncredited,
    SourceUnhealthy,
}

const TAG_TOUCH: u8 = 0x01;
const TAG_CAMERA: u8 = 0x02;
const DOMAIN: &[u8] = b"entropy-arcade/v1";
const TOUCH_BITS_PER_SAMPLE: u32 = 1;
const CAMERA_BITS_PER_FRAME: u32 = 4;
/// Camera frames earn credit only within this window after a touch sample.
const CAMERA_TOUCH_WINDOW_US: u64 = 250_000;

pub struct EntropyPool {
    wc: WordCount,
    /// Under "don't trust the chip", this running digest is the secret: wiped in `wipe()` / on drop.
    hasher: Sha256,
    last_touch: Option<(i32, i32)>,
    last_touch_us: Option<u64>,
    touch_health: TouchHealth,
    camera_health: CameraHealth,
    touch_bits: u32,
    camera_bits: u32,
    touch_ok: bool,
    camera_ok: bool,
}

impl EntropyPool {
    pub fn new(wc: WordCount) -> Self {
        Self {
            wc,
            hasher: Sha256::new(),
            last_touch: None,
            last_touch_us: None,
            touch_health: TouchHealth::default(),
            camera_health: CameraHealth::default(),
            touch_bits: 0,
            camera_bits: 0,
            touch_ok: true,
            camera_ok: true,
        }
    }

    /// Every sample is hashed in, healthy or not: `tag ‖ len(u32 LE) ‖ bytes`.
    fn absorb(&mut self, tag: u8, bytes: &[u8]) {
        self.hasher.update([tag]);
        self.hasher.update((bytes.len() as u32).to_le_bytes());
        self.hasher.update(bytes);
    }

    pub fn add_touch(&mut self, x: i32, y: i32, t_us: u64) -> SampleResult {
        let mut raw = [0u8; 16];
        raw[..4].copy_from_slice(&x.to_le_bytes());
        raw[4..8].copy_from_slice(&y.to_le_bytes());
        raw[8..].copy_from_slice(&t_us.to_le_bytes());
        self.absorb(TAG_TOUCH, &raw);
        raw.zeroize();

        let Some((px, py)) = self.last_touch.replace((x, y)) else {
            return SampleResult::Uncredited;
        };
        match self.touch_health.check(x - px, y - py) {
            Verdict::Good => {
                self.touch_ok = true;
                self.last_touch_us = Some(t_us); // only healthy touch counts as "drawing" for the camera
                self.touch_bits += TOUCH_BITS_PER_SAMPLE;
                SampleResult::Credited(TOUCH_BITS_PER_SAMPLE)
            }
            Verdict::Bad => SampleResult::Uncredited,
            Verdict::Unhealthy => {
                self.touch_ok = false;
                SampleResult::SourceUnhealthy
            }
        }
    }

    pub fn add_camera_frame(&mut self, frame: &[u8], t_us: u64) -> SampleResult {
        self.absorb(TAG_CAMERA, frame);
        match self.camera_health.check(frame) {
            Verdict::Good => self.camera_ok = true,
            Verdict::Bad => return SampleResult::Uncredited,
            Verdict::Unhealthy => {
                self.camera_ok = false;
                return SampleResult::SourceUnhealthy;
            }
        }
        let drawing = self
            .last_touch_us
            .is_some_and(|t| t_us.saturating_sub(t) <= CAMERA_TOUCH_WINDOW_US);
        let room = (self.wc.target_bits() / 2).saturating_sub(self.camera_bits);
        let bits = CAMERA_BITS_PER_FRAME.min(room);
        if !drawing || bits == 0 {
            return SampleResult::Uncredited;
        }
        self.camera_bits += bits;
        SampleResult::Credited(bits)
    }

    /// Zeroizes all pool state and resets to a fresh pool of the same word count.
    pub fn wipe(&mut self) {
        // SAFETY: Sha256 is plain fixed-size data (state words + block buffer), no pointers or Drop.
        unsafe { zeroize::zeroize_flat_type(&mut self.hasher) };
        if let Some((x, y)) = self.last_touch.as_mut() {
            x.zeroize();
            y.zeroize();
        }
        self.touch_health.wipe();
        self.camera_health.wipe();
        // Field-wise reset: `*self = Self::new(..)` would drop the old pool and recurse through Drop.
        self.hasher = Sha256::new();
        self.last_touch = None;
        self.last_touch_us = None;
        self.touch_bits = 0;
        self.camera_bits = 0;
        self.touch_ok = true;
        self.camera_ok = true;
    }

    pub fn touch_bits(&self) -> u32 {
        self.touch_bits
    }

    pub fn camera_bits(&self) -> u32 {
        self.camera_bits
    }

    pub fn credited_bits(&self) -> u32 {
        (self.touch_bits + self.camera_bits).min(self.wc.target_bits())
    }

    pub fn is_full(&self) -> bool {
        self.credited_bits() >= self.wc.target_bits()
    }

    pub fn touch_healthy(&self) -> bool {
        self.touch_ok
    }

    pub fn camera_healthy(&self) -> bool {
        self.camera_ok
    }

    /// `SHA-256(DOMAIN ‖ hw_random ‖ pool_digest)`, truncated to the word count's entropy length.
    pub fn finish(mut self, hw_random: &[u8; 32]) -> Zeroizing<Vec<u8>> {
        let pool_digest = Zeroizing::new(self.hasher.finalize_reset());
        let mut h = Sha256::new();
        h.update(DOMAIN);
        h.update(hw_random);
        h.update(&*pool_digest);
        let mut out = Zeroizing::new(h.finalize().to_vec());
        out.truncate(self.wc.entropy_len());
        out
    }
}

impl Drop for EntropyPool {
    fn drop(&mut self) {
        self.wipe();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `n + 1` touch samples (a pseudo-random walk, so deltas never repeat) from t_us = `t0`.
    fn scribble(p: &mut EntropyPool, n: u32, t0: u64) {
        for i in 0..=n {
            // murmur3 finalizer: a plain multiply is linear, so its deltas would repeat
            let mut h = (i ^ (t0 as u32)).wrapping_mul(0x9e37_79b1);
            h ^= h >> 15;
            h = h.wrapping_mul(0x85eb_ca6b);
            h ^= h >> 13;
            p.add_touch((h % 480) as i32, ((h >> 12) % 800) as i32, t0 + i as u64 * 16_000);
        }
    }

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
    fn finish_is_spec_mix() {
        let p = EntropyPool::new(WordCount::TwentyFour);
        let hw = [7u8; 32];
        let empty_digest = Sha256::new().finalize();
        let mut h = Sha256::new();
        h.update(b"entropy-arcade/v1");
        h.update(hw);
        h.update(empty_digest);
        assert_eq!(&p.finish(&hw)[..], &h.finalize()[..]);
    }

    #[test]
    fn twelve_words_truncates_to_16() {
        assert_eq!(EntropyPool::new(WordCount::Twelve).finish(&[1; 32]).len(), 16);
    }

    #[test]
    fn touch_credits_one_bit() {
        let mut p = EntropyPool::new(WordCount::TwentyFour);
        assert_eq!(p.add_touch(0, 0, 0), SampleResult::Uncredited);
        scribble(&mut p, 9, 16_000); // 10 more samples
        assert_eq!(p.touch_bits(), 10);
    }

    #[test]
    fn different_input_different_output() {
        let hw = [9u8; 32];
        let a = EntropyPool::new(WordCount::TwentyFour);
        let mut b = EntropyPool::new(WordCount::TwentyFour);
        b.add_touch(1, 2, 3);
        assert_ne!(a.finish(&hw), b.finish(&hw));
    }

    #[test]
    fn camera_needs_recent_touch() {
        let mut p = EntropyPool::new(WordCount::TwentyFour);
        p.add_camera_frame(&noisy_frame(1), 0);
        p.add_camera_frame(&noisy_frame(2), 10_000);
        assert_eq!(p.camera_bits(), 0);
        p.add_touch(5, 5, 90_000); // first sample: uncredited, not "drawing"
        p.add_touch(9, 12, 100_000);
        p.add_camera_frame(&noisy_frame(3), 300_000); // 200 ms after a credited touch
        assert_eq!(p.camera_bits(), 4);
        p.add_camera_frame(&noisy_frame(4), 400_000); // 300 ms after touch
        assert_eq!(p.camera_bits(), 4);
    }

    #[test]
    fn camera_capped_at_half() {
        let mut p = EntropyPool::new(WordCount::Twelve);
        for i in 0..200u32 {
            let t = i as u64 * 20_000;
            scribble(&mut p, 0, t);
            p.add_camera_frame(&noisy_frame(i), t + 1_000);
        }
        assert_eq!(p.camera_bits(), 64);
    }

    #[test]
    fn camera_no_credit_while_touch_unhealthy() {
        let mut p = EntropyPool::new(WordCount::TwentyFour);
        for i in 0..100u32 {
            let t = i as u64 * 20_000;
            p.add_touch(200, 300, t); // finger held on one spot
            p.add_camera_frame(&noisy_frame(i), t + 1_000);
        }
        assert!(!p.touch_healthy());
        assert_eq!(p.camera_bits(), 0);
    }

    #[test]
    fn wipe_resets_pool() {
        let hw = [5u8; 32];
        let mut p = EntropyPool::new(WordCount::TwentyFour);
        scribble(&mut p, 50, 0);
        p.wipe();
        assert_eq!(p.credited_bits(), 0);
        assert_eq!(p.finish(&hw), EntropyPool::new(WordCount::TwentyFour).finish(&hw));
    }

    #[test]
    fn full_at_target() {
        let mut p = EntropyPool::new(WordCount::Twelve);
        scribble(&mut p, 127, 0);
        assert!(!p.is_full());
        scribble(&mut p, 1, 10_000_000);
        assert!(p.is_full());
        assert_eq!(p.credited_bits(), 128);
    }
}

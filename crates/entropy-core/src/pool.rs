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
const TAG_OUTGUESS: u8 = 0x03;
const TAG_SHOT: u8 = 0x04;
// 0x05 is retired (a removed game used it): never reuse it.
const TAG_SHARD: u8 = 0x06;
const SHOT_MBITS: u32 = 1000;
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
    shot_health: TouchHealth,
    shot_tap_health: TouchHealth,
    last_shot_tap: Option<(i32, i32)>,
    touch_bits: u32,
    camera_bits: u32,
    touch_ok: bool,
    camera_ok: bool,
    /// Outguess credit in millibits (fractional bits accumulate).
    game_mbits: u32,
    game_ok: bool,
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
            shot_health: TouchHealth::default(),
            shot_tap_health: TouchHealth::default(),
            last_shot_tap: None,
            touch_bits: 0,
            camera_bits: 0,
            touch_ok: true,
            camera_ok: true,
            game_mbits: 0,
            game_ok: true,
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
        match self.touch_health.check(x.wrapping_sub(px), y.wrapping_sub(py)) {
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

    /// Absorbs a door pick (`door ‖ x ‖ y ‖ t_us`) and adds the oracle's credit for it.
    pub fn add_outguess(&mut self, door: u8, x: i32, y: i32, t_us: u64, credit_mbits: u32, healthy: bool) {
        self.add_game(TAG_OUTGUESS, Some(door), x, y, t_us, credit_mbits, healthy);
    }

    /// Absorbs both players atomically, under distinct player/role markers.
    /// Only door-choice credit is counted; tap coordinates and timing are mixed only.
    pub fn add_duel_round(
        &mut self,
        hider: u8,
        doors: [u8; 2],
        taps: [crate::outguess::DuelTap; 2],
        credit_mbits: [u32; 2],
        healthy: [bool; 2],
    ) {
        for player in 0..2 {
            let mut raw = [0u8; 19];
            raw[0] = 0xD0 | player as u8;
            raw[1] = (player as u8 == hider) as u8;
            raw[2] = doors[player];
            raw[3..7].copy_from_slice(&taps[player].0.to_le_bytes());
            raw[7..11].copy_from_slice(&taps[player].1.to_le_bytes());
            raw[11..].copy_from_slice(&taps[player].2.to_le_bytes());
            self.absorb(TAG_OUTGUESS, &raw);
            raw.zeroize();
            if healthy[player] {
                self.game_mbits = self.game_mbits
                    .saturating_add(credit_mbits[player].min(crate::outguess::MAX_MBITS));
            }
        }
        self.game_ok = healthy.into_iter().all(|ok| ok);
    }

    /// Absorbs a Shard Stack placement (`piece ‖ rot ‖ col ‖ t_us`) and adds the rank credit for it.
    pub fn add_shard(&mut self, piece: u8, rot: u8, col: u8, t_us: u64, credit_mbits: u32, healthy: bool) {
        self.add_game(TAG_SHARD, Some(piece), rot as i32, col as i32, t_us, credit_mbits, healthy);
    }

    /// Absorbs a Shard Stack drag or rotation (`0xF0|kind ‖ arg ‖ t_us`). Timing only: never credited.
    pub fn add_shard_move(&mut self, kind: u8, arg: i32, t_us: u64) {
        let mut raw = [0u8; 13];
        raw[0] = 0xF0 | kind;
        raw[1..5].copy_from_slice(&arg.to_le_bytes());
        raw[5..].copy_from_slice(&t_us.to_le_bytes());
        self.absorb(TAG_SHARD, &raw);
        raw.zeroize();
    }

    fn add_game(&mut self, tag: u8, door: Option<u8>, x: i32, y: i32, t_us: u64, credit_mbits: u32, healthy: bool) {
        let mut raw = [0u8; 17];
        let n = if let Some(d) = door { raw[0] = d; 1 } else { 0 };
        raw[n..n + 4].copy_from_slice(&x.to_le_bytes());
        raw[n + 4..n + 8].copy_from_slice(&y.to_le_bytes());
        raw[n + 8..n + 16].copy_from_slice(&t_us.to_le_bytes());
        self.absorb(tag, &raw[..n + 16]);
        raw.zeroize();
        self.game_mbits = self.game_mbits.saturating_add(credit_mbits);
        self.game_ok = healthy;
    }

    /// Absorbs a Static Hunt shot (tap position and miss offset from the ghost centre). Credit (1 bit) needs
    /// both the offset and the tap-position delta from the previous shot to pass health, so it measures the user's aim.
    pub fn add_shot(&mut self, tap_x: i32, tap_y: i32, miss_dx: i32, miss_dy: i32, t_us: u64) -> SampleResult {
        let mut raw = [0u8; 24];
        raw[..4].copy_from_slice(&tap_x.to_le_bytes());
        raw[4..8].copy_from_slice(&tap_y.to_le_bytes());
        raw[8..12].copy_from_slice(&miss_dx.to_le_bytes());
        raw[12..16].copy_from_slice(&miss_dy.to_le_bytes());
        raw[16..].copy_from_slice(&t_us.to_le_bytes());
        self.absorb(TAG_SHOT, &raw);
        raw.zeroize();
        let Some((px, py)) = self.last_shot_tap.replace((tap_x, tap_y)) else {
            return SampleResult::Uncredited; // no previous tap to measure against
        };
        let tap = self.shot_tap_health.check(tap_x.wrapping_sub(px), tap_y.wrapping_sub(py));
        match (self.shot_health.check(miss_dx, miss_dy), tap) {
            (Verdict::Good, Verdict::Good) => {
                self.game_ok = true;
                self.game_mbits = self.game_mbits.saturating_add(SHOT_MBITS);
                SampleResult::Credited(1)
            }
            (Verdict::Unhealthy, _) | (_, Verdict::Unhealthy) => {
                self.game_ok = false;
                SampleResult::SourceUnhealthy
            }
            _ => SampleResult::Uncredited,
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
        let drawing = self.touch_ok && self
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
        self.shot_health.wipe();
        self.shot_tap_health.wipe();
        if let Some((x, y)) = self.last_shot_tap.as_mut() {
            x.zeroize();
            y.zeroize();
        }
        self.last_shot_tap = None;
        // Field-wise reset: `*self = Self::new(..)` would drop the old pool and recurse through Drop.
        self.hasher = Sha256::new();
        self.last_touch = None;
        self.last_touch_us = None;
        self.touch_bits = 0;
        self.camera_bits = 0;
        self.touch_ok = true;
        self.camera_ok = true;
        self.game_mbits = 0;
        self.game_ok = true;
    }

    pub fn touch_bits(&self) -> u32 {
        self.touch_bits
    }

    pub fn camera_bits(&self) -> u32 {
        self.camera_bits
    }

    pub fn game_bits(&self) -> u32 {
        self.game_mbits / 1000
    }

    pub fn game_healthy(&self) -> bool {
        self.game_ok
    }

    pub fn credited_bits(&self) -> u32 {
        (self.touch_bits + self.camera_bits + self.game_bits()).min(self.wc.target_bits())
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

    #[test]
    fn extreme_coords_do_not_overflow() {
        let mut p = EntropyPool::new(WordCount::Twelve);
        p.add_touch(i32::MIN, i32::MIN, 0);
        p.add_touch(i32::MAX, i32::MAX, 1);
    }

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
    fn camera_stops_immediately_when_recent_touch_turns_unhealthy() {
        let mut p = EntropyPool::new(WordCount::TwentyFour);
        p.add_camera_frame(&noisy_frame(0), 0);
        p.add_touch(0, 0, 1_000);
        p.add_touch(5, 7, 2_000);
        for i in 0..8 { p.add_touch(5, 7, 3_000 + i * 1_000); }
        assert!(!p.touch_healthy());
        assert_eq!(p.add_camera_frame(&noisy_frame(1), 20_000), SampleResult::Uncredited);
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

    #[test]
    fn game_credit_fills_meter() {
        let mut p = EntropyPool::new(WordCount::Twelve);
        for i in 0..64 { p.add_outguess(1, 0, 0, i, 2000, true); }
        assert!(p.is_full());
    }

    #[test] fn shot_repeated_offset_earns_once() { let mut p = EntropyPool::new(WordCount::Twelve); for i in 0..20i32 { p.add_shot(i * i * 3, i * i * 5 + i, 3, -2, i as u64 * 1000); } assert!(p.game_bits() <= 1); }
    #[test] fn shot_bullseye_earns_nothing() { let mut p = EntropyPool::new(WordCount::Twelve); for i in 0..20i32 { p.add_shot(i * i * 3, i * i * 5 + i, 0, 0, i as u64 * 1000); } assert_eq!(p.game_bits(), 0); }
    #[test] fn shots_varied_credit_one_bit_each() { let mut p = EntropyPool::new(WordCount::Twelve); for i in 0..11i32 { p.add_shot(i * i * 3, i * i * 5 + i, i * 3 - 7, 11 - i * 5, i as u64 * 1000); } assert_eq!(p.game_bits(), 10); } // first shot has no previous tap
    #[test] fn shot_fixed_tap_spot_earns_nothing() { let mut p = EntropyPool::new(WordCount::Twelve); for i in 0..30i32 { p.add_shot(120, 200, i * 3 - 7, 11 - i * 5, i as u64 * 1000); } assert!(p.game_bits() <= 1); }

    #[test]
    fn wipe_resets_game_credit() {
        let mut p = EntropyPool::new(WordCount::Twelve);
        p.add_outguess(1, 0, 0, 0, 5000, true);
        p.add_outguess(1, 0, 0, 1, 0, false);
        assert!(!p.game_healthy());
        p.wipe();
        assert_eq!((p.game_bits(), p.game_healthy(), p.credited_bits()), (0, true, 0));
    }

    #[test]
    fn duel_absorbs_players_roles_doors_and_both_taps_without_credit() {
        fn digest(hider: u8, doors: [u8; 2], taps: [(i32, i32, u64); 2]) -> Zeroizing<Vec<u8>> {
            let mut p = EntropyPool::new(WordCount::TwentyFour);
            p.add_duel_round(hider, doors, taps, [0; 2], [true; 2]);
            assert_eq!(p.credited_bits(), 0);
            p.finish(&[7; 32])
        }
        let doors = [1, 2];
        let taps = [(10, 20, 30), (40, 50, 60)];
        let base = digest(0, doors, taps);
        assert_ne!(base, digest(1, doors, taps));
        assert_ne!(base, digest(0, [2, 1], taps));
        for player in 0..2 {
            let mut changed = taps;
            changed[player].0 += 1;
            assert_ne!(base, digest(0, doors, changed));
            changed = taps;
            changed[player].1 += 1;
            assert_ne!(base, digest(0, doors, changed));
            changed = taps;
            changed[player].2 += 1;
            assert_ne!(base, digest(0, doors, changed));
        }
        // Equal door/tap values still carry distinct player markers.
        let mut p = EntropyPool::new(WordCount::TwentyFour);
        p.add_duel_round(0, [1, 1], [taps[0]; 2], [0; 2], [true; 2]);
        let mut expected = Sha256::new();
        for player in 0..2 {
            let mut raw = vec![0xD0 | player, (player == 0) as u8, 1];
            raw.extend_from_slice(&10i32.to_le_bytes());
            raw.extend_from_slice(&20i32.to_le_bytes());
            raw.extend_from_slice(&30u64.to_le_bytes());
            expected.update([TAG_OUTGUESS]);
            expected.update((raw.len() as u32).to_le_bytes());
            expected.update(raw);
        }
        assert_eq!(p.hasher.clone().finalize(), expected.finalize());
    }

    #[test]
    fn duel_credits_only_healthy_players_and_combines_health() {
        let mut p = EntropyPool::new(WordCount::Twelve);
        p.add_duel_round(0, [1, 2], [(0, 0, 0); 2], [2000, 2000], [false, true]);
        assert_eq!(p.game_bits(), 2);
        assert!(!p.game_healthy());
        p.add_duel_round(1, [1, 2], [(0, 0, 0); 2], [1500, 1500], [true, true]);
        assert_eq!(p.game_bits(), 5);
        assert!(p.game_healthy());
    }
}

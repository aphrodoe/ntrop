//! Glitch Glider timing: tap-interval residual against the recent median, quantised to a symbol.
//! Intervals are secret entropy material: never logged, zeroized on wipe.

use zeroize::Zeroize;

const KEEP: usize = 16;
const PAUSE_US: u64 = 1_500_000;
const BIN_US: i64 = 4000;
const WIN: usize = 64;
/// Predictors: P0 median of last 16 bins, P1..P16 lag-k repeats, P17 linear extrapolation, P18..P20 lag-k extrapolation (k = 2..4).
const NPRED: usize = 21;
const SLACK_HITS: u32 = 4;
/// Health from the predictor bound only once this many taps are scored (matches the MCV warmup).
const HEALTH_MIN_N: u32 = 32;

/// Tap-interval timing: a residual symbol for the MCV estimator plus a predictor-based upper bound
/// on how predictable the rhythm is (MCV is IID-only; deterministic rhythms must not earn).
/// All fields are secret entropy material: never logged, zeroized on wipe.
#[derive(Clone, Zeroize)]
pub struct GlideTimer {
    last_us: Option<u64>,
    ivals: Vec<i64>,
    bins: Vec<i32>,
    /// Per predictor: hit bits over the last 64 scored taps (bit 0 newest).
    hits: [u64; NPRED],
    scored: u32,
}

impl Default for GlideTimer {
    fn default() -> Self {
        Self { last_us: None, ivals: Vec::with_capacity(KEEP + 1), bins: Vec::with_capacity(WIN + 1), hits: [0; NPRED], scored: 0 }
    }
}

fn median(v: &[i64]) -> i64 {
    let mut s = [0i64; KEEP];
    s[..v.len()].copy_from_slice(v);
    s[..v.len()].sort_unstable();
    let r = (s[v.len() / 2 - 1] + s[v.len() / 2]) / 2;
    s.zeroize();
    r
}

impl GlideTimer {
    fn reset(&mut self) {
        self.ivals.zeroize();
        self.ivals.clear();
        self.bins.zeroize();
        self.bins.clear();
        self.hits = [0; NPRED];
        self.scored = 0;
    }

    pub fn tap(&mut self, t_us: u64) -> Option<u16> {
        let last = self.last_us.replace(t_us)?;
        let iv = t_us.saturating_sub(last);
        if iv > PAUSE_US {
            self.reset();
            return None;
        }
        let iv = iv as i64;
        let bin = (iv / BIN_US) as i32;
        let nb = self.bins.len();
        if nb >= KEEP {
            // All predictions are committed from history before this tap is pushed.
            let mut preds = [0i32; NPRED];
            let mut last16 = [0i64; KEEP];
            for (d, &b) in last16.iter_mut().zip(&self.bins[nb - KEEP..]) { *d = b as i64; }
            preds[0] = median(&last16) as i32;
            last16.zeroize();
            for k in 1..=16 { preds[k] = self.bins[nb - k]; }
            preds[17] = 2 * self.bins[nb - 1] - self.bins[nb - 2];
            for k in 2..=4 { preds[16 + k] = 2 * self.bins[nb - k] - self.bins[nb - 2 * k]; }
            for (h, p) in self.hits.iter_mut().zip(preds) { *h = (*h << 1) | ((bin - p).abs() <= 1) as u64; }
            self.scored = (self.scored + 1).min(WIN as u32);
            preds.zeroize();
        }
        if nb == WIN { self.bins.remove(0); }
        self.bins.push(bin);
        let sym = (self.ivals.len() == KEEP).then(|| (((iv - median(&self.ivals)) / BIN_US).clamp(-16, 16) + 16) as u16);
        if self.ivals.len() == KEEP { self.ivals.remove(0); }
        self.ivals.push(iv);
        sym
    }

    /// Predictor bound after the latest tap: (credit ceiling in mbits, healthy). `None` until a tap is scored.
    pub fn bound(&self) -> Option<(u32, bool)> {
        (self.scored > 0).then(|| {
            let best = self.hits.iter().map(|h| h.count_ones()).max().unwrap_or(0);
            let pub_ = ((best + SLACK_HITS) as f64 / self.scored as f64).min(1.0);
            ((-pub_.log2() * 1000.0).floor() as u32, !(pub_ > 0.75 && self.scored >= HEALTH_MIN_N))
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::{Session, WordCount};
    use sha2::{Digest, Sha256};
    fn sess() -> Session { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); s }
    fn byte(i: u32, j: u32) -> u8 { Sha256::digest([i.to_le_bytes(), j.to_le_bytes()].concat())[0] }
    #[test] fn glide_metronome_earns_nothing() { let mut s = sess(); let total: u32 = (0..120u64).map(|i| s.glide_tap(100, 400, i * 400_000).credit_mbits).sum(); assert_eq!(total, 0); }
    #[test] fn glide_steady_drift_earns_nothing() { let mut s = sess(); let mut t = 0u64; let mut total = 0; for i in 0..120u64 { t += 400_000 + 2_000 * i; total += s.glide_tap(1, 1, t).credit_mbits; } assert_eq!(total, 0); }
    #[test] fn glide_frame_quantised_jitter_is_bounded() {
        let mut s = sess(); let mut t = 0u64; let mut total = 0;
        for i in 0..200u32 { let k = (byte(i, 0) % 3) as i64 - 1; t = (t as i64 + 400_000 + k * 16_667) as u64; let c = s.glide_tap(1, 1, t).credit_mbits; assert!(c <= 1585, "credit {c}"); total += c; }
        assert!(total > 0);
    }
    #[test] fn glide_human_jitter_earns() {
        let mut s = sess(); let mut t = 0u64; let mut total = 0;
        for i in 0..200u32 { let sum: i64 = (0..4).map(|j| byte(i, j + 1) as i64).sum(); t = (t as i64 + 400_000 + (sum - 510) * 80) as u64; total += s.glide_tap(1, 1, t).credit_mbits; }
        assert!(total >= 100_000, "total {total}");
    }
    fn run(ivs: impl Iterator<Item = i64>) -> Vec<u32> { let mut s = sess(); let mut t = 0i64; ivs.map(|iv| { t += iv; s.glide_tap(1, 1, t as u64).credit_mbits }).collect() }
    fn quant(seed: u32) -> Vec<u32> { run((0..200u32).map(|i| 400_000 + ((byte(i, 100 + seed) % 3) as i64 - 1) * 16_667)) }
    #[test] fn glide_swing_earns_little() { let t: u32 = run((0..200).map(|i| if i % 2 == 0 { 380_000 } else { 420_000 })).iter().sum(); assert!(t <= 5_000, "swing {t}"); }
    #[test] fn glide_periodic_offsets_earn_little() {
        let (a, b) = ([0, 8, -8, 16], [0, 8, -8, 16, -16, 4, 12]);
        let ta: u32 = run((0..200).map(|i| 400_000 + a[i % 4] * 1000)).iter().sum();
        let tb: u32 = run((0..200).map(|i| 400_000 + b[i % 7] * 1000)).iter().sum();
        assert!(ta <= 5_000 && tb <= 5_000, "period4 {ta} period7 {tb}");
    }
    #[test] fn glide_smooth_tempo_earns_little() { let t: u32 = run((0..200).map(|i| 400_000 + (20_000.0 * (std::f64::consts::TAU * i as f64 / 40.0).sin()) as i64)).iter().sum(); assert!(t <= 10_000, "sinus {t}"); }
    #[test] fn glide_quantised_bound_holds_across_seeds() { for seed in 0..20 { let m = quant(seed).into_iter().max().unwrap(); assert!(m <= 1585, "seed {seed} max {m}"); } }
    #[test] fn glide_swing_plus_drift_earns_little() { let sw = [0i64, 12, -12]; let t: u32 = run((0..200).map(|i| 400_000 + (20_000.0 * (std::f64::consts::TAU * i as f64 / 40.0).sin()) as i64 + sw[i % 3] * 1000)).iter().sum(); assert!(t <= 30_000, "swing+drift {t}"); }
    #[test] fn glide_honest_steady_player_not_flagged() {
        let mut s = sess(); let mut t = 0i64; let (mut total, mut bad, mut scored) = (0u32, 0u32, 0u32);
        for i in 0..200u32 { let sum: i64 = (0..4).map(|j| byte(i, j + 1) as i64).sum(); t += 400_000 + (sum - 510) * 54; let e = s.glide_tap(1, 1, t as u64); total += e.credit_mbits; if i >= 16 { scored += 1; bad += !e.healthy as u32; } }
        assert!(total > 0 && bad * 10 <= scored, "total {total} bad {bad}/{scored}");
    }
    #[test] fn glide_pause_resets_baseline() {
        let mut s = sess(); let mut t = 0u64;
        for _ in 0..20 { t += 400_000; s.glide_tap(1, 1, t); }
        t += 3_000_000;
        for _ in 0..17 { let e = s.glide_tap(1, 1, t); assert_eq!((e.credit_mbits, e.healthy), (0, true)); t += 400_000; }
    }
}

//! Glitch Glider timing: tap-interval residual against the recent median, quantised to a symbol.
//! Intervals are secret entropy material: never logged, zeroized on wipe.

use zeroize::Zeroize;

const KEEP: usize = 16;
const PAUSE_US: u64 = 1_500_000;
const BIN_US: i64 = 4000;

#[derive(Clone, Default, Zeroize)]
pub struct GlideTimer {
    last_us: Option<u64>,
    ivals: Vec<i64>,
}

impl GlideTimer {
    pub fn tap(&mut self, t_us: u64) -> Option<u16> {
        let last = self.last_us.replace(t_us)?;
        let iv = t_us.saturating_sub(last);
        if iv > PAUSE_US {
            self.ivals.zeroize();
            self.ivals.clear();
            return None;
        }
        let iv = iv as i64;
        let sym = (self.ivals.len() == KEEP).then(|| {
            let mut s = self.ivals.clone();
            s.sort_unstable();
            let med = (s[KEEP / 2 - 1] + s[KEEP / 2]) / 2;
            s.zeroize();
            ((iv - med) / BIN_US).clamp(-16, 16) + 16
        });
        if self.ivals.len() == KEEP {
            self.ivals.remove(0);
        }
        self.ivals.push(iv);
        sym.map(|s| s as u16)
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
    #[test] fn glide_pause_resets_baseline() {
        let mut s = sess(); let mut t = 0u64;
        for _ in 0..20 { t += 400_000; s.glide_tap(1, 1, t); }
        t += 3_000_000;
        for _ in 0..17 { let e = s.glide_tap(1, 1, t); assert_eq!((e.credit_mbits, e.healthy), (0, true)); t += 400_000; }
    }
}

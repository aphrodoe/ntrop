//! Outguess the Machine: a Shannon-style predictor reads the player's door history.
//! Credit is earned only while the machine guesses worse than a conservative bound.
//! The history is secret entropy material: never logged, zeroized on wipe.

use zeroize::Zeroize;

pub const DOORS: u8 = 4;
pub const WINDOW: usize = 32;
pub const WARMUP: usize = 16;
pub const MARGIN_HITS: u32 = 4;
pub const MAX_MBITS: u32 = 2000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Move {
    pub predicted: u8,
    pub hit: bool,
    pub credit_mbits: u32,
    pub read_pct: u8,
    pub healthy: bool,
}

/// Three experts, each predicting from history alone: E1 doors, E2 offset from the previous
/// revealed guess, E3 win/lose state. The shown guess is the recently best expert.
#[derive(Default, Clone, Zeroize)]
pub struct Oracle {
    doors: Vec<u8>,
    /// The guess shown (committed) for each move.
    shown: Vec<u8>,
    /// Per expert: 1 where its own prediction hit that move.
    ehits: [Vec<u8>; 3],
}

/// Context counting over `seq`: longest context in `ks` seen before wins; ties to the lowest value.
fn ctx_pred(seq: &[u8], ks: &[usize]) -> Option<u8> {
    for &k in ks {
        if seq.len() > k {
            let ctx = &seq[seq.len() - k..];
            let mut counts = [0u32; DOORS as usize];
            for i in 0..seq.len() - k {
                if &seq[i..i + k] == ctx {
                    counts[seq[i + k] as usize] += 1;
                }
            }
            if counts.iter().any(|&c| c > 0) {
                return Some(argmax(&counts));
            }
        }
    }
    None
}

fn window_hits(h: &[u8]) -> u32 {
    h[h.len().saturating_sub(WINDOW)..].iter().map(|&b| b as u32).sum()
}

impl Oracle {
    fn experts(&self) -> [u8; 3] {
        let (d, s) = (&self.doors, &self.shown);
        let n = d.len();
        let e1 = ctx_pred(d, &[3, 2, 1, 0]).unwrap_or(0);
        let e2 = if n >= 1 {
            let r: Vec<u8> = (1..n).map(|t| (d[t] + 4 - s[t - 1]) % 4).collect();
            ctx_pred(&r, &[2, 1, 0]).map_or(e1, |r| (s[n - 1] + r) % 4)
        } else {
            e1
        };
        let e3 = if n >= 2 {
            let delta = |t: usize| (d[t] + 4 - d[t - 1]) % 4;
            let state = |t: usize| (s[t] == d[t]) as u8 * 4 + delta(t);
            let cur = state(n - 1);
            let mut counts = [0u32; DOORS as usize];
            for t in 1..n - 1 {
                if state(t) == cur {
                    counts[delta(t + 1) as usize] += 1;
                }
            }
            if counts.iter().any(|&c| c > 0) { (d[n - 1] + argmax(&counts)) % 4 } else { e1 }
        } else {
            e1
        };
        [e1, e2, e3]
    }

    /// Index of the expert with the most window hits (ties: E1, E2, E3).
    fn best(&self) -> usize {
        let h = [0, 1, 2].map(|i| window_hits(&self.ehits[i]));
        let mut b = 0;
        for i in 1..3 {
            if h[i] > h[b] {
                b = i;
            }
        }
        b
    }

    /// Pure function of the history.
    pub fn predict(&self) -> u8 {
        self.experts()[self.best()]
    }

    /// The prediction is taken before the door is pushed, so it is committed by construction.
    pub fn record(&mut self, door: u8) -> Move {
        let door = door % DOORS;
        let e = self.experts();
        let predicted = e[self.best()];
        let hit = predicted == door;
        self.doors.push(door);
        self.shown.push(predicted);
        for i in 0..3 {
            self.ehits[i].push((e[i] == door) as u8);
        }
        let moves = self.doors.len();
        let n = moves.min(WINDOW);
        // Best expert in hindsight, not just the shown guess.
        let hits = (0..3).map(|i| window_hits(&self.ehits[i])).max().unwrap();
        let read_pct = (100.0 * hits as f64 / n as f64).round() as u8;
        let p_ub = ((hits + MARGIN_HITS) as f64 / n as f64).min(1.0);
        let (credit_mbits, healthy) = if moves <= WARMUP {
            (0, true)
        } else if p_ub > 0.5 {
            (0, false)
        } else {
            (((-p_ub.log2() * 1000.0).floor() as u32).min(MAX_MBITS), true)
        };
        Move { predicted, hit, credit_mbits, read_pct, healthy }
    }

    pub fn wipe(&mut self) {
        self.zeroize();
    }
}

fn argmax(c: &[u32; DOORS as usize]) -> u8 {
    let mut best = 0;
    for i in 1..c.len() {
        if c[i] > c[best] {
            best = i;
        }
    }
    best as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn prng_doors(n: u32) -> Vec<u8> {
        (0..n).map(|i| Sha256::digest(i.to_le_bytes())[0] & 3).collect()
    }

    #[test] fn oracle_reads_constant_player() { let mut o = Oracle::default(); let moves: Vec<Move> = (0..64).map(|_| o.record(2)).collect(); assert_eq!(moves.iter().map(|m| m.credit_mbits).sum::<u32>(), 0); assert!(!moves.last().unwrap().healthy); }
    #[test] fn oracle_reads_cycle() { let mut o = Oracle::default(); let moves: Vec<Move> = (0..64).map(|i| o.record((i % 4) as u8)).collect(); assert_eq!(moves[32..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0); }
    #[test] fn oracle_warmup_earns_nothing() { let mut o = Oracle::default(); for d in prng_doors(16) { assert_eq!(o.record(d).credit_mbits, 0); } }
    #[test] fn oracle_credits_random_player() { let mut o = Oracle::default(); let ms: Vec<Move> = prng_doors(256).into_iter().map(|d| o.record(d)).collect(); assert!(ms.iter().map(|m| m.credit_mbits).sum::<u32>() >= 150_000); assert!(ms.iter().all(|m| m.credit_mbits <= 2000)); }
    #[test] fn oracle_reads_offset_from_last_guess() { for off in [1u8, 3] { let mut o = Oracle::default(); let mut last = 0u8; let ms: Vec<Move> = (0..400).map(|_| { let m = o.record((last + off) % 4); last = m.predicted; m }).collect(); assert_eq!(ms[64..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0, "offset {off}"); } }
    #[test] fn oracle_reads_win_stay_lose_shift() { let mut o = Oracle::default(); let mut d = 0u8; let ms: Vec<Move> = (0..400).map(|_| { let m = o.record(d); if m.hit { d = (d + 1) % 4; } m }).collect(); assert_eq!(ms[64..].iter().map(|m| m.credit_mbits).sum::<u32>(), 0); }
    #[test] fn oracle_prediction_is_fixed_before_pick() { let mut o = Oracle::default(); for d in prng_doors(20) { o.record(d); } let p = o.predict(); for door in 0..4 { assert_eq!(o.clone().record(door).predicted, p); } }
    #[test] fn oracle_wipe_forgets_history() { let mut o = Oracle::default(); for _ in 0..40 { o.record(1); } o.wipe(); assert_eq!(o.predict(), 0); assert_eq!(o.record(3).credit_mbits, 0); }
}

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

#[derive(Default, Clone, Zeroize)]
pub struct Oracle {
    doors: Vec<u8>,
    hits: Vec<u8>,
}

impl Oracle {
    /// Pure function of the history. Ties break to the lowest door.
    pub fn predict(&self) -> u8 {
        let h = &self.doors;
        for k in [3usize, 2, 1] {
            if h.len() > k {
                let ctx = &h[h.len() - k..];
                let mut counts = [0u32; DOORS as usize];
                for i in 0..h.len() - k {
                    if &h[i..i + k] == ctx {
                        counts[h[i + k] as usize] += 1;
                    }
                }
                if counts.iter().any(|&c| c > 0) {
                    return argmax(&counts);
                }
            }
        }
        let mut counts = [0u32; DOORS as usize];
        for &d in h {
            counts[d as usize] += 1;
        }
        argmax(&counts)
    }

    /// The prediction is taken before the door is pushed, so it is committed by construction.
    pub fn record(&mut self, door: u8) -> Move {
        let door = door % DOORS;
        let predicted = self.predict();
        let hit = predicted == door;
        self.doors.push(door);
        self.hits.push(hit as u8);
        let moves = self.doors.len();
        let n = moves.min(WINDOW);
        let hits: u32 = self.hits[moves - n..].iter().map(|&b| b as u32).sum();
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
    #[test] fn oracle_credits_random_player() { let mut o = Oracle::default(); let ms: Vec<Move> = prng_doors(128).into_iter().map(|d| o.record(d)).collect(); assert!(ms.iter().map(|m| m.credit_mbits).sum::<u32>() >= 100_000); assert!(ms.iter().all(|m| m.credit_mbits <= 2000)); }
    #[test] fn oracle_prediction_is_fixed_before_pick() { let mut o = Oracle::default(); for d in prng_doors(20) { o.record(d); } let p = o.predict(); for door in 0..4 { assert_eq!(o.clone().record(door).predicted, p); } }
    #[test] fn oracle_wipe_forgets_history() { let mut o = Oracle::default(); for _ in 0..40 { o.record(1); } o.wipe(); assert_eq!(o.predict(), 0); assert_eq!(o.record(3).credit_mbits, 0); }
}

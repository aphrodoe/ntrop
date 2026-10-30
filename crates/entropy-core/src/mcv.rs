//! Most-common-value min-entropy estimator over a sliding symbol window (used by Shard Stack).
//! The window is secret entropy material: never logged, zeroized on wipe.

use zeroize::Zeroize;

pub const MCV_WINDOW: usize = 64;
pub const MCV_Z: f64 = 2.576;
pub const MCV_REPEAT: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Estimate {
    pub credit_mbits: u32,
    pub healthy: bool,
}

#[derive(Clone, Zeroize)]
pub struct Mcv {
    win: Vec<u16>,
    moves: usize,
    warmup: usize,
    cap_mbits: u32,
}

impl Mcv {
    pub fn new(warmup: usize, cap_mbits: u32) -> Self {
        Self { win: Vec::with_capacity(MCV_WINDOW + 1), moves: 0, warmup, cap_mbits }
    }

    pub fn push(&mut self, symbol: u16) -> Estimate {
        self.win.push(symbol);
        if self.win.len() > MCV_WINDOW {
            self.win.remove(0);
        }
        self.moves += 1;
        if self.moves <= self.warmup {
            return Estimate { credit_mbits: 0, healthy: true };
        }
        let n = self.win.len();
        if n >= MCV_REPEAT && self.win[n - MCV_REPEAT..].iter().all(|&s| s == symbol) {
            return Estimate { credit_mbits: 0, healthy: false };
        }
        let max = self.win.iter().map(|&s| self.win.iter().filter(|&&t| t == s).count()).max().unwrap_or(0);
        let p = max as f64 / n as f64;
        let pu = if n > 1 { (p + MCV_Z * (p * (1.0 - p) / (n - 1) as f64).sqrt()).min(1.0) } else { 1.0 };
        let credit = ((-pu.log2() * 1000.0).floor() as u32).min(self.cap_mbits);
        Estimate { credit_mbits: credit, healthy: true }
    }

    pub fn wipe(&mut self) {
        self.win.zeroize();
        self.win.clear();
        self.moves = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    #[test] fn mcv_constant_symbol_earns_nothing() { let mut m = Mcv::new(16, 2000); let es: Vec<Estimate> = (0..100).map(|_| m.push(7)).collect(); assert_eq!(es.iter().map(|e| e.credit_mbits).sum::<u32>(), 0); assert!(!es.last().unwrap().healthy); }
    #[test] fn mcv_warmup_earns_nothing() { let mut m = Mcv::new(16, 2000); for s in 0..16u16 { assert_eq!(m.push(s).credit_mbits, 0); } }
    #[test] fn mcv_matches_formula() { let mut m = Mcv::new(0, 3000); let mut last = None; for i in 0..64u16 { last = Some(m.push(if i % 4 == 0 { 0 } else { 1 + i % 13 })); } assert_eq!(last.unwrap().credit_mbits, 1356); }
    #[test] fn mcv_uniform_symbols_earn() { let mut m = Mcv::new(16, 2000); let total: u32 = (0..200u32).map(|i| m.push((Sha256::digest(i.to_le_bytes())[0] % 16) as u16).credit_mbits).sum(); assert!(total >= 150_000); }
}

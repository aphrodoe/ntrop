//! App session: owns the pool and the generated secrets; never logs them.

use zeroize::Zeroizing;

use crate::{words, EntropyPool, SampleResult, WordCount};

pub struct Session {
    pool: Option<EntropyPool>,
    wc: WordCount,
    entropy: Option<Zeroizing<Vec<u8>>>,
    words: Zeroizing<Vec<String>>,
}

impl Session {
    pub fn new() -> Self {
        Self { pool: None, wc: WordCount::Twelve, entropy: None, words: Zeroizing::new(Vec::new()) }
    }
}

impl Session {
    pub fn start_arcade(&mut self, wc: WordCount) {
        self.wipe();
        self.wc = wc;
        self.pool = Some(EntropyPool::new(wc));
    }

    pub fn on_touch(&mut self, x: i32, y: i32, t_us: u64) -> SampleResult {
        match self.pool.as_mut() {
            Some(p) => p.add_touch(x, y, t_us),
            None => SampleResult::Uncredited,
        }
    }

    pub fn pool(&self) -> Option<&EntropyPool> {
        self.pool.as_ref()
    }

    pub fn target_bits(&self) -> u32 {
        self.wc.target_bits()
    }

    /// Consumes the pool. Without hardware randomness nothing is produced (no user-only fallback).
    pub fn generate(&mut self, hw: Result<[u8; 32], ()>) -> Result<(), String> {
        let mut hw = Zeroizing::new(hw.map_err(|_| "Hardware random unavailable".to_string())?);
        let pool = self.pool.take().ok_or_else(|| "No active session".to_string())?;
        let entropy = pool.finish(&hw);
        zeroize::Zeroize::zeroize(&mut *hw);
        self.words = words(&entropy);
        self.entropy = Some(entropy);
        Ok(())
    }

    pub fn words(&self) -> &[String] {
        &self.words
    }

    pub fn wipe(&mut self) {
        if let Some(p) = self.pool.as_mut() {
            p.wipe();
        }
        self.pool = None;
        self.entropy = None; // Zeroizing drops zeroized
        self.words = Zeroizing::new(Vec::new());
    }
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn finish_requires_hw_random() { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); assert!(s.generate(Err(())).is_err()); assert!(s.words().is_empty()); }
    #[test] fn session_wipe_clears_secrets() { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); s.generate(Ok([3;32])).unwrap(); assert_eq!(s.words().len(), 12); s.wipe(); assert!(s.words().is_empty()); }
}

//! App session: owns the pool and the generated secrets; never logs them.

use zeroize::Zeroizing;

use crate::{
    master_fingerprint, make_quiz, outguess::{Move, Oracle}, randomart, seedqr_digits, words, EntropyPool, QuizQuestion, SampleResult, WordCount,
};

pub struct Session {
    pool: Option<EntropyPool>,
    wc: WordCount,
    entropy: Option<Zeroizing<Vec<u8>>>,
    words: Zeroizing<Vec<String>>,
    page: usize,
    quiz: Vec<QuizQuestion>,
    step: usize,
    oracle: Oracle,
}

pub const WORDS_PER_PAGE: usize = 6;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum QuizOutcome {
    Next,
    /// Wrong answer: quiz dropped, back to words page 1.
    Restart,
    Done,
}

impl Session {
    pub fn new() -> Self {
        Self { pool: None, wc: WordCount::Twelve, entropy: None, words: Zeroizing::new(Vec::new()), page: 0, quiz: Vec::new(), step: 0, oracle: Oracle::default() }
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

    /// Records a door pick: the oracle's prediction is fixed before the door is pushed.
    pub fn outguess_pick(&mut self, door: u8, x: i32, y: i32, t_us: u64) -> Move {
        let m = self.oracle.record(door);
        if let Some(p) = self.pool.as_mut() {
            p.add_outguess(door, x, y, t_us, m.credit_mbits, m.healthy);
        }
        m
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
        self.page = 0;
        self.quiz = Vec::new(); // QuizQuestion zeroizes on drop
        self.step = 0;
        self.oracle.wipe();
    }

    pub fn page(&self) -> usize {
        self.page
    }

    pub fn page_count(&self) -> usize {
        self.words.len().div_ceil(WORDS_PER_PAGE)
    }

    pub fn page_words(&self) -> &[String] {
        let a = (self.page * WORDS_PER_PAGE).min(self.words.len());
        &self.words[a..(a + WORDS_PER_PAGE).min(self.words.len())]
    }

    /// Returns false when already on the last page (caller moves on to the SeedQR).
    pub fn next_page(&mut self) -> bool {
        if self.page + 1 < self.page_count() {
            self.page += 1;
            true
        } else {
            false
        }
    }

    pub fn prev_page(&mut self) {
        self.page = self.page.saturating_sub(1);
    }

    /// SeedQR digits, derived on demand (never stored); None once wiped.
    pub fn qr_digits(&self) -> Option<Zeroizing<String>> {
        self.entropy.as_ref().map(|e| seedqr_digits(e))
    }

    /// 4-byte master fingerprint and its randomart; None once wiped.
    pub fn fingerprint(&self) -> Option<([u8; 4], String)> {
        let fp = master_fingerprint(self.entropy.as_ref()?);
        Some((fp, randomart(&fp)))
    }

    pub fn start_quiz(&mut self, rand: &[u8; 32]) {
        self.quiz = make_quiz(&self.words, rand);
        self.step = 0;
    }

    pub fn quiz_step(&self) -> usize {
        self.step
    }

    pub fn quiz_question(&self) -> Option<&QuizQuestion> {
        self.quiz.get(self.step)
    }

    pub fn answer(&mut self, choice: usize) -> QuizOutcome {
        let Some(q) = self.quiz.get(self.step) else { return QuizOutcome::Restart };
        if q.correct != choice {
            self.quiz = Vec::new();
            self.step = 0;
            self.page = 0;
            return QuizOutcome::Restart;
        }
        self.step += 1;
        if self.step >= self.quiz.len() {
            QuizOutcome::Done
        } else {
            QuizOutcome::Next
        }
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
    use sha2::Digest;
    #[test] fn finish_requires_hw_random() { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); assert!(s.generate(Err(())).is_err()); assert!(s.words().is_empty()); }
    #[test] fn session_wipe_clears_secrets() { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); s.generate(Ok([3;32])).unwrap(); assert_eq!(s.words().len(), 12); s.wipe(); assert!(s.words().is_empty()); }

    fn gen() -> Session { let mut s = Session::new(); s.start_arcade(WordCount::Twelve); s.generate(Ok([3;32])).unwrap(); s }
    #[test] fn pages_of_six() {
        let mut s = gen();
        assert_eq!((s.page_count(), s.page_words().len()), (2, 6));
        assert!(s.next_page()); assert_eq!(s.page_words(), &s.words()[6..]);
        assert!(!s.next_page()); s.prev_page(); assert_eq!(s.page(), 0);
    }
    #[test] fn wrong_answer_restarts_at_words_page_1() {
        let mut s = gen(); s.next_page(); s.start_quiz(&[9;32]);
        let c = s.quiz_question().unwrap().correct;
        assert_eq!(s.answer((c + 1) % 4), QuizOutcome::Restart);
        assert_eq!(s.page(), 0); assert!(s.quiz_question().is_none());
    }
    #[test] fn three_right_answers_reach_done() {
        let mut s = gen(); s.start_quiz(&[9;32]);
        for i in 0..3 {
            let c = s.quiz_question().unwrap().correct;
            assert_eq!(s.answer(c), if i == 2 { QuizOutcome::Done } else { QuizOutcome::Next });
        }
    }
    #[test] fn wipe_clears_quiz_qr_and_fingerprint() {
        let mut s = gen(); s.start_quiz(&[9;32]);
        assert!(s.qr_digits().is_some() && s.fingerprint().is_some());
        s.wipe();
        assert!(s.quiz_question().is_none() && s.qr_digits().is_none() && s.fingerprint().is_none());
    }
    #[test] fn fingerprint_matches_entropy() {
        let s = gen();
        let e = s.entropy.as_ref().unwrap();
        assert_eq!(s.fingerprint().unwrap().0, master_fingerprint(e));
        assert_eq!(&*crate::words(e), s.words());
    }
}

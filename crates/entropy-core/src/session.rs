//! App session: owns the pool and the generated secrets; never logs them.

use zeroize::{Zeroize, Zeroizing};

use crate::{
    dice_entropy, master_fingerprint, make_quiz, outguess::{Move, Oracle}, randomart, seedqr_digits, words, EntropyPool, QuizQuestion, SampleResult, WordCount,
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
    rolls: Zeroizing<Vec<u8>>,
}

pub const WORDS_PER_PAGE: usize = 6;
const MAX_ROLLS: usize = 255;

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum QuizOutcome {
    Next,
    /// Wrong answer: quiz dropped, back to words page 1.
    Restart,
    Done,
}

impl Session {
    pub fn new() -> Self {
        Self { pool: None, wc: WordCount::Twelve, entropy: None, words: Zeroizing::new(Vec::new()), page: 0, quiz: Vec::new(), step: 0, oracle: Oracle::default(), rolls: Zeroizing::new(Vec::with_capacity(MAX_ROLLS)) }
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

    pub fn on_camera_frame(&mut self, frame: &[u8], t_us: u64) -> SampleResult {
        match self.pool.as_mut() {
            Some(p) => p.add_camera_frame(frame, t_us),
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

    pub fn on_shot(&mut self, tap_x: i32, tap_y: i32, miss_dx: i32, miss_dy: i32, t_us: u64) -> SampleResult {
        match self.pool.as_mut() {
            Some(p) => p.add_shot(tap_x, tap_y, miss_dx, miss_dy, t_us),
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

    pub fn start_dice(&mut self, wc: WordCount) {
        self.wipe();
        self.wc = wc;
    }

    /// Accepts only 1..=6; returns false (and stores nothing) otherwise.
    pub fn push_roll(&mut self, r: u8) -> bool {
        let ok = (1..=6).contains(&r) && self.rolls.len() < MAX_ROLLS;
        if ok {
            self.rolls.push(r);
        }
        ok
    }

    pub fn pop_roll(&mut self) {
        // Vec::pop alone leaves the removed byte outside len, where a later wipe can miss it.
        if let Some(last) = self.rolls.last_mut() {
            last.zeroize();
            self.rolls.pop();
        }
    }

    pub fn rolls(&self) -> &[u8] {
        &self.rolls
    }

    pub fn rolls_needed(&self) -> usize {
        crate::min_rolls(self.wc)
    }

    /// Seed = SHA-256 of the rolls; no hardware RNG. The rolls are cleared once the seed exists.
    pub fn generate_from_dice(&mut self) -> Result<(), String> {
        let entropy = dice_entropy(&self.rolls, self.wc).map_err(|e| format!("{e:?}"))?;
        self.words = words(&entropy);
        self.entropy = Some(entropy);
        self.rolls.zeroize(); // keep the preallocated storage: no secret-bearing reallocations
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
        self.rolls.zeroize();
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

impl Drop for Session {
    fn drop(&mut self) { self.wipe(); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn camera_frame_without_pool_is_uncredited() { let mut s = Session::new(); assert_eq!(s.on_camera_frame(&[1, 2, 3], 0), SampleResult::Uncredited); }
    fn frame(seed: u32) -> Vec<u8> {
        let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..4096).map(|_| { x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345); (x >> 16) as u8 }).collect()
    }
    #[test] fn camera_frame_after_touch_credits_4_bits() {
        let mut s = Session::new(); s.start_arcade(WordCount::TwentyFour);
        s.on_camera_frame(&frame(1), 0); // health check needs a previous frame
        s.on_touch(5, 5, 90_000); s.on_touch(9, 12, 100_000);
        assert_eq!(s.on_camera_frame(&frame(2), 300_000), SampleResult::Credited(4));
        assert_eq!(s.pool().unwrap().camera_bits(), 4);
    }
    #[test] fn camera_frame_without_recent_touch_credits_nothing() {
        let mut s = Session::new(); s.start_arcade(WordCount::TwentyFour);
        assert_eq!(s.on_camera_frame(&frame(1), 0), SampleResult::Uncredited);
        s.on_touch(5, 5, 90_000); s.on_touch(9, 12, 100_000);
        assert_eq!(s.on_camera_frame(&frame(2), 400_000), SampleResult::Uncredited); // 300 ms: stale touch
        assert_eq!(s.pool().unwrap().camera_bits(), 0);
    }
    #[test] fn camera_credit_caps_at_half_target() {
        let mut s = Session::new(); s.start_arcade(WordCount::Twelve);
        for i in 0..200u32 {
            let t = i as u64 * 20_000;
            for k in 0..3u32 { // varied touches so the touch health check passes
                let mut h = (i * 3 + k).wrapping_mul(0x9e37_79b1); h ^= h >> 15; h = h.wrapping_mul(0x85eb_ca6b); h ^= h >> 13;
                s.on_touch((h % 480) as i32, ((h >> 12) % 800) as i32, t + k as u64 * 1_000);
            }
            s.on_camera_frame(&frame(i), t + 5_000);
        }
        assert_eq!(s.pool().unwrap().camera_bits(), s.target_bits() / 2);
    }
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
    #[test] fn outguess_pick_feeds_pool() {
        use sha2::Digest;
        let mut s = Session::new(); s.start_arcade(WordCount::Twelve);
        for i in 0..64u32 { s.outguess_pick(sha2::Sha256::digest(i.to_le_bytes())[0] & 3, 1, 2, i as u64); }
        assert!(s.pool().unwrap().game_bits() > 0);
    }
    #[test] fn on_shot_feeds_pool() { let mut s = Session::new(); assert_eq!(s.on_shot(0, 0, 1, 2, 0), SampleResult::Uncredited); s.start_arcade(WordCount::Twelve); s.on_shot(5, 5, 1, 2, 0); s.on_shot(9, 12, 4, 7, 1); assert_eq!(s.pool().unwrap().game_bits(), 1); }
    #[test] fn dice_session_matches_core() {
        let r: Vec<u8> = "12345612345612345612345612345612345612345612345612".bytes().map(|b| b - b'0').collect();
        let mut s = Session::new(); s.start_dice(WordCount::Twelve);
        assert!(!s.push_roll(7) && !s.push_roll(0));
        for &x in &r[..49] { assert!(s.push_roll(x)); }
        assert!(s.generate_from_dice().is_err() && s.words().is_empty());
        assert!(s.push_roll(r[49]));
        s.generate_from_dice().unwrap();
        assert_eq!(s.words(), &*words(&dice_entropy(&r, WordCount::Twelve).unwrap()));
        assert!(s.rolls().is_empty());
        s.push_roll(1); s.wipe(); assert!(s.rolls().is_empty() && s.words().is_empty());
    }
    #[test]
    fn dice_roll_storage_never_reallocates() {
        let mut s = Session::new();
        s.start_dice(WordCount::TwentyFour);
        let storage = s.rolls.as_ptr();
        for i in 0..255 {
            assert!(s.push_roll((i % 6 + 1) as u8));
            assert_eq!(s.rolls.as_ptr(), storage, "rolls must stay in their wipeable allocation");
        }
        assert!(!s.push_roll(1));
        assert_eq!(s.rolls().len(), 255);
        s.wipe();
        assert!(s.rolls().is_empty());
        assert_eq!(s.rolls.as_ptr(), storage);
    }

    #[test]
    fn undo_zeroizes_removed_roll() {
        let mut s = Session::new();
        s.start_dice(WordCount::Twelve);
        s.push_roll(6);
        let slot = s.rolls.as_ptr();
        s.pop_roll();
        assert!(s.rolls().is_empty());
        // SAFETY: pop keeps this allocation alive, and this u8 slot was initialized above.
        assert_eq!(unsafe { slot.read() }, 0);
    }

    #[test]
    fn dice_24_word_session_requires_99_rolls_and_matches_full_hash() {
        use sha2::{Digest, Sha256};
        let r: Vec<u8> = (0..99).map(|i| (i % 6 + 1) as u8).collect();
        let mut s = Session::new();
        s.start_dice(WordCount::TwentyFour);
        for &roll in &r[..98] { assert!(s.push_roll(roll)); }
        assert!(s.generate_from_dice().is_err());
        assert!(s.words().is_empty());
        assert_eq!(s.rolls().len(), 98);
        assert!(s.push_roll(r[98]));
        s.generate_from_dice().unwrap();
        let hash = Sha256::digest(r.iter().map(|r| b'0' + r).collect::<Vec<_>>());
        assert_eq!(s.entropy.as_ref().unwrap().as_slice(), &hash[..]);
        assert_eq!(s.words(), &*words(&hash));
        assert_eq!(s.words().len(), 24);
        assert!(s.rolls().is_empty());
    }
    #[test] fn fingerprint_matches_entropy() {
        let s = gen();
        let e = s.entropy.as_ref().unwrap();
        assert_eq!(s.fingerprint().unwrap().0, master_fingerprint(e));
        assert_eq!(&*crate::words(e), s.words());
    }
}

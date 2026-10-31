//! App session: owns the pool and the generated secrets; never logs them.

use zeroize::{Zeroize, Zeroizing};

use crate::{
    dice_entropy, master_fingerprint, make_quiz, outguess::{Duel, DuelRound, DuelTap, Move, Oracle}, randomart, seedqr_digits, words, shard::{Board, ShardPolicies}, mcv::{Estimate, Mcv}, EntropyPool, QuizQuestion, SampleResult, WordCount,
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
    duel: Duel,
    pub(crate) shard: Board,
    pub(crate) shard_piece: u8,
    shard_pol: ShardPolicies,
    shard_mcv: Mcv,
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
        Self { pool: None, wc: WordCount::Twelve, entropy: None, words: Zeroizing::new(Vec::new()), page: 0, quiz: Vec::new(), step: 0, oracle: Oracle::default(), duel: Duel::default(), shard: Board::default(), shard_piece: 0, shard_pol: ShardPolicies::default(), shard_mcv: Mcv::new(16, 3000), rolls: Zeroizing::new(Vec::with_capacity(MAX_ROLLS)) }
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

    pub fn duel(&self) -> &Duel { &self.duel }

    /// Commits a complete locked pair. A finished match ignores further input.
    pub fn duel_round(
        &mut self,
        hider_door: u8,
        seeker_guess: u8,
        hider_tap: DuelTap,
        seeker_tap: DuelTap,
    ) -> DuelRound {
        let active = self.duel.winner().is_none();
        let hider = self.duel.hider();
        let result = self.duel.round(hider_door, seeker_guess, hider_tap, seeker_tap);
        if active {
            let mut doors = [0; 2];
            let mut taps = [(0, 0, 0); 2];
            doors[hider as usize] = hider_door;
            doors[1 - hider as usize] = seeker_guess;
            taps[hider as usize] = hider_tap;
            taps[1 - hider as usize] = seeker_tap;
            if let Some(pool) = self.pool.as_mut() {
                pool.add_duel_round(hider, doors, taps, result.credit_mbits, result.healthy);
            }
            doors.zeroize();
            taps.zeroize();
        }
        result
    }

    pub fn duel_next_match(&mut self) -> bool { self.duel.next_match() }

    /// Sets the piece to place (from the app's game PRNG; never credited). A piece with no legal placement clears the board.
    pub fn shard_spawn(&mut self, piece: u8) {
        self.shard_piece = piece;
        if self.shard.placements(piece).is_empty() {
            self.shard.zeroize();
        }
    }

    /// Places the current piece. Its placement rank is scored before the drop; credit = min(rank MCV, policy bound).
    /// `None` if the placement is illegal.
    pub fn shard_place(&mut self, rot: u8, col: u8, t_us: u64) -> Option<Estimate> {
        let piece = self.shard_piece;
        self.shard.landing(piece, rot, col)?;
        let rank = self.shard.rank(piece, rot, col);
        self.shard_pol.observe(&self.shard, piece, rot, col, rank);
        let m = self.shard_mcv.push(rank as u16);
        let (ceil, ok) = self.shard_pol.bound().unwrap_or((0, true));
        let healthy = m.healthy && ok;
        let e = Estimate { credit_mbits: if healthy { m.credit_mbits.min(ceil).min(3000) } else { 0 }, healthy };
        if let Some(p) = self.pool.as_mut() {
            p.add_shard(piece, rot, col, t_us, e.credit_mbits, e.healthy);
        }
        self.shard.place(piece, rot, col);
        Some(e)
    }

    /// Absorbs a Shard Stack drag (`kind` 0, `arg` = column delta) or rotation (`kind` 1) with its time. Never credited.
    pub fn shard_move(&mut self, kind: u8, arg: i32, t_us: u64) {
        if let Some(p) = self.pool.as_mut() {
            p.add_shard_move(kind, arg, t_us);
        }
    }

    pub fn shard_cells(&self) -> [u8; 128] {
        self.shard.cells()
    }

    /// Display-only: can the current piece at (rot, col) be at top row `row` after a straight fall from the top?
    /// (Then a lock there equals the hard-drop landing the core models.)
    pub fn shard_fits(&self, rot: u8, col: u8, row: u8) -> bool {
        (0..=row).all(|r| self.shard.fits_at(self.shard_piece, rot, col, r))
    }

    /// Display-only: top row where a hard drop of the current piece at (rot, col) rests.
    pub fn shard_land_row(&self, rot: u8, col: u8) -> Option<u8> {
        self.shard.land_row(self.shard_piece, rot, col)
    }

    /// Display-only (scored pieces, warm-up length, % the best solver predictor hit).
    pub fn shard_progress(&self) -> (u32, u32, u8) {
        let (n, pct) = self.shard_pol.progress();
        (n, crate::shard::WARMUP, pct)
    }

    /// Current hard-drop geometry only: does not place, rank, absorb, or credit anything.
    pub fn shard_preview(&self, rot: u8, col: u8) -> Zeroizing<[u8; 128]> {
        let mut preview = Zeroizing::new([0u8; 128]);
        if let Some(bottom) = self.shard.landing(self.shard_piece, rot, col) {
            let cells = crate::shard::shape(self.shard_piece, rot);
            let top = bottom - cells.iter().map(|c| c.1).max().unwrap_or(0);
            for (x, y) in cells {
                preview[(top + y) as usize * 8 + (col + x) as usize] = 1;
            }
        }
        preview
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
        self.duel.wipe();
        self.shard.zeroize();
        self.shard_piece = 0;
        self.shard_pol.zeroize();
        self.shard_mcv.wipe();
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
    #[test]
    fn shard_preview_matches_actual_drop_without_crediting() {
        let mut s = Session::new();
        s.start_arcade(WordCount::Twelve);
        s.shard_spawn(0);
        let before = s.pool().unwrap().credited_bits();
        let preview = s.shard_preview(0, 2);
        assert_eq!(preview.iter().filter(|&&v| v == 1).count(), 3);
        assert!(preview[122..125].iter().all(|&v| v == 1));
        assert_eq!(s.pool().unwrap().credited_bits(), before);
        assert!(s.shard_cells().iter().all(|&v| v == 0));
        s.shard_place(0, 2, 1).unwrap();
        assert_eq!(&s.shard_cells(), &*preview);
    }

    #[test]
    fn shard_preview_never_absorbs_or_credits() {
        let play = |preview: bool| {
            let mut s = Session::new();
            s.start_arcade(WordCount::Twelve);
            for t in 0..40u64 {
                s.shard_spawn((t % 7) as u8);
                let before = s.pool().unwrap().credited_bits();
                if preview {
                    for rot in 0..4 { for col in 0..8 { let _ = s.shard_preview(rot, col); } }
                }
                assert_eq!(s.pool().unwrap().credited_bits(), before);
                let (rot, col) = s.shard.placements(s.shard_piece)[0];
                s.shard_place(rot, col, t * 1_000).unwrap();
            }
            let bits = s.pool().unwrap().credited_bits();
            s.generate(Ok([7; 32])).unwrap();
            (bits, s.words().to_vec())
        };
        assert_eq!(play(true), play(false));
    }

    #[test]
    fn blocked_shard_preview_is_empty() {
        let mut s = Session::new();
        s.start_arcade(WordCount::Twelve);
        for t in 0..5 {
            s.shard_spawn(0);
            s.shard_place(1, 0, t).unwrap();
        }
        assert!(s.shard_preview(1, 0).iter().all(|&v| v == 0));
    }
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

    #[test]
    fn duel_session_accumulates_both_players_and_retains_credit_across_matches() {
        use sha2::{Digest, Sha256};
        let mut s = Session::new();
        s.start_arcade(WordCount::TwentyFour);
        let mut total = 0u32;
        for i in 0..128u32 {
            if s.duel().winner().is_some() {
                let before = s.pool().unwrap().game_bits();
                assert!(s.duel_next_match());
                assert_eq!(s.pool().unwrap().game_bits(), before);
            }
            let hash = Sha256::digest(i.to_le_bytes());
            let doors = [hash[0] & 3, hash[1] & 3];
            let h = s.duel().hider() as usize;
            let r = s.duel_round(doors[h], doors[1 - h], (1, 2, i as u64), (3, 4, i as u64 + 1));
            total += r.credit_mbits.into_iter().sum::<u32>();
            assert_eq!(s.pool().unwrap().game_bits(), total / 1000);
        }
        assert!(total >= 100_000);
        s.wipe();
        assert!(s.pool().is_none());
        assert_eq!(s.duel().scores(), [0, 0]);
        assert_eq!(s.duel().round_number(), 0);
        s.start_arcade(WordCount::Twelve);
        assert_eq!(s.duel_round(1, 2, (0, 0, 0), (0, 0, 0)).credit_mbits, [0, 0]);
        assert_eq!(s.pool().unwrap().game_bits(), 0);
    }

    #[test]
    fn duel_session_ignores_input_after_match_winner() {
        let mut a = Session::new();
        let mut b = Session::new();
        a.start_arcade(WordCount::Twelve);
        b.start_arcade(WordCount::Twelve);
        while a.duel().winner().is_none() {
            a.duel_round(1, 2, (1, 2, 3), (4, 5, 6));
            b.duel_round(1, 2, (1, 2, 3), (4, 5, 6));
        }
        assert_eq!(a.duel_round(0, 3, (99, 99, 99), (88, 88, 88)).credit_mbits, [0, 0]);
        a.generate(Ok([7; 32])).unwrap();
        b.generate(Ok([7; 32])).unwrap();
        assert_eq!(a.words(), b.words());
    }
}

//! Backup quiz: 3 random word positions, each a 4-way multiple choice (no system keyboard).

use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

const QUESTIONS: usize = 3;

/// Holds real seed words, so it wipes itself on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct QuizQuestion {
    /// 1-based word position.
    pub position: usize,
    pub choices: [String; 4],
    pub correct: usize,
}

/// Deterministic in `rand`, which is expanded as SHA-256(rand ‖ counter) so it never runs dry.
/// Quiz positions and decoys aren't secret, so modulo bias doesn't matter here.
pub fn make_quiz(words: &[String], rand: &[u8; 32]) -> Vec<QuizQuestion> {
    let mut counter = 0u32;
    let mut next = || {
        counter += 1;
        let block = Sha256::new().chain_update(rand).chain_update(counter.to_le_bytes()).finalize();
        u32::from_le_bytes(block[..4].try_into().unwrap()) as usize
    };
    let wordlist = bip39::Language::English.word_list();

    let mut positions: Vec<usize> = Vec::with_capacity(QUESTIONS);
    while positions.len() < QUESTIONS {
        let p = next() % words.len() + 1;
        if !positions.contains(&p) {
            positions.push(p);
        }
    }

    positions
        .into_iter()
        .map(|position| {
            let answer = words[position - 1].clone();
            let correct = next() % 4;
            let mut choices: Vec<String> = Vec::with_capacity(4);
            while choices.len() < 3 {
                let w = wordlist[next() % wordlist.len()];
                if w != answer && !choices.iter().any(|c| c == w) {
                    choices.push(w.to_string());
                }
            }
            choices.insert(correct, answer);
            QuizQuestion { position, choices: choices.try_into().unwrap(), correct }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::words;

    #[test]
    fn quiz_properties() {
        for (seed, n) in [(0u8, 16usize), (1, 32), (77, 16), (200, 32), (255, 16)] {
            let w = words(&[seed; 32][..n]);
            let rand: [u8; 32] = std::array::from_fn(|i| (i as u8).wrapping_mul(37).wrapping_add(seed));
            let q = make_quiz(&w, &rand);
            assert_eq!(q.len(), 3);
            let mut positions: Vec<usize> = q.iter().map(|x| x.position).collect();
            positions.sort();
            positions.dedup();
            assert_eq!(positions.len(), 3, "positions must be distinct");
            for x in &q {
                assert!((1..=w.len()).contains(&x.position));
                assert_eq!(x.choices[x.correct], w[x.position - 1]);
                let mut c = x.choices.to_vec();
                c.sort();
                c.dedup();
                assert_eq!(c.len(), 4, "choices must be distinct");
            }
        }
    }

    #[test]
    fn quiz_question_zeroizes_on_drop() {
        fn assert_zod<T: zeroize::ZeroizeOnDrop>() {}
        assert_zod::<QuizQuestion>();
    }

    #[test]
    fn quiz_all_zero_rand_still_works() {
        let w = words(&[9u8; 16]);
        assert_eq!(make_quiz(&w, &[0u8; 32]).len(), 3);
    }
}

//! Entropy Arcade core: pool, health tests, mixing and seed outputs.
//! Pure Rust, no KeyOS deps, so it is tested on the host with `cargo test -p entropy-core`.

pub mod dice;
pub mod feedback;
pub mod glide;
pub mod health;
pub mod mcv;
pub mod outguess;
pub mod pool;
pub mod quiz;
pub mod randomart;
pub mod session;
pub mod seed;

pub use mcv::{Estimate, Mcv};
pub use dice::{dice_entropy, min_rolls, DiceError};
pub use outguess::{Move, Oracle};
pub use pool::{EntropyPool, SampleResult};
pub use quiz::{make_quiz, QuizQuestion};
pub use randomart::randomart;
pub use session::{QuizOutcome, Session, WORDS_PER_PAGE};
pub use seed::{master_fingerprint, qr_matrix, seedqr_digits, words};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WordCount {
    Twelve,
    TwentyFour,
}

impl WordCount {
    pub fn target_bits(self) -> u32 {
        match self {
            WordCount::Twelve => 128,
            WordCount::TwentyFour => 256,
        }
    }

    pub fn entropy_len(self) -> usize {
        self.target_bits() as usize / 8
    }
}

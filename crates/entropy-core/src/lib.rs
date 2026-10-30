//! Entropy Arcade core: pool, health tests, mixing and seed outputs.
//! Pure Rust, no KeyOS deps, so it is tested on the host with `cargo test -p entropy-core`.

pub mod health;
pub mod pool;

pub use pool::{EntropyPool, SampleResult};

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

//! Dice Duel: seed = SHA-256(ASCII digits of the rolls), Coldcard-compatible and verifiable on a laptop.
//! No hardware RNG is mixed in here, by design.

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::WordCount;

#[derive(Debug, PartialEq, Eq)]
pub enum DiceError {
    BadRoll(u8),
    TooFew { have: usize, need: usize },
}

/// Coldcard's minimums: 50 rolls (~129 bits) for 12 words, 99 rolls (~256 bits) for 24.
pub fn min_rolls(wc: WordCount) -> usize {
    match wc {
        WordCount::Twelve => 50,
        WordCount::TwentyFour => 99,
    }
}

/// `SHA-256` of the rolls as ASCII digits, truncated to the word count's entropy length.
pub fn dice_entropy(rolls: &[u8], wc: WordCount) -> Result<Zeroizing<Vec<u8>>, DiceError> {
    if let Some(&bad) = rolls.iter().find(|r| !(1..=6).contains(*r)) {
        return Err(DiceError::BadRoll(bad));
    }
    let need = min_rolls(wc);
    if rolls.len() < need {
        return Err(DiceError::TooFew { have: rolls.len(), need });
    }
    let ascii = Zeroizing::new(rolls.iter().map(|r| b'0' + r).collect::<Vec<u8>>());
    let mut out = Zeroizing::new(Sha256::digest(&*ascii).to_vec());
    out.truncate(wc.entropy_len());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn dice_coldcard_vector() {
        let rolls: Vec<u8> = "12345612345612345612345612345612345612345612345612"
            .bytes()
            .map(|b| b - b'0')
            .collect();
        let e = dice_entropy(&rolls, WordCount::Twelve).unwrap();
        // first 16 bytes of `echo -n <rolls> | sha256sum`
        assert_eq!(hex(&e), "ee72ae915a4e6ea7ccbeb8e5e5eecef2");
    }

    #[test]
    fn dice_24_uses_full_hash() {
        let rolls = vec![3u8; 99];
        assert_eq!(dice_entropy(&rolls, WordCount::TwentyFour).unwrap().len(), 32);
    }

    #[test]
    fn dice_rejects_bad_input() {
        assert_eq!(dice_entropy(&[7; 99], WordCount::TwentyFour), Err(DiceError::BadRoll(7)));
        assert_eq!(dice_entropy(&[0; 99], WordCount::TwentyFour), Err(DiceError::BadRoll(0)));
        assert_eq!(
            dice_entropy(&[1; 49], WordCount::Twelve),
            Err(DiceError::TooFew { have: 49, need: 50 })
        );
    }

    #[test]
    fn min_rolls_match_coldcard() {
        assert_eq!(min_rolls(WordCount::Twelve), 50);
        assert_eq!(min_rolls(WordCount::TwentyFour), 99);
    }
}

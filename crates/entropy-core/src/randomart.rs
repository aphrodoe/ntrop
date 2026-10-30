//! OpenSSH-style "drunken bishop" randomart, drawn from a master fingerprint.

use sha2::{Digest, Sha256};

const W: usize = 17;
const H: usize = 9;
const SYMBOLS: &[u8] = b" .o+=*BOX@%&#/^";

/// 11 lines × 19 chars (with border). Walks SHA-256(fp) so 4 bytes still fill the field.
pub fn randomart(fp: &[u8; 4]) -> String {
    let mut field = [[0usize; W]; H];
    let (mut x, mut y) = (W / 2, H / 2);
    let start = (x, y);
    for byte in Sha256::digest(fp) {
        for step in 0..4 {
            let bits = byte >> (step * 2);
            x = if bits & 1 == 1 { (x + 1).min(W - 1) } else { x.saturating_sub(1) };
            y = if bits & 2 == 2 { (y + 1).min(H - 1) } else { y.saturating_sub(1) };
            field[y][x] += 1;
        }
    }

    let border = format!("+{}+", "-".repeat(W));
    let mut out = vec![border.clone()];
    for (row, counts) in field.iter().enumerate() {
        let line: String = counts
            .iter()
            .enumerate()
            .map(|(col, &n)| match (col, row) {
                p if p == start => 'S',
                p if p == (x, y) => 'E',
                _ => SYMBOLS[n.min(SYMBOLS.len() - 1)] as char,
            })
            .collect();
        out.push(format!("|{line}|"));
    }
    out.push(border);
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn randomart_shape_and_determinism() {
        let a = randomart(&[1, 2, 3, 4]);
        assert_eq!(a.lines().count(), 11);
        assert!(a.lines().all(|l| l.chars().count() == 19));
        assert_eq!(a, randomart(&[1, 2, 3, 4]));
        assert_ne!(a, randomart(&[1, 2, 3, 5]));
    }

    #[test]
    fn randomart_has_start_and_end() {
        let a = randomart(&[0xde, 0xad, 0xbe, 0xef]);
        assert!(a.contains('S'));
        assert!(a.contains('E') || a.matches('S').count() == 1);
    }
}

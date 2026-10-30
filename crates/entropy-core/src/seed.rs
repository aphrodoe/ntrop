//! Seed outputs: BIP39 words, Standard SeedQR digits, QR matrix, BIP32 master fingerprint.

use bip39::Mnemonic;
use hmac::{Hmac, Mac};
use k256::elliptic_curve::sec1::ToEncodedPoint;
use ripemd::Ripemd160;
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

/// Entropy must be 16 or 32 bytes (callers only produce those); anything else is a bug.
fn mnemonic(entropy: &[u8]) -> Mnemonic {
    Mnemonic::from_entropy(entropy).expect("entropy must be 16 or 32 bytes")
}

pub fn words(entropy: &[u8]) -> Zeroizing<Vec<String>> {
    Zeroizing::new(mnemonic(entropy).words().map(String::from).collect())
}

/// Standard SeedQR: each word's BIP39 index as 4 zero-padded digits.
pub fn seedqr_digits(entropy: &[u8]) -> Zeroizing<String> {
    Zeroizing::new(mnemonic(entropy).word_indices().map(|i| format!("{i:04}")).collect())
}

/// QR modules, row-major, `true` = dark.
pub fn qr_matrix(data: &str) -> (usize, Vec<bool>) {
    let code = qrcode::QrCode::new(data).expect("SeedQR data fits in a QR code");
    let dark = code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect();
    (code.width(), dark)
}

/// BIP32 master key fingerprint (empty passphrase): first 4 bytes of HASH160(master pubkey).
pub fn master_fingerprint(entropy: &[u8]) -> [u8; 4] {
    let seed = Zeroizing::new(mnemonic(entropy).to_seed(""));
    let mut mac = Hmac::<Sha512>::new_from_slice(b"Bitcoin seed").expect("HMAC takes any key length");
    mac.update(&*seed);
    let master = Zeroizing::new(mac.finalize().into_bytes());
    let key = k256::SecretKey::from_slice(&master[..32]).expect("master key is a valid scalar");
    let pubkey = key.public_key().to_encoded_point(true);
    let hash160 = Ripemd160::digest(Sha256::digest(pubkey.as_bytes()));
    hash160[..4].try_into().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_zero_16() {
        let w = words(&[0u8; 16]);
        assert_eq!(w.len(), 12);
        assert_eq!(w[11], "about");
        assert!(w[..11].iter().all(|x| x == "abandon"));
    }

    #[test]
    fn words_zero_32() {
        let w = words(&[0u8; 32]);
        assert_eq!(w.len(), 24);
        assert_eq!(w[23], "art");
    }

    #[test]
    fn seedqr_abandon_about() {
        assert_eq!(&*seedqr_digits(&[0u8; 16]), &format!("{}0003", "0000".repeat(11)));
    }

    #[test]
    fn fingerprint_abandon_about() {
        assert_eq!(master_fingerprint(&[0u8; 16]), [0x73, 0xc5, 0xda, 0x0a]);
    }

    #[test]
    fn qr_is_square() {
        let (n, m) = qr_matrix("0123");
        assert!(n >= 21);
        assert_eq!(m.len(), n * n);
    }
}

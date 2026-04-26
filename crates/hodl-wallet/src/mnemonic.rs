//! BIP-39 mnemonic generation and seed derivation.
//!
//! Wraps the `bip39` crate. Accepts only 12-word (128-bit entropy) and
//! 24-word (256-bit entropy) mnemonics per PLAN.md — other lengths are
//! rejected on input.

use bip39::{Language, Mnemonic};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::{Result, WalletError};

/// Number of words in a BIP-39 mnemonic.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum WordCount {
    Twelve,
    TwentyFour,
}

impl WordCount {
    /// Entropy size in bytes.
    pub fn entropy_bytes(self) -> usize {
        match self {
            WordCount::Twelve => 16,
            WordCount::TwentyFour => 32,
        }
    }

    pub fn words(self) -> usize {
        match self {
            WordCount::Twelve => 12,
            WordCount::TwentyFour => 24,
        }
    }
}

/// Plaintext seed bytes (64 bytes per BIP-39).
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Seed(pub [u8; 64]);

impl Seed {
    pub fn as_bytes(&self) -> &[u8; 64] {
        &self.0
    }
}

impl std::fmt::Debug for Seed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seed").field("len", &self.0.len()).finish()
    }
}

/// Generate a fresh mnemonic of the requested length using OS RNG.
pub fn generate(words: WordCount) -> Result<Mnemonic> {
    let mut entropy = vec![0u8; words.entropy_bytes()];
    use rand::RngCore;
    rand::thread_rng().fill_bytes(&mut entropy);
    let m = Mnemonic::from_entropy_in(Language::English, &entropy)
        .map_err(|e| WalletError::Mnemonic(e.to_string()))?;
    entropy.zeroize();
    Ok(m)
}

/// Parse and validate a mnemonic phrase (English wordlist, 12 or 24 words).
pub fn parse(phrase: &str) -> Result<Mnemonic> {
    let trimmed = phrase.trim();
    let count = trimmed.split_whitespace().count();
    match count {
        12 | 24 => {}
        n => {
            return Err(WalletError::Mnemonic(format!(
                "unsupported word count {n}; only 12 or 24 are accepted"
            )));
        }
    }
    Mnemonic::parse_in(Language::English, trimmed).map_err(|e| WalletError::Mnemonic(e.to_string()))
}

/// Derive 64-byte BIP-39 seed from a parsed mnemonic and optional passphrase.
pub fn to_seed(mnemonic: &Mnemonic, passphrase: &str) -> Seed {
    Seed(mnemonic.to_seed(passphrase))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Official BIP-39 English test vectors (subset of the Trezor vectors).
    /// Source: https://github.com/trezor/python-mnemonic/blob/master/vectors.json
    /// Tuples: (entropy_hex, mnemonic, seed_hex). Passphrase = "TREZOR".
    const VECTORS: &[(&str, &str, &str)] = &[
        (
            "00000000000000000000000000000000",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
            "c55257c360c07c72029aebc1b53c05ed0362ada38ead3e3e9efa3708e53495531f09a6987599d18264c1e1c92f2cf141630c7a3c4ab7c81b2f001698e7463b04",
        ),
        (
            "7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f7f",
            "legal winner thank year wave sausage worth useful legal winner thank yellow",
            "2e8905819b8723fe2c1d161860e5ee1830318dbf49a83bd451cfb8440c28bd6fa457fe1296106559a3c80937a1c1069be3a3a5bd381ee6260e8d9739fce1f607",
        ),
        (
            "0000000000000000000000000000000000000000000000000000000000000000",
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon art",
            "bda85446c68413707090a52022edd26a1c9462295029f2e60cd7c4f2bbd3097170af7a4d73245cafa9c3cca8d561a7c3de6f5d4a10be8ed2a5e608d68f92fcc8",
        ),
        (
            "8080808080808080808080808080808080808080808080808080808080808080",
            "letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic avoid letter advice cage absurd amount doctor acoustic bless",
            "c0c519bd0e91a2ed54357d9d1ebef6f5af218a153624cf4f2da911a0ed8f7a09e2ef61af0aca007096df430022f7a2b6fb91661a9589097069720d015e4e982f",
        ),
        (
            "9e885d952ad362caeb4efe34a8e91bd2",
            "ozone drill grab fiber curtain grace pudding thank cruise elder eight picnic",
            "274ddc525802f7c828d8ef7ddbcdc5304e87ac3535913611fbbfa986d0c9e5476c91689f9c8a54fd55bd38606aa6a8595ad213d4c9c9f9aca3fb217069a41028",
        ),
    ];

    #[test]
    fn bip39_vectors_match() {
        for (entropy_hex, phrase, seed_hex) in VECTORS {
            // Wait: vector 4's mnemonic has 24 words, seed entry should align.
            let entropy = hex::decode(entropy_hex).unwrap();
            let m = bip39::Mnemonic::from_entropy_in(bip39::Language::English, &entropy).unwrap();
            assert_eq!(
                m.to_string(),
                *phrase,
                "entropy {entropy_hex} mnemonic round-trip"
            );

            let parsed = parse(phrase).unwrap();
            assert_eq!(parsed.to_string(), *phrase);

            let seed = to_seed(&parsed, "TREZOR");
            assert_eq!(hex::encode(seed.as_bytes()), *seed_hex);
        }
    }

    #[test]
    fn rejects_18_words() {
        // Generate by stitching 18 valid words together; should fail length check
        // even before checksum validation.
        let phrase = "abandon ".repeat(17) + "about";
        let err = parse(&phrase).unwrap_err();
        match err {
            WalletError::Mnemonic(msg) => assert!(msg.contains("18")),
            _ => panic!("expected Mnemonic error"),
        }
    }

    #[test]
    fn rejects_bad_checksum() {
        let bad = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon";
        assert!(parse(bad).is_err());
    }

    #[test]
    fn generate_12_words() {
        let m = generate(WordCount::Twelve).unwrap();
        assert_eq!(m.to_string().split_whitespace().count(), 12);
    }

    #[test]
    fn generate_24_words() {
        let m = generate(WordCount::TwentyFour).unwrap();
        assert_eq!(m.to_string().split_whitespace().count(), 24);
    }
}

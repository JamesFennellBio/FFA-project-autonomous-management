//! CRYPTO-REVIEW: hub identity from a 24-word BIP39 seed phrase.
//!
//! If the Pi dies, flash a new SD card, run `hub-node restore`, and type the
//! 24 words. You get the same Ed25519 key back, so every node (which pinned
//! the hub's public key at pairing) keeps working without re-pairing.
//!
//! phrase -> BIP39 seed (PBKDF2-HMAC-SHA512, 2048 rounds, empty passphrase)
//!        -> HKDF-SHA256(info = "FFA hub identity v1") -> 32-byte Ed25519 seed

use std::path::Path;

use anyhow::{Context, Result};
use bip39::Mnemonic;
use hkdf::Hkdf;
use sha2::Sha256;
use zeroize::Zeroize;

pub const IDENTITY_FILE: &str = "identity.key";

/// Makes a new 24-word phrase from 256 bits of OS randomness.
pub fn generate() -> Result<Mnemonic> {
    let mut entropy = [0u8; 32];
    uplink::getrandom(&mut entropy)?;
    let m = Mnemonic::from_entropy(&entropy)?;
    entropy.zeroize();
    Ok(m)
}

pub fn parse(words: &str) -> Result<Mnemonic> {
    let cleaned = words
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    Mnemonic::parse_normalized(&cleaned)
        .context("not a valid 24-word BIP39 phrase (check spelling/order)")
}

pub fn identity_seed(m: &Mnemonic) -> [u8; 32] {
    let mut bip_seed = m.to_seed_normalized("");
    let hk = Hkdf::<Sha256>::new(Some(b"FFA-HUB"), &bip_seed);
    let mut out = [0u8; 32];
    hk.expand(b"FFA hub identity v1", &mut out)
        .expect("hkdf length");
    bip_seed.zeroize();
    out
}

/// Saves the derived seed (not the phrase) with 0600 permissions.
pub fn save(data_dir: &Path, seed: &[u8; 32]) -> Result<()> {
    uplink::write_secret(&data_dir.join(IDENTITY_FILE), &hex::encode(seed))
}

pub fn load(data_dir: &Path) -> Result<[u8; 32]> {
    let p = data_dir.join(IDENTITY_FILE);
    let s = std::fs::read_to_string(&p).with_context(|| {
        format!(
            "{} missing; run `hub-node init` (new hub) or `hub-node restore` first",
            p.display()
        )
    })?;
    uplink::key_from_hex(s.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_gives_same_identity() {
        let m = generate().unwrap();
        assert_eq!(m.word_count(), 24);
        let phrase = m.to_string();
        let again = parse(&format!("  {}  ", phrase.to_uppercase())).unwrap();
        assert_eq!(identity_seed(&m), identity_seed(&again));
    }
}

//! CRYPTO-REVIEW: long-term Ed25519 identities for nodes, the hub and the PC daemon.

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::{Error, Result};

pub const PUBLIC_KEY_LEN: usize = 32;
pub const SIGNATURE_LEN: usize = 64;
pub const SEED_LEN: usize = 32;

/// A device's long-term signing key.
///
/// CRYPTO-REVIEW: the 32-byte seed is the whole secret. Nodes keep it in flash,
/// the hub derives it from its BIP39 phrase, the PC keeps it in its config dir.
/// `SigningKey` zeroizes itself on drop (ed25519-dalek "zeroize" feature).
pub struct Identity {
    sk: SigningKey,
}

impl Identity {
    /// CRYPTO-REVIEW: `seed` must come from a CSPRNG (or the BIP39 phrase on the hub).
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        Self {
            sk: SigningKey::from_bytes(seed),
        }
    }

    /// Raw seed, for saving to storage. Handle as a secret.
    pub fn seed(&self) -> [u8; SEED_LEN] {
        self.sk.to_bytes()
    }

    pub fn public_key(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.sk.verifying_key().to_bytes()
    }

    /// CRYPTO-REVIEW: plain Ed25519 (RFC 8032) over the message. Every signed
    /// message starts with a unique ASCII label (domain separation), so a
    /// signature from one message type can't be passed off as another.
    pub fn sign(&self, msg: &[u8]) -> [u8; SIGNATURE_LEN] {
        self.sk.sign(msg).to_bytes()
    }
}

/// CRYPTO-REVIEW: `verify_strict` rejects weak/small-order public keys and
/// non-canonical signatures (no malleability).
pub fn verify(public_key: &[u8; PUBLIC_KEY_LEN], msg: &[u8], sig: &[u8]) -> Result<()> {
    let vk = VerifyingKey::from_bytes(public_key).map_err(|_| Error::BadSignature)?;
    let sig: [u8; SIGNATURE_LEN] = sig.try_into().map_err(|_| Error::BadSignature)?;
    vk.verify_strict(msg, &Signature::from_bytes(&sig))
        .map_err(|_| Error::BadSignature)
}

/// Short fingerprint shown in the app and on serial logs so a person can check
/// that a pairing request really came from the device in front of them.
///
/// CRYPTO-REVIEW: 64 bits of SHA-256(public key). Fine for a person comparing it
/// by eye; it is never used as a key or an identifier in the protocol itself.
pub fn fingerprint(public_key: &[u8; PUBLIC_KEY_LEN]) -> [u8; 8] {
    let h = Sha256::digest(public_key);
    let mut out = [0u8; 8];
    out.copy_from_slice(&h[..8]);
    out
}

/// Fingerprint as `AB12-CD34-EF56-7890` (writes 19 ASCII bytes).
pub fn fingerprint_str(public_key: &[u8; PUBLIC_KEY_LEN]) -> [u8; 19] {
    format_fingerprint(&fingerprint(public_key))
}

/// Formats 8 fingerprint bytes as `AB12-CD34-EF56-7890` (also used by v2 keys).
pub(crate) fn format_fingerprint(fp: &[u8; 8]) -> [u8; 19] {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = [b'-'; 19];
    let mut o = 0;
    for (i, byte) in fp.iter().enumerate() {
        if i > 0 && i % 2 == 0 {
            o += 1; // leave the dash
        }
        out[o] = HEX[(byte >> 4) as usize];
        out[o + 1] = HEX[(byte & 0xF) as usize];
        o += 2;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify() {
        let id = Identity::from_seed(&[7u8; 32]);
        let sig = id.sign(b"FFA-TEST hello");
        assert!(verify(&id.public_key(), b"FFA-TEST hello", &sig).is_ok());
        assert_eq!(
            verify(&id.public_key(), b"FFA-TEST hellp", &sig),
            Err(Error::BadSignature)
        );
    }

    #[test]
    fn fingerprint_format() {
        let s = fingerprint_str(&[0u8; 32]);
        let s = core::str::from_utf8(&s).unwrap();
        assert_eq!(s.len(), 19);
        assert_eq!(s.matches('-').count(), 3);
    }
}

//! CRYPTO-REVIEW: hybrid long-term identity: Ed25519 + ML-DSA-65 (FIPS 204).
//!
//! ```text
//! seed (32 bytes, the whole secret)
//!   ├─ HKDF-SHA256(salt "FFA-v2-identity", info "FFA-v2 ed25519")   -> Ed25519 seed
//!   └─ HKDF-SHA256(salt "FFA-v2-identity", info "FFA-v2 ml-dsa-65") -> ML-DSA-65 seed ξ
//!
//! HybridPublic = ed25519_pub(32) || mldsa65_pub(1952)               1984 bytes
//! KeyId        = SHA-256("FFA-v2-KEYID" || HybridPublic)            32 bytes
//!
//! sign(label, parts):
//!   d   = SHA-512("FFA-v2-SIG" || len(label) || label || for each part: len32le || part)
//!   sig = Ed25519(d) || ML-DSA-65.Sign(d, ctx = label)              64 + 3309 bytes
//! ```
//!
//! Properties this aims for (please check):
//! - **AND-composition:** [`verify`] checks both signatures and fails if
//!   either fails. Forging needs a break of Ed25519 *and* ML-DSA-65.
//! - **Domain separation:** every message type has its own label. The label is
//!   in the digest and is also the ML-DSA context string.
//! - **Unambiguous transcripts:** each part is length-prefixed in the digest,
//!   so different splits of the same bytes give different digests.
//! - **Signing a digest:** both schemes sign the 64-byte SHA-512 digest rather
//!   than the multi-kilobyte transcript, so the crate needs no heap buffers.
//!   Collision resistance is 256 bits, above ML-DSA-65's category-3 target.
//! - **Deterministic ML-DSA:** FIPS 204's deterministic variant is used, so the
//!   crate stays sans-IO (no RNG needed to sign). Ed25519 is deterministic anyway.
//! - **One seed:** the hub's BIP39 recovery phrase restores both keys.

use ed25519_dalek::{Signer as _, SigningKey as EdSigningKey, VerifyingKey as EdVerifyingKey};
use hkdf::Hkdf;
use ml_dsa::{EncodedVerifyingKey, Keypair as _, MlDsa65, Signature as MlSignature};
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroize;

use crate::crypto::identity::{SEED_LEN, format_fingerprint};
use crate::{Error, Result};

pub const ED25519_PUBLIC_LEN: usize = 32;
pub const MLDSA65_PUBLIC_LEN: usize = 1952;
pub const PUBLIC_KEY_LEN: usize = ED25519_PUBLIC_LEN + MLDSA65_PUBLIC_LEN; // 1984
pub const ED25519_SIGNATURE_LEN: usize = 64;
pub const MLDSA65_SIGNATURE_LEN: usize = 3309;
pub const SIGNATURE_LEN: usize = ED25519_SIGNATURE_LEN + MLDSA65_SIGNATURE_LEN; // 3373
pub const KEY_ID_LEN: usize = 32;

/// Ed25519 public key followed by the ML-DSA-65 public key.
pub type HybridPublic = [u8; PUBLIC_KEY_LEN];
/// SHA-256 of a [`HybridPublic`]. This is what QR codes and config pin.
pub type KeyId = [u8; KEY_ID_LEN];

/// A device's long-term hybrid signing key.
///
/// CRYPTO-REVIEW: both inner keys zeroize on drop (ed25519-dalek and ml-dsa
/// "zeroize" features).
pub struct HybridIdentity {
    ed: EdSigningKey,
    ml: ml_dsa::SigningKey<MlDsa65>,
    public: HybridPublic,
}

impl HybridIdentity {
    /// CRYPTO-REVIEW: `seed` must come from a CSPRNG, or from the BIP39 phrase on the hub.
    pub fn from_seed(seed: &[u8; SEED_LEN]) -> Self {
        let hk = Hkdf::<Sha256>::new(Some(b"FFA-v2-identity"), seed);
        let mut ed_seed = [0u8; 32];
        let mut ml_seed = [0u8; 32];
        hk.expand(b"FFA-v2 ed25519", &mut ed_seed)
            .expect("hkdf length");
        hk.expand(b"FFA-v2 ml-dsa-65", &mut ml_seed)
            .expect("hkdf length");

        let ed = EdSigningKey::from_bytes(&ed_seed);
        let ml = ml_dsa::SigningKey::<MlDsa65>::from_seed(&ml_dsa::B32::from(ml_seed));
        ed_seed.zeroize();
        ml_seed.zeroize();

        let mut public = [0u8; PUBLIC_KEY_LEN];
        public[..ED25519_PUBLIC_LEN].copy_from_slice(ed.verifying_key().as_bytes());
        public[ED25519_PUBLIC_LEN..].copy_from_slice(ml.verifying_key().encode().as_slice());
        Self { ed, ml, public }
    }

    pub fn public_key(&self) -> &HybridPublic {
        &self.public
    }

    pub fn key_id(&self) -> KeyId {
        key_id(&self.public)
    }

    pub fn fingerprint_str(&self) -> [u8; 19] {
        fingerprint_str(&self.public)
    }

    /// CRYPTO-REVIEW: hybrid signature over the length-prefixed parts (see module docs).
    pub fn sign(&self, label: &[u8], parts: &[&[u8]]) -> [u8; SIGNATURE_LEN] {
        let d = digest(label, parts);
        let mut sig = [0u8; SIGNATURE_LEN];
        sig[..ED25519_SIGNATURE_LEN].copy_from_slice(&self.ed.sign(&d).to_bytes());
        // Only fails when the context is over 255 bytes; labels are short constants.
        let ml = self
            .ml
            .expanded_key()
            .sign_deterministic(&d, label)
            .expect("label fits the ML-DSA context");
        sig[ED25519_SIGNATURE_LEN..].copy_from_slice(ml.encode().as_slice());
        sig
    }
}

/// CRYPTO-REVIEW: the message both schemes sign. Each part is length-prefixed.
fn digest(label: &[u8], parts: &[&[u8]]) -> [u8; 64] {
    let mut h = Sha512::new();
    h.update(b"FFA-v2-SIG");
    h.update([label.len() as u8]);
    h.update(label);
    for p in parts {
        h.update((p.len() as u32).to_le_bytes());
        h.update(p);
    }
    h.finalize().into()
}

/// CRYPTO-REVIEW: succeeds only if **both** the Ed25519 signature
/// (`verify_strict`: no weak keys, no malleable signatures) and the ML-DSA-65
/// signature verify. Both are always checked, so timing doesn't show which one failed.
pub fn verify(public: &HybridPublic, label: &[u8], parts: &[&[u8]], sig: &[u8]) -> Result<()> {
    if sig.len() != SIGNATURE_LEN {
        return Err(Error::BadSignature);
    }
    let d = digest(label, parts);

    let ed_ok = (|| {
        let pk: [u8; ED25519_PUBLIC_LEN] = public[..ED25519_PUBLIC_LEN].try_into().ok()?;
        let vk = EdVerifyingKey::from_bytes(&pk).ok()?;
        let s: [u8; ED25519_SIGNATURE_LEN] = sig[..ED25519_SIGNATURE_LEN].try_into().ok()?;
        vk.verify_strict(&d, &ed25519_dalek::Signature::from_bytes(&s))
            .ok()
    })()
    .is_some();

    let ml_ok = (|| {
        let enc = EncodedVerifyingKey::<MlDsa65>::try_from(&public[ED25519_PUBLIC_LEN..]).ok()?;
        let vk = ml_dsa::VerifyingKey::<MlDsa65>::decode(&enc);
        let s = MlSignature::<MlDsa65>::try_from(&sig[ED25519_SIGNATURE_LEN..]).ok()?;
        vk.verify_with_context(&d, label, &s).then_some(())
    })()
    .is_some();

    if ed_ok & ml_ok {
        Ok(())
    } else {
        Err(Error::BadSignature)
    }
}

/// CRYPTO-REVIEW: the identifier QR codes, config files and pairing pin.
/// SHA-256 over both public keys, so swapping either half changes it.
pub fn key_id(public: &HybridPublic) -> KeyId {
    let mut h = Sha256::new();
    h.update(b"FFA-v2-KEYID");
    h.update(public);
    h.finalize().into()
}

/// The first 64 bits of the [`KeyId`] as `AB12-CD34-EF56-7890`, for people
/// comparing by eye. It is never used as an identifier in the protocol.
pub fn fingerprint_str(public: &HybridPublic) -> [u8; 19] {
    fingerprint_of_key_id(&key_id(public))
}

/// Same fingerprint, for when only the [`KeyId`] is at hand.
pub fn fingerprint_of_key_id(id: &KeyId) -> [u8; 19] {
    let mut fp = [0u8; 8];
    fp.copy_from_slice(&id[..8]);
    format_fingerprint(&fp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_from_seed() {
        let a = HybridIdentity::from_seed(&[7u8; 32]);
        let b = HybridIdentity::from_seed(&[7u8; 32]);
        let c = HybridIdentity::from_seed(&[8u8; 32]);
        assert_eq!(a.public_key(), b.public_key());
        assert_ne!(a.public_key(), c.public_key());
    }

    #[test]
    fn length_prefix_separates_parts() {
        assert_ne!(digest(b"L", &[b"ab", b"c"]), digest(b"L", &[b"a", b"bc"]));
        assert_ne!(digest(b"L1", &[b"x"]), digest(b"L2", &[b"x"]));
    }
}

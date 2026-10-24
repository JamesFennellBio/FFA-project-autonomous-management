//! CRYPTO-REVIEW: everything in this module is cryptography.
//!
//! Summary (full write-up in docs/CRYPTO_REVIEW.md):
//! - Long-term identity: Ed25519 ([`identity`])
//! - Pairing: trust on first use plus approval in the app ([`pairing`])
//! - Session setup: signed ephemeral X25519 ECDH ([`handshake`])
//! - Key derivation: HKDF-SHA256 into two directional keys ([`kdf`])
//! - Transport: ChaCha20-Poly1305 AEAD with a counter nonce and replay window ([`session`])
//! - Rekey policy: new ECDH every 10 messages or 1 hour, whichever comes first ([`session::MAX_MSGS`], [`session::MAX_AGE_SECS`])

pub mod handshake;
pub mod identity;
pub mod kdf;
pub mod pairing;
pub mod responder;
pub mod session;

/// Joins byte slices into one buffer for signing/hashing without `alloc`.
/// The biggest transcript is about 140 bytes.
pub(crate) struct Transcript {
    buf: [u8; 256],
    len: usize,
}

impl Transcript {
    pub(crate) fn new() -> Self {
        Self {
            buf: [0u8; 256],
            len: 0,
        }
    }

    pub(crate) fn push(mut self, part: &[u8]) -> Self {
        // Every caller pushes fixed-size parts, so overflowing is a programming bug.
        assert!(
            self.len + part.len() <= self.buf.len(),
            "transcript overflow"
        );
        self.buf[self.len..self.len + part.len()].copy_from_slice(part);
        self.len += part.len();
        self
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

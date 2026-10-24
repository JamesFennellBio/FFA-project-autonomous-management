//! CRYPTO-REVIEW: turns the X25519 shared secret into session keys.

use hkdf::Hkdf;
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const KEY_LEN: usize = 32;

/// One key per direction, so the node and hub never encrypt under the same
/// (key, nonce) pair even if their counters line up.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct SessionKeys {
    pub init_to_resp: [u8; KEY_LEN],
    pub resp_to_init: [u8; KEY_LEN],
}

/// CRYPTO-REVIEW: SHA-256 of the signed bytes of HELLO and HELLO-ACK. It is the
/// HKDF salt, so the keys are bound to this exact handshake (both ephemeral
/// public keys, both random nonces, node id and epoch).
pub fn transcript_hash(hello_signed: &[u8], ack_signed: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"FFA-TRANSCRIPT-v1");
    h.update(hello_signed);
    h.update(ack_signed);
    h.finalize().into()
}

/// CRYPTO-REVIEW: HKDF-SHA256 (RFC 5869).
/// - IKM  = X25519 shared secret
/// - salt = transcript hash
/// - info = a different label for each direction
pub fn derive_session_keys(shared_secret: &[u8; 32], transcript: &[u8; 32]) -> SessionKeys {
    let hk = Hkdf::<Sha256>::new(Some(transcript), shared_secret);
    let mut keys = SessionKeys {
        init_to_resp: [0u8; KEY_LEN],
        resp_to_init: [0u8; KEY_LEN],
    };
    // expand() only fails if you ask for more than 255*32 bytes; 32 is fine.
    hk.expand(b"FFA-v1 key init->resp", &mut keys.init_to_resp)
        .expect("hkdf length");
    hk.expand(b"FFA-v1 key resp->init", &mut keys.resp_to_init)
        .expect("hkdf length");
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directions_differ_and_transcript_matters() {
        let a = derive_session_keys(&[1u8; 32], &[2u8; 32]);
        assert_ne!(a.init_to_resp, a.resp_to_init);
        let b = derive_session_keys(&[1u8; 32], &[3u8; 32]);
        assert_ne!(a.init_to_resp, b.init_to_resp);
    }
}

//! CRYPTO-REVIEW: the v2 hybrid handshake (X25519 + ML-KEM-768, hybrid-signed).
//!
//! ```text
//! HELLO2 (initiator -> responder, 4608 bytes)
//!   0      1..3      3..35        35..1219         1219..1235    1235..4608
//!   0x10 | node_id | x25519_I   | mlkem768_ek    | nonce_I(16) | hybrid_sig_I
//!   sig_I = Hybrid_I("FFA-HELLO-v2", [bytes 0..1235])
//!
//! HELLO-ACK2 (responder -> initiator, 4516 bytes)
//!   0      1..3      3..7    7..39       39..1127        1127..1143    1143..4516
//!   0x11 | node_id | epoch | x25519_R  | mlkem768_ct   | nonce_R(16) | hybrid_sig_R
//!   sig_R = Hybrid_R("FFA-HELLO-ACK-v2", [HELLO2 0..1235, ACK2 0..1143])
//!
//! ss_x  = X25519(eph_I, eph_R)                (all-zero result rejected)
//! ss_k  = ML-KEM-768.Decaps(dk_I, ct)         (= Encaps(ek_I) on the responder)
//! keys  = HKDF-SHA256(ikm = ss_x || ss_k,
//!                     salt = SHA-256("FFA-TRANSCRIPT-v2" || KeyId_I || KeyId_R
//!                                    || HELLO2[0..1235] || ACK2[0..1143]))
//! ```
//!
//! Properties this aims for (please check):
//! - Mutual authentication under Ed25519 **and** ML-DSA-65; each side
//!   verifies against a key pinned at pairing time.
//! - Confidentiality and forward secrecy hold while X25519 **or** ML-KEM-768
//!   holds. Both ephemeral secrets (X25519 secret, ML-KEM decapsulation key)
//!   are wiped once the handshake ends.
//! - sig_R covers the whole signed HELLO2, so an ACK can't be spliced onto a
//!   different HELLO, and the ML-KEM ciphertext can't be swapped.
//! - Replay and key confirmation work as in v1: the responder makes fresh
//!   ephemerals each time, and [`crate::crypto::responder::PeerSessions`] keeps
//!   the session pending until a frame under it verifies.
//! - The responder verifies the HELLO2 signature before doing any X25519 or
//!   ML-KEM work.

use ml_kem::kem::{Decapsulate, Key, KeyExport};
use ml_kem::{DecapsulationKey, EncapsulationKey, MlKem768};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use super::identity::{self, HybridIdentity, HybridPublic, SIGNATURE_LEN};
use crate::crypto::handshake::ecdh;
use crate::crypto::kdf;
use crate::crypto::session::{Role, Session, V2_MAX_MSGS};
use crate::frame::FrameType;
use crate::{Error, Result};

const HELLO_LABEL: &[u8] = b"FFA-HELLO-v2";
const ACK_LABEL: &[u8] = b"FFA-HELLO-ACK-v2";

pub const X25519_LEN: usize = 32;
pub const MLKEM768_EK_LEN: usize = 1184;
pub const MLKEM768_CT_LEN: usize = 1088;
pub const NONCE_LEN: usize = 16;

const H_X: usize = 3;
const H_EK: usize = H_X + X25519_LEN; // 35
const H_NONCE: usize = H_EK + MLKEM768_EK_LEN; // 1219
pub const HELLO2_SIGNED: usize = H_NONCE + NONCE_LEN; // 1235
pub const HELLO2_LEN: usize = HELLO2_SIGNED + SIGNATURE_LEN; // 4608

const A_X: usize = 7;
const A_CT: usize = A_X + X25519_LEN; // 39
const A_NONCE: usize = A_CT + MLKEM768_CT_LEN; // 1127
pub const ACK2_SIGNED: usize = A_NONCE + NONCE_LEN; // 1143
pub const ACK2_LEN: usize = ACK2_SIGNED + SIGNATURE_LEN; // 4516

/// Random bytes one side of a v2 handshake needs:
/// - initiator: X25519 secret (32) | nonce (16) | ML-KEM keygen seed d‖z (64)
/// - responder: X25519 secret (32) | nonce (16) | ML-KEM encapsulation m (32), last 32 unused
///
/// CRYPTO-REVIEW: MUST come from a CSPRNG and must never be reused. ML-KEM's
/// deterministic keygen and encapsulation are only safe with fresh uniform
/// randomness every time.
pub type HandshakeRandom = [u8; 112];

/// The initiator's state between sending HELLO2 and getting HELLO-ACK2.
pub struct Initiator {
    node_id: u16,
    own_key_id: identity::KeyId,
    // CRYPTO-REVIEW: both ephemeral secrets zeroize on drop.
    eph: StaticSecret,
    dk: DecapsulationKey<MlKem768>,
    hello_signed: [u8; HELLO2_SIGNED],
}

/// Initiator step 1: build a HELLO2.
pub fn initiate(
    identity: &HybridIdentity,
    node_id: u16,
    rand: &HandshakeRandom,
) -> (Initiator, [u8; HELLO2_LEN]) {
    let mut x = [0u8; 32];
    x.copy_from_slice(&rand[..32]);
    let eph = StaticSecret::from(x);
    x.zeroize();

    let mut kem_seed = ml_kem::Seed::default();
    kem_seed.copy_from_slice(&rand[48..112]);
    let dk = DecapsulationKey::<MlKem768>::from_seed(kem_seed);
    kem_seed.zeroize();

    let mut frame = [0u8; HELLO2_LEN];
    frame[0] = FrameType::Hello2 as u8;
    frame[1..3].copy_from_slice(&node_id.to_le_bytes());
    frame[H_X..H_EK].copy_from_slice(PublicKey::from(&eph).as_bytes());
    frame[H_EK..H_NONCE].copy_from_slice(dk.encapsulation_key().to_bytes().as_slice());
    frame[H_NONCE..HELLO2_SIGNED].copy_from_slice(&rand[32..48]);
    let sig = identity.sign(HELLO_LABEL, &[&frame[..HELLO2_SIGNED]]);
    frame[HELLO2_SIGNED..].copy_from_slice(&sig);

    let mut hello_signed = [0u8; HELLO2_SIGNED];
    hello_signed.copy_from_slice(&frame[..HELLO2_SIGNED]);
    (
        Initiator {
            node_id,
            own_key_id: identity.key_id(),
            eph,
            dk,
            hello_signed,
        },
        frame,
    )
}

fn check_hello(frame: &[u8]) -> Result<()> {
    if frame.len() != HELLO2_LEN || frame[0] != FrameType::Hello2 as u8 {
        return Err(Error::BadFrame);
    }
    Ok(())
}

/// Reads the node id from a HELLO2 without checking it (to find whose pinned
/// key to verify with).
pub fn hello_node_id(frame: &[u8]) -> Result<u16> {
    check_hello(frame)?;
    Ok(u16::from_le_bytes([frame[1], frame[2]]))
}

/// The HELLO2's 16-byte random nonce (for the responder's replay list).
pub fn hello_nonce(frame: &[u8]) -> Result<[u8; NONCE_LEN]> {
    check_hello(frame)?;
    let mut n = [0u8; NONCE_LEN];
    n.copy_from_slice(&frame[H_NONCE..HELLO2_SIGNED]);
    Ok(n)
}

/// Responder: check a HELLO2 and answer it.
///
/// `initiator_pub` is the hybrid key pinned at pairing time. Returns the
/// HELLO-ACK2 to send and the new session, which the caller keeps *pending*
/// until the first frame under it verifies.
pub fn respond(
    hello: &[u8],
    initiator_pub: &HybridPublic,
    identity: &HybridIdentity,
    epoch: u32,
    rand: &HandshakeRandom,
    now: u64,
) -> Result<([u8; ACK2_LEN], Session)> {
    let node_id = hello_node_id(hello)?;
    // CRYPTO-REVIEW: verify the hybrid signature before any X25519 or ML-KEM work.
    identity::verify(
        initiator_pub,
        HELLO_LABEL,
        &[&hello[..HELLO2_SIGNED]],
        &hello[HELLO2_SIGNED..],
    )?;

    // CRYPTO-REVIEW: EncapsulationKey::new runs FIPS 203's modulus check on
    // the peer's key, so a malformed key is rejected here.
    let ek_bytes = Key::<EncapsulationKey<MlKem768>>::try_from(&hello[H_EK..H_NONCE])
        .map_err(|_| Error::BadFrame)?;
    let ek = EncapsulationKey::<MlKem768>::new(&ek_bytes).map_err(|_| Error::BadFrame)?;
    let mut m = ml_kem::B32::default();
    m.copy_from_slice(&rand[48..80]);
    let (ct, mut ss_k) = ek.encapsulate_deterministic(&m);
    m.zeroize();

    let mut x = [0u8; 32];
    x.copy_from_slice(&rand[..32]);
    let eph = StaticSecret::from(x);
    x.zeroize();

    let mut ack = [0u8; ACK2_LEN];
    ack[0] = FrameType::HelloAck2 as u8;
    ack[1..3].copy_from_slice(&node_id.to_le_bytes());
    ack[3..7].copy_from_slice(&epoch.to_le_bytes());
    ack[A_X..A_CT].copy_from_slice(PublicKey::from(&eph).as_bytes());
    ack[A_CT..A_NONCE].copy_from_slice(ct.as_slice());
    ack[A_NONCE..ACK2_SIGNED].copy_from_slice(&rand[32..48]);
    let sig = identity.sign(ACK_LABEL, &[&hello[..HELLO2_SIGNED], &ack[..ACK2_SIGNED]]);
    ack[ACK2_SIGNED..].copy_from_slice(&sig);

    let ss_x = ecdh(&eph, &hello[H_X..H_EK]);
    let mut ss_x = match ss_x {
        Ok(s) => s,
        Err(e) => {
            ss_k.zeroize();
            return Err(e);
        }
    };
    let th = kdf::transcript_hash_v2(
        &identity::key_id(initiator_pub),
        &identity.key_id(),
        &hello[..HELLO2_SIGNED],
        &ack[..ACK2_SIGNED],
    );
    let keys = kdf::derive_session_keys_v2(&ikm(&ss_x, ss_k.as_slice()), &th);
    ss_x.zeroize();
    ss_k.zeroize();
    Ok((
        ack,
        Session::new(node_id, epoch, Role::Responder, &keys, now).with_max_msgs(V2_MAX_MSGS),
    ))
}

/// X25519 secret || ML-KEM secret. The caller wipes the inputs; this copy
/// lives only until the keys are derived.
fn ikm(ss_x: &[u8; 32], ss_k: &[u8]) -> [u8; 64] {
    let mut out = [0u8; 64];
    out[..32].copy_from_slice(ss_x);
    out[32..].copy_from_slice(ss_k);
    out
}

impl Initiator {
    pub fn node_id(&self) -> u16 {
        self.node_id
    }

    /// Initiator step 2: check the HELLO-ACK2 against the pinned responder key
    /// and derive the session. Takes `&self`, so a forged reply leaves the
    /// handshake pending (as in v1).
    pub fn finish(&self, ack: &[u8], responder_pub: &HybridPublic, now: u64) -> Result<Session> {
        if ack.len() != ACK2_LEN || ack[0] != FrameType::HelloAck2 as u8 {
            return Err(Error::BadFrame);
        }
        if u16::from_le_bytes([ack[1], ack[2]]) != self.node_id {
            return Err(Error::NotPaired);
        }
        // CRYPTO-REVIEW: the signature covers OUR hello2, the ML-KEM
        // ciphertext and the responder's X25519 key.
        identity::verify(
            responder_pub,
            ACK_LABEL,
            &[&self.hello_signed, &ack[..ACK2_SIGNED]],
            &ack[ACK2_SIGNED..],
        )?;
        let epoch = u32::from_le_bytes([ack[3], ack[4], ack[5], ack[6]]);
        let ct = ml_kem::Ciphertext::<MlKem768>::try_from(&ack[A_CT..A_NONCE])
            .map_err(|_| Error::BadFrame)?;
        let mut ss_x = ecdh(&self.eph, &ack[A_X..A_CT])?;
        let mut ss_k = self.dk.decapsulate(&ct);
        let th = kdf::transcript_hash_v2(
            &self.own_key_id,
            &identity::key_id(responder_pub),
            &self.hello_signed,
            &ack[..ACK2_SIGNED],
        );
        let keys = kdf::derive_session_keys_v2(&ikm(&ss_x, ss_k.as_slice()), &th);
        ss_x.zeroize();
        ss_k.zeroize();
        Ok(
            Session::new(self.node_id, epoch, Role::Initiator, &keys, now)
                .with_max_msgs(V2_MAX_MSGS),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A correctly signed HELLO2 carrying a small-order X25519 key must not
    /// produce a session (the all-zero ECDH result is rejected).
    #[test]
    fn small_order_x25519_rejected() {
        let node = HybridIdentity::from_seed(&[1u8; 32]);
        let hub = HybridIdentity::from_seed(&[2u8; 32]);
        let (_init, mut hello) = initiate(&node, 5, &[9u8; 112]);
        hello[H_X..H_EK].fill(0); // the identity point
        let sig = node.sign(HELLO_LABEL, &[&hello[..HELLO2_SIGNED]]);
        hello[HELLO2_SIGNED..].copy_from_slice(&sig);
        let r = respond(&hello, node.public_key(), &hub, 1, &[3u8; 112], 0);
        assert!(matches!(r, Err(Error::WeakKey)));
    }
}

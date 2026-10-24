//! CRYPTO-REVIEW: the ECDH handshake. Runs once per session (every 10 messages
//! or every hour).
//!
//! ```text
//! HELLO (initiator -> responder, 128 bytes)
//!   0      1..3      3..35          35..51         51..115
//!   0x03 | node_id | eph_pub_I(32) | nonce_I(16) | sig_I(64) | zero pad
//!   sig_I = Ed25519_I("FFA-HELLO-v1" || bytes[0..51])
//!
//! HELLO-ACK (responder -> initiator, 128 bytes)
//!   0      1..3      3..7    7..39          39..55         55..119
//!   0x04 | node_id | epoch | eph_pub_R(32) | nonce_R(16) | sig_R(64) | zero pad
//!   sig_R = Ed25519_R("FFA-HELLO-ACK-v1" || HELLO[0..51] || ACK[0..55])
//!
//! shared = X25519(eph_I, eph_R)
//! keys   = HKDF-SHA256(ikm = shared, salt = SHA256(transcript))
//! ```
//!
//! Properties this aims for (please check):
//! - Mutual authentication: each side signs with its pinned long-term key.
//! - Forward secrecy: ephemeral keys are wiped after use, so a stolen long-term
//!   key doesn't decrypt old sessions.
//! - Binding: sig_R covers the whole HELLO, so an ACK can't be spliced onto a
//!   different HELLO.
//! - Replayed HELLO: the responder makes fresh ephemeral keys every time, and
//!   the attacker doesn't have eph_I, so a replay can't produce a usable session.
//!   The responder keeps a new session *pending* until the first frame under it
//!   verifies (key confirmation, see [`super::responder`]), so a replayed HELLO
//!   also can't knock out the working session.

use x25519_dalek::{PublicKey, StaticSecret};

use super::Transcript;
use super::identity::{self, Identity, PUBLIC_KEY_LEN, SIGNATURE_LEN};
use super::kdf;
use super::session::{Role, Session};
use crate::frame::{FrameType, HS_FRAME_LEN};
use crate::{Error, Result};

const HELLO_LABEL: &[u8] = b"FFA-HELLO-v1";
const ACK_LABEL: &[u8] = b"FFA-HELLO-ACK-v1";
const HELLO_SIGNED: usize = 51;
const ACK_SIGNED: usize = 55;

/// Random bytes the caller must supply for one side of a handshake:
/// 32 for the ephemeral X25519 secret + 16 for the nonce.
///
/// CRYPTO-REVIEW: these MUST come from a CSPRNG. On the ESP32-C3, esp-hal's
/// `Rng` is only truly random while the Wi-Fi/BT radio is on (see data-node/src/rng.rs).
pub type HandshakeRandom = [u8; 48];

/// The initiator's state between sending HELLO and getting HELLO-ACK.
pub struct Initiator {
    node_id: u16,
    // CRYPTO-REVIEW: StaticSecret zeroizes on drop ("zeroize" feature). It is
    // dropped as soon as finish() returns, success or not.
    eph: StaticSecret,
    hello_signed: [u8; HELLO_SIGNED],
}

/// Initiator step 1: build a HELLO.
pub fn initiate(
    identity: &Identity,
    node_id: u16,
    rand: &HandshakeRandom,
) -> (Initiator, [u8; HS_FRAME_LEN]) {
    let mut eph_bytes = [0u8; 32];
    eph_bytes.copy_from_slice(&rand[..32]);
    let eph = StaticSecret::from(eph_bytes);
    zeroize::Zeroize::zeroize(&mut eph_bytes);
    let eph_pub = PublicKey::from(&eph);

    let mut frame = [0u8; HS_FRAME_LEN];
    frame[0] = FrameType::Hello as u8;
    frame[1..3].copy_from_slice(&node_id.to_le_bytes());
    frame[3..35].copy_from_slice(eph_pub.as_bytes());
    frame[35..51].copy_from_slice(&rand[32..48]);

    let sig = identity.sign(
        Transcript::new()
            .push(HELLO_LABEL)
            .push(&frame[..HELLO_SIGNED])
            .as_bytes(),
    );
    frame[HELLO_SIGNED..HELLO_SIGNED + SIGNATURE_LEN].copy_from_slice(&sig);

    let mut hello_signed = [0u8; HELLO_SIGNED];
    hello_signed.copy_from_slice(&frame[..HELLO_SIGNED]);
    (
        Initiator {
            node_id,
            eph,
            hello_signed,
        },
        frame,
    )
}

/// Reads the node id from a HELLO without checking it (to find whose pinned
/// key to verify with).
pub fn hello_node_id(frame: &[u8]) -> Result<u16> {
    if frame.len() != HS_FRAME_LEN || frame[0] != FrameType::Hello as u8 {
        return Err(Error::BadFrame);
    }
    Ok(u16::from_le_bytes([frame[1], frame[2]]))
}

/// CRYPTO-REVIEW: X25519, then reject a non-contributory (all-zero) result
/// that a small-order peer key would give.
fn ecdh(secret: &StaticSecret, peer: &[u8]) -> Result<[u8; 32]> {
    let mut p = [0u8; 32];
    p.copy_from_slice(peer);
    let shared = secret.diffie_hellman(&PublicKey::from(p));
    if !shared.was_contributory() {
        return Err(Error::WeakKey);
    }
    Ok(*shared.as_bytes())
}

/// Responder: check a HELLO and answer it.
///
/// `initiator_pub` is the Ed25519 key that was pinned when this node was
/// paired. Returns the HELLO-ACK to send and the new session, which the caller
/// keeps *pending* until the first frame under it verifies.
pub fn respond(
    hello: &[u8],
    initiator_pub: &[u8; PUBLIC_KEY_LEN],
    identity: &Identity,
    epoch: u32,
    rand: &HandshakeRandom,
    now: u64,
) -> Result<([u8; HS_FRAME_LEN], Session)> {
    let node_id = hello_node_id(hello)?;
    // CRYPTO-REVIEW: verify the initiator's signature *before* any ECDH work.
    identity::verify(
        initiator_pub,
        Transcript::new()
            .push(HELLO_LABEL)
            .push(&hello[..HELLO_SIGNED])
            .as_bytes(),
        &hello[HELLO_SIGNED..HELLO_SIGNED + SIGNATURE_LEN],
    )?;

    let mut eph_bytes = [0u8; 32];
    eph_bytes.copy_from_slice(&rand[..32]);
    let eph = StaticSecret::from(eph_bytes);
    zeroize::Zeroize::zeroize(&mut eph_bytes);
    let eph_pub = PublicKey::from(&eph);

    let mut ack = [0u8; HS_FRAME_LEN];
    ack[0] = FrameType::HelloAck as u8;
    ack[1..3].copy_from_slice(&node_id.to_le_bytes());
    ack[3..7].copy_from_slice(&epoch.to_le_bytes());
    ack[7..39].copy_from_slice(eph_pub.as_bytes());
    ack[39..55].copy_from_slice(&rand[32..48]);
    let sig = identity.sign(
        Transcript::new()
            .push(ACK_LABEL)
            .push(&hello[..HELLO_SIGNED])
            .push(&ack[..ACK_SIGNED])
            .as_bytes(),
    );
    ack[ACK_SIGNED..ACK_SIGNED + SIGNATURE_LEN].copy_from_slice(&sig);

    let mut shared = ecdh(&eph, &hello[3..35])?;
    let th = kdf::transcript_hash(&hello[..HELLO_SIGNED], &ack[..ACK_SIGNED]);
    let keys = kdf::derive_session_keys(&shared, &th);
    zeroize::Zeroize::zeroize(&mut shared);
    Ok((
        ack,
        Session::new(node_id, epoch, Role::Responder, &keys, now),
    ))
}

impl Initiator {
    pub fn node_id(&self) -> u16 {
        self.node_id
    }

    /// Initiator step 2: check the HELLO-ACK against the pinned responder key
    /// and derive the session.
    /// Takes `&self` so a forged or broken HELLO-ACK leaves the handshake
    /// pending: the caller only drops it once a reply verifies.
    pub fn finish(
        &self,
        ack: &[u8],
        responder_pub: &[u8; PUBLIC_KEY_LEN],
        now: u64,
    ) -> Result<Session> {
        if ack.len() != HS_FRAME_LEN || ack[0] != FrameType::HelloAck as u8 {
            return Err(Error::BadFrame);
        }
        if u16::from_le_bytes([ack[1], ack[2]]) != self.node_id {
            return Err(Error::NotPaired);
        }
        // CRYPTO-REVIEW: the signature covers OUR hello, so this ACK answers
        // this handshake and not an older one.
        identity::verify(
            responder_pub,
            Transcript::new()
                .push(ACK_LABEL)
                .push(&self.hello_signed)
                .push(&ack[..ACK_SIGNED])
                .as_bytes(),
            &ack[ACK_SIGNED..ACK_SIGNED + SIGNATURE_LEN],
        )?;
        let epoch = u32::from_le_bytes([ack[3], ack[4], ack[5], ack[6]]);
        let mut shared = ecdh(&self.eph, &ack[7..39])?;
        let th = kdf::transcript_hash(&self.hello_signed, &ack[..ACK_SIGNED]);
        let keys = kdf::derive_session_keys(&shared, &th);
        zeroize::Zeroize::zeroize(&mut shared);
        Ok(Session::new(
            self.node_id,
            epoch,
            Role::Initiator,
            &keys,
            now,
        ))
    }
}

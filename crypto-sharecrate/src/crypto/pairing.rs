//! CRYPTO-REVIEW: pairing (exchanging long-term public keys once).
//!
//! ```text
//! PAIR_REQUEST (unpaired device -> responder, 128 bytes)
//!   0      1..33         33..97
//!   0x08 | dev_pub(32) | sig(64) | zero pad
//!   sig = Ed25519_dev("FFA-PAIR-REQ-v1" || bytes[0..33])   (proves it holds the key)
//!
//! PAIR_ACCEPT (responder -> device, 128 bytes)
//!   0      1..3       3..35         35..99
//!   0x09 | node_id | resp_pub(32) | sig(64) | zero pad
//!   sig = Ed25519_resp("FFA-PAIR-ACC-v1" || dev_pub || bytes[0..35])
//! ```
//!
//! Trust model (please check):
//! - Responder side: a person approves the device's fingerprint in the app.
//!   Nothing is accepted automatically.
//! - Device side: trust on first use. The device pins the first PAIR_ACCEPT it
//!   gets while its pairing button is held. An attacker transmitting during
//!   that window could pair the node to themselves instead. To narrow the
//!   window: the node only listens for 60 s after the button press, and it logs
//!   the hub's fingerprint on serial so you can compare it with the app.
//! - The same flow pairs the hub to the PC daemon (the hub plays the device).

use super::Transcript;
use super::identity::{self, Identity, PUBLIC_KEY_LEN, SIGNATURE_LEN};
use crate::frame::{FrameType, HS_FRAME_LEN};
use crate::{Error, Result};

const REQ_LABEL: &[u8] = b"FFA-PAIR-REQ-v1";
const ACC_LABEL: &[u8] = b"FFA-PAIR-ACC-v1";

pub fn pair_request(identity: &Identity) -> [u8; HS_FRAME_LEN] {
    let mut f = [0u8; HS_FRAME_LEN];
    f[0] = FrameType::PairRequest as u8;
    f[1..33].copy_from_slice(&identity.public_key());
    let sig = identity.sign(Transcript::new().push(REQ_LABEL).push(&f[..33]).as_bytes());
    f[33..33 + SIGNATURE_LEN].copy_from_slice(&sig);
    f
}

/// Responder: check a PAIR_REQUEST and return the device's public key.
pub fn parse_pair_request(f: &[u8]) -> Result<[u8; PUBLIC_KEY_LEN]> {
    if f.len() != HS_FRAME_LEN || f[0] != FrameType::PairRequest as u8 {
        return Err(Error::BadFrame);
    }
    let mut pk = [0u8; PUBLIC_KEY_LEN];
    pk.copy_from_slice(&f[1..33]);
    identity::verify(
        &pk,
        Transcript::new().push(REQ_LABEL).push(&f[..33]).as_bytes(),
        &f[33..33 + SIGNATURE_LEN],
    )?;
    Ok(pk)
}

/// Responder: build the PAIR_ACCEPT, but only after a person approved the
/// device in the app.
pub fn pair_accept(
    responder: &Identity,
    device_pub: &[u8; PUBLIC_KEY_LEN],
    node_id: u16,
) -> [u8; HS_FRAME_LEN] {
    let mut f = [0u8; HS_FRAME_LEN];
    f[0] = FrameType::PairAccept as u8;
    f[1..3].copy_from_slice(&node_id.to_le_bytes());
    f[3..35].copy_from_slice(&responder.public_key());
    let sig = responder.sign(
        Transcript::new()
            .push(ACC_LABEL)
            .push(device_pub)
            .push(&f[..35])
            .as_bytes(),
    );
    f[35..35 + SIGNATURE_LEN].copy_from_slice(&sig);
    f
}

/// Device: check a PAIR_ACCEPT meant for *our* public key and return
/// `(assigned node id, responder public key to pin)`.
pub fn parse_pair_accept(
    f: &[u8],
    own_pub: &[u8; PUBLIC_KEY_LEN],
) -> Result<(u16, [u8; PUBLIC_KEY_LEN])> {
    if f.len() != HS_FRAME_LEN || f[0] != FrameType::PairAccept as u8 {
        return Err(Error::BadFrame);
    }
    let mut rpk = [0u8; PUBLIC_KEY_LEN];
    rpk.copy_from_slice(&f[3..35]);
    // CRYPTO-REVIEW: the signature includes own_pub, so an accept that was
    // made for another device fails here.
    identity::verify(
        &rpk,
        Transcript::new()
            .push(ACC_LABEL)
            .push(own_pub)
            .push(&f[..35])
            .as_bytes(),
        &f[35..35 + SIGNATURE_LEN],
    )?;
    Ok((u16::from_le_bytes([f[1], f[2]]), rpk))
}

//! CRYPTO-REVIEW: v2 pairing (exchanging long-term hybrid public keys once).
//!
//! ```text
//! PAIR_REQUEST2 (device -> responder, 5391 bytes)
//!   0      1..1985          1985         1986..2018      2018..5391
//!   0x12 | dev_pub(1984)  | has_ticket | ticket_mac(32) | hybrid_sig
//!   ticket_mac = HMAC-SHA256(token, "FFA-v2-TICKET" || dev_pub || responder KeyId)
//!                (all zero and has_ticket = 0 when there is no ticket)
//!   sig = Hybrid_dev("FFA-PAIR-REQ-v2", [bytes 0..2018])   (proves it holds the key)
//!
//! PAIR_ACCEPT2 (responder -> device, 5360 bytes)
//!   0      1..3      3..1987           1987..5360
//!   0x13 | node_id | resp_pub(1984)  | hybrid_sig
//!   sig = Hybrid_resp("FFA-PAIR-ACC-v2", [KeyId(dev_pub), bytes 0..1987])
//! ```
//!
//! Trust model (please check):
//! - **Phone, QR ticket** (Phase 3): the desktop shows a QR with the hub's
//!   address, its [`KeyId`] and a one-time 16-byte token. The phone pins the
//!   KeyId, so it rejects any other responder, and proves it saw the QR with
//!   `ticket_mac`. The MAC covers the phone's own public key, so it can't be
//!   moved to another device, and the hub's KeyId, so it can't be relayed to
//!   another hub. The hub approves on a valid ticket alone.
//! - **No ticket:** as in v1, a person approves the fingerprint in the app.
//! - **Hub <-> PC:** that pairing is set up by the SPAKE2 code flow (Phase 3).
//!   These frames are what that flow exchanges once the code checks out.

use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::identity::{self, HybridIdentity, HybridPublic, KeyId, PUBLIC_KEY_LEN, SIGNATURE_LEN};
use crate::frame::FrameType;
use crate::{Error, Result};

const REQ_LABEL: &[u8] = b"FFA-PAIR-REQ-v2";
const ACC_LABEL: &[u8] = b"FFA-PAIR-ACC-v2";
const TICKET_LABEL: &[u8] = b"FFA-v2-TICKET";

/// One-time token from the pairing QR code.
pub const TICKET_TOKEN_LEN: usize = 16;
pub const TICKET_MAC_LEN: usize = 32;

const R_PUB: usize = 1;
const R_FLAG: usize = R_PUB + PUBLIC_KEY_LEN; // 1985
const R_MAC: usize = R_FLAG + 1; // 1986
pub const PAIR_REQUEST2_SIGNED: usize = R_MAC + TICKET_MAC_LEN; // 2018
pub const PAIR_REQUEST2_LEN: usize = PAIR_REQUEST2_SIGNED + SIGNATURE_LEN; // 5391

const A_PUB: usize = 3;
pub const PAIR_ACCEPT2_SIGNED: usize = A_PUB + PUBLIC_KEY_LEN; // 1987
pub const PAIR_ACCEPT2_LEN: usize = PAIR_ACCEPT2_SIGNED + SIGNATURE_LEN; // 5360

/// A QR pairing ticket as the phone holds it.
#[derive(Clone, Copy)]
pub struct Ticket<'a> {
    pub token: &'a [u8; TICKET_TOKEN_LEN],
    /// The hub's KeyId from the same QR code.
    pub responder_key_id: &'a KeyId,
}

/// A verified PAIR_REQUEST2.
pub struct PairRequest {
    pub device_pub: HybridPublic,
    /// Present when the device claims a QR ticket; check it with [`verify_ticket`].
    pub ticket_mac: Option<[u8; TICKET_MAC_LEN]>,
}

/// CRYPTO-REVIEW: HMAC-SHA256 keyed by the one-time token.
fn ticket_mac(
    token: &[u8; TICKET_TOKEN_LEN],
    device_pub: &HybridPublic,
    responder: &KeyId,
) -> Hmac<Sha256> {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(token).expect("hmac takes any key length");
    mac.update(TICKET_LABEL);
    mac.update(device_pub);
    mac.update(responder);
    mac
}

pub fn pair_request(
    identity: &HybridIdentity,
    ticket: Option<Ticket<'_>>,
) -> [u8; PAIR_REQUEST2_LEN] {
    let mut f = [0u8; PAIR_REQUEST2_LEN];
    f[0] = FrameType::PairRequest2 as u8;
    f[R_PUB..R_FLAG].copy_from_slice(identity.public_key());
    if let Some(t) = ticket {
        f[R_FLAG] = 1;
        let tag = ticket_mac(t.token, identity.public_key(), t.responder_key_id).finalize();
        f[R_MAC..PAIR_REQUEST2_SIGNED].copy_from_slice(&tag.into_bytes());
    }
    let sig = identity.sign(REQ_LABEL, &[&f[..PAIR_REQUEST2_SIGNED]]);
    f[PAIR_REQUEST2_SIGNED..].copy_from_slice(&sig);
    f
}

/// Responder: check a PAIR_REQUEST2's signature and return what it claims.
/// A ticket claim still has to pass [`verify_ticket`].
pub fn parse_pair_request(f: &[u8]) -> Result<PairRequest> {
    if f.len() != PAIR_REQUEST2_LEN || f[0] != FrameType::PairRequest2 as u8 {
        return Err(Error::BadFrame);
    }
    let mut device_pub = [0u8; PUBLIC_KEY_LEN];
    device_pub.copy_from_slice(&f[R_PUB..R_FLAG]);
    identity::verify(
        &device_pub,
        REQ_LABEL,
        &[&f[..PAIR_REQUEST2_SIGNED]],
        &f[PAIR_REQUEST2_SIGNED..],
    )?;
    let ticket_mac = match f[R_FLAG] {
        0 => None,
        1 => {
            let mut m = [0u8; TICKET_MAC_LEN];
            m.copy_from_slice(&f[R_MAC..PAIR_REQUEST2_SIGNED]);
            Some(m)
        }
        _ => return Err(Error::BadFrame),
    };
    Ok(PairRequest {
        device_pub,
        ticket_mac,
    })
}

/// CRYPTO-REVIEW: constant-time check that `req` proves knowledge of `token`
/// for this device key and this responder. The caller must burn the token
/// after one successful use and expire it after its time limit.
pub fn verify_ticket(
    req: &PairRequest,
    token: &[u8; TICKET_TOKEN_LEN],
    responder_key_id: &KeyId,
) -> Result<()> {
    let claimed = req.ticket_mac.ok_or(Error::BadTicket)?;
    ticket_mac(token, &req.device_pub, responder_key_id)
        .verify_slice(&claimed)
        .map_err(|_| Error::BadTicket)
}

/// Responder: build the PAIR_ACCEPT2, only after the device was approved
/// (by a person, by a valid ticket, or by the SPAKE2 flow).
pub fn pair_accept(
    responder: &HybridIdentity,
    device_pub: &HybridPublic,
    node_id: u16,
) -> [u8; PAIR_ACCEPT2_LEN] {
    let mut f = [0u8; PAIR_ACCEPT2_LEN];
    f[0] = FrameType::PairAccept2 as u8;
    f[1..3].copy_from_slice(&node_id.to_le_bytes());
    f[A_PUB..PAIR_ACCEPT2_SIGNED].copy_from_slice(responder.public_key());
    let sig = responder.sign(
        ACC_LABEL,
        &[&identity::key_id(device_pub), &f[..PAIR_ACCEPT2_SIGNED]],
    );
    f[PAIR_ACCEPT2_SIGNED..].copy_from_slice(&sig);
    f
}

/// Device: check a PAIR_ACCEPT2 meant for *our* key and return
/// `(assigned node id, responder public key to pin)`.
pub fn parse_pair_accept(f: &[u8], own_key_id: &KeyId) -> Result<(u16, HybridPublic)> {
    if f.len() != PAIR_ACCEPT2_LEN || f[0] != FrameType::PairAccept2 as u8 {
        return Err(Error::BadFrame);
    }
    let mut rpk = [0u8; PUBLIC_KEY_LEN];
    rpk.copy_from_slice(&f[A_PUB..PAIR_ACCEPT2_SIGNED]);
    // CRYPTO-REVIEW: the signature covers our KeyId, so an accept made for
    // another device fails here.
    identity::verify(
        &rpk,
        ACC_LABEL,
        &[own_key_id, &f[..PAIR_ACCEPT2_SIGNED]],
        &f[PAIR_ACCEPT2_SIGNED..],
    )?;
    Ok((u16::from_le_bytes([f[1], f[2]]), rpk))
}

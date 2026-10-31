//! Shared protocol + crypto for the autonomous farm/lab management system.
//!
//! Used by every Rust part of the system:
//! - `data-node` (ESP32-C3, no_std)  -> uses [`node::NodeCore`]
//! - `hub-node`  (Raspberry Pi)      -> responder side of [`crypto::handshake`]
//! - `pc-daemon` (user's PC)         -> responder for the hub's uplink
//! - `node-sim`  (desktop simulator) -> uses [`node::NodeCore`]
//!
//! The crate is "sans-IO": it never touches a radio, socket, clock or RNG.
//! Callers pass in random bytes and the current time, and get bytes back to send.
//! That keeps it testable on a PC and identical on every device.
//!
//! Anything that is cryptography is tagged `CRYPTO-REVIEW:` (see docs/CRYPTO_REVIEW.md).

#![cfg_attr(not(any(test, feature = "std")), no_std)]

pub mod crypto;
pub mod frame;
pub mod node;
pub mod packet;
pub mod radio;

/// Errors from anything in this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Frame is the wrong length or has a type byte we don't know.
    BadFrame,
    /// Payload inside a frame didn't decode.
    BadPacket,
    /// Output buffer too small.
    BufferTooSmall,
    /// Ed25519 signature didn't verify.
    BadSignature,
    /// AEAD tag didn't verify (tampered, wrong key, or wrong header).
    DecryptFailed,
    /// Counter not newer than the last one we accepted.
    Replay,
    /// Frame belongs to a different session epoch than the one we hold.
    WrongEpoch,
    /// Frame direction byte doesn't match what we expect to receive.
    WrongDirection,
    /// Session used up its 10 messages or 1 hour; a new handshake is needed.
    RekeyRequired,
    /// X25519 produced a non-contributory (all-zero) shared secret.
    WeakKey,
    /// The node isn't paired, or the frame is for a different node id.
    NotPaired,
    /// A handshake reply arrived but we have no handshake in progress.
    NoHandshake,
    /// Validly signed, but by a key other than the one pinned in advance.
    WrongPeer,
    /// This exact HELLO was already seen (replay).
    ReplayedHello,
    /// A v2 pairing request's QR-ticket proof is missing or wrong.
    BadTicket,
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self, f)
    }
}

#[cfg(any(test, feature = "std"))]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

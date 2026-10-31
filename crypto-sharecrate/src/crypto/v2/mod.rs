//! CRYPTO-REVIEW: the v2 hybrid post-quantum protocol (feature `pq`).
//!
//! Used only on the hub <-> PC and hub <-> phone links. ESP32 nodes keep the
//! v1 protocol, because their 64/128-byte radio frames can't carry
//! post-quantum keys or signatures.
//!
//! What changes compared with v1:
//! - **Identity:** one 32-byte seed derives an Ed25519 key *and* an ML-DSA-65
//!   key ([`identity`]). Every signature is both, and verification needs
//!   **both** to pass, so the scheme stays secure while either one does.
//! - **Session setup:** the handshake runs X25519 *and* ML-KEM-768 and feeds
//!   both shared secrets into HKDF ([`handshake`]). Traffic recorded today
//!   stays confidential unless both X25519 and ML-KEM-768 are broken.
//! - **Pairing:** v2 frames carry the hybrid public key, plus an optional
//!   proof of a one-time QR ticket ([`pairing`]).
//! - **Sessions:** the same ChaCha20-Poly1305 [`super::session::Session`],
//!   with [`super::session::V2_MAX_MSGS`] frames per key instead of 10.
//! - **No downgrade:** v2 frames have their own type bytes (0x10..=0x13).
//!   v1 parsers reject them, and v2 parsers reject v1 frames.
//!
//! [`link::LinkCore`] is the initiator state machine (hub uplink, phone).
//! The responder side is [`super::responder::PeerSessions::on_hello_v2`].

pub mod handshake;
pub mod identity;
pub mod link;
pub mod pairing;

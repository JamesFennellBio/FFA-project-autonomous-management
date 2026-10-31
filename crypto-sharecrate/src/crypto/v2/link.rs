//! Initiator-side state machine for v2 links: the hub's uplink to the PC and
//! the phone's link to the hub. It is the v2 counterpart of
//! [`crate::node::NodeCore`].
//!
//! It does no I/O. The caller's loop looks like this:
//! ```text
//! if !core.is_paired()            -> send core.pair_request(ticket)
//! if core.needs_handshake(now)    -> send core.start_handshake(rand, now)
//! else                            -> seal JSON with core.session_mut()
//! on a PAIR_ACCEPT2 / HELLO-ACK2  -> core.handle_frame(frame, now)
//! on an encrypted frame           -> core.session_mut().open(..)
//! ```

use super::handshake::{self, ACK2_LEN, HELLO2_LEN, HandshakeRandom, Initiator};
use super::identity::{self, HybridIdentity, HybridPublic, KeyId};
use super::pairing::{self, PAIR_REQUEST2_LEN, Ticket};
use crate::crypto::identity::SEED_LEN;
use crate::crypto::session::Session;
use crate::frame::FrameType;
use crate::{Error, Result};

/// Give up on a HELLO2 with no reply after this many seconds and send a new one.
/// Longer than v1's 5 s, because a Pi 3B spends a few ms on hybrid crypto and
/// the phone may be on a slow network.
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 10;

/// What the caller has to save once pairing succeeds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LinkPairing {
    pub node_id: u16,
    /// CRYPTO-REVIEW: the pinned responder hybrid key.
    pub peer_pub: HybridPublic,
}

impl core::fmt::Debug for LinkPairing {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let fp = identity::fingerprint_str(&self.peer_pub);
        f.debug_struct("LinkPairing")
            .field("node_id", &self.node_id)
            .field("peer", &core::str::from_utf8(&fp).unwrap_or("?"))
            .finish()
    }
}

// The crate has no heap, so the pinned key travels inline. Events are
// short-lived, so the 2 KB variant is fine.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkEvent {
    /// Pairing accepted. Save this.
    Paired(LinkPairing),
    /// A new session is ready (new keys, epoch).
    SessionUp { epoch: u32 },
    /// Frame wasn't expected; nothing to do.
    Ignored,
}

pub struct LinkCore {
    identity: HybridIdentity,
    /// CRYPTO-REVIEW: if set, pairing only accepts a responder with this KeyId
    /// (from the QR code, or from the SPAKE2 pairing on the hub).
    expected_peer: Option<KeyId>,
    pairing: Option<LinkPairing>,
    session: Option<Session>,
    pending: Option<(Initiator, u64)>,
    sessions_started: u32,
}

impl LinkCore {
    /// CRYPTO-REVIEW: `seed` is this device's long-term secret (Keychain or
    /// Keystore on the phone, the BIP39-derived seed on the hub).
    pub fn new(seed: &[u8; SEED_LEN], pairing: Option<LinkPairing>) -> Self {
        Self {
            identity: HybridIdentity::from_seed(seed),
            expected_peer: None,
            pairing,
            session: None,
            pending: None,
            sessions_started: 0,
        }
    }

    /// CRYPTO-REVIEW: pin the responder ahead of time. A saved pairing for a
    /// different key is dropped, so the device pairs again with the right peer.
    pub fn with_expected_peer(mut self, key_id: KeyId) -> Self {
        self.expected_peer = Some(key_id);
        if self
            .pairing
            .is_some_and(|p| identity::key_id(&p.peer_pub) != key_id)
        {
            self.pairing = None;
        }
        self
    }

    pub fn expected_peer(&self) -> Option<KeyId> {
        self.expected_peer
    }

    pub fn public_key(&self) -> &HybridPublic {
        self.identity.public_key()
    }

    pub fn key_id(&self) -> KeyId {
        self.identity.key_id()
    }

    pub fn fingerprint(&self) -> [u8; 19] {
        self.identity.fingerprint_str()
    }

    pub fn pairing(&self) -> Option<LinkPairing> {
        self.pairing
    }

    pub fn is_paired(&self) -> bool {
        self.pairing.is_some()
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Seal and open variable-length frames (JSON uplink, phone requests).
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    pub fn sessions_started(&self) -> u32 {
        self.sessions_started
    }

    pub fn pair_request(&self, ticket: Option<Ticket<'_>>) -> [u8; PAIR_REQUEST2_LEN] {
        pairing::pair_request(&self.identity, ticket)
    }

    /// Forget the pairing (unpair or factory reset).
    pub fn unpair(&mut self) {
        self.pairing = None;
        self.session = None;
        self.pending = None;
    }

    /// CRYPTO-REVIEW: true when there's no session, or the current one hit
    /// [`crate::crypto::session::V2_MAX_MSGS`] frames or 1 hour.
    pub fn needs_handshake(&self, now: u64) -> bool {
        if self.pairing.is_none() || self.handshake_pending(now) {
            return false;
        }
        match &self.session {
            None => true,
            Some(s) => s.needs_rekey(now),
        }
    }

    /// True if a HELLO2 is out and we're waiting for its HELLO-ACK2.
    pub fn handshake_pending(&self, now: u64) -> bool {
        matches!(&self.pending, Some((_, t)) if now.saturating_sub(*t) < HANDSHAKE_TIMEOUT_SECS)
    }

    pub fn start_handshake(
        &mut self,
        rand: &HandshakeRandom,
        now: u64,
    ) -> Result<[u8; HELLO2_LEN]> {
        let p = self.pairing.ok_or(Error::NotPaired)?;
        let (init, hello) = handshake::initiate(&self.identity, p.node_id, rand);
        self.pending = Some((init, now));
        Ok(hello)
    }

    /// Handles a PAIR_ACCEPT2 or HELLO-ACK2. Encrypted frames go to
    /// [`Self::session_mut`] instead.
    pub fn handle_frame(&mut self, frame: &[u8], now: u64) -> Result<LinkEvent> {
        let Some(&t) = frame.first() else {
            return Err(Error::BadFrame);
        };
        match FrameType::from_byte(t)? {
            FrameType::PairAccept2 => {
                if self.pairing.is_some() {
                    return Ok(LinkEvent::Ignored);
                }
                let (node_id, peer_pub) = pairing::parse_pair_accept(frame, &self.key_id())?;
                // CRYPTO-REVIEW: with a pinned KeyId, an accept from anyone
                // else is refused even though its signature is valid.
                if self
                    .expected_peer
                    .is_some_and(|k| k != identity::key_id(&peer_pub))
                {
                    return Err(Error::WrongPeer);
                }
                let p = LinkPairing { node_id, peer_pub };
                self.pairing = Some(p);
                Ok(LinkEvent::Paired(p))
            }
            FrameType::HelloAck2 => {
                let pairing = self.pairing.ok_or(Error::NotPaired)?;
                let (init, _) = self.pending.as_ref().ok_or(Error::NoHandshake)?;
                if frame.len() != ACK2_LEN
                    || crate::frame::peek_node_id(frame) != Some(init.node_id())
                {
                    return Ok(LinkEvent::Ignored);
                }
                // CRYPTO-REVIEW: a reply that fails verification leaves the
                // handshake pending (as in v1, FYEO scan 2).
                let s = init.finish(frame, &pairing.peer_pub, now)?;
                self.pending = None; // ephemeral X25519 + ML-KEM secrets wiped
                let epoch = s.epoch;
                self.session = Some(s);
                self.sessions_started += 1;
                Ok(LinkEvent::SessionUp { epoch })
            }
            // CRYPTO-REVIEW: v1 handshake and pairing frames are never accepted
            // on a v2 link (no downgrade).
            FrameType::Hello
            | FrameType::HelloAck
            | FrameType::PairRequest
            | FrameType::PairAccept => Err(Error::BadFrame),
            _ => Ok(LinkEvent::Ignored),
        }
    }
}

//! Initiator-side state machine, shared by the ESP32-C3 firmware, the desktop
//! node simulator, and the hub's uplink to the PC.
//!
//! It does no I/O. The caller's loop looks like this:
//! ```text
//! if !core.is_paired()            -> send core.pair_request() while the button is held
//! if core.needs_handshake(now)    -> send core.start_handshake(rand, now)
//! else                            -> send core.seal(&packet, now)
//! on any received frame           -> core.handle_frame(frame, now)
//! ```

use crate::crypto::handshake::{self, HandshakeRandom, Initiator};
use crate::crypto::identity::{self, Identity, PUBLIC_KEY_LEN, SEED_LEN};
use crate::crypto::pairing;
use crate::crypto::session::Session;
use crate::frame::{FRAME_LEN, FrameType, HS_FRAME_LEN};
use crate::packet::Packet;
use crate::{Error, Result};

/// Give up on a HELLO with no reply after this many seconds and send a new one.
pub const HANDSHAKE_TIMEOUT_SECS: u64 = 5;

/// What the caller has to save in flash once pairing succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pairing {
    pub node_id: u16,
    /// CRYPTO-REVIEW: the pinned responder (hub/PC) Ed25519 key.
    pub peer_pub: [u8; PUBLIC_KEY_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeEvent {
    /// Pairing accepted. Save this to flash.
    Paired(Pairing),
    /// A new session is ready (new keys, epoch).
    SessionUp { epoch: u32 },
    /// The hub asked us to set an output.
    Command { channel: u8, value: i32 },
    /// Any other decrypted packet (message, ack...).
    Packet(Packet),
    /// Frame wasn't for us or wasn't expected; nothing to do.
    Ignored,
}

pub struct NodeCore {
    identity: Identity,
    /// CRYPTO-REVIEW: if set, pairing only accepts this responder key.
    expected_peer: Option<[u8; PUBLIC_KEY_LEN]>,
    pairing: Option<Pairing>,
    session: Option<Session>,
    pending: Option<(Initiator, u64)>,
    sessions_started: u32,
}

impl NodeCore {
    /// CRYPTO-REVIEW: `seed` is this device's long-term Ed25519 secret, loaded
    /// from flash (made from the hardware RNG on first boot).
    pub fn new(seed: &[u8; SEED_LEN], pairing: Option<Pairing>) -> Self {
        Self {
            identity: Identity::from_seed(seed),
            expected_peer: None,
            pairing,
            session: None,
            pending: None,
            sessions_started: 0,
        }
    }

    /// CRYPTO-REVIEW: pin the responder's public key ahead of time (built into
    /// the node firmware, or set in hub.toml for the PC). Pairing then accepts
    /// only a PAIR_ACCEPT signed by this key, which closes the trust-on-first-use
    /// race. A saved pairing for a different key is dropped, so the device
    /// pairs again with the right peer.
    pub fn with_expected_peer(mut self, key: [u8; PUBLIC_KEY_LEN]) -> Self {
        self.expected_peer = Some(key);
        if self.pairing.is_some_and(|p| p.peer_pub != key) {
            self.pairing = None;
        }
        self
    }

    pub fn expected_peer(&self) -> Option<[u8; PUBLIC_KEY_LEN]> {
        self.expected_peer
    }

    pub fn public_key(&self) -> [u8; PUBLIC_KEY_LEN] {
        self.identity.public_key()
    }

    pub fn fingerprint(&self) -> [u8; 19] {
        identity::fingerprint_str(&self.identity.public_key())
    }

    pub fn pairing(&self) -> Option<Pairing> {
        self.pairing
    }

    pub fn is_paired(&self) -> bool {
        self.pairing.is_some()
    }

    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Access the session directly, e.g. to seal variable-length uplink frames.
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// How many handshakes have finished since boot (for logs/tests).
    pub fn sessions_started(&self) -> u32 {
        self.sessions_started
    }

    pub fn pair_request(&self) -> [u8; HS_FRAME_LEN] {
        pairing::pair_request(&self.identity)
    }

    /// Forget the pairing (factory reset).
    pub fn unpair(&mut self) {
        self.pairing = None;
        self.session = None;
        self.pending = None;
    }

    /// CRYPTO-REVIEW: true when there's no session, or the current one hit
    /// 10 messages / 1 hour. A HELLO still waiting for a reply doesn't count
    /// until it times out.
    pub fn needs_handshake(&self, now: u64) -> bool {
        if self.pairing.is_none() {
            return false;
        }
        if let Some((_, started)) = &self.pending
            && now.saturating_sub(*started) < HANDSHAKE_TIMEOUT_SECS
        {
            return false;
        }
        match &self.session {
            None => true,
            Some(s) => s.needs_rekey(now),
        }
    }

    /// True if a HELLO is out and we're waiting for its HELLO-ACK.
    pub fn handshake_pending(&self, now: u64) -> bool {
        matches!(&self.pending, Some((_, t)) if now.saturating_sub(*t) < HANDSHAKE_TIMEOUT_SECS)
    }

    pub fn start_handshake(
        &mut self,
        rand: &HandshakeRandom,
        now: u64,
    ) -> Result<[u8; HS_FRAME_LEN]> {
        let p = self.pairing.ok_or(Error::NotPaired)?;
        let (init, hello) = handshake::initiate(&self.identity, p.node_id, rand);
        self.pending = Some((init, now));
        Ok(hello)
    }

    /// Seals a packet. Returns `RekeyRequired` if you have to handshake first.
    pub fn seal(&mut self, p: &Packet, now: u64) -> Result<[u8; FRAME_LEN]> {
        let s = self.session.as_mut().ok_or(Error::RekeyRequired)?;
        s.seal_packet(p, now)
    }

    /// Handles any frame from the radio.
    pub fn handle_frame(&mut self, frame: &[u8], now: u64) -> Result<NodeEvent> {
        let Some(&t) = frame.first() else {
            return Err(Error::BadFrame);
        };
        match FrameType::from_byte(t)? {
            FrameType::PairAccept => {
                if self.pairing.is_some() {
                    return Ok(NodeEvent::Ignored);
                }
                let (node_id, peer_pub) =
                    pairing::parse_pair_accept(frame, &self.identity.public_key())?;
                // CRYPTO-REVIEW: with a pinned key, an accept from anyone else
                // is refused even though its signature is valid.
                if self.expected_peer.is_some_and(|k| k != peer_pub) {
                    return Err(Error::WrongPeer);
                }
                let p = Pairing { node_id, peer_pub };
                self.pairing = Some(p);
                Ok(NodeEvent::Paired(p))
            }
            FrameType::HelloAck => {
                let pairing = self.pairing.ok_or(Error::NotPaired)?;
                let (init, _) = self.pending.as_ref().ok_or(Error::NoHandshake)?;
                if crate::frame::peek_node_id(frame) != Some(init.node_id()) {
                    // Not for us (shared radio channel). Keep waiting; the
                    // timeout keeps counting from the original HELLO.
                    return Ok(NodeEvent::Ignored);
                }
                // CRYPTO-REVIEW: a reply that fails verification (forged, or for
                // an older HELLO) leaves our handshake pending, so injected
                // garbage can't cancel it (FYEO scan 2, HELLO-ACK starvation).
                let s = init.finish(frame, &pairing.peer_pub, now)?;
                self.pending = None; // ephemeral secret dropped and wiped
                let epoch = s.epoch;
                // CRYPTO-REVIEW: the old session (and its keys) is dropped here.
                self.session = Some(s);
                self.sessions_started += 1;
                Ok(NodeEvent::SessionUp { epoch })
            }
            t if t.is_encrypted() && frame.len() == FRAME_LEN => {
                let Some(p) = self.pairing else {
                    return Ok(NodeEvent::Ignored);
                };
                if crate::frame::peek_node_id(frame) != Some(p.node_id) {
                    return Ok(NodeEvent::Ignored);
                }
                let s = self.session.as_mut().ok_or(Error::RekeyRequired)?;
                match s.open_packet(frame, now)? {
                    Packet::Command { channel, value } => Ok(NodeEvent::Command { channel, value }),
                    other => Ok(NodeEvent::Packet(other)),
                }
            }
            _ => Ok(NodeEvent::Ignored),
        }
    }
}

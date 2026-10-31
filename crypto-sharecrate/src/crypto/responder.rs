//! CRYPTO-REVIEW: responder-side session bookkeeping for one peer (the hub
//! keeps one per node; the PC keeps one for the hub).
//!
//! A new handshake makes a *pending* session. It only replaces the current
//! session once a frame under it verifies (key confirmation). That way a
//! replayed or attacker-sent HELLO can't take down a working session.
//!
//! Replayed HELLOs (scan finding 29): an attacker who recorded an old, validly
//! signed HELLO could replay it every time the real node starts a handshake,
//! overwriting the real pending session so it never confirms. Two defences:
//! - the nonces of recent HELLOs are remembered, and a repeat is rejected;
//! - several pending sessions are kept (keyed by epoch), so an older replay
//!   that slipped past the nonce list can't evict the genuine one.
//!
//! Residual risk: an attacker with a big stash of recorded HELLOs, replaying
//! fast, can still fill the slots. That's no worse than jamming the radio,
//! which can't be prevented anyway.

use super::handshake::{self, HandshakeRandom};
use super::identity::{Identity, PUBLIC_KEY_LEN};
use super::session::Session;
use crate::frame::{HS_FRAME_LEN, Header};
use crate::{Error, Result};

/// Pending (unconfirmed) sessions kept per peer.
pub const PENDING_SLOTS: usize = 4;
/// Recent HELLO nonces remembered per peer.
pub const SEEN_NONCES: usize = 64;

pub struct PeerSessions {
    current: Option<Session>,
    /// Oldest first.
    pending: [Option<Session>; PENDING_SLOTS],
    /// Ring buffer of recent HELLO nonces (bytes 35..51 of the HELLO).
    seen: [[u8; 16]; SEEN_NONCES],
    seen_next: usize,
    seen_len: usize,
}

impl Default for PeerSessions {
    fn default() -> Self {
        Self {
            current: None,
            pending: Default::default(),
            seen: [[0u8; 16]; SEEN_NONCES],
            seen_next: 0,
            seen_len: 0,
        }
    }
}

impl PeerSessions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn current(&self) -> Option<&Session> {
        self.current.as_ref()
    }

    pub fn current_mut(&mut self) -> Option<&mut Session> {
        self.current.as_mut()
    }

    /// `epoch_rand` picks the new epoch id. It only has to differ from the
    /// current one (keys are fresh each time); a random start makes it harder
    /// to guess how many sessions have happened.
    pub fn on_hello(
        &mut self,
        hello: &[u8],
        peer_pub: &[u8; PUBLIC_KEY_LEN],
        identity: &Identity,
        rand: &HandshakeRandom,
        epoch_rand: u32,
        now: u64,
    ) -> Result<[u8; HS_FRAME_LEN]> {
        let epoch = self.free_epoch(epoch_rand);
        // respond() checks the length and signature first, so a forged frame
        // never gets into the nonce list.
        let (ack, session) = handshake::respond(hello, peer_pub, identity, epoch, rand, now)?;
        let mut nonce = [0u8; 16];
        nonce.copy_from_slice(&hello[35..51]);
        self.admit(nonce, session)?;
        Ok(ack)
    }

    /// v2 (hybrid) version of [`Self::on_hello`] for the hub <-> PC and
    /// hub <-> phone links. Same replay and pending-session rules.
    #[cfg(feature = "pq")]
    pub fn on_hello_v2(
        &mut self,
        hello: &[u8],
        peer_pub: &super::v2::identity::HybridPublic,
        identity: &super::v2::identity::HybridIdentity,
        rand: &super::v2::handshake::HandshakeRandom,
        epoch_rand: u32,
        now: u64,
    ) -> Result<[u8; super::v2::handshake::ACK2_LEN]> {
        let epoch = self.free_epoch(epoch_rand);
        let (ack, session) =
            super::v2::handshake::respond(hello, peer_pub, identity, epoch, rand, now)?;
        self.admit(super::v2::handshake::hello_nonce(hello)?, session)?;
        Ok(ack)
    }

    fn free_epoch(&self, epoch_rand: u32) -> u32 {
        let mut epoch = epoch_rand;
        while self.epoch_in_use(epoch) {
            epoch = epoch.wrapping_add(1);
        }
        epoch
    }

    /// Records a verified HELLO's nonce and parks its session as pending.
    fn admit(&mut self, nonce: [u8; 16], session: Session) -> Result<()> {
        // CRYPTO-REVIEW: a HELLO nonce is 16 random bytes from the node, so a
        // repeat means a replay. The ACK and session made by the caller are
        // thrown away.
        if self.seen[..self.seen_len].contains(&nonce) {
            return Err(Error::ReplayedHello);
        }
        self.seen[self.seen_next] = nonce;
        self.seen_next = (self.seen_next + 1) % SEEN_NONCES;
        self.seen_len = (self.seen_len + 1).min(SEEN_NONCES);

        // Keep up to PENDING_SLOTS pending sessions; the oldest is dropped
        // (and its keys wiped) when they're all full.
        if self.pending.iter().all(Option::is_some) {
            self.pending.rotate_left(1);
            self.pending[PENDING_SLOTS - 1] = None;
        }
        if let Some(slot) = self.pending.iter_mut().find(|p| p.is_none()) {
            *slot = Some(session);
        }
        Ok(())
    }

    fn epoch_in_use(&self, epoch: u32) -> bool {
        self.current.as_ref().is_some_and(|s| s.epoch == epoch)
            || self.pending.iter().flatten().any(|s| s.epoch == epoch)
    }

    /// Opens a frame under the current or pending session, chosen by epoch.
    /// A good frame under the pending session promotes it to current.
    pub fn open<'a>(
        &mut self,
        frame: &[u8],
        out: &'a mut [u8],
        now: u64,
    ) -> Result<(Header, &'a [u8])> {
        let h = Header::decode(frame)?;
        if let Some(cur) = self.current.as_mut()
            && cur.epoch == h.epoch
        {
            return cur.open(frame, out, now);
        }
        if let Some(i) = self
            .pending
            .iter()
            .position(|p| p.as_ref().is_some_and(|s| s.epoch == h.epoch))
        {
            let slot = &mut self.pending[i];
            let r = slot.as_mut().expect("found above").open(frame, out, now)?;
            // CRYPTO-REVIEW: key confirmation passed. This session becomes
            // current; the old current and every other pending session are
            // dropped (keys wiped).
            self.current = slot.take();
            self.pending = Default::default();
            return Ok(r);
        }
        Err(Error::WrongEpoch)
    }

    /// Forget all sessions (node unpaired / removed).
    pub fn clear(&mut self) {
        self.current = None;
        self.pending = Default::default();
    }
}

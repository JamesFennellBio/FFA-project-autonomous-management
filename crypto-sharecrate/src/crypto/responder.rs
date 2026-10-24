//! CRYPTO-REVIEW: responder-side session bookkeeping for one peer (the hub
//! keeps one per node; the PC keeps one for the hub).
//!
//! A new handshake makes a *pending* session. It only replaces the current
//! session once a frame under it verifies (key confirmation). That way a
//! replayed or attacker-sent HELLO can't take down a working session.

use super::handshake::{self, HandshakeRandom};
use super::identity::{Identity, PUBLIC_KEY_LEN};
use super::session::Session;
use crate::frame::{HS_FRAME_LEN, Header};
use crate::{Error, Result};

#[derive(Default)]
pub struct PeerSessions {
    current: Option<Session>,
    pending: Option<Session>,
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
        let mut epoch = epoch_rand;
        if let Some(cur) = &self.current
            && cur.epoch == epoch
        {
            epoch ^= 1;
        }
        let (ack, session) = handshake::respond(hello, peer_pub, identity, epoch, rand, now)?;
        // Overwrites any older pending session (its keys are wiped on drop).
        self.pending = Some(session);
        Ok(ack)
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
        if let Some(p) = self.pending.as_mut()
            && p.epoch == h.epoch
        {
            let r = p.open(frame, out, now)?;
            // CRYPTO-REVIEW: key confirmation passed, switch sessions and
            // drop (and wipe) the old one.
            self.current = self.pending.take();
            return Ok(r);
        }
        Err(Error::WrongEpoch)
    }

    /// Forget all sessions (node unpaired / removed).
    pub fn clear(&mut self) {
        self.current = None;
        self.pending = None;
    }
}

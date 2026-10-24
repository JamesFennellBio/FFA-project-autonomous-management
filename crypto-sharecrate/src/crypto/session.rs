//! CRYPTO-REVIEW: an established session. Seals and opens frames with
//! ChaCha20-Poly1305 and enforces the rekey policy and replay protection.

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce, Tag};
use zeroize::Zeroize;

use super::kdf::{KEY_LEN, SessionKeys};
use crate::frame::{Direction, FRAME_LEN, FrameType, HEADER_LEN, Header, TAG_LEN};
use crate::packet::Packet;
use crate::{Error, Result};

/// CRYPTO-REVIEW: rekey after this many frames in either direction.
/// Counters run 0..=9, so frame 10 can never be sent or accepted.
pub const MAX_MSGS: u32 = 10;
/// CRYPTO-REVIEW: rekey after this many seconds, even if few frames were sent.
pub const MAX_AGE_SECS: u64 = 3600;
/// Extra time the receiving side allows for frames still in flight when the
/// sender's hour runs out.
pub const GRACE_SECS: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Started the handshake (a node, or the hub on its uplink to the PC).
    Initiator,
    /// Answered the handshake (the hub for nodes, the PC for the hub).
    Responder,
}

pub struct Session {
    pub node_id: u16,
    pub epoch: u32,
    pub role: Role,
    send_key: [u8; KEY_LEN],
    recv_key: [u8; KEY_LEN],
    /// Counter for the next frame we send.
    send_ctr: u32,
    /// Lowest counter we will still accept (one past the last good frame).
    recv_next: u32,
    /// How many good frames we've received.
    recv_count: u32,
    /// Time the session was created, in caller seconds (monotonic).
    created: u64,
}

impl Drop for Session {
    fn drop(&mut self) {
        // CRYPTO-REVIEW: wipe the keys when the session is replaced or dropped.
        self.send_key.zeroize();
        self.recv_key.zeroize();
    }
}

/// CRYPTO-REVIEW: the 96-bit ChaCha20 nonce is
/// `direction(1) | epoch(4) | counter(4) | 0x000000(3)`.
/// Within one session key the (direction, counter) pair never repeats: the
/// counter only goes up and stops at MAX_MSGS, and each direction has its own
/// key anyway. Every handshake makes brand-new keys, so a nonce reused across
/// epochs is still under a different key.
fn nonce_for(h: &Header) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0] = h.dir as u8;
    n[1..5].copy_from_slice(&h.epoch.to_le_bytes());
    n[5..9].copy_from_slice(&h.counter.to_le_bytes());
    n
}

impl Session {
    pub(crate) fn new(node_id: u16, epoch: u32, role: Role, keys: &SessionKeys, now: u64) -> Self {
        let (send_key, recv_key) = match role {
            Role::Initiator => (keys.init_to_resp, keys.resp_to_init),
            Role::Responder => (keys.resp_to_init, keys.init_to_resp),
        };
        Self {
            node_id,
            epoch,
            role,
            send_key,
            recv_key,
            send_ctr: 0,
            recv_next: 0,
            recv_count: 0,
            created: now,
        }
    }

    fn send_dir(&self) -> Direction {
        match self.role {
            Role::Initiator => Direction::InitToResp,
            Role::Responder => Direction::RespToInit,
        }
    }

    fn recv_dir(&self) -> Direction {
        match self.role {
            Role::Initiator => Direction::RespToInit,
            Role::Responder => Direction::InitToResp,
        }
    }

    pub fn age(&self, now: u64) -> u64 {
        now.saturating_sub(self.created)
    }

    pub fn sent(&self) -> u32 {
        self.send_ctr
    }

    pub fn received(&self) -> u32 {
        self.recv_count
    }

    /// CRYPTO-REVIEW: the "1 hour or 10 transmissions" rule. The initiator checks
    /// this before every send and starts a new handshake when it's true. It
    /// counts both directions, so a responder that has used up its 10 sends
    /// gets a fresh session the next time the initiator talks.
    pub fn needs_rekey(&self, now: u64) -> bool {
        self.send_ctr >= MAX_MSGS || self.recv_count >= MAX_MSGS || self.age(now) >= MAX_AGE_SECS
    }

    /// True if [`Session::seal`] would succeed right now.
    pub fn can_send(&self, now: u64) -> bool {
        self.send_ctr < MAX_MSGS && self.age(now) < MAX_AGE_SECS
    }

    /// Encrypts `plaintext` into `out` as `header | ciphertext | tag` and
    /// returns the frame length.
    ///
    /// CRYPTO-REVIEW: the 12-byte header is the AEAD associated data, so the
    /// type, node id, epoch, counter and direction can't be changed in transit.
    pub fn seal(
        &mut self,
        ftype: FrameType,
        plaintext: &[u8],
        out: &mut [u8],
        now: u64,
    ) -> Result<usize> {
        if !ftype.is_encrypted() {
            return Err(Error::BadFrame);
        }
        if !self.can_send(now) {
            return Err(Error::RekeyRequired);
        }
        let total = HEADER_LEN + plaintext.len() + TAG_LEN;
        if out.len() < total {
            return Err(Error::BufferTooSmall);
        }
        let header = Header {
            ftype,
            node_id: self.node_id,
            epoch: self.epoch,
            counter: self.send_ctr,
            dir: self.send_dir(),
        };
        let hb = header.encode();
        out[..HEADER_LEN].copy_from_slice(&hb);
        let body = &mut out[HEADER_LEN..HEADER_LEN + plaintext.len()];
        body.copy_from_slice(plaintext);

        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.send_key));
        let tag = cipher
            .encrypt_in_place_detached(Nonce::from_slice(&nonce_for(&header)), &hb, body)
            .map_err(|_| Error::DecryptFailed)?;
        out[HEADER_LEN + plaintext.len()..total].copy_from_slice(&tag);

        // CRYPTO-REVIEW: bump the counter only after a successful seal, so a
        // counter value is never used twice.
        self.send_ctr += 1;
        Ok(total)
    }

    /// Checks and decrypts a frame. The plaintext is written to `out`; returns
    /// the header and the plaintext slice.
    ///
    /// CRYPTO-REVIEW: the order matters. Cheap header checks come first, then
    /// AEAD verification, and the replay state only updates once the tag has
    /// verified, so forged frames can't move the replay window.
    pub fn open<'a>(
        &mut self,
        frame: &[u8],
        out: &'a mut [u8],
        now: u64,
    ) -> Result<(Header, &'a [u8])> {
        if frame.len() < HEADER_LEN + TAG_LEN {
            return Err(Error::BadFrame);
        }
        let header = Header::decode(frame)?;
        if header.node_id != self.node_id {
            return Err(Error::NotPaired);
        }
        if header.epoch != self.epoch {
            return Err(Error::WrongEpoch);
        }
        if header.dir != self.recv_dir() {
            return Err(Error::WrongDirection);
        }
        // CRYPTO-REVIEW: refuse anything from an expired session or past the
        // message limit, even if the tag is valid.
        if header.counter >= MAX_MSGS || self.age(now) > MAX_AGE_SECS + GRACE_SECS {
            return Err(Error::RekeyRequired);
        }
        // CRYPTO-REVIEW: replay protection. The counter must be strictly newer
        // than the last good frame. Gaps are allowed (lost LoRa packets).
        if header.counter < self.recv_next {
            return Err(Error::Replay);
        }

        let ct_len = frame.len() - HEADER_LEN - TAG_LEN;
        if out.len() < ct_len {
            return Err(Error::BufferTooSmall);
        }
        let body = &mut out[..ct_len];
        body.copy_from_slice(&frame[HEADER_LEN..HEADER_LEN + ct_len]);
        let tag = Tag::from_slice(&frame[HEADER_LEN + ct_len..]);

        let cipher = ChaCha20Poly1305::new(Key::from_slice(&self.recv_key));
        if cipher
            .decrypt_in_place_detached(
                Nonce::from_slice(&nonce_for(&header)),
                &frame[..HEADER_LEN],
                body,
                tag,
            )
            .is_err()
        {
            // CRYPTO-REVIEW: don't leave unauthenticated plaintext in the caller's buffer.
            body.zeroize();
            return Err(Error::DecryptFailed);
        }

        self.recv_next = header.counter + 1;
        self.recv_count += 1;
        Ok((header, &out[..ct_len]))
    }

    /// Seals a [`Packet`] into a fixed 64-byte radio frame.
    pub fn seal_packet(&mut self, p: &Packet, now: u64) -> Result<[u8; FRAME_LEN]> {
        let mut out = [0u8; FRAME_LEN];
        self.seal(p.frame_type(), &p.encode(), &mut out, now)?;
        Ok(out)
    }

    /// Opens a 64-byte radio frame into a [`Packet`].
    pub fn open_packet(&mut self, frame: &[u8], now: u64) -> Result<Packet> {
        if frame.len() != FRAME_LEN {
            return Err(Error::BadFrame);
        }
        let mut buf = [0u8; FRAME_LEN];
        let (h, pt) = self.open(frame, &mut buf, now)?;
        Packet::decode(h.ftype, pt)
    }
}

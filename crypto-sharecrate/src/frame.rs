//! Over-the-air frame layout.
//!
//! The first byte of every frame says what it is (the original design from
//! the first version of this crate: `0x00` data, `0x01` message, `0x02` alert),
//! extended with handshake, pairing and control types.
//!
//! Two sizes go over the radio (LoRa or Wi-Fi UDP, same bytes on both):
//!
//! ```text
//! Encrypted frame (64 bytes, fixed so every packet looks the same length):
//!   0      1..3     3..7    7..11     11    12..48           48..64
//!   type | node_id | epoch | counter | dir | ciphertext(36) | poly1305 tag(16)
//!   \________________ header (AAD) _______/
//!
//! Handshake / pairing frame (128 bytes, plaintext but Ed25519-signed):
//!   layout per type, see crypto::handshake and crypto::pairing
//! ```
//!
//! The hub -> PC uplink uses the same header, but the ciphertext length varies
//! (each TCP frame has a u16 length in front).

use crate::{Error, Result};

/// Length of an encrypted radio frame.
pub const FRAME_LEN: usize = 64;
/// Length of a handshake or pairing frame.
pub const HS_FRAME_LEN: usize = 128;
/// Encrypted frame header length (authenticated as AAD, sent in the clear).
pub const HEADER_LEN: usize = 12;
/// Poly1305 tag length.
pub const TAG_LEN: usize = 16;
/// Plaintext bytes that fit in one 64-byte radio frame.
pub const RADIO_PLAINTEXT_LEN: usize = FRAME_LEN - HEADER_LEN - TAG_LEN; // 36

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameType {
    // ---- encrypted, 64 bytes ----
    /// Sensor readings, node -> hub.
    Data = 0x00,
    /// Short text message (debug / status), either direction.
    Message = 0x01,
    /// Something went wrong on the node (sensor fault, threshold), node -> hub.
    Alert = 0x02,
    /// Set an output pin, hub -> node.
    Command = 0x05,
    /// Node tells the hub which channels it has, node -> hub.
    Announce = 0x06,
    /// Acknowledge a command, node -> hub.
    Ack = 0x07,
    // ---- handshake / pairing, 128 bytes, signed ----
    /// Start a session (ephemeral X25519 key), initiator -> responder.
    Hello = 0x03,
    /// Session reply (ephemeral X25519 key), responder -> initiator.
    HelloAck = 0x04,
    /// Unpaired device asks to be paired (button held).
    PairRequest = 0x08,
    /// Responder accepts the pairing and assigns an id.
    PairAccept = 0x09,
    // ---- hub <-> PC only, encrypted, variable length ----
    /// JSON message between hub and PC daemon.
    Uplink = 0x0A,
}

impl FrameType {
    pub fn from_byte(b: u8) -> Result<Self> {
        Ok(match b {
            0x00 => Self::Data,
            0x01 => Self::Message,
            0x02 => Self::Alert,
            0x03 => Self::Hello,
            0x04 => Self::HelloAck,
            0x05 => Self::Command,
            0x06 => Self::Announce,
            0x07 => Self::Ack,
            0x08 => Self::PairRequest,
            0x09 => Self::PairAccept,
            0x0A => Self::Uplink,
            _ => return Err(Error::BadFrame), // unknown type -> drop the frame
        })
    }

    /// True for frames sealed with ChaCha20-Poly1305.
    pub fn is_encrypted(self) -> bool {
        !matches!(
            self,
            Self::Hello | Self::HelloAck | Self::PairRequest | Self::PairAccept
        )
    }
}

/// Which way an encrypted frame travels. It goes into the nonce, so the
/// two directions can never produce the same nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Direction {
    /// Initiator (node, or hub on the uplink) -> responder.
    InitToResp = 0x01,
    /// Responder (hub, or PC on the uplink) -> initiator.
    RespToInit = 0x02,
}

impl Direction {
    pub fn from_byte(b: u8) -> Result<Self> {
        match b {
            0x01 => Ok(Self::InitToResp),
            0x02 => Ok(Self::RespToInit),
            _ => Err(Error::BadFrame),
        }
    }
}

/// The 12-byte clear header of an encrypted frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub ftype: FrameType,
    pub node_id: u16,
    pub epoch: u32,
    pub counter: u32,
    pub dir: Direction,
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0] = self.ftype as u8;
        h[1..3].copy_from_slice(&self.node_id.to_le_bytes());
        h[3..7].copy_from_slice(&self.epoch.to_le_bytes());
        h[7..11].copy_from_slice(&self.counter.to_le_bytes());
        h[11] = self.dir as u8;
        h
    }

    pub fn decode(b: &[u8]) -> Result<Self> {
        if b.len() < HEADER_LEN {
            return Err(Error::BadFrame);
        }
        let ftype = FrameType::from_byte(b[0])?;
        if !ftype.is_encrypted() {
            return Err(Error::BadFrame);
        }
        Ok(Self {
            ftype,
            node_id: u16::from_le_bytes([b[1], b[2]]),
            epoch: u32::from_le_bytes([b[3], b[4], b[5], b[6]]),
            counter: u32::from_le_bytes([b[7], b[8], b[9], b[10]]),
            dir: Direction::from_byte(b[11])?,
        })
    }
}

/// Node id from any frame type that carries one (bytes 1..3).
/// `PairRequest` has no id yet and returns `None`.
pub fn peek_node_id(frame: &[u8]) -> Option<u16> {
    let t = FrameType::from_byte(*frame.first()?).ok()?;
    if t == FrameType::PairRequest || frame.len() < 3 {
        return None;
    }
    Some(u16::from_le_bytes([frame[1], frame[2]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let h = Header {
            ftype: FrameType::Data,
            node_id: 7,
            epoch: 0xDEAD_BEEF,
            counter: 3,
            dir: Direction::InitToResp,
        };
        assert_eq!(Header::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn unknown_type_dropped() {
        let mut b = [0u8; FRAME_LEN];
        b[0] = 0xFF;
        assert_eq!(Header::decode(&b), Err(Error::BadFrame));
    }
}

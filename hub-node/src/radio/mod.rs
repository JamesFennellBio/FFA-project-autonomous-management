//! Radios the hub listens on. Each one sends `Inbound` frames to the hub
//! task and takes frames to transmit. Wi-Fi and LoRa carry the exact same
//! bytes; the crypto doesn't care which one a frame came over.

pub mod udp;

#[cfg(feature = "lora")]
pub mod lora;

use crate::hub::Link;

pub struct Inbound {
    pub frame: Vec<u8>,
    pub link: Link,
}

/// Largest frame on either radio (handshake/pairing frames are 128 bytes).
pub const MAX_RADIO_FRAME: usize = crypto_sharecrate::frame::HS_FRAME_LEN;

//! Saves the node's identity and pairing to flash so they survive reboots.
//!
//! One 80-byte record at 0x9000 (the "nvs" partition in espflash's default
//! partition table; this firmware doesn't use ESP-IDF NVS).
//!
//! ```text
//! "FFA1" | version | flags | node_id(2) | seed(32) | hub_pub(32) | check(8)
//! ```
//!
//! CRYPTO-REVIEW: the Ed25519 seed is stored *unencrypted* in flash. Anyone
//! who has the board in hand can read it with espflash. Fixing that needs
//! ESP32 flash encryption (an eFuse setting you can't undo); see
//! docs/CRYPTO_REVIEW.md. The check bytes catch corruption, not tampering.

use crypto_sharecrate::node::Pairing;
use esp_storage::FlashStorage;
use sha2::{Digest, Sha256};

const OFFSET: u32 = 0x9000;
const MAGIC: &[u8; 4] = b"FFA1";
const VERSION: u8 = 1;
const LEN: usize = 80;

pub struct Persist {
    /// CRYPTO-REVIEW: long-term Ed25519 secret seed.
    pub seed: [u8; 32],
    pub pairing: Option<Pairing>,
}

fn check(body: &[u8]) -> [u8; 8] {
    let h = Sha256::digest(body);
    let mut c = [0u8; 8];
    c.copy_from_slice(&h[..8]);
    c
}

pub fn load(flash: &mut FlashStorage<'_>) -> Option<Persist> {
    let mut b = [0u8; LEN];
    flash.read(OFFSET, &mut b).ok()?;
    if &b[0..4] != MAGIC || b[4] != VERSION || b[72..80] != check(&b[..72]) {
        return None;
    }
    let mut seed = [0u8; 32];
    seed.copy_from_slice(&b[8..40]);
    let pairing = if b[5] & 1 == 1 {
        let mut peer_pub = [0u8; 32];
        peer_pub.copy_from_slice(&b[40..72]);
        Some(Pairing {
            node_id: u16::from_le_bytes([b[6], b[7]]),
            peer_pub,
        })
    } else {
        None
    };
    Some(Persist { seed, pairing })
}

pub fn save(flash: &mut FlashStorage<'_>, p: &Persist) -> bool {
    let mut b = [0u8; LEN];
    b[0..4].copy_from_slice(MAGIC);
    b[4] = VERSION;
    if let Some(pr) = p.pairing {
        b[5] = 1;
        b[6..8].copy_from_slice(&pr.node_id.to_le_bytes());
        b[40..72].copy_from_slice(&pr.peer_pub);
    }
    b[8..40].copy_from_slice(&p.seed);
    let c = check(&b[..72]);
    b[72..80].copy_from_slice(&c);
    flash.write(OFFSET, &b).is_ok()
}

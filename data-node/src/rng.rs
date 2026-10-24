//! CRYPTO-REVIEW: random numbers for keys and handshakes on the ESP32-C3.
//!
//! The problem: the C3's hardware RNG is only *truly* random while the
//! Wi-Fi/BT radio is on, or while the ADC is used as an entropy source.
//! LoRa-only nodes keep Wi-Fi off, and the ADC is busy reading the photoresistor.
//!
//! The fix:
//! 1. At boot, run esp-hal's `TrngSource` (RNG + ADC1 noise) and take a
//!    256-bit seed from it, then give ADC1 back to the sensors.
//! 2. Every later request is a SHA-256 hash-DRBG over that seed, a counter,
//!    and fresh hardware-RNG output (true randomness if the radio is on;
//!    at worst it adds nothing, and the seed still protects us).
//! 3. After each request the internal key is replaced by a one-way hash of
//!    itself (forward secrecy: if the chip's RAM is dumped later, earlier
//!    outputs can't be recomputed).
//!
//! Please check the construction. If you want something standard, swap in
//! HMAC-DRBG (NIST SP 800-90A).

use esp_hal::rng::Rng;
use sha2::{Digest, Sha256};

pub struct NodeRng {
    key: [u8; 32],
    counter: u64,
    hw: Rng,
}

impl NodeRng {
    /// `trng_seed` must come from `Trng` while the TrngSource is running.
    pub fn new(trng_seed: [u8; 32]) -> Self {
        Self {
            key: trng_seed,
            counter: 0,
            hw: Rng::new(),
        }
    }

    pub fn fill(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(32) {
            let mut fresh = [0u8; 32];
            self.hw.read(&mut fresh);
            let block = Sha256::new()
                .chain_update(b"FFA-DRBG-out")
                .chain_update(self.key)
                .chain_update(self.counter.to_le_bytes())
                .chain_update(fresh)
                .finalize();
            chunk.copy_from_slice(&block[..chunk.len()]);
            self.counter = self.counter.wrapping_add(1);
        }
        let next: [u8; 32] = Sha256::new()
            .chain_update(b"FFA-DRBG-next")
            .chain_update(self.key)
            .chain_update(self.counter.to_le_bytes())
            .finalize()
            .into();
        self.key = next;
    }

    pub fn bytes<const N: usize>(&mut self) -> [u8; N] {
        let mut b = [0u8; N];
        self.fill(&mut b);
        b
    }

    pub fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.bytes())
    }
}

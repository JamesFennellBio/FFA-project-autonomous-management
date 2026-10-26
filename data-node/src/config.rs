//! Per-node settings. Wi-Fi details come from environment variables at build
//! time so passwords never end up in git:
//!
//! ```bash
//! export FARMLAB_WIFI_SSID="FarmNet"
//! export FARMLAB_WIFI_PASSWORD="your-password"
//! export FARMLAB_HUB_IP="192.168.1.10"     # the Pi's IP address
//! export FARMLAB_TRANSPORT="auto"          # wifi | lora | auto
//! export FARMLAB_HUB_KEY="<64 hex chars>"  # from `hub-node fingerprint` (recommended)
//! cargo run --release
//! ```
//!
//! Which sensor/output is on which pin is set in `main.rs` (`build_channels`).

/// How the node talks to the hub.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransportMode {
    /// Wi-Fi UDP only.
    Wifi,
    /// SX1262 LoRa only (Wi-Fi radio stays off).
    Lora,
    /// Wi-Fi while it's connected, LoRa when it isn't.
    Auto,
}

pub const WIFI_SSID: &str = match option_env!("FARMLAB_WIFI_SSID") {
    Some(s) => s,
    None => "",
};
pub const WIFI_PASSWORD: &str = match option_env!("FARMLAB_WIFI_PASSWORD") {
    Some(s) => s,
    None => "",
};
const HUB_IP_STR: &str = match option_env!("FARMLAB_HUB_IP") {
    Some(s) => s,
    None => "192.168.1.10",
};
/// CRYPTO-REVIEW: the hub's 64-hex-char public key (`hub-node fingerprint`).
/// Strongly recommended: the node then refuses to pair with any other hub.
/// It's a public key, so putting it in the firmware is fine.
const HUB_KEY_STR: Option<&str> = option_env!("FARMLAB_HUB_KEY");
const TRANSPORT_STR: &str = match option_env!("FARMLAB_TRANSPORT") {
    Some(s) => s,
    None => "wifi",
};

/// Seconds between sensor reports. Every report uses one of the session's 10
/// messages, so at 10 s there's a fresh ECDH handshake about every 100 s.
pub const SAMPLE_INTERVAL_SECS: u64 = 10;

/// Hold BOOT this long to enter pairing mode...
pub const PAIR_HOLD_MS: u64 = 3_000;
/// ...or this long to forget the hub (unpair).
pub const UNPAIR_HOLD_MS: u64 = 10_000;
/// How long pairing mode stays open.
pub const PAIR_WINDOW_SECS: u64 = 60;

pub fn transport() -> TransportMode {
    match TRANSPORT_STR {
        "lora" => TransportMode::Lora,
        "auto" => TransportMode::Auto,
        _ => TransportMode::Wifi,
    }
}

/// The pinned hub key, if FARMLAB_HUB_KEY was set. A malformed value panics at
/// boot so a typo can't silently fall back to trust-on-first-use.
pub fn hub_key() -> Option<[u8; 32]> {
    let s = HUB_KEY_STR?.trim();
    if s.is_empty() {
        return None;
    }
    let b = s.as_bytes();
    assert!(b.len() == 64, "FARMLAB_HUB_KEY must be 64 hex characters");
    let nib = |c: u8| match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        b'A'..=b'F' => c - b'A' + 10,
        _ => panic!("FARMLAB_HUB_KEY must be hex"),
    };
    let mut k = [0u8; 32];
    for (i, out) in k.iter_mut().enumerate() {
        *out = (nib(b[2 * i]) << 4) | nib(b[2 * i + 1]);
    }
    Some(k)
}

pub fn hub_ip() -> [u8; 4] {
    let mut out = [0u8; 4];
    for (i, part) in HUB_IP_STR.split('.').take(4).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    out
}

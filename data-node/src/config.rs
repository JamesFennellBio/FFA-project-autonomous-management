//! Per-node settings. Wi-Fi details come from environment variables at build
//! time so passwords never end up in git:
//!
//! ```bash
//! export FFA_WIFI_SSID="FarmNet"
//! export FFA_WIFI_PASSWORD="hunter22"
//! export FFA_HUB_IP="192.168.1.10"     # the Pi's IP address
//! export FFA_TRANSPORT="auto"          # wifi | lora | auto
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

pub const WIFI_SSID: &str = match option_env!("FFA_WIFI_SSID") {
    Some(s) => s,
    None => "",
};
pub const WIFI_PASSWORD: &str = match option_env!("FFA_WIFI_PASSWORD") {
    Some(s) => s,
    None => "",
};
const HUB_IP_STR: &str = match option_env!("FFA_HUB_IP") {
    Some(s) => s,
    None => "192.168.1.10",
};
const TRANSPORT_STR: &str = match option_env!("FFA_TRANSPORT") {
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

pub fn hub_ip() -> [u8; 4] {
    let mut out = [0u8; 4];
    for (i, part) in HUB_IP_STR.split('.').take(4).enumerate() {
        out[i] = part.parse().unwrap_or(0);
    }
    out
}

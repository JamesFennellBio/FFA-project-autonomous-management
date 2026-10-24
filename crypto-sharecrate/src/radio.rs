//! Radio settings that the nodes and the hub must agree on.
//! (Not crypto; it's here because every device compiles this crate.)

/// UDP port the hub listens on for Wi-Fi nodes.
pub const UDP_PORT: u16 = 7300;

/// LoRa carrier frequency. 915 MHz is the US ISM band (902-928 MHz).
/// In the EU use 868_100_000.
pub const LORA_FREQUENCY_HZ: u32 = 915_000_000;
/// Spreading factor (7-12). Higher = more range but longer time on air.
/// At SF7/125 kHz a 64-byte frame is ~120 ms on air and a 128-byte handshake
/// frame ~215 ms, both under the 400 ms per-transmission dwell limit US
/// rules (FCC Part 15.247) put on 915 MHz. SF9 would be ~680 ms for the
/// handshake, over that limit. Not legal advice; check your local rules.
pub const LORA_SPREADING_FACTOR: u8 = 7;
/// Bandwidth in kHz.
pub const LORA_BANDWIDTH_KHZ: u16 = 125;
/// Coding rate 4/5.
pub const LORA_CODING_RATE_DENOM: u8 = 5;
pub const LORA_PREAMBLE_SYMBOLS: u16 = 8;
/// `false` = private sync word (0x1424), so LoRaWAN gateways nearby ignore us.
pub const LORA_PUBLIC_NETWORK: bool = false;
/// Transmit power in dBm (SX1262 max is +22). Check local rules.
pub const LORA_TX_POWER_DBM: i32 = 14;
/// How long a LoRa node listens for a reply after it transmits.
pub const LORA_RX_WINDOW_MS: u64 = 2_000;

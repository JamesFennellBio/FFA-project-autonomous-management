use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::Deserialize;

/// `/etc/ffa-hub/hub.toml` (see hub-node/hub.example.toml).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// identity.key, paired.json, pc_pairing.json and hub.db go here.
    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,
    /// Where Wi-Fi nodes send their UDP frames.
    #[serde(default = "default_udp")]
    pub udp_listen: String,
    /// Where the phone app sends its UDP frames.
    #[serde(default = "default_app")]
    pub app_listen: String,
    /// The PC daemon (`pc-daemon --hub-listen`), e.g. "192.168.1.50:7400".
    pub pc_addr: String,
    /// CRYPTO-REVIEW: the PC daemon's public key (64 hex chars), shown in the
    /// app under Pairing and in pc-daemon's startup log. The hub refuses any
    /// other PC (finding 12).
    pub pc_public_key: String,
    /// How often the RAM database is written to the SD card.
    #[serde(default = "default_flush")]
    pub flush_interval_secs: u64,
    #[serde(default)]
    pub lora: LoraConfig,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "lora"), allow(dead_code))]
pub struct LoraConfig {
    #[serde(default)]
    pub enabled: bool,
    /// SPI bus 0, chip select 0 = /dev/spidev0.0
    #[serde(default)]
    pub spi_bus: u8,
    #[serde(default)]
    pub spi_cs: u8,
    /// BCM GPIO numbers (not physical pin numbers).
    #[serde(default = "d_reset")]
    pub reset_pin: u8,
    #[serde(default = "d_busy")]
    pub busy_pin: u8,
    #[serde(default = "d_dio1")]
    pub dio1_pin: u8,
    /// Hz. 915 MHz for the US (902-928 MHz ISM band); 868 MHz in the EU.
    #[serde(default = "d_freq")]
    pub frequency: u32,
    /// Spreading factor 7-12. Higher = longer range, slower. Must match the nodes.
    #[serde(default = "d_sf")]
    pub spreading_factor: u8,
    /// Some SX1262 breakouts power their TCXO from DIO3; set true for those (e.g. Ebyte E22).
    #[serde(default)]
    pub tcxo: bool,
}

impl Default for LoraConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            spi_bus: 0,
            spi_cs: 0,
            reset_pin: d_reset(),
            busy_pin: d_busy(),
            dio1_pin: d_dio1(),
            frequency: d_freq(),
            spreading_factor: d_sf(),
            tcxo: false,
        }
    }
}

fn default_data_dir() -> PathBuf {
    "/var/lib/ffa-hub".into()
}
fn default_udp() -> String {
    "0.0.0.0:7300".into()
}
fn default_app() -> String {
    format!("0.0.0.0:{}", uplink::app::APP_PORT)
}
fn default_flush() -> u64 {
    86_400
}
fn d_reset() -> u8 {
    22
}
fn d_busy() -> u8 {
    23
}
fn d_dio1() -> u8 {
    24
}
fn d_freq() -> u32 {
    915_000_000
}
fn d_sf() -> u8 {
    crypto_sharecrate::radio::LORA_SPREADING_FACTOR
}

impl Config {
    pub fn load(path: &PathBuf) -> Result<Self> {
        let s =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&s).with_context(|| format!("parsing {}", path.display()))
    }

    pub fn pc_key(&self) -> Result<[u8; 32]> {
        uplink::key_from_hex(self.pc_public_key.trim()).context(
            "pc_public_key in hub.toml must be the PC's 64-character public key \
             (shown on the app's Pairing page and in pc-daemon's log)",
        )
    }
}

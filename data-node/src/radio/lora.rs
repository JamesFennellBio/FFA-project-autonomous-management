//! SX1262 LoRa link (build with `--features lora`).
//!
//! LoRa is half-duplex and slow, so it works LoRaWAN "class A" style: the
//! node transmits, then listens for `LORA_RX_WINDOW_MS`. The hub only sends to
//! a LoRa node inside that window (see hub-node/src/hub.rs).
//!
//! Wiring (ESP32-C3 -> SX1262 breakout):
//!   GPIO10 SCK · GPIO7 MOSI · GPIO21 MISO · GPIO2 NSS · GPIO0 RESET · GPIO1 BUSY · GPIO20 DIO1 · 3V3 · GND

use crypto_sharecrate::radio::*;
use embassy_time::{Delay, Duration, with_timeout};
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::Async;
use esp_hal::gpio::{Input, Output};
use esp_hal::spi::master::SpiDma;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::mod_params::{
    Bandwidth, CodingRate, ModulationParams, PacketParams, SpreadingFactor,
};
use lora_phy::sx126x::{self, Sx126x, Sx1262, TcxoCtrlVoltage};
use lora_phy::{LoRa, RxMode};

use super::Frame;

type Spi = ExclusiveDevice<SpiDma<'static, Async>, Output<'static>, Delay>;
type Iv = GenericSx126xInterfaceVariant<Output<'static>, Input<'static>>;
type Radio = LoRa<Sx126x<Spi, Iv, Sx1262>, Delay>;

pub struct LoraLink {
    radio: Radio,
    modulation: ModulationParams,
    tx_params: PacketParams,
    rx_params: PacketParams,
}

fn sf() -> SpreadingFactor {
    match LORA_SPREADING_FACTOR {
        7 => SpreadingFactor::_7,
        8 => SpreadingFactor::_8,
        10 => SpreadingFactor::_10,
        11 => SpreadingFactor::_11,
        12 => SpreadingFactor::_12,
        _ => SpreadingFactor::_9,
    }
}

fn bw() -> Bandwidth {
    match LORA_BANDWIDTH_KHZ {
        250 => Bandwidth::_250KHz,
        500 => Bandwidth::_500KHz,
        _ => Bandwidth::_125KHz,
    }
}

fn cr() -> CodingRate {
    match LORA_CODING_RATE_DENOM {
        6 => CodingRate::_4_6,
        7 => CodingRate::_4_7,
        8 => CodingRate::_4_8,
        _ => CodingRate::_4_5,
    }
}

impl LoraLink {
    /// `tcxo`: true for breakouts that power a TCXO from DIO3 (e.g. Ebyte E22-900M).
    pub async fn new(
        spi: Spi,
        reset: Output<'static>,
        busy: Input<'static>,
        dio1: Input<'static>,
        tcxo: bool,
    ) -> Option<Self> {
        let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, None, None).ok()?;
        let cfg = sx126x::Config {
            chip: Sx1262,
            tcxo_ctrl: if tcxo {
                Some(TcxoCtrlVoltage::Ctrl1V8)
            } else {
                None
            },
            use_dcdc: true,
            rx_boost: true,
        };
        let mut radio = match LoRa::new(Sx126x::new(spi, iv, cfg), LORA_PUBLIC_NETWORK, Delay).await
        {
            Ok(r) => r,
            Err(e) => {
                log::error!("SX1262 init failed (check wiring): {e:?}");
                return None;
            }
        };
        let modulation = radio
            .create_modulation_params(sf(), bw(), cr(), LORA_FREQUENCY_HZ)
            .ok()?;
        let tx_params = radio
            .create_tx_packet_params(LORA_PREAMBLE_SYMBOLS, false, true, false, &modulation)
            .ok()?;
        let rx_params = radio
            .create_rx_packet_params(LORA_PREAMBLE_SYMBOLS, false, 255, true, false, &modulation)
            .ok()?;
        log::info!(
            "LoRa ready: {} Hz SF{}",
            LORA_FREQUENCY_HZ,
            LORA_SPREADING_FACTOR
        );
        Some(Self {
            radio,
            modulation,
            tx_params,
            rx_params,
        })
    }

    pub async fn send(&mut self, frame: &[u8]) -> bool {
        if let Err(e) = self
            .radio
            .prepare_for_tx(
                &self.modulation,
                &mut self.tx_params,
                LORA_TX_POWER_DBM,
                frame,
            )
            .await
        {
            log::warn!("lora prepare tx: {e:?}");
            return false;
        }
        match self.radio.tx().await {
            Ok(()) => true,
            Err(e) => {
                log::warn!("lora tx: {e:?}");
                false
            }
        }
    }

    pub async fn recv(&mut self, timeout_ms: u64) -> Option<Frame> {
        if self
            .radio
            .prepare_for_rx(RxMode::Continuous, &self.modulation, &self.rx_params)
            .await
            .is_err()
        {
            return None;
        }
        let mut buf = [0u8; 255];
        let r = with_timeout(
            Duration::from_millis(timeout_ms),
            self.radio.rx(&self.rx_params, &mut buf),
        )
        .await;
        let _ = self.radio.enter_standby().await;
        match r {
            Ok(Ok((n, _status))) => Frame::from_slice(&buf[..n as usize]).ok(),
            _ => None,
        }
    }
}

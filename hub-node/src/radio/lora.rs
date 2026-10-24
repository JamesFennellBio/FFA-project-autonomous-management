//! SX1262 LoRa on the Raspberry Pi (build with `--features lora`).
//!
//! Wiring (Pi header, BCM numbering):
//!   SCK  -> GPIO11 (pin 23)   MOSI -> GPIO10 (pin 19)   MISO -> GPIO9 (pin 21)
//!   NSS  -> GPIO25 (pin 22)   *software chip-select, not CE0* (see below)
//!   RESET-> GPIO22 (pin 15)   BUSY -> GPIO23 (pin 16)   DIO1 -> GPIO24 (pin 18)
//!   3V3  -> pin 17            GND  -> pin 20
//! Enable SPI first: `sudo raspi-config` -> Interface Options -> SPI.
//!
//! Why software NSS: Linux spidev toggles CE0 between the separate
//! read/write calls of one SX1262 command, and the chip drops the command.
//! So NSS is a GPIO held low for the whole transaction.
//!
//! lora-phy is async but rppal is blocking. The adapters below wrap the
//! blocking calls. The radio runs on its own thread with its own small tokio
//! runtime, so the busy-wait polling never stalls the main hub loop.

use std::time::Duration;

use anyhow::{Context, Result};
use crypto_sharecrate::radio::*;
use embedded_hal::spi::{ErrorType, Operation, SpiDevice};
use embedded_hal_async::digital::Wait;
use lora_phy::iv::GenericSx126xInterfaceVariant;
use lora_phy::mod_params::{Bandwidth, CodingRate, SpreadingFactor};
use lora_phy::sx126x::{self, Sx126x, Sx1262, TcxoCtrlVoltage};
use lora_phy::{DelayNs, LoRa, RxMode};
use rppal::gpio::{Gpio, InputPin, OutputPin};
use rppal::spi::{Bus, Mode, SlaveSelect, Spi};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use super::{Inbound, MAX_RADIO_FRAME};
use crate::config::LoraConfig;
use crate::hub::Link;

/// NSS (chip select) BCM pin. Not in hub.toml since the wiring above assumes it.
const NSS_PIN: u8 = 25;

/// Blocking SpiDevice -> async SpiDevice.
struct AsyncSpi<D>(D);

impl<D: ErrorType> ErrorType for AsyncSpi<D> {
    type Error = D::Error;
}

impl<D: SpiDevice> embedded_hal_async::spi::SpiDevice for AsyncSpi<D> {
    async fn transaction(
        &mut self,
        operations: &mut [Operation<'_, u8>],
    ) -> Result<(), Self::Error> {
        self.0.transaction(operations)
    }
}

/// Polls an input pin for the async `Wait` trait (BUSY and DIO1).
struct PollWait(InputPin);

impl embedded_hal::digital::ErrorType for PollWait {
    type Error = core::convert::Infallible;
}

impl PollWait {
    async fn until(&mut self, high: bool) {
        while self.0.is_high() != high {
            tokio::time::sleep(Duration::from_micros(200)).await;
        }
    }
}

impl Wait for PollWait {
    async fn wait_for_high(&mut self) -> Result<(), Self::Error> {
        self.until(true).await;
        Ok(())
    }
    async fn wait_for_low(&mut self) -> Result<(), Self::Error> {
        self.until(false).await;
        Ok(())
    }
    async fn wait_for_rising_edge(&mut self) -> Result<(), Self::Error> {
        self.until(false).await;
        self.until(true).await;
        Ok(())
    }
    async fn wait_for_falling_edge(&mut self) -> Result<(), Self::Error> {
        self.until(true).await;
        self.until(false).await;
        Ok(())
    }
    async fn wait_for_any_edge(&mut self) -> Result<(), Self::Error> {
        let now = self.0.is_high();
        self.until(!now).await;
        Ok(())
    }
}

/// tokio-based delay for lora-phy.
struct TokioDelay;

impl DelayNs for TokioDelay {
    async fn delay_ns(&mut self, ns: u32) {
        tokio::time::sleep(Duration::from_nanos(ns as u64)).await;
    }
}

fn sf(v: u8) -> SpreadingFactor {
    match v {
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

/// Starts the radio thread. Returns a sender for frames to transmit.
pub fn spawn(cfg: LoraConfig, inbound: mpsc::Sender<Inbound>) -> Result<mpsc::Sender<Vec<u8>>> {
    let (tx, rx) = mpsc::channel::<Vec<u8>>(32);
    std::thread::Builder::new()
        .name("lora".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .expect("lora runtime");
            rt.block_on(async move {
                if let Err(e) = run(cfg, inbound, rx).await {
                    error!("LoRa radio stopped: {e:#}");
                }
            });
        })
        .context("spawning lora thread")?;
    Ok(tx)
}

async fn run(
    cfg: LoraConfig,
    inbound: mpsc::Sender<Inbound>,
    mut outbound: mpsc::Receiver<Vec<u8>>,
) -> Result<()> {
    let gpio = Gpio::new().context("opening GPIO (are you on a Pi?)")?;
    let bus_id = if cfg.spi_bus == 1 {
        Bus::Spi1
    } else {
        Bus::Spi0
    };
    let ss = if cfg.spi_cs == 1 {
        SlaveSelect::Ss1
    } else {
        SlaveSelect::Ss0
    };
    let bus = Spi::new(bus_id, ss, 8_000_000, Mode::Mode0)
        .context("opening SPI (enable it in raspi-config)")?;
    let nss: OutputPin = gpio.get(NSS_PIN)?.into_output_high();
    let spi = embedded_hal_bus::spi::ExclusiveDevice::new(bus, nss, rppal::hal::Delay::new())
        .map_err(|e| anyhow::anyhow!("nss: {e:?}"))?;
    let reset = gpio.get(cfg.reset_pin)?.into_output_high();
    let busy = PollWait(gpio.get(cfg.busy_pin)?.into_input());
    let dio1 = PollWait(gpio.get(cfg.dio1_pin)?.into_input());

    let iv = GenericSx126xInterfaceVariant::new(reset, dio1, busy, None, None)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let radio_cfg = sx126x::Config {
        chip: Sx1262,
        tcxo_ctrl: cfg.tcxo.then_some(TcxoCtrlVoltage::Ctrl1V8),
        use_dcdc: true,
        rx_boost: true,
    };
    let mut lora = LoRa::new(
        Sx126x::new(AsyncSpi(spi), iv, radio_cfg),
        LORA_PUBLIC_NETWORK,
        TokioDelay,
    )
    .await
    .map_err(|e| anyhow::anyhow!("SX1262 init failed (check wiring): {e:?}"))?;

    let modulation = lora
        .create_modulation_params(sf(cfg.spreading_factor), bw(), cr(), cfg.frequency)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let mut tx_params = lora
        .create_tx_packet_params(LORA_PREAMBLE_SYMBOLS, false, true, false, &modulation)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let rx_params = lora
        .create_rx_packet_params(LORA_PREAMBLE_SYMBOLS, false, 255, true, false, &modulation)
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
    info!(
        "LoRa ready: {} Hz, SF{}",
        cfg.frequency, cfg.spreading_factor
    );

    let mut buf = [0u8; 255];
    loop {
        if let Err(e) = lora
            .prepare_for_rx(RxMode::Continuous, &modulation, &rx_params)
            .await
        {
            warn!("lora rx setup: {e:?}");
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }
        // Listen until a packet arrives OR we have something to send.
        let to_send = tokio::select! {
            r = lora.rx(&rx_params, &mut buf) => {
                match r {
                    Ok((n, status)) => {
                        let n = n as usize;
                        if n > 0 && n <= MAX_RADIO_FRAME {
                            tracing::debug!("lora rx {n} bytes rssi {} snr {}", status.rssi, status.snr);
                            let _ = inbound.send(Inbound { frame: buf[..n].to_vec(), link: Link::Lora }).await;
                        }
                    }
                    Err(e) => warn!("lora rx: {e:?}"),
                }
                None
            }
            f = outbound.recv() => match f {
                Some(f) => Some(f),
                None => return Ok(()),
            },
        };
        if let Some(frame) = to_send {
            let _ = lora.enter_standby().await;
            match lora
                .prepare_for_tx(&modulation, &mut tx_params, LORA_TX_POWER_DBM, &frame)
                .await
            {
                Ok(()) => {
                    if let Err(e) = lora.tx().await {
                        warn!("lora tx: {e:?}");
                    }
                }
                Err(e) => warn!("lora tx setup: {e:?}"),
            }
        }
    }
}

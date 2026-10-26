//! Picks Wi-Fi or LoRa for each transmission, depending on `FARMLAB_TRANSPORT`.

#[cfg(feature = "lora")]
pub mod lora;
pub mod wifi;

use crate::config::TransportMode;

/// One received frame (handshake frames are the biggest at 128 bytes).
pub type Frame = heapless::Vec<u8, 128>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Via {
    Wifi,
    #[cfg(feature = "lora")]
    Lora,
}

pub struct Radios {
    pub mode: TransportMode,
    pub wifi: Option<wifi::WifiLink>,
    #[cfg(feature = "lora")]
    pub lora: Option<lora::LoraLink>,
}

impl Radios {
    /// The link to use right now, if any.
    pub fn pick(&self) -> Option<Via> {
        let wifi_up = self.wifi.as_ref().is_some_and(|w| w.is_up());
        #[cfg(feature = "lora")]
        let lora_ok = self.lora.is_some();
        #[cfg(not(feature = "lora"))]
        let lora_ok = false;
        match self.mode {
            TransportMode::Wifi => wifi_up.then_some(Via::Wifi),
            #[cfg(feature = "lora")]
            TransportMode::Lora => lora_ok.then_some(Via::Lora),
            #[cfg(feature = "lora")]
            TransportMode::Auto if wifi_up => Some(Via::Wifi),
            #[cfg(feature = "lora")]
            TransportMode::Auto => lora_ok.then_some(Via::Lora),
            #[cfg(not(feature = "lora"))]
            _ => {
                let _ = lora_ok;
                wifi_up.then_some(Via::Wifi)
            }
        }
    }

    pub async fn send(&mut self, via: Via, frame: &[u8]) -> bool {
        match via {
            Via::Wifi => match &mut self.wifi {
                Some(w) => w.send(frame).await,
                None => false,
            },
            #[cfg(feature = "lora")]
            Via::Lora => match &mut self.lora {
                Some(l) => l.send(frame).await,
                None => false,
            },
        }
    }

    pub async fn recv(&mut self, via: Via, timeout_ms: u64) -> Option<Frame> {
        match via {
            Via::Wifi => self.wifi.as_mut()?.recv(timeout_ms).await,
            #[cfg(feature = "lora")]
            Via::Lora => self.lora.as_mut()?.recv(timeout_ms).await,
        }
    }

    /// How long to keep listening after a send before the link goes quiet.
    /// Wi-Fi nodes listen the whole time between reports; LoRa only in its window.
    pub fn listen_window_ms(&self, via: Via, until_next_report_ms: u64) -> u64 {
        match via {
            Via::Wifi => until_next_report_ms,
            #[cfg(feature = "lora")]
            Via::Lora => crypto_sharecrate::radio::LORA_RX_WINDOW_MS.min(until_next_report_ms),
        }
    }
}

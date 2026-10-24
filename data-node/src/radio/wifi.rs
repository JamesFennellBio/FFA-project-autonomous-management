//! 2.4 GHz link: join the Wi-Fi network, then send each frame to the hub as
//! one UDP datagram. (Same bytes as LoRa; the crypto is end to end.)

use embassy_executor::Spawner;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{IpAddress, IpEndpoint, Runner, Stack, StackResources};
use embassy_time::{Duration, Timer, with_timeout};
use esp_hal::peripherals::WIFI;
use esp_radio::wifi::sta::StationConfig;
use esp_radio::wifi::{AuthenticationMethodConfig, Config, Interface, WifiController};
use static_cell::StaticCell;

use super::Frame;
use crate::config;

pub struct WifiLink {
    stack: Stack<'static>,
    socket: UdpSocket<'static>,
    hub: IpEndpoint,
}

static RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
static RX_META: StaticCell<[PacketMetadata; 8]> = StaticCell::new();
static TX_META: StaticCell<[PacketMetadata; 8]> = StaticCell::new();
static RX_BUF: StaticCell<[u8; 2048]> = StaticCell::new();
static TX_BUF: StaticCell<[u8; 2048]> = StaticCell::new();

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

/// Keeps the station connected, reconnecting after drops.
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) {
    loop {
        match controller.connect_async().await {
            Ok(_) => {
                log::info!("wifi connected to {}", config::WIFI_SSID);
                let _ = controller.wait_for_disconnect_async().await;
                log::warn!("wifi disconnected");
            }
            Err(e) => log::warn!("wifi connect failed: {e:?}"),
        }
        Timer::after_secs(5).await;
    }
}

pub fn start(spawner: &Spawner, wifi: WIFI<'static>, net_seed: u64) -> Option<WifiLink> {
    if config::WIFI_SSID.is_empty() {
        log::warn!("FFA_WIFI_SSID was not set at build time; Wi-Fi disabled");
        return None;
    }
    let mut controller = match WifiController::new(wifi, Default::default()) {
        Ok(c) => c,
        Err(e) => {
            log::error!("wifi init failed: {e:?}");
            return None;
        }
    };
    let auth = if config::WIFI_PASSWORD.is_empty() {
        AuthenticationMethodConfig::Open
    } else {
        AuthenticationMethodConfig::Wpa2Personal(config::WIFI_PASSWORD.try_into().ok()?)
    };
    let station = StationConfig::default()
        .with_ssid(config::WIFI_SSID.try_into().ok()?)
        .with_authentication(auth);
    if let Err(e) = controller.set_config(&Config::Station(station)) {
        log::error!("wifi config failed: {e:?}");
        return None;
    }

    let (stack, runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        net_seed,
    );
    spawner.spawn(net_task(runner).unwrap());
    spawner.spawn(connection_task(controller).unwrap());

    let mut socket = UdpSocket::new(
        stack,
        RX_META.init([PacketMetadata::EMPTY; 8]),
        RX_BUF.init([0; 2048]),
        TX_META.init([PacketMetadata::EMPTY; 8]),
        TX_BUF.init([0; 2048]),
    );
    socket.bind(0).ok()?;
    let [a, b, c, d] = config::hub_ip();
    Some(WifiLink {
        stack,
        socket,
        hub: IpEndpoint::new(
            IpAddress::v4(a, b, c, d),
            crypto_sharecrate::radio::UDP_PORT,
        ),
    })
}

impl WifiLink {
    /// Connected and has an IP address.
    pub fn is_up(&self) -> bool {
        self.stack.is_config_up()
    }

    pub async fn send(&mut self, frame: &[u8]) -> bool {
        self.socket.send_to(frame, self.hub).await.is_ok()
    }

    /// Waits up to `timeout_ms` for a datagram from the hub.
    pub async fn recv(&mut self, timeout_ms: u64) -> Option<Frame> {
        let mut buf = [0u8; 256];
        let hub = self.hub;
        match with_timeout(
            Duration::from_millis(timeout_ms),
            self.socket.recv_from(&mut buf),
        )
        .await
        {
            Ok(Ok((n, meta))) if meta.endpoint.addr == hub.addr => {
                Frame::from_slice(&buf[..n]).ok()
            }
            _ => None,
        }
    }
}

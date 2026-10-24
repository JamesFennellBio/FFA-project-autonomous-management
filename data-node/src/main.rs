//! ESP32-C3 data node firmware (no_std, async on esp-rtos + embassy).
//!
//! Demo node wiring (ESP32-C3 SuperMini / DevKitM-1):
//!
//! | Ch | What                    | GPIO | Notes                                         |
//! |----|-------------------------|------|-----------------------------------------------|
//! | 0  | DS18B20 thermometer     | 4    | 4.7 kΩ pull-up to 3V3                         |
//! | 1  | Photoresistor (LDR)     | 3    | LDR to 3V3 + 10 kΩ to GND, middle -> GPIO3   |
//! | 2  | Siren                   | 5    | through a logic-level MOSFET or relay module  |
//! | 3  | Solenoid valve          | 6    | MOSFET + flyback diode across the coil!       |
//! | –  | Pairing button          | 9    | the on-board BOOT button                      |
//! | –  | Status LED              | 8    | on-board LED (active low)                     |
//!
//! LoRa SX1262 pins are listed in radio/lora.rs.
//!
//! Hold BOOT for 3 s (after boot) to pair, or 10 s to forget the hub.
//! Don't hold it while plugging in: GPIO9 low at reset puts the chip in
//! download mode.

#![no_std]
#![no_main]

extern crate alloc;

mod config;
mod onewire;
mod pins;
mod radio;
mod rng;
mod storage;

use core::sync::atomic::{AtomicBool, Ordering};

use crypto_sharecrate::node::{NodeCore, NodeEvent};
use crypto_sharecrate::packet::{Packet, alert_code};
use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::analog::adc::{Adc, AdcCalCurve, AdcConfig, Attenuation};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::peripherals::ADC1;
use esp_hal::rng::{Trng, TrngSource};
use esp_hal::timer::timg::TimerGroup;
use esp_storage::FlashStorage;

use crate::config::TransportMode;
use crate::pins::Channels;
use crate::radio::{Radios, Via};
use crate::rng::NodeRng;
use crate::storage::Persist;

esp_bootloader_esp_idf::esp_app_desc!();

/// Turn every output off if we hear nothing valid from the hub for this
/// long. A stuck-open solenoid could flood a greenhouse.
const FAILSAFE_SECS: u64 = 300;

static PAIR_PRESSED: AtomicBool = AtomicBool::new(false);
static UNPAIR_PRESSED: AtomicBool = AtomicBool::new(false);

#[embassy_executor::task]
async fn button_task(mut button: Input<'static>) {
    loop {
        button.wait_for_low().await;
        let start = Instant::now();
        button.wait_for_high().await;
        let held = start.elapsed().as_millis();
        if held >= config::UNPAIR_HOLD_MS {
            UNPAIR_PRESSED.store(true, Ordering::Relaxed);
        } else if held >= config::PAIR_HOLD_MS {
            PAIR_PRESSED.store(true, Ordering::Relaxed);
        }
    }
}

fn take(flag: &AtomicBool) -> bool {
    // No atomic swap on the C3's riscv32imc, but only one task clears these.
    let v = flag.load(Ordering::Relaxed);
    if v {
        flag.store(false, Ordering::Relaxed);
    }
    v
}

fn secs() -> u64 {
    Instant::now().as_secs()
}

struct App<'d> {
    core: NodeCore,
    chans: Channels<'d>,
    flash: FlashStorage<'d>,
    seed: [u8; 32],
    radios: Radios,
    rng: NodeRng,
    led: Output<'d>,
    last_contact: Instant,
    failsafe_tripped: bool,
    announced: bool,
    last_faults: heapless::Vec<u8, 7>,
}

impl App<'_> {
    fn save(&mut self) {
        let p = Persist {
            seed: self.seed,
            pairing: self.core.pairing(),
        };
        if !storage::save(&mut self.flash, &p) {
            log::error!("flash write failed");
        }
    }

    fn handle(&mut self, frame: &[u8]) {
        match self.core.handle_frame(frame, secs()) {
            Ok(NodeEvent::Paired(p)) => {
                self.save();
                let fp = uplink_fp(&p.peer_pub);
                log::info!("PAIRED as node {} with hub {}", p.node_id, fp.as_str());
                self.last_contact = Instant::now();
            }
            Ok(NodeEvent::SessionUp { epoch }) => {
                log::info!(
                    "session #{} up (epoch {:#010x})",
                    self.core.sessions_started(),
                    epoch
                );
                self.last_contact = Instant::now();
                self.failsafe_tripped = false;
            }
            Ok(NodeEvent::Command { channel, value }) => {
                self.last_contact = Instant::now();
                if self.chans.set_output(channel, value) {
                    log::info!("command: channel {channel} = {value}");
                } else {
                    log::warn!("command for channel {channel}, which is not an output");
                }
            }
            Ok(NodeEvent::Packet(p)) => {
                self.last_contact = Instant::now();
                log::info!("hub says: {}", p.text());
            }
            Ok(NodeEvent::Ignored) => {}
            Err(e) => log::debug!("dropped frame: {e:?}"),
        }
    }

    /// Listens on `via` for up to `ms`, handling everything that arrives.
    /// Stops early once `stop()` is true.
    async fn listen(&mut self, via: Via, ms: u64, stop: impl Fn(&NodeCore) -> bool) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let left = (deadline - Instant::now()).as_millis().max(1);
            if let Some(f) = self.radios.recv(via, left).await {
                self.handle(&f);
                if stop(&self.core) {
                    return;
                }
            }
        }
    }

    async fn send(&mut self, via: Via, frame: &[u8]) -> bool {
        self.led.set_low(); // LED on while transmitting
        let ok = self.radios.send(via, frame).await;
        self.led.set_high();
        ok
    }

    async fn run(&mut self) -> ! {
        let mut pairing_until: Option<u64> = None;
        let mut next_report = Instant::now();
        let mut last_nag = 0u64;

        loop {
            if take(&UNPAIR_PRESSED) {
                log::warn!("BOOT held 10 s: forgetting the hub");
                self.core.unpair();
                self.save();
            }
            if take(&PAIR_PRESSED) {
                if self.core.is_paired() {
                    log::info!("already paired (hold BOOT 10 s to unpair first)");
                } else {
                    log::info!("pairing mode for {} s", config::PAIR_WINDOW_SECS);
                    pairing_until = Some(secs() + config::PAIR_WINDOW_SECS);
                }
            }
            if !self.failsafe_tripped && self.last_contact.elapsed().as_secs() > FAILSAFE_SECS {
                log::warn!("no contact with hub for {FAILSAFE_SECS} s: outputs OFF");
                self.chans.all_outputs_off();
                self.failsafe_tripped = true;
            }

            let Some(via) = self.radios.pick() else {
                Timer::after_secs(1).await;
                continue;
            };

            // ---- pairing ----
            if !self.core.is_paired() {
                match pairing_until {
                    Some(t) if secs() < t => {
                        let fp = self.core.fingerprint();
                        log::info!(
                            "asking to pair; approve {} in the app",
                            core::str::from_utf8(&fp).unwrap_or("?")
                        );
                        let req = self.core.pair_request();
                        self.send(via, &req).await;
                        self.listen(via, 3_000, |c| c.is_paired()).await;
                        if self.core.is_paired() {
                            pairing_until = None;
                        }
                    }
                    _ => {
                        pairing_until = None;
                        if secs() - last_nag >= 30 {
                            last_nag = secs();
                            log::info!("not paired: hold BOOT for 3 s to pair");
                        }
                        Timer::after_secs(1).await;
                    }
                }
                continue;
            }

            // ---- CRYPTO-REVIEW: new ECDH session every 10 messages / 1 hour ----
            if self.core.needs_handshake(secs()) {
                let r = self.rng.bytes::<48>(); // CRYPTO-REVIEW: ephemeral key + nonce
                match self.core.start_handshake(&r, secs()) {
                    Ok(hello) => {
                        self.send(via, &hello).await;
                        let before = self.core.sessions_started();
                        self.listen(via, 3_000, move |c| c.sessions_started() != before)
                            .await;
                    }
                    Err(e) => log::warn!("handshake: {e:?}"),
                }
                if self.core.needs_handshake(secs()) && !self.core.handshake_pending(secs()) {
                    Timer::after_secs(2).await; // hub didn't answer; back off a bit
                }
                continue;
            }

            // ---- between reports, keep listening on Wi-Fi for commands ----
            let now = Instant::now();
            if now < next_report {
                let left = (next_report - now).as_millis();
                match via {
                    Via::Wifi => self.listen(via, left, |_| false).await,
                    #[allow(unreachable_patterns)]
                    _ => Timer::after_millis(left).await,
                }
                continue;
            }
            next_report = Instant::now() + Duration::from_secs(config::SAMPLE_INTERVAL_SECS);

            // ---- report ----
            let packet = if !self.announced {
                self.announced = true;
                let info = self.chans.info();
                Packet::announce(&info)
            } else {
                let (readings, faults) = self.chans.sample().await;
                if faults != self.last_faults {
                    self.last_faults = faults.clone();
                    if let Some(&ch) = faults.first() {
                        // Uses one of this session's 10 messages; only sent when faults change.
                        if let Ok(f) = self.core.seal(
                            &Packet::Alert {
                                channel: ch,
                                code: alert_code::SENSOR_FAULT,
                                value: 0,
                            },
                            secs(),
                        ) {
                            self.send(via, &f).await;
                        }
                    }
                }
                Packet::data(&readings)
            };
            match self.core.seal(&packet, secs()) {
                Ok(f) => {
                    if !self.send(via, &f).await {
                        log::warn!("send failed on {via:?}");
                    }
                }
                Err(e) => {
                    log::debug!("seal: {e:?}");
                    continue;
                }
            }
            // LoRa nodes only hear the hub in this short window after sending.
            let window = self
                .radios
                .listen_window_ms(via, config::SAMPLE_INTERVAL_SECS * 1000);
            if window < config::SAMPLE_INTERVAL_SECS * 1000 {
                self.listen(via, window, |_| false).await;
            }
        }
    }
}

/// "AB12-CD34-..." for a public key, without std.
fn uplink_fp(pk: &[u8; 32]) -> heapless::String<19> {
    let fp = crypto_sharecrate::crypto::identity::fingerprint_str(pk);
    let mut s = heapless::String::new();
    let _ = s.push_str(core::str::from_utf8(&fp).unwrap_or(""));
    s
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();
    let mut p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 36 * 1024);

    let timg0 = TimerGroup::new(p.TIMG0);
    esp_rtos::start(timg0.timer0, p.FROM_CPU_INTR0);

    // CRYPTO-REVIEW: true-random 256-bit seed from the RNG with ADC noise as
    // the entropy source (works before Wi-Fi is on). ADC1 is handed back to
    // the photoresistor afterwards.
    let trng_seed = {
        let _source = TrngSource::new(p.RNG.reborrow(), p.ADC1.reborrow());
        let trng = Trng::try_new().expect("TRNG source active");
        let mut s = [0u8; 32];
        trng.read(&mut s);
        s
    };
    let mut rng = NodeRng::new(trng_seed);

    let mut flash = FlashStorage::new(p.FLASH);
    let persist = match storage::load(&mut flash) {
        Some(ps) => ps,
        None => {
            // CRYPTO-REVIEW: first boot, so make this node's long-term identity.
            log::info!("first boot: generating node identity");
            let ps = Persist {
                seed: rng.bytes(),
                pairing: None,
            };
            storage::save(&mut flash, &ps);
            ps
        }
    };
    let core = NodeCore::new(&persist.seed, persist.pairing);
    let fp = core.fingerprint();
    log::info!(
        "FFA data node, fingerprint {}",
        core::str::from_utf8(&fp).unwrap_or("?")
    );
    match core.pairing() {
        Some(pr) => log::info!(
            "paired as node {} with hub {}",
            pr.node_id,
            uplink_fp(&pr.peer_pub).as_str()
        ),
        None => log::info!("not paired yet: hold BOOT for 3 s"),
    }

    // ---- channels: edit here to add/move sensors and outputs ----
    let mut adc_cfg = AdcConfig::new();
    let photo = adc_cfg.enable_pin_with_cal::<_, AdcCalCurve<ADC1>>(p.GPIO3, Attenuation::_11dB);
    let adc = Adc::new(p.ADC1, adc_cfg);
    let mut chans = Channels::new(Some(adc));
    chans.ds18b20(0, p.GPIO4);
    chans.analog_in(1, photo);
    chans.digital_out(2, p.GPIO5); // siren
    chans.digital_out(3, p.GPIO6); // solenoid
    // more examples:
    // chans.digital_in(4, p.GPIO20, Pull::Up);   // float switch to GND

    let button = Input::new(p.GPIO9, InputConfig::default().with_pull(Pull::Up));
    spawner.spawn(button_task(button).unwrap());
    let led = Output::new(p.GPIO8, Level::High, OutputConfig::default());

    // ---- radios ----
    let mode = config::transport();
    log::info!("transport: {mode:?}");
    let wifi = if mode != TransportMode::Lora {
        radio::wifi::start(&spawner, p.WIFI, rng.u64())
    } else {
        None
    };

    #[cfg(feature = "lora")]
    let lora = if mode != TransportMode::Wifi {
        setup_lora(
            p.SPI2, p.DMA_CH0, p.GPIO10, p.GPIO7, p.GPIO21, p.GPIO2, p.GPIO0, p.GPIO1, p.GPIO20,
        )
        .await
    } else {
        None
    };
    #[cfg(not(feature = "lora"))]
    if mode != TransportMode::Wifi {
        log::warn!("FFA_TRANSPORT wants LoRa but this firmware was built without --features lora");
    }

    let mut app = App {
        core,
        chans,
        flash,
        seed: persist.seed,
        radios: Radios {
            mode,
            wifi,
            #[cfg(feature = "lora")]
            lora,
        },
        rng,
        led,
        last_contact: Instant::now(),
        failsafe_tripped: false,
        announced: false,
        last_faults: heapless::Vec::new(),
    };
    app.run().await
}

#[cfg(feature = "lora")]
#[allow(clippy::too_many_arguments)]
async fn setup_lora(
    spi2: esp_hal::peripherals::SPI2<'static>,
    dma: esp_hal::peripherals::DMA_CH0<'static>,
    sck: esp_hal::peripherals::GPIO10<'static>,
    mosi: esp_hal::peripherals::GPIO7<'static>,
    miso: esp_hal::peripherals::GPIO21<'static>,
    nss: esp_hal::peripherals::GPIO2<'static>,
    reset: esp_hal::peripherals::GPIO0<'static>,
    busy: esp_hal::peripherals::GPIO1<'static>,
    dio1: esp_hal::peripherals::GPIO20<'static>,
) -> Option<radio::lora::LoraLink> {
    use esp_hal::spi::Mode;
    use esp_hal::spi::master::{Config as SpiConfig, Spi};
    use esp_hal::time::Rate;

    let rx = esp_hal::dma_rx_buffer!(512).ok()?;
    let tx = esp_hal::dma_tx_buffer!(512).ok()?;
    let bus = Spi::new(
        spi2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(8))
            .with_mode(Mode::_0),
    )
    .ok()?
    .with_sck(sck)
    .with_mosi(mosi)
    .with_miso(miso)
    .with_dma(dma)
    .with_buffers(rx, tx)
    .into_async();
    let cs = Output::new(nss, Level::High, OutputConfig::default());
    let spi = embedded_hal_bus::spi::ExclusiveDevice::new(bus, cs, embassy_time::Delay).ok()?;
    let reset = Output::new(reset, Level::High, OutputConfig::default());
    let busy = Input::new(busy, InputConfig::default());
    let dio1 = Input::new(dio1, InputConfig::default());
    radio::lora::LoraLink::new(spi, reset, busy, dio1, false).await
}

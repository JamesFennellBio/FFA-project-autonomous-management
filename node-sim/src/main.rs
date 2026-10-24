//! Pretends to be an ESP32-C3 data node, using the same `NodeCore` as the
//! firmware. Lets you test the hub, PC daemon and app with no hardware.
//!
//! Fake demo channels (same as the real demo node):
//!   0 thermometer (DS18B20)   1 photoresistor (ADC)   2 siren (out)   3 solenoid (out)
//!
//! ```bash
//! cargo run -p node-sim -- --state sim1.json --pair     # first run: asks to pair
//! cargo run -p node-sim -- --state sim1.json            # later runs
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crypto_sharecrate::node::{NodeCore, NodeEvent, Pairing};
use crypto_sharecrate::packet::{ChannelInfo, ChannelKind, Packet, Reading};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tracing::{info, warn};
use uplink::mono_now;

#[derive(Parser)]
struct Args {
    /// Hub UDP address.
    #[arg(long, default_value = "127.0.0.1:7300")]
    hub: String,
    /// File holding this simulated node's key and pairing (like the ESP's flash).
    #[arg(long, default_value = "sim-node.json")]
    state: PathBuf,
    /// Seconds between readings.
    #[arg(long, default_value_t = 5)]
    interval: u64,
    /// Act like the pairing button is held.
    #[arg(long)]
    pair: bool,
}

#[derive(Serialize, Deserialize)]
struct SimState {
    /// CRYPTO-REVIEW: the node's Ed25519 seed (on the ESP this lives in flash).
    seed: String,
    node_id: Option<u16>,
    hub_public_key: Option<String>,
}

const CHANNELS: [ChannelInfo; 4] = [
    ChannelInfo {
        channel: 0,
        kind: ChannelKind::OneWireTemp,
    },
    ChannelInfo {
        channel: 1,
        kind: ChannelKind::AnalogIn,
    },
    ChannelInfo {
        channel: 2,
        kind: ChannelKind::DigitalOut,
    },
    ChannelInfo {
        channel: 3,
        kind: ChannelKind::DigitalOut,
    },
];

fn load_state(path: &Path) -> Result<([u8; 32], Option<Pairing>)> {
    if let Ok(s) = std::fs::read_to_string(path) {
        let st: SimState = serde_json::from_str(&s)?;
        let seed = uplink::key_from_hex(&st.seed)?;
        let pairing = match (st.node_id, st.hub_public_key) {
            (Some(id), Some(k)) => Some(Pairing {
                node_id: id,
                peer_pub: uplink::key_from_hex(&k)?,
            }),
            _ => None,
        };
        return Ok((seed, pairing));
    }
    // CRYPTO-REVIEW: new identity from the OS RNG, like the ESP's first boot.
    let mut seed = [0u8; 32];
    uplink::getrandom(&mut seed)?;
    save_state(path, &seed, None)?;
    Ok((seed, None))
}

fn save_state(path: &Path, seed: &[u8; 32], p: Option<Pairing>) -> Result<()> {
    let st = SimState {
        seed: hex::encode(seed),
        node_id: p.map(|p| p.node_id),
        hub_public_key: p.map(|p| uplink::key_to_hex(&p.peer_pub)),
    };
    uplink::write_secret(path, &serde_json::to_string_pretty(&st)?)
}

/// Fake sensors: temperature drifts 20-32 °C, light follows a slow "day".
fn sample(t: u64) -> [Reading; 2] {
    let x = t as f64;
    let temp = 26.0 + 6.0 * (x / 120.0).sin();
    let light = 1650.0 + 1500.0 * (x / 300.0).sin();
    [
        Reading {
            channel: 0,
            value: (temp * 1000.0) as i32,
        },
        Reading {
            channel: 1,
            value: light as i32,
        },
    ]
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let (seed, pairing) = load_state(&args.state)?;
    let mut core = NodeCore::new(&seed, pairing);
    info!(
        "node fingerprint {}",
        String::from_utf8_lossy(&core.fingerprint())
    );

    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    sock.connect(&args.hub).await?;

    let mut outputs = [0i32; 4];
    let mut announced = false;
    let mut tick = tokio::time::interval(Duration::from_secs(args.interval.max(1)));
    let mut buf = [0u8; 256];

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = mono_now();
                if !core.is_paired() {
                    if args.pair {
                        info!("pairing: approve fingerprint {} in the app", String::from_utf8_lossy(&core.fingerprint()));
                        sock.send(&core.pair_request()).await?;
                    } else {
                        warn!("not paired; run with --pair");
                    }
                    continue;
                }
                if core.handshake_pending(now) {
                    continue;
                }
                if core.needs_handshake(now) {
                    let mut r = [0u8; 48];
                    uplink::getrandom(&mut r)?; // CRYPTO-REVIEW: handshake randomness
                    sock.send(&core.start_handshake(&r, now)?).await?;
                    continue;
                }
                let packet = if !announced {
                    announced = true;
                    Packet::announce(&CHANNELS)
                } else {
                    let [t, l] = sample(now);
                    Packet::data(&[
                        t,
                        l,
                        Reading { channel: 2, value: outputs[2] },
                        Reading { channel: 3, value: outputs[3] },
                    ])
                };
                match core.seal(&packet, now) {
                    Ok(f) => { sock.send(&f).await?; }
                    Err(e) => warn!("seal: {e}"),
                }
            }
            r = sock.recv(&mut buf) => {
                let n = match r {
                    Ok(n) => n,
                    Err(e) => { warn!("recv: {e}"); continue; }
                };
                match core.handle_frame(&buf[..n], mono_now()) {
                    Ok(NodeEvent::Paired(p)) => {
                        save_state(&args.state, &seed, Some(p))?;
                        info!("paired as node {} with hub {}", p.node_id, uplink::fingerprint_hex(&p.peer_pub));
                    }
                    Ok(NodeEvent::SessionUp { epoch }) => {
                        info!("session #{} up (epoch {epoch:#010x})", core.sessions_started());
                        // Send something right away so the hub gets key confirmation.
                        tick.reset_immediately();
                    }
                    Ok(NodeEvent::Command { channel, value }) => {
                        if let Some(o) = outputs.get_mut(channel as usize) {
                            *o = value;
                        }
                        let what = match channel { 2 => "SIREN", 3 => "SOLENOID", _ => "output" };
                        info!(">>> {what} (ch {channel}) set to {}", if value != 0 { "ON" } else { "OFF" });
                    }
                    Ok(_) => {}
                    Err(e) => warn!("bad frame from hub: {e}"),
                }
            }
        }
    }
}

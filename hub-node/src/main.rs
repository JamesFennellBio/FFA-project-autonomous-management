//! Raspberry Pi hub.
//!
//! ```text
//!  ESP32 nodes ──LoRa (SX1262)──┐
//!                               ├─> hub task (pairing, ECDH sessions) ──> RAM SQLite ──> PC daemon (TCP, encrypted)
//!  ESP32 nodes ──Wi-Fi UDP──────┘          │                               (flushed to SD once a day)
//!                                          ├── recipes (hub runs them, PC can be off)
//!  phone app ───Wi-Fi TCP (port 7302)──────┘
//! ```

mod app_tcp;
mod config;
mod hub;
mod pair_cmd;
mod pc_link;
mod radio;
mod recipes;
mod seed;
mod store;

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::hub::{Hub, Link, Output};
use crate::radio::Inbound;
use crate::store::Store;

#[derive(Parser)]
#[command(about = "Farm/lab hub (Raspberry Pi)")]
struct Args {
    #[arg(long, default_value = "/etc/farmlab-hub/hub.toml")]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the hub (the default).
    Run,
    /// First-time setup: make the hub identity and show its 24-word recovery phrase.
    Init,
    /// Rebuild the hub identity from its 24-word phrase (read from stdin).
    Restore {
        /// Replace an existing identity.key (nodes paired to the old key stop working).
        #[arg(long)]
        force: bool,
    },
    /// Print the hub's public key and fingerprint.
    Fingerprint,
    /// Pair with the PC: opens a 10-minute pairing window and prints a code
    /// to enter in the desktop app.
    Pair {
        /// Replace the current PC pairing.
        #[arg(long)]
        replace: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();
    let cfg = config::Config::load(&args.config)?;
    std::fs::create_dir_all(&cfg.data_dir)
        .with_context(|| format!("creating {}", cfg.data_dir.display()))?;

    match args.cmd.unwrap_or(Cmd::Run) {
        Cmd::Init => {
            if cfg.data_dir.join(seed::IDENTITY_FILE).exists() {
                anyhow::bail!(
                    "this hub already has an identity; delete identity.key first if you really mean it"
                );
            }
            // CRYPTO-REVIEW: the phrase is printed once and never saved. The
            // key derived from it IS saved (identity.key) so the hub can start
            // unattended; protect that file like the phrase.
            let m = seed::generate()?;
            seed::save(&cfg.data_dir, &seed::identity_seed(&m))?;
            println!("\nWRITE THESE 24 WORDS DOWN AND KEEP THEM SAFE. They restore this hub:\n");
            for (i, w) in m.words().enumerate() {
                println!("{:>2}. {w}", i + 1);
            }
            println!(
                "\nThe words themselves are not saved. The hub's private key made from them IS saved in\n\
                 {} (readable only by this user) so the hub can start on its own.\n\
                 Anyone who can read that file can pretend to be this hub: keep the SD card safe.",
                cfg.data_dir.join(seed::IDENTITY_FILE).display()
            );
            print_fingerprint(&cfg)?;
            Ok(())
        }
        Cmd::Restore { force } => {
            if cfg.data_dir.join(seed::IDENTITY_FILE).exists() && !force {
                anyhow::bail!(
                    "this hub already has an identity. Restoring would replace it, and every node \
                     paired to the current key would stop working. Re-run with --force if you mean it."
                );
            }
            println!("Type the 24 words, separated by spaces, then press Enter:");
            let mut line = String::new();
            std::io::stdin().lock().read_line(&mut line)?;
            let m = seed::parse(&line)?;
            seed::save(&cfg.data_dir, &seed::identity_seed(&m))?;
            println!("Hub identity restored.");
            print_fingerprint(&cfg)
        }
        Cmd::Fingerprint => print_fingerprint(&cfg),
        Cmd::Pair { replace } => {
            pair_cmd::run(
                &cfg.data_dir,
                &cfg.data_dir.join("pc_pairing.json"),
                &cfg.pair_listen,
                cfg.pc_key()?,
                replace,
            )
            .await
        }
        Cmd::Run => run(cfg).await,
    }
}

fn print_fingerprint(cfg: &config::Config) -> Result<()> {
    let seed = seed::load(&cfg.data_dir)?;
    let pk = crypto_sharecrate::crypto::identity::Identity::from_seed(&seed).public_key();
    let pq = crypto_sharecrate::crypto::v2::identity::HybridIdentity::from_seed(&seed);
    println!(
        "Hub fingerprint (phones, PC): {}",
        uplink::v2::fingerprint(pq.public_key())
    );
    println!("Hub key ID (phones, PC):      {}", hex::encode(pq.key_id()));
    println!(
        "Hub fingerprint (nodes):      {}",
        uplink::fingerprint_hex(&pk)
    );
    println!("Hub public key (nodes):       {}", uplink::key_to_hex(&pk));
    println!(
        "(build node firmware with FARMLAB_HUB_KEY=<public key (nodes)> so nodes only pair with this hub)"
    );
    Ok(())
}

async fn run(cfg: config::Config) -> Result<()> {
    let seed = seed::load(&cfg.data_dir)?;
    let pc_pin = cfg.pc_key()?;
    let mut hub = Hub::new(&seed, Some(&cfg.data_dir))?;
    info!(
        "hub fingerprint {} (nodes), key ID {} (phones, PC)",
        uplink::fingerprint_hex(&hub.public_key()),
        hex::encode(hub.pq_key_id())
    );

    let store = Arc::new(Mutex::new(Store::open(&cfg.data_dir.join("hub.db"))?));
    hub.restore_live(store.lock().unwrap().latest()?);

    let (in_tx, mut in_rx) = mpsc::channel::<Inbound>(256);
    let (pc_tx, mut pc_rx) = mpsc::unbounded_channel();
    // Live-only messages to the PC (dropped if the PC is away or this fills up).
    let (live_tx, live_rx) = mpsc::channel(64);

    let udp = Arc::new(UdpSocket::bind(&cfg.udp_listen).await?);
    info!("listening for Wi-Fi nodes on udp://{}", cfg.udp_listen);
    tokio::spawn(radio::udp::recv_loop(
        udp.clone(),
        in_tx.clone(),
        radio::MAX_RADIO_FRAME,
        Link::Udp,
    ));
    let phones = app_tcp::Phones::default();
    tokio::spawn(app_tcp::serve(
        app_tcp::bind(&cfg.app_listen).await?,
        in_tx.clone(),
        phones.clone(),
    ));

    #[cfg(feature = "lora")]
    let lora_tx = if cfg.lora.enabled {
        Some(radio::lora::spawn(cfg.lora.clone(), in_tx.clone())?)
    } else {
        None
    };
    #[cfg(not(feature = "lora"))]
    if cfg.lora.enabled {
        warn!(
            "lora.enabled = true but this build has no LoRa support; rebuild with --features lora"
        );
    }

    tokio::spawn(pc_link::run(
        seed,
        cfg.pc_addr.clone(),
        pc_pin,
        cfg.data_dir.join("pc_pairing.json"),
        store.clone(),
        pc_tx,
        live_rx,
    ));

    let mut flush = tokio::time::interval(Duration::from_secs(cfg.flush_interval_secs.max(60)));
    flush.tick().await; // the first tick fires right away; skip it

    loop {
        let out: Output = tokio::select! {
            Some(inb) = in_rx.recv() => match inb.link {
                Link::App(conn) => {
                    let st = store.lock().unwrap();
                    hub.on_app_frame(&inb.frame, conn, uplink::mono_now(), uplink::unix_now(), &st)
                }
                link => hub.on_frame(&inb.frame, link, uplink::mono_now(), uplink::unix_now()),
            },
            Some(msg) = pc_rx.recv() => hub.on_pc(msg, uplink::mono_now(), uplink::unix_now()),
            _ = flush.tick() => {
                if let Err(e) = store.lock().unwrap().flush() {
                    error!("daily flush failed: {e:#}");
                }
                continue;
            }
            _ = tokio::signal::ctrl_c() => {
                info!("shutting down; flushing DB");
                store.lock().unwrap().flush()?;
                return Ok(());
            }
        };
        for (link, frame) in out.frames {
            match link {
                Link::Udp(addr) => {
                    if let Err(e) = udp.send_to(&frame, addr).await {
                        warn!("udp send to {addr}: {e}");
                    }
                }
                Link::App(conn) => phones.send(conn, frame),
                Link::Lora =>
                {
                    #[cfg(feature = "lora")]
                    if let Some(tx) = &lora_tx {
                        let _ = tx.try_send(frame);
                    }
                }
            }
        }
        for m in out.live {
            let _ = live_tx.try_send(m);
        }
        if !out.to_pc.is_empty() {
            let mut st = store.lock().unwrap();
            for m in &out.to_pc {
                if let Err(e) = st.push(m) {
                    error!("storing message: {e:#}");
                }
            }
        }
    }
}

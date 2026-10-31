//! PC daemon: the "brain" of the system.
//!
//! - Listens for the Pi hub on TCP (default 0.0.0.0:7400). That link is encrypted
//!   with the same ECDH + ChaCha20-Poly1305 protocol the nodes use.
//! - Stores everything in SQLite (full history).
//! - Passes the desktop app's recipe edits to the hub, which runs them.
//! - Serves the REST API for the desktop app on 127.0.0.1:7401 only.

mod api;
mod auth;
mod db;
mod hub_link;
mod state;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use tracing::info;

#[derive(Parser)]
#[command(about = "Farm/lab management daemon")]
struct Args {
    /// Where the identity key and API token live (default: OS config dir).
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Where the SQLite database lives (default: OS data dir).
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// REST API address. Keep this on localhost.
    #[arg(long, default_value = "127.0.0.1:7401")]
    api: SocketAddr,
    /// Address the Pi hub connects to.
    #[arg(long, default_value = "0.0.0.0:7400")]
    hub_listen: SocketAddr,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let args = Args::parse();

    let dirs = directories::ProjectDirs::from("org", "farmlab", "farmlab-daemon");
    let config_dir = args
        .config_dir
        .or_else(|| dirs.as_ref().map(|d| d.config_dir().to_path_buf()))
        .context("no config dir; pass --config-dir")?;
    let data_dir = args
        .data_dir
        .or_else(|| dirs.as_ref().map(|d| d.data_dir().to_path_buf()))
        .context("no data dir; pass --data-dir")?;
    std::fs::create_dir_all(&config_dir)?;
    std::fs::create_dir_all(&data_dir)?;

    // CRYPTO-REVIEW: the PC's long-term identity seed. It derives the hybrid
    // Ed25519 + ML-DSA-65 key the hub pins at pairing.
    let seed = uplink::load_or_create_secret(&config_dir.join("identity.key"))?;
    let token = auth::load_or_create_token(&config_dir.join("api-token"))?;
    let conn = db::open(&data_dir.join("farmlab.db"))?;

    let state = Arc::new(state::AppState::new(conn, &seed, token));
    state
        .hub_listen_port
        .store(args.hub_listen.port(), std::sync::atomic::Ordering::Relaxed);
    info!(
        "PC identity fingerprint {}",
        uplink::v2::fingerprint(state.identity.public_key())
    );
    info!(
        "PC key ID (put this in hub.toml as pc_key_id): {}",
        hex::encode(state.identity.key_id())
    );
    info!("API token file: {}", config_dir.join("api-token").display());

    tokio::spawn(hub_link::serve(state.clone(), args.hub_listen));

    let listener = tokio::net::TcpListener::bind(args.api).await?;
    info!("REST API on http://{}", args.api);
    axum::serve(listener, api::router(state)).await?;
    Ok(())
}

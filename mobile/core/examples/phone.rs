//! Command-line stand-in for the phone app (same Rust core). Used by the
//! end-to-end test, and handy for poking at the hub from a laptop.
//!
//! ```text
//! cargo run -p farmlab-mobile-core --example phone -- --hub 127.0.0.1:7302 pair
//! cargo run -p farmlab-mobile-core --example phone -- --hub 127.0.0.1:7302 ask '{"type":"channels"}'
//! cargo run -p farmlab-mobile-core --example phone -- pair-qr 'farmlab://pair?v=2&a=...'
//! ```

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use farmlab_mobile_core::{HubClient, PhonePairing, fingerprint_of_seed, new_identity_seed};
use serde::{Deserialize, Serialize};

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:7302")]
    hub: String,
    /// Where this fake phone keeps its identity and pairing (like the app's
    /// secure storage). Contains a private key: keep it to test machines.
    #[arg(long, default_value = "phone-state.json")]
    state: PathBuf,
    /// Only pair with this hub (its key ID from `hub-node fingerprint`, or full key, hex).
    #[arg(long)]
    hub_key: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Ask the hub to pair; approve the phone in the desktop app.
    Pair {
        #[arg(long, default_value_t = 120)]
        timeout: u32,
    },
    /// Pair with the QR code text from the desktop app (Pairing > Add phone);
    /// no approval needed. `--hub` and `--hub-key` come from the code.
    PairQr {
        uri: String,
        #[arg(long, default_value_t = 30)]
        timeout: u32,
    },
    /// Send one JSON request and print the reply.
    Ask { json: String },
}

#[derive(Serialize, Deserialize)]
struct State {
    seed: String,
    phone_id: Option<u16>,
    hub_public_key: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let mut st: State = match std::fs::read_to_string(&args.state) {
        Ok(s) => serde_json::from_str(&s)?,
        Err(_) => State {
            seed: new_identity_seed()?,
            phone_id: None,
            hub_public_key: None,
        },
    };
    let pairing = match (st.phone_id, &st.hub_public_key) {
        (Some(phone_id), Some(k)) => Some(PhonePairing {
            phone_id,
            hub_public_key: k.clone(),
        }),
        _ => None,
    };
    if let Cmd::PairQr { uri, timeout } = &args.cmd {
        let qr = farmlab_mobile_core::parse_pairing_qr(uri.clone())?;
        println!("Hub {} at {}", qr.hub_fingerprint, qr.hub_addr);
        let client = HubClient::new(st.seed.clone(), qr.hub_addr, None, Some(qr.hub_key_id))?;
        let p = client
            .pair_with_qr(uri.clone(), *timeout)
            .await
            .context("pairing")?;
        println!("Paired as phone {} (no approval needed)", p.phone_id);
        st.phone_id = Some(p.phone_id);
        st.hub_public_key = Some(p.hub_public_key);
        uplink::write_secret(&args.state, &serde_json::to_string(&st)?)?;
        return Ok(());
    }
    let client = HubClient::new(st.seed.clone(), args.hub, pairing, args.hub_key)?;
    match args.cmd {
        Cmd::Pair { timeout } => {
            uplink::write_secret(&args.state, &serde_json::to_string(&st)?)?;
            println!(
                "Phone fingerprint: {}",
                fingerprint_of_seed(st.seed.clone())?
            );
            println!("Waiting for approval in the desktop app...");
            let p = client.pair(timeout).await.context("pairing")?;
            println!(
                "Paired as phone {} with hub {}",
                p.phone_id,
                farmlab_mobile_core::fingerprint_of_key(p.hub_public_key.clone())?
            );
            st.phone_id = Some(p.phone_id);
            st.hub_public_key = Some(p.hub_public_key);
            uplink::write_secret(&args.state, &serde_json::to_string(&st)?)?;
        }
        Cmd::Ask { json } => println!("{}", client.request(json).await?),
        Cmd::PairQr { .. } => unreachable!("handled above"),
    }
    Ok(())
}

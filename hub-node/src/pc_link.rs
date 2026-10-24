//! Encrypted TCP uplink from the hub to the PC daemon.
//!
//! Toward the PC the hub plays the "node" role: it pairs (the PC user approves
//! the hub's fingerprint), then runs the same signed-ECDH handshake every 10
//! frames / 1 hour, using `NodeCore` from crypto-sharecrate.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use crypto_sharecrate::frame::FrameType;
use crypto_sharecrate::node::{NodeCore, NodeEvent, Pairing};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{info, warn};
use uplink::{
    HubBatch, HubMsg, MAX_BATCH, PcBatch, PcMsg, mono_now, read_frame, unix_now, write_frame,
};

use crate::store::Store;

const HEARTBEAT_SECS: u64 = 30;

#[derive(Serialize, Deserialize)]
struct PcPairingFile {
    hub_id: u16,
    /// CRYPTO-REVIEW: pinned PC daemon Ed25519 key.
    pc_public_key: String,
}

fn load_pairing(path: &PathBuf) -> Option<Pairing> {
    let s = std::fs::read_to_string(path).ok()?;
    let f: PcPairingFile = serde_json::from_str(&s).ok()?;
    Some(Pairing {
        node_id: f.hub_id,
        peer_pub: uplink::key_from_hex(&f.pc_public_key).ok()?,
    })
}

fn save_pairing(path: &PathBuf, p: &Pairing) -> Result<()> {
    let f = PcPairingFile {
        hub_id: p.node_id,
        pc_public_key: uplink::key_to_hex(&p.peer_pub),
    };
    std::fs::write(path, serde_json::to_vec_pretty(&f)?)?;
    Ok(())
}

/// Keeps reconnecting to the PC for as long as the hub runs.
///
/// CRYPTO-REVIEW: `pc_key` is the PC daemon's public key from hub.toml. The
/// hub only pairs with (and only trusts) that key, so losing pc_pairing.json
/// can't make it trust whoever answers first on pc_addr (finding 12).
pub async fn run(
    seed: [u8; 32],
    pc_addr: String,
    pc_key: [u8; 32],
    pairing_path: PathBuf,
    store: Arc<Mutex<Store>>,
    to_hub: mpsc::UnboundedSender<PcMsg>,
    mut live: mpsc::Receiver<HubMsg>,
) {
    loop {
        let core = NodeCore::new(&seed, load_pairing(&pairing_path)).with_expected_peer(pc_key);
        match TcpStream::connect(&pc_addr).await {
            Ok(stream) => {
                info!("connected to PC daemon at {pc_addr}");
                if let Err(e) =
                    session(core, stream, &pairing_path, &store, &to_hub, &mut live).await
                {
                    warn!("PC link dropped: {e:#}");
                }
            }
            Err(e) => warn!("can't reach PC daemon at {pc_addr}: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/// CRYPTO-REVIEW: randomness for a handshake, from the OS RNG.
fn hs_random() -> Result<[u8; 48]> {
    let mut r = [0u8; 48];
    uplink::getrandom(&mut r)?;
    Ok(r)
}

async fn session(
    mut core: NodeCore,
    stream: TcpStream,
    pairing_path: &PathBuf,
    store: &Arc<Mutex<Store>>,
    to_hub: &mpsc::UnboundedSender<PcMsg>,
    live: &mut mpsc::Receiver<HubMsg>,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let (mut rd, mut wr) = stream.into_split();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // Highest row id sent on this connection (not acked yet). On reconnect we
    // start from 0 and resend anything still unacked; the PC skips duplicates.
    let mut sent_upto: i64 = 0;
    // None = send one as soon as the session is up, so the PC sees the hub at once.
    let mut last_heartbeat: Option<u64> = None;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                let now = mono_now();
                if !core.is_paired() {
                    info!(
                        "not paired with the PC yet: approve hub {} in the app",
                        String::from_utf8_lossy(&core.fingerprint())
                    );
                    write_frame(&mut wr, &core.pair_request()).await?;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                if core.handshake_pending(now) {
                    continue;
                }
                if core.needs_handshake(now) {
                    let hello = core.start_handshake(&hs_random()?, now).map_err(|e| anyhow::anyhow!("{e}"))?;
                    write_frame(&mut wr, &hello).await?;
                    continue;
                }
                let mut msgs = store.lock().unwrap().unsent(sent_upto, MAX_BATCH)?;
                if last_heartbeat.is_none_or(|t| now.saturating_sub(t) >= HEARTBEAT_SECS) {
                    last_heartbeat = Some(now);
                    let backlog = store.lock().unwrap().backlog()?;
                    // Heartbeats aren't stored, so they get id 0 (the PC never dedupes 0).
                    msgs.push((0, HubMsg::Heartbeat { ts: unix_now(), backlog }));
                }
                // Live-only messages (pairing requests): id 0, never stored.
                while msgs.len() < MAX_BATCH + 10 {
                    match live.try_recv() {
                        Ok(m) => msgs.push((0, m)),
                        Err(_) => break,
                    }
                }
                if msgs.is_empty() {
                    continue;
                }
                let max_id = msgs.iter().map(|(id, _)| *id).max().unwrap_or(0);
                let session = core.session_mut().expect("session exists when no handshake needed");
                let frame = uplink::seal_json(session, &HubBatch { msgs }, now)?;
                write_frame(&mut wr, &frame).await?;
                sent_upto = sent_upto.max(max_id);
            }
            frame = read_frame(&mut rd) => {
                let frame = frame?;
                let now = mono_now();
                match FrameType::from_byte(frame[0]).map_err(|e| anyhow::anyhow!("{e}"))? {
                    FrameType::PairAccept | FrameType::HelloAck => {
                        let event = core.handle_frame(&frame, now).map_err(|e| match e {
                            crypto_sharecrate::Error::WrongPeer => anyhow::anyhow!(
                                "the PC at pc_addr answered with a key that is not pc_public_key in hub.toml; refusing to pair"
                            ),
                            e => anyhow::anyhow!("{e}"),
                        })?;
                        match event {
                            NodeEvent::Paired(p) => {
                                save_pairing(pairing_path, &p)?;
                                info!(
                                    "paired with PC {} as hub {}",
                                    uplink::fingerprint_hex(&p.peer_pub),
                                    p.node_id
                                );
                            }
                            NodeEvent::SessionUp { epoch } => {
                                tracing::debug!("PC session up, epoch {epoch:#x} (#{})", core.sessions_started());
                            }
                            _ => {}
                        }
                    }
                    FrameType::Uplink => {
                        let session = core.session_mut().ok_or_else(|| anyhow::anyhow!("uplink before session"))?;
                        let batch: PcBatch = uplink::open_json(&frame, |f, out| {
                            session.open(f, out, now).map(|(_, pt)| pt.len())
                        })?;
                        for m in batch.msgs {
                            match m {
                                PcMsg::Ack { upto } => store.lock().unwrap().mark_forwarded(upto)?,
                                other => {
                                    let _ = to_hub.send(other);
                                }
                            }
                        }
                    }
                    other => bail!("unexpected {other:?} from PC"),
                }
            }
        }
    }
}

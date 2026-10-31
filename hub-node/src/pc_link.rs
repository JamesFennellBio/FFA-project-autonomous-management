//! Encrypted TCP uplink from the hub to the PC daemon.
//!
//! Toward the PC the hub plays the initiator: once paired with the one-time
//! code (`hub-node pair`, see uplink::pake), it runs the v2 hybrid handshake (Ed25519 + ML-DSA-65
//! signatures, X25519 + ML-KEM-768 keys) every 1000 frames / 1 hour, using
//! `LinkCore` from crypto-sharecrate.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use crypto_sharecrate::crypto::v2::identity::{HybridPublic, KeyId};
use crypto_sharecrate::crypto::v2::link::{LinkCore, LinkEvent, LinkPairing};
use crypto_sharecrate::frame::FrameType;
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
    /// CRYPTO-REVIEW: the pinned PC daemon hybrid key (Ed25519 + ML-DSA-65, hex).
    pc_public_key: String,
    /// Where the PC daemon listens for the hub, learned while pairing.
    #[serde(default)]
    pc_addr: Option<String>,
}

/// The saved PC pairing, if any. A v1 file (32-byte key) doesn't load, so the
/// hub has to be paired again with `hub-node pair`.
fn load_pairing(path: &PathBuf) -> Option<(LinkPairing, Option<String>)> {
    let s = std::fs::read_to_string(path).ok()?;
    let f: PcPairingFile = serde_json::from_str(&s).ok()?;
    match uplink::v2::public_from_hex(&f.pc_public_key) {
        Ok(k) => Some((
            LinkPairing {
                node_id: f.hub_id,
                peer_pub: *k,
            },
            f.pc_addr,
        )),
        Err(_) => {
            warn!("pc_pairing.json is from before v2; pair with the PC again (`hub-node pair`)");
            None
        }
    }
}

/// Saves a PC pairing made with the pairing code (`hub-node pair`).
pub fn save_pairing(
    path: &PathBuf,
    hub_id: u16,
    pc_pub: &HybridPublic,
    pc_addr: &str,
) -> Result<()> {
    let f = PcPairingFile {
        hub_id,
        pc_public_key: uplink::v2::public_to_hex(pc_pub),
        pc_addr: Some(pc_addr.to_string()),
    };
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// Keeps reconnecting to the PC for as long as the hub runs. Until the hub is
/// paired (`hub-node pair` writes pc_pairing.json) it just waits, checking
/// again every few seconds.
///
/// CRYPTO-REVIEW: the PC's key comes from pairing with the one-time code, and
/// the uplink only trusts that key. `pc_pin` (pc_key_id in hub.toml) is an
/// optional extra pin; a pairing for a different key is then ignored
/// (finding 12).
pub async fn run(
    seed: [u8; 32],
    cfg_pc_addr: Option<String>,
    pc_pin: Option<KeyId>,
    pairing_path: PathBuf,
    store: Arc<Mutex<Store>>,
    to_hub: mpsc::UnboundedSender<PcMsg>,
    mut live: mpsc::Receiver<HubMsg>,
) {
    let mut told = false;
    loop {
        let saved = load_pairing(&pairing_path).filter(|(p, _)| {
            pc_pin.is_none_or(|k| k == crypto_sharecrate::crypto::v2::identity::key_id(&p.peer_pub))
        });
        let Some((pairing, saved_addr)) = saved else {
            if !told {
                info!(
                    "not paired with a PC yet: run `hub-node pair` and enter the code in the desktop app"
                );
                told = true;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };
        told = false;
        // hub.toml's pc_addr, if set, wins (the PC moved).
        let Some(pc_addr) = cfg_pc_addr.clone().or(saved_addr) else {
            warn!("paired, but no PC address known: set pc_addr in hub.toml");
            tokio::time::sleep(Duration::from_secs(30)).await;
            continue;
        };
        let core = LinkCore::new(&seed, Some(pairing));
        match TcpStream::connect(&pc_addr).await {
            Ok(stream) => {
                info!("connected to PC daemon at {pc_addr}");
                if let Err(e) = session(core, stream, &store, &to_hub, &mut live).await {
                    warn!("PC link dropped: {e:#}");
                }
            }
            Err(e) => warn!("can't reach PC daemon at {pc_addr}: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn session(
    mut core: LinkCore,
    stream: TcpStream,
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
                if core.handshake_pending(now) {
                    continue;
                }
                if core.needs_handshake(now) {
                    let hello = core.start_handshake(&uplink::v2::hs_random()?, now).map_err(|e| anyhow::anyhow!("{e}"))?;
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
                while msgs.len() < uplink::MAX_BATCH_MSGS {
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
                    FrameType::HelloAck2 => {
                        let event = core.handle_frame(&frame, now).map_err(|e| anyhow::anyhow!("{e}"))?;
                        if let LinkEvent::SessionUp { epoch } = event {
                            tracing::debug!("PC session up, epoch {epoch:#x} (#{})", core.sessions_started());
                        }
                    }
                    FrameType::Uplink => {
                        let session = core.session_mut().ok_or_else(|| anyhow::anyhow!("uplink before session"))?;
                        let batch: PcBatch = uplink::open_json(&frame, |f, out| {
                            session.open(f, out, now).map(|(_, pt)| pt.len())
                        })?;
                        if batch.msgs.len() > uplink::MAX_BATCH_MSGS {
                            bail!("PC batch of {} messages is over the cap", batch.msgs.len());
                        }
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

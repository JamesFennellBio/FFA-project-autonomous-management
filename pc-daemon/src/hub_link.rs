//! TCP link to the Pi hub. The PC is the *responder*: the hub pairs with it,
//! then starts an ECDH handshake every 10 frames / 1 hour, like a node does
//! with the hub.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, bail};
use crypto_sharecrate::crypto::{handshake, pairing, responder::PeerSessions};
use crypto_sharecrate::frame::FrameType;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tracing::{info, warn};
use uplink::{HubBatch, HubMsg, PcBatch, PcMsg, mono_now, read_frame, unix_now, write_frame};

use crate::db;
use crate::state::{AppState, PairKind, PendingPair};

/// Max simultaneous TCP connections on the hub port (there's one real hub).
const MAX_CONNECTIONS: usize = 4;
/// A new connection must send its first frame this quickly...
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// ...and an established one must send something this often (the hub sends a
/// heartbeat every 30 s).
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

pub async fn serve(state: Arc<AppState>, addr: SocketAddr) {
    let listener = match TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("can't listen for hub on {addr}: {e}");
            return;
        }
    };
    info!("waiting for hub on {addr}");
    // Finding 04: cap connections so idle sockets can't use up the daemon.
    let slots = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    warn!("too many connections on the hub port; dropping {peer}");
                    continue;
                };
                let st = state.clone();
                let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
                tokio::spawn(async move {
                    let _permit = permit;
                    info!("hub connection from {peer}");
                    if let Err(e) = handle(st.clone(), stream, conn_id).await {
                        warn!("hub connection {peer} closed: {e:#}");
                    }
                    // Finding 14: only the connection that registered as the
                    // hub may mark the hub offline.
                    let mut hub = st.hub.lock().unwrap();
                    if hub.conn_id == Some(conn_id) {
                        hub.connected = false;
                        hub.tx = None;
                        hub.conn_id = None;
                    }
                });
            }
            Err(e) => warn!("accept: {e}"),
        }
    }
}

/// CRYPTO-REVIEW: 48 random bytes for one handshake (ephemeral key + nonce)
/// plus 4 for the epoch id, from the OS RNG.
fn hs_random() -> Result<([u8; 48], u32)> {
    let mut r = [0u8; 52];
    uplink::getrandom(&mut r)?;
    let mut hs = [0u8; 48];
    hs.copy_from_slice(&r[..48]);
    Ok((hs, u32::from_le_bytes([r[48], r[49], r[50], r[51]])))
}

async fn handle(state: Arc<AppState>, stream: TcpStream, conn_id: u64) -> Result<()> {
    stream.set_nodelay(true)?;
    let (mut rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<PcMsg>();
    let mut peer = PeerSessions::new();
    let mut queue: Vec<PcMsg> = Vec::new();
    let mut registered = false;
    // Set once the peer sends anything valid (a signed pairing request or a
    // HELLO from the pinned hub). Until then the short timeout applies.
    let mut proven = false;

    loop {
        tokio::select! {
            frame = tokio::time::timeout(
                if proven { IDLE_TIMEOUT } else { FIRST_FRAME_TIMEOUT },
                read_frame(&mut rd),
            ) => {
                let frame = frame.map_err(|_| anyhow::anyhow!("timed out waiting for a frame"))??;
                match FrameType::from_byte(frame[0]).map_err(|e| anyhow::anyhow!("{e}"))? {
                    FrameType::PairRequest => {
                        let reply = on_pair_request(&state, &frame)?;
                        proven = true;
                        if let Some(reply) = reply {
                            write_frame(&mut wr, &reply).await?;
                        }
                    }
                    FrameType::Hello => {
                        let paired = db::paired_hub(&state.db.lock().unwrap())?;
                        let Some((hub_id, hub_pub)) = paired else {
                            bail!("HELLO from an unpaired hub");
                        };
                        if handshake::hello_node_id(&frame).ok() != Some(hub_id as u16) {
                            bail!("HELLO with wrong hub id");
                        }
                        let (rand, epoch) = hs_random()?;
                        // CRYPTO-REVIEW: verifies the hub's signature with the pinned key.
                        let ack = peer
                            .on_hello(&frame, &hub_pub, &state.identity, &rand, epoch, mono_now())
                            .map_err(|e| anyhow::anyhow!("hello: {e}"))?;
                        proven = true;
                        write_frame(&mut wr, &ack).await?;
                        state.hub.lock().unwrap().sessions += 1;
                    }
                    FrameType::Uplink => {
                        let now = mono_now();
                        let batch: HubBatch = uplink::open_json(&frame, |f, out| {
                            peer.open(f, out, now).map(|(_, pt)| pt.len())
                        })?;
                        if !registered {
                            // First frame that verified under a session: this is the real hub.
                            let mut hub = state.hub.lock().unwrap();
                            hub.connected = true;
                            hub.tx = Some(tx.clone());
                            hub.conn_id = Some(conn_id);
                            registered = true;
                            info!("hub session established");
                        }
                        let (upto, cmds) = store_batch(&state, &batch)?;
                        queue.retain(|m| !matches!(m, PcMsg::Ack { .. }));
                        if let Some(upto) = upto {
                            queue.insert(0, PcMsg::Ack { upto });
                        }
                        queue.extend(cmds);
                        flush(&mut peer, &mut queue, &mut wr).await?;
                    }
                    other => bail!("unexpected frame {other:?} from hub"),
                }
            }
            Some(msg) = rx.recv() => {
                queue.push(msg);
                flush(&mut peer, &mut queue, &mut wr).await?;
            }
        }
    }
}

/// Sends everything queued, if the current session still has messages left.
/// Otherwise it waits: the hub rekeys before its next send (it counts our
/// frames too), and we flush after that.
async fn flush(
    peer: &mut PeerSessions,
    queue: &mut Vec<PcMsg>,
    wr: &mut tokio::net::tcp::OwnedWriteHalf,
) -> Result<()> {
    if queue.is_empty() {
        return Ok(());
    }
    let now = mono_now();
    let Some(s) = peer.current_mut() else {
        return Ok(());
    };
    if !s.can_send(now) {
        return Ok(());
    }
    let batch = PcBatch {
        msgs: std::mem::take(queue),
    };
    let frame = uplink::seal_json(s, &batch, now)?;
    write_frame(wr, &frame).await
}

/// CRYPTO-REVIEW: hub pairing. A hub is only pinned after a person approved
/// its fingerprint in the app.
fn on_pair_request(state: &AppState, frame: &[u8]) -> Result<Option<Vec<u8>>> {
    let hub_pub =
        pairing::parse_pair_request(frame).map_err(|e| anyhow::anyhow!("pair request: {e}"))?;
    let conn = state.db.lock().unwrap();
    if let Some((id, pinned)) = db::paired_hub(&conn)?
        && pinned == hub_pub
    {
        // Already paired; our earlier accept must have been lost. Send it again.
        return Ok(Some(
            pairing::pair_accept(&state.identity, &hub_pub, id as u16).to_vec(),
        ));
    }
    let approved = *state.approved_hub.lock().unwrap();
    if approved == Some(hub_pub) {
        let id = db::pair_hub(&conn, &hub_pub, unix_now())?;
        *state.approved_hub.lock().unwrap() = None;
        info!("hub {} paired", uplink::fingerprint_hex(&hub_pub));
        return Ok(Some(
            pairing::pair_accept(&state.identity, &hub_pub, id as u16).to_vec(),
        ));
    }
    state.add_pending(PendingPair {
        kind: PairKind::Hub,
        public_key: uplink::key_to_hex(&hub_pub),
        fingerprint: uplink::fingerprint_hex(&hub_pub),
        transport: None,
        seen_at: unix_now(),
    });
    Ok(None)
}

/// Stores a batch and returns (highest hub row id stored, rule commands to send).
fn store_batch(state: &AppState, batch: &HubBatch) -> Result<(Option<i64>, Vec<PcMsg>)> {
    let mut conn = state.db.lock().unwrap();
    let last_seq = db::hub_last_seq(&conn)?;
    let tx = conn.transaction()?;
    let mut upto = None;
    let mut cmds = Vec::new();
    for (seq, msg) in &batch.msgs {
        upto = Some(upto.map_or(*seq, |u: i64| u.max(*seq)));
        match msg {
            HubMsg::PairRequest {
                public_key,
                fingerprint,
                transport,
                ts,
            } => state.add_pending(PendingPair {
                kind: PairKind::Node,
                public_key: public_key.clone(),
                fingerprint: fingerprint.clone(),
                transport: Some(*transport),
                seen_at: *ts,
            }),
            HubMsg::Heartbeat { backlog, ts } => {
                let mut hub = state.hub.lock().unwrap();
                hub.backlog = *backlog;
                hub.last_seen = Some(*ts);
            }
            _ => {}
        }
        if *seq <= last_seq {
            continue; // already stored before a reconnect
        }
        cmds.extend(db::store_hub_msg(&tx, *seq, msg)?);
    }
    tx.execute("UPDATE hubs SET last_seen = ?1", [unix_now()])?;
    tx.commit()?;
    state.hub.lock().unwrap().last_seen = Some(unix_now());
    Ok((upto, cmds))
}

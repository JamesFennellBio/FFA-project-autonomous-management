//! TCP link to the Pi hub. The PC is the *responder*: the hub pairs with it,
//! then starts an ECDH handshake every 10 frames / 1 hour, like a node does
//! with the hub.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Result, bail};
use crypto_sharecrate::crypto::{handshake, pairing, responder::PeerSessions};
use crypto_sharecrate::frame::FrameType;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio::task::AbortHandle;
use tokio::time::Instant;
use tracing::{info, warn};
use uplink::{
    DeviceKind, HubBatch, HubMsg, MAX_BATCH_MSGS, PcBatch, PcMsg, mono_now, read_frame, unix_now,
    write_frame,
};

use crate::db;
use crate::state::{AppState, PairKind, PendingPair};

/// Max simultaneous TCP connections on the hub port (there's one real hub).
const MAX_CONNECTIONS: usize = 4;
/// A connection that hasn't proven it's the paired (or approved) hub must send
/// each frame within this time...
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(10);
/// ...and is closed after this long no matter what it sends. An unpaired real
/// hub just reconnects (every 5 s) and keeps asking.
const UNPROVEN_LIFETIME: Duration = Duration::from_secs(30);
/// A proven hub must send something this often (it sends a heartbeat every 30 s).
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

static NEXT_CONN_ID: AtomicU64 = AtomicU64::new(1);

struct Slot {
    proven: bool,
    opened: Instant,
    abort: Option<AbortHandle>,
}

/// Open connections on the hub port. When it's full, the oldest connection
/// that hasn't proven itself is closed to make room, so junk connections can't
/// lock the real hub out (FYEO scan 2). Proven connections are never evicted.
#[derive(Clone, Default)]
struct Slots(Arc<Mutex<HashMap<u64, Slot>>>);

impl Slots {
    /// Reserves a slot for a new connection, evicting if needed.
    fn admit(&self, conn_id: u64) -> bool {
        let mut map = self.0.lock().unwrap();
        if map.len() >= MAX_CONNECTIONS {
            let victim = map
                .iter()
                .filter(|(_, s)| !s.proven)
                .min_by_key(|(_, s)| s.opened)
                .map(|(id, _)| *id);
            match victim {
                Some(id) => {
                    if let Some(slot) = map.remove(&id)
                        && let Some(h) = slot.abort
                    {
                        h.abort();
                    }
                }
                None => return false,
            }
        }
        map.insert(
            conn_id,
            Slot {
                proven: false,
                opened: Instant::now(),
                abort: None,
            },
        );
        true
    }

    fn set_abort(&self, conn_id: u64, h: AbortHandle) {
        if let Some(s) = self.0.lock().unwrap().get_mut(&conn_id) {
            s.abort = Some(h);
        }
    }

    fn mark_proven(&self, conn_id: u64) {
        if let Some(s) = self.0.lock().unwrap().get_mut(&conn_id) {
            s.proven = true;
        }
    }

    fn release(&self, conn_id: u64) {
        self.0.lock().unwrap().remove(&conn_id);
    }
}

pub async fn serve(state: Arc<AppState>, addr: SocketAddr) {
    match TcpListener::bind(addr).await {
        Ok(l) => {
            info!("waiting for hub on {addr}");
            serve_on(state, l).await
        }
        Err(e) => tracing::error!("can't listen for hub on {addr}: {e}"),
    }
}

async fn serve_on(state: Arc<AppState>, listener: TcpListener) {
    let slots = Slots::default();
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let conn_id = NEXT_CONN_ID.fetch_add(1, Ordering::Relaxed);
                if !slots.admit(conn_id) {
                    warn!("hub port full of verified connections; dropping {peer}");
                    continue;
                }
                let st = state.clone();
                let sl = slots.clone();
                let task = tokio::spawn(async move {
                    info!("hub connection from {peer}");
                    if let Err(e) = handle(st.clone(), stream, conn_id, &sl).await {
                        warn!("hub connection {peer} closed: {e:#}");
                    }
                    sl.release(conn_id);
                    // Finding 14: only the connection that registered as the
                    // hub may mark the hub offline.
                    let mut hub = st.hub.lock().unwrap();
                    if hub.conn_id == Some(conn_id) {
                        hub.connected = false;
                        hub.tx = None;
                        hub.conn_id = None;
                    }
                });
                slots.set_abort(conn_id, task.abort_handle());
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

async fn handle(
    state: Arc<AppState>,
    stream: TcpStream,
    conn_id: u64,
    slots: &Slots,
) -> Result<()> {
    stream.set_nodelay(true)?;
    let (mut rd, mut wr) = stream.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<PcMsg>();
    let mut peer = PeerSessions::new();
    let mut queue: Vec<PcMsg> = Vec::new();
    let mut registered = false;
    // Set once the peer proves it's the paired hub (HELLO signed with the
    // pinned key) or the hub a person approved (its pairing request gets an
    // accept). A signed pairing request from any other key does NOT count:
    // anyone can make keys.
    let mut proven = false;
    let unproven_deadline = Instant::now() + UNPROVEN_LIFETIME;

    loop {
        tokio::select! {
            frame = tokio::time::timeout_at(
                if proven {
                    Instant::now() + IDLE_TIMEOUT
                } else {
                    unproven_deadline.min(Instant::now() + FIRST_FRAME_TIMEOUT)
                },
                read_frame(&mut rd),
            ) => {
                let frame = frame.map_err(|_| anyhow::anyhow!("timed out waiting for a frame"))??;
                match FrameType::from_byte(frame[0]).map_err(|e| anyhow::anyhow!("{e}"))? {
                    FrameType::PairRequest => {
                        if !state.take_sig_budget(mono_now()) {
                            bail!("signature budget used up");
                        }
                        // Some(reply) only for the pinned or approved hub.
                        if let Some(reply) = on_pair_request(&state, &frame)? {
                            proven = true;
                            slots.mark_proven(conn_id);
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
                        if !state.take_sig_budget(mono_now()) {
                            bail!("signature budget used up");
                        }
                        let (rand, epoch) = hs_random()?;
                        // CRYPTO-REVIEW: verifies the hub's signature with the pinned key.
                        let ack = peer
                            .on_hello(&frame, &hub_pub, &state.identity, &rand, epoch, mono_now())
                            .map_err(|e| anyhow::anyhow!("hello: {e}"))?;
                        proven = true;
                        slots.mark_proven(conn_id);
                        write_frame(&mut wr, &ack).await?;
                        state.hub.lock().unwrap().sessions += 1;
                    }
                    FrameType::Uplink => {
                        let now = mono_now();
                        let batch: HubBatch = uplink::open_json(&frame, |f, out| {
                            peer.open(f, out, now).map(|(_, pt)| pt.len())
                        })?;
                        if batch.msgs.len() > MAX_BATCH_MSGS {
                            bail!("hub batch of {} messages is over the cap", batch.msgs.len());
                        }
                        if !registered {
                            // First frame that verified under a session: this is the real hub.
                            let mut hub = state.hub.lock().unwrap();
                            hub.connected = true;
                            hub.tx = Some(tx.clone());
                            hub.conn_id = Some(conn_id);
                            registered = true;
                            // The phone app shows these names.
                            queue.push(db::labels(&state.db.lock().unwrap())?);
                            info!("hub session established");
                        }
                        let (upto, approvals) = store_batch(&state, &batch)?;
                        for (key, name, phone_id) in approvals {
                            match crate::api::approve_key(&state, &key, name, true) {
                                Ok(_) => info!("node approved from phone {phone_id}"),
                                Err(e) => warn!("approval from phone {phone_id} refused: {}", e.message()),
                            }
                        }
                        queue.retain(|m| !matches!(m, PcMsg::Ack { .. }));
                        if let Some(upto) = upto {
                            queue.insert(0, PcMsg::Ack { upto });
                        }
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
    // Never send more than the hub accepts in one batch; the rest goes next time.
    let n = queue.len().min(MAX_BATCH_MSGS);
    let batch = PcBatch {
        msgs: queue.drain(..n).collect(),
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

/// A node approval a phone asked for: (public key, name, phone id).
type PhoneApproval = (String, Option<String>, u16);

/// Stores a batch and returns (highest hub row id stored, approvals from
/// phones to run once the DB lock is released).
fn store_batch(state: &AppState, batch: &HubBatch) -> Result<(Option<i64>, Vec<PhoneApproval>)> {
    let mut conn = state.db.lock().unwrap();
    let last_seq = db::hub_last_seq(&conn)?;
    let tx = conn.transaction()?;
    let mut upto = None;
    let mut approvals = Vec::new();
    for (seq, msg) in &batch.msgs {
        upto = Some(upto.map_or(*seq, |u: i64| u.max(*seq)));
        match msg {
            HubMsg::PairRequest {
                public_key,
                fingerprint,
                transport,
                kind,
                ts,
            } => state.add_pending(PendingPair {
                kind: match kind {
                    DeviceKind::Node => PairKind::Node,
                    DeviceKind::Phone => PairKind::Phone,
                },
                public_key: public_key.clone(),
                fingerprint: fingerprint.clone(),
                transport: Some(*transport),
                seen_at: *ts,
            }),
            HubMsg::ApproveRequested {
                public_key,
                name,
                phone_id,
                ..
            } => approvals.push((public_key.clone(), name.clone(), *phone_id)),
            HubMsg::RecipeReply { req, ok, err } => state.recipe_reply(
                *req,
                match (ok, err) {
                    (_, Some(e)) => Err(e.clone()),
                    (Some(v), None) => Ok(v.clone()),
                    (None, None) => Err("empty reply from the hub".into()),
                },
            ),
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
        db::store_hub_msg(&tx, *seq, msg)?;
    }
    tx.execute("UPDATE hubs SET last_seen = ?1", [unix_now()])?;
    tx.commit()?;
    state.hub.lock().unwrap().last_seen = Some(unix_now());
    Ok((upto, approvals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_sharecrate::crypto::identity::Identity;
    use crypto_sharecrate::node::{NodeCore, Pairing};
    use tokio::io::AsyncReadExt;

    async fn start(state: Arc<AppState>) -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(serve_on(state, l));
        addr
    }

    fn state() -> Arc<AppState> {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        Arc::new(AppState::new(conn, &[5u8; 32], "t".repeat(64)))
    }

    /// A connection that sends a validly signed pairing request from a
    /// throwaway key (what an attacker can do), then sits idle.
    async fn junk(addr: SocketAddr, seed: u8) -> TcpStream {
        let mut s = TcpStream::connect(addr).await.unwrap();
        let req = NodeCore::new(&[seed; 32], None).pair_request();
        write_frame(&mut s, &req).await.unwrap();
        s
    }

    /// True if the server closes `s` within `wait`. Closing is checked with
    /// a generous wait (a busy CI runner can be slow to drop a socket), and
    /// staying open with a short one.
    async fn closed_within(s: &mut TcpStream, wait: Duration) -> bool {
        let mut b = [0u8; 1];
        matches!(
            tokio::time::timeout(wait, s.read(&mut b)).await,
            Ok(Ok(0)) | Ok(Err(_))
        )
    }

    async fn is_closed(s: &mut TcpStream) -> bool {
        closed_within(s, Duration::from_secs(5)).await
    }

    async fn stays_open(s: &mut TcpStream) -> bool {
        !closed_within(s, Duration::from_millis(300)).await
    }

    #[tokio::test]
    async fn junk_connections_are_evicted_for_new_ones() {
        let addr = start(state()).await;
        let mut first = junk(addr, 10).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut others = Vec::new();
        for i in 0..MAX_CONNECTIONS as u8 {
            others.push(junk(addr, 11 + i).await);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            is_closed(&mut first).await,
            "oldest unproven connection evicted"
        );
        assert!(stays_open(&mut others[MAX_CONNECTIONS - 1]).await);
    }

    #[tokio::test]
    async fn proven_hub_is_never_evicted() {
        let st = state();
        let pc_pub = st.identity.public_key();
        let hub_seed = [7u8; 32];
        let hub_pub = Identity::from_seed(&hub_seed).public_key();
        db::pair_hub(&st.db.lock().unwrap(), &hub_pub, 0).unwrap();
        let addr = start(st).await;

        // real hub: HELLO signed with its pinned key
        let mut hub = TcpStream::connect(addr).await.unwrap();
        let mut core = NodeCore::new(
            &hub_seed,
            Some(Pairing {
                node_id: 1,
                peer_pub: pc_pub,
            }),
        );
        let mut r = [0u8; 48];
        uplink::getrandom(&mut r).unwrap();
        write_frame(&mut hub, &core.start_handshake(&r, 0).unwrap())
            .await
            .unwrap();
        let ack = read_frame(&mut hub).await.unwrap();
        assert!(core.handle_frame(&ack, 0).is_ok());

        // a flood of junk connections can't push it out
        let mut junks = Vec::new();
        for i in 0..(MAX_CONNECTIONS as u8 * 2) {
            junks.push(junk(addr, 40 + i).await);
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(stays_open(&mut hub).await, "verified hub keeps its slot");
    }

    #[test]
    fn signature_budget_is_per_second() {
        let st = state();
        for _ in 0..crate::state::SIG_CHECKS_PER_SEC {
            assert!(st.take_sig_budget(100));
        }
        assert!(!st.take_sig_budget(100));
        assert!(st.take_sig_budget(101));
    }
}

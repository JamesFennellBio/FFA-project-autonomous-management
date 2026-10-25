use std::collections::HashMap;
use std::sync::Mutex;

use crypto_sharecrate::crypto::identity::Identity;
use rusqlite::Connection;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};
use uplink::{PcMsg, RecipeOp, Transport};

/// Pairing requests disappear from the app if not seen for this long.
pub const PENDING_TTL_SECS: i64 = 300;
/// Max pairing requests shown at once.
pub const MAX_PENDING: usize = 32;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PairKind {
    Hub,
    Node,
    Phone,
}

/// The hub's answer to a recipe request: the result, or why it refused.
pub type RecipeReply = Result<Value, String>;

#[derive(Debug, Clone, Serialize)]
pub struct PendingPair {
    pub kind: PairKind,
    pub public_key: String,
    pub fingerprint: String,
    pub transport: Option<Transport>,
    pub seen_at: i64,
}

#[derive(Default)]
pub struct HubLive {
    pub connected: bool,
    pub last_seen: Option<i64>,
    pub backlog: u64,
    pub sessions: u64,
    pub tx: Option<mpsc::UnboundedSender<PcMsg>>,
    /// Which TCP connection owns `tx` (only that one may clear it).
    pub conn_id: Option<u64>,
}

pub struct AppState {
    pub db: Mutex<Connection>,
    /// CRYPTO-REVIEW: the PC daemon's long-term signing key.
    pub identity: Identity,
    pub token: String,
    pub hub: Mutex<HubLive>,
    pub pending: Mutex<HashMap<String, PendingPair>>,
    /// CRYPTO-REVIEW: hub key a person approved in the app; it's pinned once
    /// that hub asks again.
    pub approved_hub: Mutex<Option<[u8; 32]>>,
    /// (second, signature checks done in that second) on the hub port.
    sig_budget: Mutex<(u64, u32)>,
    /// (last request id, API calls waiting for the hub's recipe reply).
    recipe_waiters: Mutex<(u32, HashMap<u32, oneshot::Sender<RecipeReply>>)>,
}

/// Max Ed25519 verifications per second on the hub port, across all
/// connections (FYEO scan 2: unbounded verifies per connection).
pub const SIG_CHECKS_PER_SEC: u32 = 20;

impl AppState {
    pub fn new(conn: Connection, seed: &[u8; 32], token: String) -> Self {
        Self {
            db: Mutex::new(conn),
            identity: Identity::from_seed(seed),
            token,
            hub: Mutex::new(HubLive::default()),
            pending: Mutex::new(HashMap::new()),
            approved_hub: Mutex::new(None),
            sig_budget: Mutex::new((0, 0)),
            recipe_waiters: Mutex::new((0, HashMap::new())),
        }
    }

    /// Takes one signature check from this second's budget.
    pub fn take_sig_budget(&self, now_secs: u64) -> bool {
        let mut b = self.sig_budget.lock().unwrap();
        if b.0 != now_secs {
            *b = (now_secs, 0);
        }
        if b.1 >= SIG_CHECKS_PER_SEC {
            return false;
        }
        b.1 += 1;
        true
    }

    /// Queues a message for the hub. Returns false if the hub isn't connected.
    pub fn send_to_hub(&self, msg: PcMsg) -> bool {
        let hub = self.hub.lock().unwrap();
        match &hub.tx {
            Some(tx) => tx.send(msg).is_ok(),
            None => false,
        }
    }

    /// Sends a recipe request to the hub. The reply arrives on the returned
    /// receiver; None if the hub isn't connected.
    pub fn send_recipe(&self, op: RecipeOp) -> Option<oneshot::Receiver<RecipeReply>> {
        let (tx, rx) = oneshot::channel();
        let req = {
            let mut w = self.recipe_waiters.lock().unwrap();
            // Callers that gave up (timed out) dropped their receiver.
            w.1.retain(|_, waiter| !waiter.is_closed());
            w.0 = w.0.wrapping_add(1);
            let req = w.0;
            w.1.insert(req, tx);
            req
        };
        if !self.send_to_hub(PcMsg::Recipe { req, op }) {
            self.recipe_waiters.lock().unwrap().1.remove(&req);
            return None;
        }
        Some(rx)
    }

    /// Hands the hub's reply to the API call waiting for it (if it still is).
    pub fn recipe_reply(&self, req: u32, reply: RecipeReply) {
        if let Some(tx) = self.recipe_waiters.lock().unwrap().1.remove(&req) {
            let _ = tx.send(reply);
        }
    }

    /// Anyone in radio range can send pairing requests with fresh keys, so the
    /// list is capped: when it's full, the oldest request is dropped.
    pub fn add_pending(&self, p: PendingPair) {
        let mut map = self.pending.lock().unwrap();
        if !map.contains_key(&p.public_key)
            && map.len() >= MAX_PENDING
            && let Some(oldest) = map
                .values()
                .min_by_key(|q| q.seen_at)
                .map(|q| q.public_key.clone())
        {
            map.remove(&oldest);
        }
        map.insert(p.public_key.clone(), p);
    }

    pub fn pending_list(&self, now: i64) -> Vec<PendingPair> {
        let mut map = self.pending.lock().unwrap();
        map.retain(|_, p| now - p.seen_at < PENDING_TTL_SECS);
        let mut v: Vec<_> = map.values().cloned().collect();
        v.sort_by_key(|p| -p.seen_at);
        v
    }
}

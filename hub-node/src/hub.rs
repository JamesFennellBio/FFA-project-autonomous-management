//! The hub's logic: pairing, handshakes, turning node frames into messages
//! for the PC, running recipes, and answering the phone app (see `app`).
//! No I/O here: frames come in, frames to send and messages for the PC go
//! out. That keeps it easy to test.

mod app;

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use anyhow::Result;
use crypto_sharecrate::crypto::identity::Identity;
use crypto_sharecrate::crypto::v2::identity::{HybridIdentity, HybridPublic, KeyId};
use crypto_sharecrate::crypto::{handshake, pairing, responder::PeerSessions};
use crypto_sharecrate::frame::{FRAME_LEN, FrameType, peek_node_id};
use crypto_sharecrate::packet::{ChannelKind, Packet};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uplink::{DeviceKind, HubMsg, PcMsg, Transport};

use crate::recipes::{Action, Fired, Recipes};

/// Where to send a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Udp(SocketAddr),
    #[cfg_attr(not(feature = "lora"), allow(dead_code))]
    Lora,
    /// A phone's TCP connection (id from `app_tcp`).
    App(u64),
}

impl Link {
    pub fn transport(self) -> Transport {
        match self {
            Link::Udp(_) | Link::App(_) => Transport::Wifi,
            Link::Lora => Transport::Lora,
        }
    }
}

/// Only forward the same node's pairing request to the PC this often.
const PAIR_FORWARD_EVERY: i64 = 10;
/// ...and at most one pairing request (from anyone) per second.
const PAIR_FORWARD_MIN_GAP: i64 = 1;
/// Cap on the per-key rate-limit table (anyone can make new keys).
const MAX_PAIR_KEYS_TRACKED: usize = 1024;
/// Max commands queued per node while it's out of reach.
const MAX_QUEUED_COMMANDS: usize = 16;
/// Per node (or phone): at most this many HELLOs per HELLO_WINDOW_SECS.
const HELLOS_PER_WINDOW: usize = 5;
const HELLO_WINDOW_SECS: u64 = 10;
/// Whole hub: at most this many Ed25519 verifications (HELLO + PAIR_REQUEST)
/// per second, so a flood can't eat the Pi's CPU.
const SIG_CHECKS_PER_SEC: u32 = 50;
/// Nodes waiting for approval that the phone app can see.
const MAX_SHOWN_PAIR_REQUESTS: usize = 16;
const SHOWN_PAIR_REQUEST_TTL: i64 = 300;
/// Longest node/channel name kept from the desktop app.
const MAX_LABEL: usize = 32;

/// A paired node or phone.
struct NodeEntry {
    /// CRYPTO-REVIEW: pinned Ed25519 key from pairing.
    public_key: [u8; 32],
    sessions: PeerSessions,
    link: Option<Link>,
    commands: VecDeque<(u8, i32)>,
    /// Monotonic times of recent HELLOs (rate limit).
    recent_hellos: VecDeque<u64>,
}

impl NodeEntry {
    fn new(public_key: [u8; 32]) -> Self {
        Self {
            public_key,
            sessions: PeerSessions::new(),
            link: None,
            commands: VecDeque::new(),
            recent_hellos: VecDeque::new(),
        }
    }
}

/// A paired phone. Phones use the v2 hybrid protocol, so they pin a hybrid key.
struct PhoneEntry {
    /// CRYPTO-REVIEW: pinned Ed25519 + ML-DSA-65 key from pairing.
    public_key: Box<HybridPublic>,
    key_id: KeyId,
    sessions: PeerSessions,
    /// Monotonic times of recent HELLO2s (rate limit).
    recent_hellos: VecDeque<u64>,
}

impl PhoneEntry {
    fn new(public_key: Box<HybridPublic>) -> Self {
        Self {
            key_id: crypto_sharecrate::crypto::v2::identity::key_id(&public_key),
            public_key,
            sessions: PeerSessions::new(),
            recent_hellos: VecDeque::new(),
        }
    }
}

/// A one-time phone pairing ticket from the desktop app's QR code. Kept in
/// memory only: a hub restart cancels open QR codes.
struct PhoneTicket {
    /// CRYPTO-REVIEW: HMAC key the phone must prove it knows (from the QR code).
    token: [u8; 16],
    phone_id: u16,
    /// Monotonic second after which the ticket is refused.
    expires: u64,
}

/// Open QR tickets at once (each is one "Add phone" click in the desktop app).
const MAX_TICKETS: usize = 4;

/// Saved in `paired.json` as soon as anything changes. Pairings are too
/// important to wait for the daily flush.
#[derive(Serialize, Deserialize, Default)]
struct PairedFile {
    nodes: Vec<(u16, String)>,
    approved: Vec<(u16, String)>,
    /// Full hybrid public keys (hex).
    #[serde(default)]
    phones: Vec<(u16, String)>,
    /// Key IDs (hex).
    #[serde(default)]
    approved_phones: Vec<(u16, String)>,
}

/// Names from the desktop app (`labels.json`).
#[derive(Serialize, Deserialize, Default)]
struct Labels {
    nodes: Vec<(u16, String)>,
    channels: Vec<(u16, u8, String)>,
}

#[derive(Debug, Clone, Serialize)]
struct ShownPairRequest {
    public_key: String,
    fingerprint: String,
    transport: Transport,
    seen_at: i64,
}

/// What the phone app shows. Rebuilt from the store when the hub starts.
#[derive(Default)]
struct Live {
    /// node -> (channel, kind) from its last Announce
    channels: HashMap<u16, Vec<(u8, u8)>>,
    /// (node, channel) -> (value, unix ts)
    values: HashMap<(u16, u8), (i32, i64)>,
    last_seen: HashMap<u16, i64>,
    labels: Labels,
    /// Nodes waiting for approval. Oldest first.
    pair_requests: VecDeque<ShownPairRequest>,
}

impl Live {
    fn value(&self, node: u16, channel: u8) -> Option<i32> {
        self.values.get(&(node, channel)).map(|v| v.0)
    }

    /// Some(true/false) if the node announced this channel, None if unknown.
    fn is_output(&self, node: u16, channel: u8) -> Option<bool> {
        let chans = self.channels.get(&node)?;
        let (_, kind) = chans.iter().find(|(c, _)| *c == channel)?;
        Some(ChannelKind::from_byte(*kind).is_ok_and(|k| k.is_output()))
    }

    fn node_name(&self, node: u16) -> String {
        self.labels
            .nodes
            .iter()
            .find(|(n, _)| *n == node)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| format!("Node {node}"))
    }

    fn channel_name(&self, node: u16, channel: u8) -> String {
        self.labels
            .channels
            .iter()
            .find(|(n, c, _)| *n == node && *c == channel)
            .map(|(_, _, s)| s.clone())
            .unwrap_or_else(|| format!("Channel {channel}"))
    }
}

pub struct Hub {
    /// CRYPTO-REVIEW: v1 Ed25519 identity, for the nodes' radio links.
    identity: Identity,
    /// CRYPTO-REVIEW: v2 hybrid identity from the same seed, for phones and
    /// the PC (the PC link builds its own copy in pc_link).
    pq_identity: HybridIdentity,
    nodes: HashMap<u16, NodeEntry>,
    phones: HashMap<u16, PhoneEntry>,
    /// CRYPTO-REVIEW: keys a person approved in the app, waiting for the
    /// device's next PAIR_REQUEST. Nodes and phones are kept apart, so an
    /// approved node key can never pair on the phone port or the other way round.
    approved: HashMap<[u8; 32], u16>,
    /// Phones are approved by KeyId (the hash of their hybrid key).
    approved_phones: HashMap<KeyId, u16>,
    /// CRYPTO-REVIEW: open QR tickets, oldest first. Single use.
    tickets: VecDeque<PhoneTicket>,
    last_pair_forward: HashMap<[u8; 32], i64>,
    last_any_pair_forward: i64,
    /// (second, signature checks done in that second)
    sig_budget: (u64, u32),
    paired_path: Option<PathBuf>,
    labels_path: Option<PathBuf>,
    live: Live,
    recipes: Recipes,
}

#[derive(Default, Debug)]
pub struct Output {
    pub frames: Vec<(Link, Vec<u8>)>,
    /// Stored in the hub DB and forwarded to the PC (resent until acked).
    pub to_pc: Vec<HubMsg>,
    /// Forwarded to the PC only if it's connected right now; never stored.
    /// Used for pairing requests, which anyone can generate, so they must
    /// not fill the hub's database (scan findings 02/08).
    pub live: Vec<HubMsg>,
}

/// CRYPTO-REVIEW: randomness for one handshake: 48 bytes (ephemeral key +
/// nonce) plus 4 for the epoch id, from the OS RNG.
fn hs_random() -> Result<([u8; 48], u32)> {
    let mut r = [0u8; 52];
    uplink::getrandom(&mut r)?;
    let mut hs = [0u8; 48];
    hs.copy_from_slice(&r[..48]);
    Ok((hs, u32::from_le_bytes([r[48], r[49], r[50], r[51]])))
}

fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::Node => "node",
        DeviceKind::Phone => "phone",
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.trim().chars().take(max).collect()
}

impl Hub {
    /// `data_dir` holds paired.json, recipes.json and labels.json. `None`
    /// keeps everything in memory (tests).
    pub fn new(seed: &[u8; 32], data_dir: Option<&Path>) -> Result<Self> {
        let mut hub = Self {
            identity: Identity::from_seed(seed),
            pq_identity: HybridIdentity::from_seed(seed),
            nodes: HashMap::new(),
            phones: HashMap::new(),
            approved: HashMap::new(),
            approved_phones: HashMap::new(),
            tickets: VecDeque::new(),
            last_pair_forward: HashMap::new(),
            last_any_pair_forward: i64::MIN,
            sig_budget: (0, 0),
            paired_path: data_dir.map(|d| d.join("paired.json")),
            labels_path: data_dir.map(|d| d.join("labels.json")),
            live: Live::default(),
            recipes: Recipes::load(data_dir.map(|d| d.join("recipes.json")))?,
        };
        if let Some(p) = &hub.paired_path
            && let Ok(s) = std::fs::read_to_string(p)
        {
            let f: PairedFile = serde_json::from_str(&s)?;
            for (id, k) in f.nodes {
                hub.nodes
                    .insert(id, NodeEntry::new(uplink::key_from_hex(&k)?));
            }
            for (id, k) in f.approved {
                hub.approved.insert(uplink::key_from_hex(&k)?, id);
            }
            for (id, k) in f.phones {
                match uplink::v2::public_from_hex(&k) {
                    Ok(pk) => {
                        hub.phones.insert(id, PhoneEntry::new(pk));
                    }
                    // A phone paired before v2: it has to pair again.
                    Err(_) => {
                        warn!("phone {id} was paired before v2; pair it again from the desktop app")
                    }
                }
            }
            for (id, k) in f.approved_phones {
                hub.approved_phones.insert(uplink::key_from_hex(&k)?, id);
            }
        }
        if let Some(p) = &hub.labels_path
            && let Ok(s) = std::fs::read_to_string(p)
        {
            hub.live.labels = serde_json::from_str(&s).unwrap_or_default();
        }
        info!(
            "hub has {} paired node(s), {} phone(s), {} recipe(s)",
            hub.nodes.len(),
            hub.phones.len(),
            hub.recipes.list().len()
        );
        Ok(hub)
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.identity.public_key()
    }

    /// The v2 key phones and the PC pin.
    pub fn pq_key_id(&self) -> KeyId {
        self.pq_identity.key_id()
    }

    /// Fills the phone app's view from the store after a restart.
    pub fn restore_live(&mut self, msgs: Vec<HubMsg>) {
        for m in msgs {
            match m {
                HubMsg::Announce {
                    node_id,
                    channels,
                    ts,
                } => {
                    self.live.channels.insert(node_id, channels);
                    self.seen(node_id, ts);
                }
                HubMsg::Reading {
                    node_id,
                    channel,
                    value,
                    ts,
                } => {
                    self.live.values.insert((node_id, channel), (value, ts));
                    self.seen(node_id, ts);
                }
                _ => {}
            }
        }
    }

    fn seen(&mut self, node_id: u16, ts: i64) {
        let t = self.live.last_seen.entry(node_id).or_default();
        *t = (*t).max(ts);
    }

    fn save(&self) -> Result<()> {
        let Some(p) = &self.paired_path else {
            return Ok(());
        };
        let list = |m: &HashMap<u16, NodeEntry>| {
            m.iter()
                .map(|(id, n)| (*id, uplink::key_to_hex(&n.public_key)))
                .collect()
        };
        let approved = |m: &HashMap<[u8; 32], u16>| {
            m.iter()
                .map(|(k, id)| (*id, uplink::key_to_hex(k)))
                .collect()
        };
        let f = PairedFile {
            nodes: list(&self.nodes),
            approved: approved(&self.approved),
            phones: self
                .phones
                .iter()
                .map(|(id, p)| (*id, uplink::v2::public_to_hex(&p.public_key)))
                .collect(),
            approved_phones: approved(&self.approved_phones),
        };
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
    }

    /// Global limit on signature checks per second (finding 09).
    fn take_sig_budget(&mut self, mono: u64) -> bool {
        if self.sig_budget.0 != mono {
            self.sig_budget = (mono, 0);
        }
        if self.sig_budget.1 >= SIG_CHECKS_PER_SEC {
            return false;
        }
        self.sig_budget.1 += 1;
        true
    }

    /// Handles one frame from a radio. Bad frames are logged and dropped:
    /// anyone can transmit on these bands.
    pub fn on_frame(&mut self, frame: &[u8], link: Link, mono: u64, unix: i64) -> Output {
        let mut out = Output::default();
        if let Err(e) = self.on_frame_inner(frame, link, mono, unix, &mut out) {
            debug!("dropped frame from {link:?}: {e:#}");
        }
        out
    }

    fn on_frame_inner(
        &mut self,
        frame: &[u8],
        link: Link,
        mono: u64,
        unix: i64,
        out: &mut Output,
    ) -> Result<()> {
        let ftype = FrameType::from_byte(*frame.first().ok_or_else(|| anyhow::anyhow!("empty"))?)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        match ftype {
            FrameType::PairRequest => {
                if !self.take_sig_budget(mono) {
                    anyhow::bail!("signature budget used up; pair request dropped");
                }
                self.on_pair_request(frame, link, unix, out)?
            }
            FrameType::Hello => self.on_hello(frame, link, mono, out)?,
            t if t.is_encrypted() && frame.len() == FRAME_LEN => {
                let node_id = peek_node_id(frame).unwrap_or(0);
                let node = self
                    .nodes
                    .get_mut(&node_id)
                    .ok_or_else(|| anyhow::anyhow!("frame from unknown node {node_id}"))?;
                let was_epoch = node.sessions.current().map(|s| s.epoch);
                let mut buf = [0u8; FRAME_LEN];
                let (h, pt) = node
                    .sessions
                    .open(frame, &mut buf, mono)
                    .map_err(|e| anyhow::anyhow!("node {node_id}: {e}"))?;
                let packet = Packet::decode(h.ftype, pt).map_err(|e| anyhow::anyhow!("{e}"))?;
                node.link = Some(link);
                if was_epoch != Some(h.epoch) {
                    // First good frame of a new session (key confirmation passed).
                    out.to_pc.push(HubMsg::NodeOnline {
                        node_id,
                        transport: link.transport(),
                        epoch: h.epoch,
                        ts: unix,
                    });
                }
                packet_to_pc(node_id, &packet, unix, &mut out.to_pc);
                // A node only listens right after it sends (on LoRa), so
                // queued commands go out now.
                deliver(node_id, node, mono, unix, out);
                self.observe(node_id, &packet, mono, unix, out);
            }
            other => anyhow::bail!("unexpected {other:?}"),
        }
        Ok(())
    }

    /// CRYPTO-REVIEW: HELLO from a paired node. The signature is checked with
    /// the pinned key; the new session waits as pending until key confirmation.
    /// (Phones use the v2 [`Hub::on_phone_hello`].)
    fn on_hello(&mut self, frame: &[u8], link: Link, mono: u64, out: &mut Output) -> Result<()> {
        let id = handshake::hello_node_id(frame).map_err(|e| anyhow::anyhow!("{e}"))?;
        let what = "node";
        // Cheap checks first: unknown device, per-device rate, global budget.
        // The per-device window only counts HELLOs that verified (below),
        // so forged HELLOs can't use up a real device's allowance (FYEO
        // scan 2). Forged ones still cost the global budget, which is the
        // cap on how much CPU an attacker can burn.
        let peer = self
            .nodes
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("HELLO from unknown {what} {id}"))?;
        while peer
            .recent_hellos
            .front()
            .is_some_and(|t| mono.saturating_sub(*t) >= HELLO_WINDOW_SECS)
        {
            peer.recent_hellos.pop_front();
        }
        if peer.recent_hellos.len() >= HELLOS_PER_WINDOW {
            anyhow::bail!("too many HELLOs from {what} {id}");
        }
        if !self.take_sig_budget(mono) {
            anyhow::bail!("signature budget used up; HELLO dropped");
        }
        let identity = &self.identity;
        let peer = self.nodes.get_mut(&id).expect("checked above");
        let (rand, epoch) = hs_random()?;
        let ack = peer
            .sessions
            .on_hello(frame, &peer.public_key, identity, &rand, epoch, mono)
            .map_err(|e| anyhow::anyhow!("hello from {what} {id}: {e}"))?;
        peer.recent_hellos.push_back(mono);
        out.frames.push((link, ack.to_vec()));
        Ok(())
    }

    /// CRYPTO-REVIEW: hub side of pairing for nodes; see crypto::pairing for
    /// the trust model. Needs a person's approval in the desktop app first.
    /// (Phones use the v2 [`Hub::on_phone_pair_request`].)
    fn on_pair_request(
        &mut self,
        frame: &[u8],
        link: Link,
        unix: i64,
        out: &mut Output,
    ) -> Result<()> {
        let kind = DeviceKind::Node;
        let pk =
            pairing::parse_pair_request(frame).map_err(|e| anyhow::anyhow!("pair request: {e}"))?;
        let (peers, approved) = (&mut self.nodes, &mut self.approved);
        if let Some((id, _)) = peers.iter().find(|(_, n)| n.public_key == pk) {
            // Already paired; our accept was probably lost. Send it again.
            out.frames.push((
                link,
                pairing::pair_accept(&self.identity, &pk, *id).to_vec(),
            ));
            return Ok(());
        }
        if let Some(id) = approved.remove(&pk) {
            info!(
                "pairing {} {id} ({})",
                kind_name(kind),
                uplink::fingerprint_hex(&pk)
            );
            peers.insert(id, NodeEntry::new(pk));
            self.save()?;
            let key = uplink::key_to_hex(&pk);
            self.live.pair_requests.retain(|r| r.public_key != key);
            out.frames
                .push((link, pairing::pair_accept(&self.identity, &pk, id).to_vec()));
            return Ok(());
        }
        self.show_pair_request(&pk, link.transport(), unix);
        self.forward_pair_request(
            pk,
            uplink::key_to_hex(&pk),
            uplink::fingerprint_hex(&pk),
            link.transport(),
            kind,
            unix,
            out,
        );
        Ok(())
    }

    /// Tells the PC (live only, never stored) that a device is asking to pair.
    /// `rate_key` is the node key or the phone's KeyId.
    ///
    /// Rate limits (findings 02/08): per key, overall, and a cap on how many
    /// keys we track. Requests that don't fit are just dropped; the device
    /// keeps asking every few seconds while its pairing window is open.
    #[allow(clippy::too_many_arguments)]
    fn forward_pair_request(
        &mut self,
        rate_key: [u8; 32],
        public_key: String,
        fingerprint: String,
        transport: Transport,
        kind: DeviceKind,
        unix: i64,
        out: &mut Output,
    ) {
        let due = self
            .last_pair_forward
            .get(&rate_key)
            .is_none_or(|last| unix.saturating_sub(*last) >= PAIR_FORWARD_EVERY)
            && unix.saturating_sub(self.last_any_pair_forward) >= PAIR_FORWARD_MIN_GAP;
        if self.last_pair_forward.len() >= MAX_PAIR_KEYS_TRACKED {
            self.last_pair_forward
                .retain(|_, t| unix.saturating_sub(*t) < PAIR_FORWARD_EVERY);
        }
        if due && self.last_pair_forward.len() < MAX_PAIR_KEYS_TRACKED {
            self.last_pair_forward.insert(rate_key, unix);
            self.last_any_pair_forward = unix;
            out.live.push(HubMsg::PairRequest {
                public_key,
                fingerprint,
                transport,
                kind,
                ts: unix,
            });
        }
    }

    /// Remembers a waiting node so the phone app can list it (capped and
    /// expiring, since anyone can make pairing requests).
    fn show_pair_request(&mut self, pk: &[u8; 32], transport: Transport, unix: i64) {
        let list = &mut self.live.pair_requests;
        let key = uplink::key_to_hex(pk);
        list.retain(|r| r.public_key != key && unix - r.seen_at < SHOWN_PAIR_REQUEST_TTL);
        if list.len() >= MAX_SHOWN_PAIR_REQUESTS {
            list.pop_front();
        }
        list.push_back(ShownPairRequest {
            public_key: key,
            fingerprint: uplink::fingerprint_hex(pk),
            transport,
            seen_at: unix,
        });
    }

    /// Keeps the phone app's view up to date and runs recipes.
    fn observe(&mut self, node_id: u16, p: &Packet, mono: u64, unix: i64, out: &mut Output) {
        self.seen(node_id, unix);
        let mut changed = Vec::new();
        match p {
            Packet::Data { .. } => {
                for r in p.readings() {
                    self.live
                        .values
                        .insert((node_id, r.channel), (r.value, unix));
                    changed.push(r.channel);
                }
            }
            // An acked command is the output's new state.
            Packet::Ack { channel, value } => {
                self.live.values.insert((node_id, *channel), (*value, unix));
                changed.push(*channel);
            }
            Packet::Announce { .. } => {
                self.live.channels.insert(
                    node_id,
                    p.channels()
                        .iter()
                        .map(|c| (c.channel, c.kind as u8))
                        .collect(),
                );
            }
            _ => {}
        }
        for ch in changed {
            let live = &self.live;
            let fired = self
                .recipes
                .on_reading(node_id, ch, |n, c| live.value(n, c));
            self.run(fired, mono, unix, out);
        }
    }

    /// Runs the actions of recipes that fired.
    fn run(&mut self, fired: Vec<Fired>, mono: u64, unix: i64, out: &mut Output) {
        for f in fired {
            info!("recipe {} \"{}\" fired", f.id, f.name);
            for a in f.actions {
                match a {
                    Action::Set {
                        node,
                        channel,
                        value,
                    } => {
                        let src = format!("recipe:{}", f.id);
                        if let Err(e) =
                            self.queue_command((node, channel, value), Some(src), mono, unix, out)
                        {
                            warn!("recipe {}: {e}", f.id);
                        }
                    }
                    Action::Alert { text } => out.to_pc.push(HubMsg::RecipeAlert {
                        recipe_id: f.id,
                        name: f.name.clone(),
                        text,
                        ts: unix,
                    }),
                }
            }
        }
    }

    /// Queues a command for a node and sends it right away if it's on Wi-Fi.
    /// `source` is set for commands that start on the hub (phone, recipe),
    /// so the PC can log them.
    fn queue_command(
        &mut self,
        (node_id, channel, value): (u16, u8, i32),
        source: Option<String>,
        mono: u64,
        unix: i64,
        out: &mut Output,
    ) -> Result<(), String> {
        let Some(node) = self.nodes.get_mut(&node_id) else {
            return Err(format!("no node {node_id}"));
        };
        if let Some(source) = source {
            out.to_pc.push(HubMsg::CommandRequested {
                node_id,
                channel,
                value,
                source,
                ts: unix,
            });
        }
        // A newer command for the same channel replaces an older one still queued.
        node.commands.retain(|(c, _)| *c != channel);
        if node.commands.len() >= MAX_QUEUED_COMMANDS
            && let Some((c, v)) = node.commands.pop_front()
        {
            // Tell the PC so the app can show it failed (finding 30).
            warn!("node {node_id} command queue full; dropped ch {c} = {v}");
            out.to_pc.push(HubMsg::CommandDropped {
                node_id,
                channel: c,
                value: v,
                ts: unix,
            });
        }
        node.commands.push_back((channel, value));
        // Wi-Fi nodes listen all the time, so send now. LoRa nodes get
        // it in the receive window after their next uplink.
        if matches!(node.link, Some(Link::Udp(_))) {
            deliver(node_id, node, mono, unix, out);
        }
        Ok(())
    }

    /// Handles a message from the PC daemon.
    pub fn on_pc(&mut self, msg: PcMsg, mono: u64, unix: i64) -> Output {
        let mut out = Output::default();
        match msg {
            PcMsg::PhoneTicket {
                token,
                phone_id,
                ttl_secs,
            } => {
                let token: Option<[u8; 16]> =
                    hex::decode(&token).ok().and_then(|t| t.try_into().ok());
                let taken = self.phones.contains_key(&phone_id)
                    || self.approved_phones.values().any(|id| *id == phone_id);
                match token {
                    Some(token) if !taken => {
                        // CRYPTO-REVIEW: capped list, oldest dropped; each
                        // ticket expires after ttl_secs (at most 10 minutes).
                        if self.tickets.len() >= MAX_TICKETS {
                            self.tickets.pop_front();
                        }
                        self.tickets.push_back(PhoneTicket {
                            token,
                            phone_id,
                            expires: mono + u64::from(ttl_secs.min(600)),
                        });
                        info!("phone pairing QR code {phone_id} is open for {ttl_secs} s");
                    }
                    Some(_) => warn!("refusing QR ticket: phone id {phone_id} is already in use"),
                    None => warn!("refusing QR ticket with a bad token"),
                }
            }
            PcMsg::Ack { .. } => {} // handled by pc_link
            PcMsg::ApprovePair {
                public_key,
                node_id,
                kind,
            } => match uplink::key_from_hex(&public_key) {
                Ok(pk) => {
                    let what = kind_name(kind);
                    // CRYPTO-REVIEW: never silently re-point an id at a new
                    // key, or one key at two ids (findings 22/25). For phones
                    // `pk` is the KeyId of their hybrid key.
                    let conflict = match kind {
                        DeviceKind::Node => match self.nodes.get(&node_id) {
                            Some(n) if n.public_key != pk => Some(format!(
                                "{what} id {node_id} is already paired to another device"
                            )),
                            _ => self
                                .nodes
                                .iter()
                                .find(|(id, n)| n.public_key == pk && **id != node_id)
                                .map(|(id, _)| {
                                    format!("this device is already paired as {what} {id}")
                                }),
                        },
                        DeviceKind::Phone => match self.phones.get(&node_id) {
                            Some(p) if p.key_id != pk => Some(format!(
                                "{what} id {node_id} is already paired to another device"
                            )),
                            _ => self
                                .phones
                                .iter()
                                .find(|(id, p)| p.key_id == pk && **id != node_id)
                                .map(|(id, _)| {
                                    format!("this device is already paired as {what} {id}")
                                }),
                        },
                    };
                    if let Some(reason) = conflict {
                        warn!("refusing approval: {reason}");
                        out.to_pc.push(HubMsg::ApproveFailed {
                            public_key,
                            node_id,
                            reason,
                            ts: unix,
                        });
                        return out;
                    }
                    let approved = match kind {
                        DeviceKind::Node => &mut self.approved,
                        DeviceKind::Phone => &mut self.approved_phones,
                    };
                    approved.retain(|_, id| *id != node_id);
                    approved.insert(pk, node_id);
                    if let Err(e) = self.save() {
                        warn!("saving pairings: {e:#}");
                    }
                    info!("{what} {node_id} approved; waiting for its next pair request");
                }
                Err(e) => warn!("bad approve key: {e:#}"),
            },
            PcMsg::Unpair { node_id, kind } => {
                match kind {
                    DeviceKind::Node => {
                        self.nodes.remove(&node_id);
                        // Also forget an approval still waiting for that id, or the
                        // deleted device could still pair later.
                        self.approved.retain(|_, id| *id != node_id);
                        self.live.channels.remove(&node_id);
                        self.live.values.retain(|(n, _), _| *n != node_id);
                        self.live.last_seen.remove(&node_id);
                        let users = self.recipes.uses_node(node_id);
                        if !users.is_empty() {
                            warn!(
                                "node {node_id} unpaired; these recipes use it and won't work: {}",
                                users.join(", ")
                            );
                        }
                    }
                    DeviceKind::Phone => {
                        self.phones.remove(&node_id);
                        self.approved_phones.retain(|_, id| *id != node_id);
                    }
                }
                if let Err(e) = self.save() {
                    warn!("saving pairings: {e:#}");
                }
            }
            PcMsg::Labels { nodes, channels } => {
                self.live.labels = Labels {
                    nodes: nodes
                        .into_iter()
                        .map(|(n, s)| (n, truncate(&s, MAX_LABEL)))
                        .collect(),
                    channels: channels
                        .into_iter()
                        .map(|(n, c, s)| (n, c, truncate(&s, MAX_LABEL)))
                        .collect(),
                };
                if let Some(p) = &self.labels_path
                    && let Err(e) = serde_json::to_vec(&self.live.labels)
                        .map_err(anyhow::Error::from)
                        .and_then(|b| Ok(std::fs::write(p, b)?))
                {
                    warn!("saving labels: {e:#}");
                }
            }
            PcMsg::Command {
                node_id,
                channel,
                value,
            } => {
                if let Err(e) =
                    self.queue_command((node_id, channel, value), None, mono, unix, &mut out)
                {
                    warn!("command from PC: {e}");
                }
            }
            PcMsg::Recipe { req, op } => {
                let r = self.on_pc_recipe(op, mono, unix, &mut out);
                out.live.push(HubMsg::RecipeReply {
                    req,
                    ok: r.as_ref().ok().cloned(),
                    err: r.err(),
                });
            }
        }
        out
    }
}

/// Sends queued commands while the session still has messages left. If it
/// runs out, the node sees 10 received frames and rekeys, and we carry on then.
fn deliver(node_id: u16, node: &mut NodeEntry, mono: u64, unix: i64, out: &mut Output) {
    let Some(link) = node.link else { return };
    let Some(session) = node.sessions.current_mut() else {
        return;
    };
    while let Some(&(channel, value)) = node.commands.front() {
        if !session.can_send(mono) {
            break;
        }
        match session.seal_packet(&Packet::Command { channel, value }, mono) {
            Ok(f) => {
                node.commands.pop_front();
                out.frames.push((link, f.to_vec()));
                out.to_pc.push(HubMsg::CommandSent {
                    node_id,
                    channel,
                    value,
                    ts: unix,
                });
            }
            Err(e) => {
                warn!("sealing command for node {node_id}: {e}");
                break;
            }
        }
    }
}

fn packet_to_pc(node_id: u16, p: &Packet, ts: i64, out: &mut Vec<HubMsg>) {
    match p {
        Packet::Data { .. } => {
            for r in p.readings() {
                out.push(HubMsg::Reading {
                    node_id,
                    channel: r.channel,
                    value: r.value,
                    ts,
                });
            }
        }
        Packet::Announce { .. } => out.push(HubMsg::Announce {
            node_id,
            channels: p
                .channels()
                .iter()
                .map(|c| (c.channel, c.kind as u8))
                .collect(),
            ts,
        }),
        Packet::Alert {
            channel,
            code,
            value,
        } => out.push(HubMsg::Alert {
            node_id,
            channel: *channel,
            code: *code,
            value: *value,
            ts,
        }),
        Packet::Message { .. } => out.push(HubMsg::Message {
            node_id,
            text: p.text().to_string(),
            ts,
        }),
        Packet::Ack { channel, value } => out.push(HubMsg::CommandAcked {
            node_id,
            channel: *channel,
            value: *value,
            ts,
        }),
        Packet::Command { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_sharecrate::node::{NodeCore, NodeEvent};
    use crypto_sharecrate::packet::Reading;

    fn rand48() -> [u8; 48] {
        let mut r = [0u8; 48];
        uplink::getrandom(&mut r).unwrap();
        r
    }

    #[test]
    fn pair_handshake_data_and_command() {
        let addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        let link = Link::Udp(addr);
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let mut node = NodeCore::new(&[2u8; 32], None);

        // 1. pairing request goes to the PC, nothing is sent back yet
        let out = hub.on_frame(&node.pair_request(), link, 0, 100);
        assert!(out.frames.is_empty());
        assert!(out.to_pc.is_empty(), "pair requests are never stored");
        let HubMsg::PairRequest { public_key, .. } = &out.live[0] else {
            panic!()
        };

        // 2. a person approves it in the app, the node asks again and gets accepted
        hub.on_pc(
            PcMsg::ApprovePair {
                public_key: public_key.clone(),
                node_id: 4,
                kind: DeviceKind::Node,
            },
            0,
            100,
        );
        let out = hub.on_frame(&node.pair_request(), link, 0, 101);
        assert!(matches!(
            node.handle_frame(&out.frames[0].1, 0).unwrap(),
            NodeEvent::Paired(_)
        ));

        // 3. handshake
        let hello = node.start_handshake(&rand48(), 0).unwrap();
        let out = hub.on_frame(&hello, link, 0, 102);
        node.handle_frame(&out.frames[0].1, 0).unwrap();

        // 4. data -> NodeOnline + Reading for the PC
        let f = node
            .seal(
                &Packet::data(&[Reading {
                    channel: 0,
                    value: 21_000,
                }]),
                1,
            )
            .unwrap();
        let out = hub.on_frame(&f, link, 1, 103);
        assert!(matches!(
            out.to_pc[0],
            HubMsg::NodeOnline { node_id: 4, .. }
        ));
        assert!(matches!(
            out.to_pc[1],
            HubMsg::Reading {
                node_id: 4,
                value: 21_000,
                ..
            }
        ));

        // 5. command from PC goes straight out over Wi-Fi
        let out = hub.on_pc(
            PcMsg::Command {
                node_id: 4,
                channel: 2,
                value: 1,
            },
            2,
            104,
        );
        assert_eq!(
            node.handle_frame(&out.frames[0].1, 2).unwrap(),
            NodeEvent::Command {
                channel: 2,
                value: 1
            }
        );
    }

    /// Pairs `node` as `id` through the normal approve flow and starts a session.
    fn pair_and_connect(hub: &mut Hub, node: &mut NodeCore, id: u16, link: Link) {
        hub.on_pc(
            PcMsg::ApprovePair {
                public_key: uplink::key_to_hex(&node.public_key()),
                node_id: id,
                kind: DeviceKind::Node,
            },
            0,
            0,
        );
        let out = hub.on_frame(&node.pair_request(), link, 0, 0);
        node.handle_frame(&out.frames[0].1, 0).unwrap();
        let out = hub.on_frame(&node.start_handshake(&rand48(), 0).unwrap(), link, 0, 0);
        node.handle_frame(&out.frames[0].1, 0).unwrap();
    }

    fn udp() -> Link {
        Link::Udp("127.0.0.1:9999".parse().unwrap())
    }

    #[test]
    fn approval_for_taken_id_is_refused() {
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let mut a = NodeCore::new(&[2u8; 32], None);
        pair_and_connect(&mut hub, &mut a, 1, udp());
        let b = NodeCore::new(&[3u8; 32], None);
        let out = hub.on_pc(
            PcMsg::ApprovePair {
                public_key: uplink::key_to_hex(&b.public_key()),
                node_id: 1,
                kind: DeviceKind::Node,
            },
            0,
            0,
        );
        assert!(matches!(
            out.to_pc[0],
            HubMsg::ApproveFailed { node_id: 1, .. }
        ));
        // node 1 still belongs to A
        assert_eq!(hub.nodes[&1].public_key, a.public_key());
    }

    #[test]
    fn pair_request_flood_is_rate_limited_and_never_stored() {
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let mut forwarded = 0;
        for i in 0..20u8 {
            let n = NodeCore::new(&[i + 10; 32], None);
            let out = hub.on_frame(&n.pair_request(), udp(), 0, 1000);
            assert!(out.to_pc.is_empty());
            forwarded += out.live.len();
        }
        assert_eq!(forwarded, 1, "at most one pairing request per second");
    }

    #[test]
    fn hello_flood_is_rate_limited() {
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let mut node = NodeCore::new(&[2u8; 32], None);
        pair_and_connect(&mut hub, &mut node, 1, udp()); // 1 HELLO used
        let mut answered = 0;
        for _ in 0..10 {
            let hello = node.start_handshake(&rand48(), 1).unwrap();
            answered += hub.on_frame(&hello, udp(), 1, 0).frames.len();
        }
        assert_eq!(answered, HELLOS_PER_WINDOW - 1);
    }

    #[test]
    fn ack_and_dropped_commands_reach_the_pc() {
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let mut node = NodeCore::new(&[2u8; 32], None);
        pair_and_connect(&mut hub, &mut node, 1, Link::Lora);
        // node's first frame confirms the session
        hub.on_frame(&node.seal(&Packet::data(&[]), 0).unwrap(), Link::Lora, 0, 0);
        // LoRa node: commands queue up until the node transmits
        let mut dropped = 0;
        for ch in 0..(MAX_QUEUED_COMMANDS as u8 + 3) {
            let out = hub.on_pc(
                PcMsg::Command {
                    node_id: 1,
                    channel: ch,
                    value: 1,
                },
                0,
                0,
            );
            dropped += out
                .to_pc
                .iter()
                .filter(|m| matches!(m, HubMsg::CommandDropped { .. }))
                .count();
        }
        assert_eq!(dropped, 3);
        let ack = node
            .seal(
                &Packet::Ack {
                    channel: 5,
                    value: 1,
                },
                0,
            )
            .unwrap();
        let out = hub.on_frame(&ack, Link::Lora, 0, 0);
        assert!(out.to_pc.iter().any(|m| matches!(
            m,
            HubMsg::CommandAcked {
                node_id: 1,
                channel: 5,
                value: 1,
                ..
            }
        )));
    }

    #[test]
    fn forged_hellos_do_not_use_up_a_nodes_allowance() {
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let mut node = NodeCore::new(&[2u8; 32], None);
        pair_and_connect(&mut hub, &mut node, 1, udp());
        // attacker signs HELLOs for node 1 with their own key
        let mut evil = NodeCore::new(
            &[9u8; 32],
            Some(crypto_sharecrate::node::Pairing {
                node_id: 1,
                peer_pub: hub.public_key(),
            }),
        );
        for _ in 0..20 {
            let forged = evil.start_handshake(&rand48(), 1).unwrap();
            assert!(hub.on_frame(&forged, udp(), 1, 0).frames.is_empty());
        }
        // the real node can still handshake in the same second... (budget is
        // per second, so move to the next one)
        let hello = node.start_handshake(&rand48(), 2).unwrap();
        assert_eq!(hub.on_frame(&hello, udp(), 2, 0).frames.len(), 1);
    }

    #[test]
    fn unpair_clears_waiting_approval() {
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let node = NodeCore::new(&[2u8; 32], None);
        hub.on_pc(
            PcMsg::ApprovePair {
                public_key: uplink::key_to_hex(&node.public_key()),
                node_id: 7,
                kind: DeviceKind::Node,
            },
            0,
            0,
        );
        hub.on_pc(
            PcMsg::Unpair {
                node_id: 7,
                kind: DeviceKind::Node,
            },
            0,
            0,
        );
        let out = hub.on_frame(&node.pair_request(), udp(), 0, 100);
        assert!(out.frames.is_empty(), "no PAIR_ACCEPT after unpair");
    }
}

//! The phone app's link to the hub (TCP port `uplink::app::APP_PORT`, see
//! `app_tcp` for the sockets).
//!
//! A phone pairs and handshakes with the v2 hybrid protocol: PAIR_REQUEST2
//! (approved in the desktop app) and HELLO2 (Ed25519 + ML-DSA-65 signatures,
//! X25519 + ML-KEM-768 keys), with new keys every 1000 messages or 1 hour. It
//! has its own id range. After the handshake every request is one `Uplink`
//! frame (ChaCha20-Poly1305 with JSON inside, at most `MAX_APP_PLAINTEXT`
//! bytes) and gets exactly one reply.
//!
//! ```text
//! phone -> hub   {"id": 7, "req": {"type": "channels", "offset": 0}}
//! hub -> phone   {"id": 7, "ok": {...}}      or      {"id": 7, "err": "why"}
//! ```
//!
//! What a phone may do: read live values, history and alerts; set outputs;
//! approve waiting *nodes* (the PC still assigns the id); manage recipes. It
//! can't pair other phones, unpair anything, or change hub settings.
//!
//! The desktop app manages the same recipes through the PC daemon
//! (`PcMsg::Recipe`, see [`Hub::on_pc_recipe`]), with the same checks.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use uplink::RecipeOp;
use uplink::app::{MAX_APP_FRAME, MAX_APP_PLAINTEXT};

use super::*;
use crate::recipes::{MAX_RECIPES, Recipe};
use crate::store::{RETENTION_SECS, Store};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Request {
    Info,
    Nodes {
        #[serde(default)]
        offset: usize,
    },
    Channels {
        #[serde(default)]
        offset: usize,
    },
    History {
        node: u16,
        channel: u8,
        since: i64,
        #[serde(default = "max_points")]
        points: usize,
    },
    Events {
        before: Option<i64>,
    },
    Command {
        node: u16,
        channel: u8,
        value: i32,
    },
    PairRequests {
        #[serde(default)]
        offset: usize,
    },
    ApproveNode {
        public_key: String,
        name: Option<String>,
    },
    Recipes {
        #[serde(default)]
        offset: usize,
    },
    GetRecipe {
        id: u32,
    },
    SaveRecipe {
        recipe: Recipe,
    },
    DeleteRecipe {
        id: u32,
    },
    SetRecipeEnabled {
        id: u32,
        enabled: bool,
    },
}

/// History points per reply. `[1700000000,21000],` is 19 bytes, so 180 fit.
const MAX_POINTS: usize = 180;
fn max_points() -> usize {
    MAX_POINTS
}
/// Events read from the store per reply (fewer if they don't fit).
const EVENTS_PER_PAGE: usize = 12;
/// Room for list items once the envelope and other fields are in.
const PAGE_BUDGET: usize = MAX_APP_PLAINTEXT - 160;
/// A saved recipe must fit in a get_recipe reply.
const MAX_RECIPE_JSON: usize = MAX_APP_PLAINTEXT - 64;

/// As many items from `offset` as fit in one reply, and where the next page starts.
fn page<T: Serialize>(items: &[T], offset: usize) -> (Vec<&T>, Option<usize>) {
    let mut used = 0;
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate().skip(offset) {
        let len = serde_json::to_vec(item)
            .map(|v| v.len() + 1)
            .unwrap_or(usize::MAX);
        if used + len > PAGE_BUDGET && !out.is_empty() {
            return (out, Some(i));
        }
        used = used.saturating_add(len);
        out.push(item);
    }
    (out, None)
}

fn kind_label(kind: Option<u8>) -> &'static str {
    match kind {
        Some(0) => "digital_in",
        Some(1) => "digital_out",
        Some(2) => "analog_in",
        Some(3) => "temperature",
        _ => "unknown",
    }
}

impl Hub {
    /// Handles one frame from phone connection `conn`.
    pub fn on_app_frame(
        &mut self,
        frame: &[u8],
        conn: u64,
        mono: u64,
        unix: i64,
        store: &Store,
    ) -> Output {
        let mut out = Output::default();
        if let Err(e) = self.on_app_frame_inner(frame, Link::App(conn), mono, unix, store, &mut out)
        {
            debug!("dropped frame from phone connection {conn}: {e:#}");
        }
        out
    }

    fn on_app_frame_inner(
        &mut self,
        frame: &[u8],
        link: Link,
        mono: u64,
        unix: i64,
        store: &Store,
        out: &mut Output,
    ) -> Result<()> {
        if frame.is_empty() || frame.len() > uplink::MAX_TCP_FRAME {
            anyhow::bail!("bad length {}", frame.len());
        }
        // CRYPTO-REVIEW: only v2 frames are accepted on the phone port. v1
        // handshake and pairing frames fall through to "unexpected" (no downgrade).
        match FrameType::from_byte(frame[0]).map_err(|e| anyhow::anyhow!("{e}"))? {
            FrameType::PairRequest2 => {
                if !self.take_sig_budget(mono) {
                    anyhow::bail!("signature budget used up; pair request dropped");
                }
                self.on_phone_pair_request(frame, link, unix, out)
            }
            FrameType::Hello2 => self.on_phone_hello(frame, link, mono, out),
            FrameType::Uplink => {
                if frame.len() > MAX_APP_FRAME {
                    anyhow::bail!("request frame of {} bytes is too large", frame.len());
                }
                let phone_id = peek_node_id(frame).unwrap_or(0);
                let phone = self
                    .phones
                    .get_mut(&phone_id)
                    .ok_or_else(|| anyhow::anyhow!("frame from unknown phone {phone_id}"))?;
                // CRYPTO-REVIEW: same AEAD + replay checks as the PC uplink;
                // the first good frame of a new session confirms it.
                let v: Value = uplink::open_json(frame, |f, buf| {
                    phone.sessions.open(f, buf, mono).map(|(_, pt)| pt.len())
                })?;
                let id = v["id"].as_u64().unwrap_or(0);
                let result = match serde_json::from_value::<Request>(v["req"].clone()) {
                    Ok(req) => self.handle(phone_id, req, mono, unix, store, out),
                    Err(e) => Err(format!("bad request: {e}")),
                };
                let mut body = match result {
                    Ok(ok) => json!({ "id": id, "ok": ok }),
                    Err(err) => json!({ "id": id, "err": err }),
                };
                if serde_json::to_vec(&body)?.len() > MAX_APP_PLAINTEXT {
                    body = json!({ "id": id, "err": "reply too large" });
                }
                let session = self
                    .phones
                    .get_mut(&phone_id)
                    .and_then(|p| p.sessions.current_mut())
                    .ok_or_else(|| anyhow::anyhow!("phone {phone_id} has no session"))?;
                // If this session has no sends left the phone times out and
                // rekeys (it counts our replies too, so that's rare).
                let f = uplink::seal_json(session, &body, mono)?;
                out.frames.push((link, f));
                Ok(())
            }
            other => anyhow::bail!("unexpected {other:?} on the phone port"),
        }
    }

    /// CRYPTO-REVIEW: v2 pairing for phones. The phone's hybrid key is only
    /// pinned once a person approved its KeyId in the desktop app. (Phase 3
    /// adds the QR-ticket path, `pairing::verify_ticket`.)
    fn on_phone_pair_request(
        &mut self,
        frame: &[u8],
        link: Link,
        unix: i64,
        out: &mut Output,
    ) -> Result<()> {
        use crypto_sharecrate::crypto::v2::{identity, pairing as pairing2};
        let req = pairing2::parse_pair_request(frame)
            .map_err(|e| anyhow::anyhow!("pair request: {e}"))?;
        let kid = identity::key_id(&req.device_pub);
        if let Some((id, _)) = self.phones.iter().find(|(_, p)| p.key_id == kid) {
            // Already paired; our accept was probably lost. Send it again.
            out.frames.push((
                link,
                pairing2::pair_accept(&self.pq_identity, &req.device_pub, *id).to_vec(),
            ));
            return Ok(());
        }
        if let Some(id) = self.approved_phones.remove(&kid) {
            info!(
                "pairing phone {id} ({})",
                uplink::v2::fingerprint(&req.device_pub)
            );
            self.phones
                .insert(id, PhoneEntry::new(Box::new(req.device_pub)));
            self.save()?;
            out.frames.push((
                link,
                pairing2::pair_accept(&self.pq_identity, &req.device_pub, id).to_vec(),
            ));
            return Ok(());
        }
        self.forward_pair_request(
            kid,
            hex::encode(kid),
            uplink::v2::fingerprint_of_key_id(&kid),
            Transport::Wifi,
            DeviceKind::Phone,
            unix,
            out,
        );
        Ok(())
    }

    /// CRYPTO-REVIEW: HELLO2 from a paired phone, checked against its pinned
    /// hybrid key. Same rate limits as node HELLOs; the new session waits as
    /// pending until key confirmation.
    fn on_phone_hello(
        &mut self,
        frame: &[u8],
        link: Link,
        mono: u64,
        out: &mut Output,
    ) -> Result<()> {
        use crypto_sharecrate::crypto::v2::handshake as handshake2;
        let id = handshake2::hello_node_id(frame).map_err(|e| anyhow::anyhow!("{e}"))?;
        let phone = self
            .phones
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("HELLO2 from unknown phone {id}"))?;
        while phone
            .recent_hellos
            .front()
            .is_some_and(|t| mono.saturating_sub(*t) >= HELLO_WINDOW_SECS)
        {
            phone.recent_hellos.pop_front();
        }
        if phone.recent_hellos.len() >= HELLOS_PER_WINDOW {
            anyhow::bail!("too many HELLO2s from phone {id}");
        }
        if !self.take_sig_budget(mono) {
            anyhow::bail!("signature budget used up; HELLO2 dropped");
        }
        let mut e = [0u8; 4];
        uplink::getrandom(&mut e)?;
        let rand = uplink::v2::hs_random()?;
        let phone = self.phones.get_mut(&id).expect("checked above");
        let ack = phone
            .sessions
            .on_hello_v2(
                frame,
                &phone.public_key,
                &self.pq_identity,
                &rand,
                u32::from_le_bytes(e),
                mono,
            )
            .map_err(|e| anyhow::anyhow!("hello2 from phone {id}: {e}"))?;
        phone.recent_hellos.push_back(mono);
        out.frames.push((link, ack.to_vec()));
        Ok(())
    }

    fn handle(
        &mut self,
        phone_id: u16,
        req: Request,
        mono: u64,
        unix: i64,
        store: &Store,
        out: &mut Output,
    ) -> Result<Value, String> {
        let db = |e: anyhow::Error| format!("hub database: {e:#}");
        Ok(match req {
            Request::Info => json!({
                // The v2 key the phone pinned, not the nodes' v1 key.
                "hub_fingerprint": uplink::v2::fingerprint(self.pq_identity.public_key()),
                "phone_id": phone_id,
                "nodes": self.nodes.len(),
                "recipes": self.recipes.list().len(),
                "max_recipes": MAX_RECIPES,
                "time": unix,
            }),
            Request::Nodes { offset } => {
                let mut ids: Vec<u16> = self.nodes.keys().copied().collect();
                ids.sort_unstable();
                let items: Vec<Value> = ids
                    .iter()
                    .map(|id| {
                        json!({
                            "id": id,
                            "name": self.live.node_name(*id),
                            "last_seen": self.live.last_seen.get(id),
                        })
                    })
                    .collect();
                let (items, next) = page(&items, offset);
                json!({ "nodes": items, "next": next, "time": unix })
            }
            Request::Channels { offset } => {
                // Announced channels, plus any with readings but no announce yet.
                let mut all: BTreeMap<(u16, u8), Option<u8>> = BTreeMap::new();
                for (node, chans) in &self.live.channels {
                    for (c, k) in chans {
                        all.insert((*node, *c), Some(*k));
                    }
                }
                for key in self.live.values.keys() {
                    all.entry(*key).or_insert(None);
                }
                let items: Vec<Value> = all
                    .into_iter()
                    .filter(|((n, _), _)| self.nodes.contains_key(n))
                    .map(|((n, c), k)| {
                        let v = self.live.values.get(&(n, c));
                        json!({
                            "node": n,
                            "channel": c,
                            "kind": kind_label(k),
                            "output": k == Some(ChannelKind::DigitalOut as u8),
                            "name": self.live.channel_name(n, c),
                            "value": v.map(|v| v.0),
                            "ts": v.map(|v| v.1),
                        })
                    })
                    .collect();
                let (items, next) = page(&items, offset);
                json!({ "channels": items, "next": next, "time": unix })
            }
            Request::History {
                node,
                channel,
                since,
                points,
            } => {
                let since = since.max(unix - RETENTION_SECS);
                let pts = store
                    .history(node, channel, since, points.clamp(2, MAX_POINTS))
                    .map_err(db)?;
                json!({ "points": pts })
            }
            Request::Events { before } => {
                let rows = store.events(before, EVENTS_PER_PAGE).map_err(db)?;
                let items: Vec<Value> = rows
                    .iter()
                    .map(|(id, m)| json!({ "id": id, "event": m }))
                    .collect();
                let (items, more) = page(&items, 0);
                let next = if more.is_some() || rows.len() == EVENTS_PER_PAGE {
                    items.last().and_then(|v| v["id"].as_i64())
                } else {
                    None
                };
                json!({ "events": items, "next": next })
            }
            Request::Command {
                node,
                channel,
                value,
            } => {
                if self.live.is_output(node, channel) != Some(true) {
                    return Err(format!(
                        "node {node} channel {channel} isn't a known output"
                    ));
                }
                let src = format!("phone:{phone_id}");
                self.queue_command((node, channel, value), Some(src), mono, unix, out)?;
                let online = self.nodes.get(&node).is_some_and(|n| n.link.is_some());
                json!({ "queued": true, "node_online": online })
            }
            Request::PairRequests { offset } => {
                let list: Vec<&ShownPairRequest> = self
                    .live
                    .pair_requests
                    .iter()
                    .rev()
                    .filter(|r| unix - r.seen_at < SHOWN_PAIR_REQUEST_TTL)
                    .collect();
                let (items, next) = page(&list, offset);
                json!({ "requests": items, "next": next })
            }
            Request::ApproveNode { public_key, name } => {
                // CRYPTO-REVIEW: a phone can only approve a node that is
                // asking to pair right now, never a phone. The PC then runs
                // its normal approval (assigns the id, sends ApprovePair).
                if !self.live.pair_requests.iter().any(|r| {
                    r.public_key == public_key && unix - r.seen_at < SHOWN_PAIR_REQUEST_TTL
                }) {
                    return Err("that node isn't asking to pair (hold its button again)".into());
                }
                out.live.push(HubMsg::ApproveRequested {
                    public_key,
                    name: name
                        .map(|n| truncate(&n, MAX_LABEL))
                        .filter(|n| !n.is_empty()),
                    phone_id,
                    ts: unix,
                });
                json!({ "forwarded": true })
            }
            Request::Recipes { offset } => {
                let items: Vec<Value> = self
                    .recipes
                    .list()
                    .iter()
                    .map(|r| json!({ "id": r.id, "name": r.name, "enabled": r.enabled }))
                    .collect();
                let (items, next) = page(&items, offset);
                json!({ "recipes": items, "next": next })
            }
            Request::GetRecipe { id } => self.get_recipe(id)?,
            Request::SaveRecipe { recipe } => self.save_recipe(recipe, mono, unix, out)?,
            Request::DeleteRecipe { id } => self.delete_recipe(id, mono, unix, out)?,
            Request::SetRecipeEnabled { id, enabled } => {
                self.set_recipe_enabled(id, enabled, mono, unix, out)?
            }
        })
    }

    /// A recipe request from the PC daemon (the desktop app). The answer goes
    /// back as `HubMsg::RecipeReply`. The uplink carries 32 KB frames, so the
    /// list isn't paged like the phone's.
    pub fn on_pc_recipe(
        &mut self,
        op: RecipeOp,
        mono: u64,
        unix: i64,
        out: &mut Output,
    ) -> Result<Value, String> {
        match op {
            RecipeOp::List => Ok(json!({
                "recipes": self
                    .recipes
                    .list()
                    .iter()
                    .map(|r| json!({ "id": r.id, "name": r.name, "enabled": r.enabled }))
                    .collect::<Vec<_>>(),
                "max_recipes": MAX_RECIPES,
            })),
            RecipeOp::Get { id } => self.get_recipe(id),
            RecipeOp::Save { recipe } => {
                let recipe: Recipe =
                    serde_json::from_value(recipe).map_err(|e| format!("bad recipe: {e}"))?;
                self.save_recipe(recipe, mono, unix, out)
            }
            RecipeOp::Delete { id } => self.delete_recipe(id, mono, unix, out),
            RecipeOp::SetEnabled { id, enabled } => {
                self.set_recipe_enabled(id, enabled, mono, unix, out)
            }
        }
    }

    fn get_recipe(&self, id: u32) -> Result<Value, String> {
        Ok(json!({ "recipe": self.recipes.get(id).ok_or("no such recipe")? }))
    }

    fn save_recipe(
        &mut self,
        recipe: Recipe,
        mono: u64,
        unix: i64,
        out: &mut Output,
    ) -> Result<Value, String> {
        self.check_recipe(&recipe)?;
        let (id, off) = self.recipes.upsert(recipe)?;
        if let Some(f) = off {
            self.run(vec![f], mono, unix, out);
        }
        Ok(json!({ "id": id }))
    }

    fn delete_recipe(
        &mut self,
        id: u32,
        mono: u64,
        unix: i64,
        out: &mut Output,
    ) -> Result<Value, String> {
        if let Some(f) = self.recipes.remove(id)? {
            self.run(vec![f], mono, unix, out);
        }
        Ok(json!({ "deleted": id }))
    }

    fn set_recipe_enabled(
        &mut self,
        id: u32,
        enabled: bool,
        mono: u64,
        unix: i64,
        out: &mut Output,
    ) -> Result<Value, String> {
        if let Some(f) = self.recipes.set_enabled(id, enabled)? {
            self.run(vec![f], mono, unix, out);
        }
        Ok(json!({ "id": id, "enabled": enabled }))
    }

    /// Shape checks plus: every node it uses is paired, and every "set"
    /// targets a channel the node announced as an output.
    fn check_recipe(&self, r: &Recipe) -> Result<(), String> {
        r.validate()?;
        if serde_json::to_vec(r).map_or(usize::MAX, |v| v.len()) > MAX_RECIPE_JSON {
            return Err("recipe is too big".into());
        }
        let mut sensors = Vec::new();
        r.when.sensors(&mut sensors);
        for (n, _) in &sensors {
            if !self.nodes.contains_key(n) {
                return Err(format!("node {n} isn't paired"));
            }
        }
        for (n, c, _) in r.outputs() {
            if !self.nodes.contains_key(&n) {
                return Err(format!("node {n} isn't paired"));
            }
            if self.live.is_output(n, c) != Some(true) {
                return Err(format!("node {n} channel {c} isn't a known output"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto_sharecrate::crypto::v2::link::{LinkCore, LinkEvent};
    use crypto_sharecrate::node::NodeCore;
    use crypto_sharecrate::packet::{ChannelInfo, Reading};

    fn rand48() -> [u8; 48] {
        let mut r = [0u8; 48];
        uplink::getrandom(&mut r).unwrap();
        r
    }

    /// The test phone's connection id.
    fn phone_link() -> u64 {
        5
    }

    struct Rig {
        hub: Hub,
        store: Store,
        _dir: tempfile::TempDir,
        node: NodeCore,
        phone: LinkCore,
        req_id: u64,
    }

    /// A hub with node 1 (temperature on ch 0, siren on ch 1, on Wi-Fi) and
    /// phone 1, both paired, with sessions up.
    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("hub.db")).unwrap();
        let mut hub = Hub::new(&[1u8; 32], None).unwrap();
        let mut node = NodeCore::new(&[2u8; 32], None);
        let mut phone = LinkCore::new(&[3u8; 32], None);
        let udp = Link::Udp("127.0.0.1:9999".parse().unwrap());

        for (key, kind) in [
            (uplink::key_to_hex(&node.public_key()), DeviceKind::Node),
            (hex::encode(phone.key_id()), DeviceKind::Phone),
        ] {
            hub.on_pc(
                PcMsg::ApprovePair {
                    public_key: key,
                    node_id: 1,
                    kind,
                },
                0,
                0,
            );
        }
        let out = hub.on_frame(&node.pair_request(), udp, 0, 0);
        node.handle_frame(&out.frames[0].1, 0).unwrap();
        let out = hub.on_frame(&node.start_handshake(&rand48(), 0).unwrap(), udp, 0, 0);
        node.handle_frame(&out.frames[0].1, 0).unwrap();
        let announce = Packet::announce(&[
            ChannelInfo {
                channel: 0,
                kind: ChannelKind::OneWireTemp,
            },
            ChannelInfo {
                channel: 1,
                kind: ChannelKind::DigitalOut,
            },
        ]);
        hub.on_frame(&node.seal(&announce, 0).unwrap(), udp, 0, 0);

        let out = hub.on_app_frame(&phone.pair_request(None), phone_link(), 0, 0, &store);
        assert!(matches!(
            phone.handle_frame(&out.frames[0].1, 0).unwrap(),
            LinkEvent::Paired(_)
        ));
        Rig {
            hub,
            store,
            _dir: dir,
            node,
            phone,
            req_id: 0,
        }
    }

    impl Rig {
        /// Sends one request from the phone (handshaking first if needed)
        /// and returns the reply plus everything else the hub produced.
        fn ask(&mut self, req: Value) -> (Value, Output) {
            if self.phone.needs_handshake(1) {
                let hello = self
                    .phone
                    .start_handshake(&uplink::v2::hs_random().unwrap(), 1)
                    .unwrap();
                let out = self
                    .hub
                    .on_app_frame(&hello, phone_link(), 1, 0, &self.store);
                self.phone.handle_frame(&out.frames[0].1, 1).unwrap();
            }
            self.req_id += 1;
            let s = self.phone.session_mut().unwrap();
            let f = uplink::seal_json(s, &json!({ "id": self.req_id, "req": req }), 1).unwrap();
            let mut out = self.hub.on_app_frame(&f, phone_link(), 1, 100, &self.store);
            let (link, reply) = out
                .frames
                .iter()
                .position(|(l, _)| matches!(l, Link::App(_)))
                .map(|i| out.frames.remove(i))
                .expect("a reply");
            assert_eq!(link, Link::App(phone_link()));
            let s = self.phone.session_mut().unwrap();
            let v: Value =
                uplink::open_json(&reply, |f, b| s.open(f, b, 1).map(|(_, pt)| pt.len())).unwrap();
            assert_eq!(v["id"], self.req_id);
            (v, out)
        }

        fn reading(&mut self, value: i32) -> Output {
            let f = self
                .node
                .seal(&Packet::data(&[Reading { channel: 0, value }]), 1)
                .unwrap();
            self.hub
                .on_frame(&f, Link::Udp("127.0.0.1:9999".parse().unwrap()), 1, 100)
        }
    }

    #[test]
    fn phone_reads_channels_and_sets_an_output() {
        let mut r = rig();
        r.reading(21_500);
        let (v, _) = r.ask(json!({ "type": "channels" }));
        let chans = v["ok"]["channels"].as_array().unwrap();
        assert_eq!(chans.len(), 2);
        assert_eq!(chans[0]["value"], 21_500);
        assert_eq!(chans[1]["output"], true);

        let (v, _) = r.ask(json!({ "type": "command", "node": 1, "channel": 0, "value": 1 }));
        assert!(
            v["err"].as_str().unwrap().contains("output"),
            "sensors can't be set"
        );

        let (v, out) = r.ask(json!({ "type": "command", "node": 1, "channel": 1, "value": 1 }));
        assert_eq!(v["ok"]["queued"], true);
        assert!(matches!(
            &out.to_pc[0],
            HubMsg::CommandRequested { source, .. } if source == "phone:1"
        ));
        assert!(
            out.frames.iter().any(|(l, _)| matches!(l, Link::Udp(_))),
            "Wi-Fi node gets the command right away"
        );
    }

    #[test]
    fn info_reports_the_v2_key_the_phone_pinned() {
        let mut r = rig();
        let (v, _) = r.ask(json!({ "type": "info" }));
        let pinned = r.phone.pairing().unwrap().peer_pub;
        assert_eq!(v["ok"]["hub_fingerprint"], uplink::v2::fingerprint(&pinned));
    }

    #[test]
    fn many_requests_rekey_and_keep_working() {
        let mut r = rig();
        // v2 sessions allow 1000 frames per direction, so request 1001 rekeys.
        for _ in 0..1001 {
            let (v, _) = r.ask(json!({ "type": "info" }));
            assert_eq!(v["ok"]["phone_id"], 1);
        }
        assert!(
            r.phone.sessions_started() >= 2,
            "new keys every 1000 messages"
        );
    }

    #[test]
    fn recipe_saved_from_phone_runs_on_the_hub() {
        let mut r = rig();
        let recipe = json!({
            "name": "Too hot",
            "when": { "type": "compare", "node": 1, "channel": 0, "op": "gt", "value": 30000 },
            "then": [ { "type": "set", "node": 1, "channel": 1, "value": 1 },
                      { "type": "alert", "text": "Hot!" } ],
            "otherwise": [ { "type": "set", "node": 1, "channel": 1, "value": 0 } ]
        });
        let (v, _) = r.ask(json!({ "type": "save_recipe", "recipe": recipe }));
        let id = v["ok"]["id"].as_u64().expect("saved");

        let out = r.reading(31_000);
        assert!(out.to_pc.iter().any(|m| matches!(
            m,
            HubMsg::CommandRequested { channel: 1, value: 1, source, .. } if *source == format!("recipe:{id}")
        )));
        assert!(
            out.to_pc
                .iter()
                .any(|m| matches!(m, HubMsg::RecipeAlert { text, .. } if text == "Hot!"))
        );

        // Disabling it while it's on runs its "otherwise" (siren off).
        let (_, out) = r.ask(json!({ "type": "set_recipe_enabled", "id": id, "enabled": false }));
        assert!(out.to_pc.iter().any(|m| matches!(
            m,
            HubMsg::CommandRequested {
                channel: 1,
                value: 0,
                ..
            }
        )));

        let (v, _) = r.ask(json!({ "type": "get_recipe", "id": id }));
        assert_eq!(v["ok"]["recipe"]["enabled"], false);
    }

    #[test]
    fn desktop_and_phone_edit_the_same_recipes() {
        let mut r = rig();
        // Sends a recipe request as the PC daemon does and returns the hub's reply.
        let pc = |hub: &mut Hub, req: u32, op: RecipeOp| {
            let out = hub.on_pc(PcMsg::Recipe { req, op }, 1, 100);
            match out.live.as_slice() {
                [HubMsg::RecipeReply { req: got, ok, err }] => {
                    assert_eq!(*got, req);
                    (ok.clone(), err.clone())
                }
                other => panic!("expected one reply, got {other:?}"),
            }
        };

        let recipe = json!({
            "name": "Desk",
            "when": { "type": "compare", "node": 1, "channel": 0, "op": "gt", "value": 30000 },
            "then": [ { "type": "set", "node": 1, "channel": 1, "value": 1 } ]
        });
        let (ok, err) = pc(&mut r.hub, 1, RecipeOp::Save { recipe });
        assert_eq!(err, None);
        let id = ok.unwrap()["id"].as_u64().unwrap();

        // The phone sees it.
        let (v, _) = r.ask(json!({ "type": "recipes" }));
        assert_eq!(v["ok"]["recipes"][0]["name"], "Desk");

        // A recipe the phone saves shows up in the desktop's list.
        let phone_recipe = json!({
            "name": "Phone",
            "when": { "type": "compare", "node": 1, "channel": 0, "op": "lt", "value": 5000 },
            "then": [ { "type": "alert", "text": "Cold" } ]
        });
        r.ask(json!({ "type": "save_recipe", "recipe": phone_recipe }));
        let (ok, _) = pc(&mut r.hub, 2, RecipeOp::List);
        let names: Vec<_> = ok.unwrap()["recipes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(names, ["Desk", "Phone"]);

        // Same checks as the phone: a "set" on a sensor channel is refused.
        let bad = json!({
            "name": "bad",
            "when": { "type": "compare", "node": 1, "channel": 0, "op": "gt", "value": 1 },
            "then": [ { "type": "set", "node": 1, "channel": 0, "value": 1 } ]
        });
        let (ok, err) = pc(&mut r.hub, 3, RecipeOp::Save { recipe: bad });
        assert!(ok.is_none() && err.is_some());
        let (_, err) = pc(
            &mut r.hub,
            4,
            RecipeOp::Save {
                recipe: json!({ "name": 5 }),
            },
        );
        assert!(err.unwrap().starts_with("bad recipe"));

        let (ok, _) = pc(
            &mut r.hub,
            5,
            RecipeOp::SetEnabled {
                id: id as u32,
                enabled: false,
            },
        );
        assert_eq!(ok.unwrap()["enabled"], false);
        let (ok, _) = pc(&mut r.hub, 6, RecipeOp::Get { id: id as u32 });
        assert_eq!(ok.unwrap()["recipe"]["enabled"], false);
        let (ok, _) = pc(&mut r.hub, 7, RecipeOp::Delete { id: id as u32 });
        assert_eq!(ok.unwrap()["deleted"], id);
        let (_, err) = pc(&mut r.hub, 8, RecipeOp::Get { id: id as u32 });
        assert_eq!(err.as_deref(), Some("no such recipe"));
    }

    #[test]
    fn recipes_with_bad_targets_are_refused() {
        let mut r = rig();
        for (when_node, set_ch) in [(9, 1), (1, 0)] {
            let recipe = json!({
                "name": "bad",
                "when": { "type": "compare", "node": when_node, "channel": 0, "op": "gt", "value": 1 },
                "then": [ { "type": "set", "node": 1, "channel": set_ch, "value": 1 } ]
            });
            let (v, _) = r.ask(json!({ "type": "save_recipe", "recipe": recipe }));
            assert!(v["err"].is_string(), "{v}");
        }
        let (v, _) = r.ask(json!({ "type": "recipes" }));
        assert!(v["ok"]["recipes"].as_array().unwrap().is_empty());
    }

    #[test]
    fn phone_approves_only_nodes_that_are_asking() {
        let mut r = rig();
        let waiting = NodeCore::new(&[7u8; 32], None);
        let key = uplink::key_to_hex(&waiting.public_key());
        let (v, out) = r.ask(json!({ "type": "approve_node", "public_key": key }));
        assert!(v["err"].is_string());
        assert!(out.live.is_empty());

        r.hub.on_frame(
            &waiting.pair_request(),
            Link::Udp("127.0.0.1:9998".parse().unwrap()),
            1,
            100,
        );
        let (v, _) = r.ask(json!({ "type": "pair_requests" }));
        assert_eq!(v["ok"]["requests"][0]["public_key"], key);
        let (v, out) = r.ask(json!({ "type": "approve_node", "public_key": key, "name": "Barn" }));
        assert_eq!(v["ok"]["forwarded"], true);
        assert!(matches!(
            &out.live[0],
            HubMsg::ApproveRequested { phone_id: 1, name: Some(n), .. } if n == "Barn"
        ));

        // A phone asking to pair is never listed for other phones to approve.
        let other_phone = LinkCore::new(&[8u8; 32], None);
        r.hub.on_app_frame(
            &other_phone.pair_request(None),
            phone_link(),
            1,
            101,
            &r.store,
        );
        let (v, _) = r.ask(json!({ "type": "pair_requests" }));
        assert_eq!(v["ok"]["requests"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn node_frames_on_the_phone_port_and_unknown_phones_are_dropped() {
        let mut r = rig();
        // a node key isn't a phone, even with a valid HELLO
        let hello = r.node.start_handshake(&rand48(), 1).unwrap();
        assert!(
            r.hub
                .on_app_frame(&hello, phone_link(), 1, 0, &r.store)
                .frames
                .is_empty()
        );
        // unpaired phone: its pair request goes to the PC as a phone, nothing comes back
        let stranger = LinkCore::new(&[9u8; 32], None);
        let out = r
            .hub
            .on_app_frame(&stranger.pair_request(None), phone_link(), 2, 200, &r.store);
        assert!(out.frames.is_empty());
        assert!(matches!(
            out.live[0],
            HubMsg::PairRequest {
                kind: DeviceKind::Phone,
                ..
            }
        ));
    }

    #[test]
    fn long_lists_are_paged() {
        let items: Vec<String> = (0..600).map(|i| format!("item number {i:04}")).collect();
        let (first, next) = page(&items, 0);
        let next = next.expect("more pages");
        assert_eq!(first.len(), next);
        let (_, after) = page(&items, next);
        assert!(after.is_none_or(|n| n > next));
        assert!(serde_json::to_vec(&first).unwrap().len() <= PAGE_BUDGET + 2);
    }
}

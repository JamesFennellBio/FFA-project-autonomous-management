//! The hub's node-facing logic: pairing, handshakes, and turning frames into
//! messages for the PC. No I/O here: frames come in, frames to send and
//! messages for the PC go out. That keeps it easy to test.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Result;
use crypto_sharecrate::crypto::identity::Identity;
use crypto_sharecrate::crypto::{handshake, pairing, responder::PeerSessions};
use crypto_sharecrate::frame::{FRAME_LEN, FrameType, peek_node_id};
use crypto_sharecrate::packet::Packet;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use uplink::{HubMsg, PcMsg, Transport};

/// Where to send a reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Udp(SocketAddr),
    #[cfg_attr(not(feature = "lora"), allow(dead_code))]
    Lora,
}

impl Link {
    pub fn transport(self) -> Transport {
        match self {
            Link::Udp(_) => Transport::Wifi,
            Link::Lora => Transport::Lora,
        }
    }
}

/// Only forward the same node's pairing request to the PC this often.
const PAIR_FORWARD_EVERY: i64 = 10;
/// Max commands queued per node while it's out of reach.
const MAX_QUEUED_COMMANDS: usize = 16;

struct NodeEntry {
    /// CRYPTO-REVIEW: pinned Ed25519 key from pairing.
    public_key: [u8; 32],
    sessions: PeerSessions,
    link: Option<Link>,
    commands: VecDeque<(u8, i32)>,
}

/// Saved in `paired.json` as soon as anything changes. Pairings are too
/// important to wait for the daily flush.
#[derive(Serialize, Deserialize, Default)]
struct PairedFile {
    nodes: Vec<(u16, String)>,
    approved: Vec<(u16, String)>,
}

pub struct Hub {
    identity: Identity,
    nodes: HashMap<u16, NodeEntry>,
    /// CRYPTO-REVIEW: keys a person approved in the app, waiting for the node's next PAIR_REQUEST.
    approved: HashMap<[u8; 32], u16>,
    last_pair_forward: HashMap<[u8; 32], i64>,
    paired_path: Option<PathBuf>,
}

#[derive(Default, Debug)]
pub struct Output {
    pub frames: Vec<(Link, Vec<u8>)>,
    pub to_pc: Vec<HubMsg>,
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

impl Hub {
    pub fn new(seed: &[u8; 32], paired_path: Option<PathBuf>) -> Result<Self> {
        let mut hub = Self {
            identity: Identity::from_seed(seed),
            nodes: HashMap::new(),
            approved: HashMap::new(),
            last_pair_forward: HashMap::new(),
            paired_path,
        };
        if let Some(p) = &hub.paired_path
            && let Ok(s) = std::fs::read_to_string(p)
        {
            let f: PairedFile = serde_json::from_str(&s)?;
            for (id, k) in f.nodes {
                hub.insert_node(id, uplink::key_from_hex(&k)?);
            }
            for (id, k) in f.approved {
                hub.approved.insert(uplink::key_from_hex(&k)?, id);
            }
        }
        info!("hub has {} paired node(s)", hub.nodes.len());
        Ok(hub)
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.identity.public_key()
    }

    fn insert_node(&mut self, id: u16, public_key: [u8; 32]) {
        self.nodes.insert(
            id,
            NodeEntry {
                public_key,
                sessions: PeerSessions::new(),
                link: None,
                commands: VecDeque::new(),
            },
        );
    }

    fn save(&self) -> Result<()> {
        let Some(p) = &self.paired_path else {
            return Ok(());
        };
        let f = PairedFile {
            nodes: self
                .nodes
                .iter()
                .map(|(id, n)| (*id, uplink::key_to_hex(&n.public_key)))
                .collect(),
            approved: self
                .approved
                .iter()
                .map(|(k, id)| (*id, uplink::key_to_hex(k)))
                .collect(),
        };
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&f)?)?;
        std::fs::rename(tmp, p)?;
        Ok(())
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
            FrameType::PairRequest => self.on_pair_request(frame, link, unix, out)?,
            FrameType::Hello => {
                let node_id =
                    handshake::hello_node_id(frame).map_err(|e| anyhow::anyhow!("{e}"))?;
                let identity = &self.identity;
                let node = self
                    .nodes
                    .get_mut(&node_id)
                    .ok_or_else(|| anyhow::anyhow!("HELLO from unknown node {node_id}"))?;
                let (rand, epoch) = hs_random()?;
                // CRYPTO-REVIEW: verifies the node's signature with its pinned key;
                // the new session waits as pending until key confirmation.
                let ack = node
                    .sessions
                    .on_hello(frame, &node.public_key, identity, &rand, epoch, mono)
                    .map_err(|e| anyhow::anyhow!("hello from node {node_id}: {e}"))?;
                out.frames.push((link, ack.to_vec()));
            }
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
            }
            other => anyhow::bail!("unexpected {other:?}"),
        }
        Ok(())
    }

    /// CRYPTO-REVIEW: hub side of node pairing; see crypto::pairing for the trust model.
    fn on_pair_request(
        &mut self,
        frame: &[u8],
        link: Link,
        unix: i64,
        out: &mut Output,
    ) -> Result<()> {
        let pk =
            pairing::parse_pair_request(frame).map_err(|e| anyhow::anyhow!("pair request: {e}"))?;
        if let Some((id, _)) = self.nodes.iter().find(|(_, n)| n.public_key == pk) {
            // Already paired; our accept was probably lost. Send it again.
            out.frames.push((
                link,
                pairing::pair_accept(&self.identity, &pk, *id).to_vec(),
            ));
            return Ok(());
        }
        if let Some(id) = self.approved.remove(&pk) {
            info!("pairing node {id} ({})", uplink::fingerprint_hex(&pk));
            self.insert_node(id, pk);
            self.save()?;
            out.frames
                .push((link, pairing::pair_accept(&self.identity, &pk, id).to_vec()));
            return Ok(());
        }
        let due = self
            .last_pair_forward
            .get(&pk)
            .is_none_or(|last| unix.saturating_sub(*last) >= PAIR_FORWARD_EVERY);
        if due {
            self.last_pair_forward.insert(pk, unix);
            out.to_pc.push(HubMsg::PairRequest {
                public_key: uplink::key_to_hex(&pk),
                fingerprint: uplink::fingerprint_hex(&pk),
                transport: link.transport(),
                ts: unix,
            });
        }
        Ok(())
    }

    /// Handles a message from the PC daemon.
    pub fn on_pc(&mut self, msg: PcMsg, mono: u64, unix: i64) -> Output {
        let mut out = Output::default();
        match msg {
            PcMsg::Ack { .. } => {} // handled by pc_link
            PcMsg::ApprovePair {
                public_key,
                node_id,
            } => match uplink::key_from_hex(&public_key) {
                Ok(pk) => {
                    self.approved.insert(pk, node_id);
                    if let Err(e) = self.save() {
                        warn!("saving pairings: {e:#}");
                    }
                    info!("node {node_id} approved; waiting for its next pair request");
                }
                Err(e) => warn!("bad approve key: {e:#}"),
            },
            PcMsg::Unpair { node_id } => {
                self.nodes.remove(&node_id);
                if let Err(e) = self.save() {
                    warn!("saving pairings: {e:#}");
                }
            }
            PcMsg::Command {
                node_id,
                channel,
                value,
            } => {
                let Some(node) = self.nodes.get_mut(&node_id) else {
                    warn!("command for unknown node {node_id}");
                    return out;
                };
                // A newer command for the same channel replaces an older one still queued.
                node.commands.retain(|(c, _)| *c != channel);
                if node.commands.len() >= MAX_QUEUED_COMMANDS {
                    node.commands.pop_front();
                }
                node.commands.push_back((channel, value));
                // Wi-Fi nodes listen all the time, so send now. LoRa nodes get
                // it in the receive window after their next uplink.
                if matches!(node.link, Some(Link::Udp(_))) {
                    deliver(node_id, node, mono, unix, &mut out);
                }
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
        Packet::Ack { .. } | Packet::Command { .. } => {}
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
        let HubMsg::PairRequest { public_key, .. } = &out.to_pc[0] else {
            panic!()
        };

        // 2. a person approves it in the app, the node asks again and gets accepted
        hub.on_pc(
            PcMsg::ApprovePair {
                public_key: public_key.clone(),
                node_id: 4,
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
}

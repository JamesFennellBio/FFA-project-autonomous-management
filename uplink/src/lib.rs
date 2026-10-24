//! Messages between the Pi hub and the PC daemon, and how they go over TCP.
//!
//! On the wire every TCP frame is `u16 big-endian length | frame bytes`:
//! - pairing and handshake frames: the same signed 128-byte frames the radio uses
//! - after that: `FrameType::Uplink` frames sealed with ChaCha20-Poly1305, whose
//!   plaintext is a JSON [`HubBatch`] (hub -> PC) or [`PcBatch`] (PC -> hub)
//!
//! The hub is the initiator (it plays the "node" role toward the PC), so the
//! same rekey rule applies: a new ECDH every 10 frames or 1 hour.

use anyhow::{Context, Result, bail};
use crypto_sharecrate::crypto::session::Session;
use crypto_sharecrate::frame::{FrameType, HEADER_LEN, TAG_LEN};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Largest TCP frame we accept (guards against a huge length prefix).
pub const MAX_TCP_FRAME: usize = 32 * 1024;
/// Max messages the hub packs into one encrypted uplink frame.
pub const MAX_BATCH: usize = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Wifi,
    Lora,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HubMsg {
    Reading {
        node_id: u16,
        channel: u8,
        value: i32,
        ts: i64,
    },
    /// The node's channel list (sent after it boots).
    Announce {
        node_id: u16,
        /// (channel, kind) with kind = `ChannelKind as u8`
        channels: Vec<(u8, u8)>,
        ts: i64,
    },
    Alert {
        node_id: u16,
        channel: u8,
        code: u8,
        value: i32,
        ts: i64,
    },
    Message {
        node_id: u16,
        text: String,
        ts: i64,
    },
    /// An unpaired node is asking to pair (its button is held). Shown in the app.
    PairRequest {
        public_key: String,
        fingerprint: String,
        transport: Transport,
        ts: i64,
    },
    /// The node finished pairing on its side (it got our PAIR_ACCEPT and sent a HELLO).
    NodeOnline {
        node_id: u16,
        transport: Transport,
        epoch: u32,
        ts: i64,
    },
    /// The hub transmitted a command (not proof the node got it).
    CommandSent {
        node_id: u16,
        channel: u8,
        value: i32,
        ts: i64,
    },
    /// The node confirmed it applied the command (its encrypted Ack).
    CommandAcked {
        node_id: u16,
        channel: u8,
        value: i32,
        ts: i64,
    },
    /// The hub's per-node queue was full and this command was thrown away.
    CommandDropped {
        node_id: u16,
        channel: u8,
        value: i32,
        ts: i64,
    },
    /// The hub refused a pairing approval (e.g. the node id is already pinned
    /// to a different key).
    ApproveFailed {
        public_key: String,
        node_id: u16,
        reason: String,
        ts: i64,
    },
    Heartbeat {
        ts: i64,
        /// Messages stored on the hub that the PC hasn't acknowledged yet.
        backlog: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PcMsg {
    /// The PC has stored everything up to and including this hub row id.
    Ack {
        upto: i64,
    },
    Command {
        node_id: u16,
        channel: u8,
        value: i32,
    },
    /// A person approved this node in the app. The hub sends PAIR_ACCEPT next
    /// time the node asks.
    ApprovePair {
        public_key: String,
        node_id: u16,
    },
    Unpair {
        node_id: u16,
    },
}

/// One encrypted hub -> PC frame. Each message carries the hub's row id so
/// the PC can ack them and the hub can resend after a disconnect.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubBatch {
    pub msgs: Vec<(i64, HubMsg)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcBatch {
    pub msgs: Vec<PcMsg>,
}

pub async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, frame: &[u8]) -> Result<()> {
    if frame.len() > MAX_TCP_FRAME {
        bail!("frame too large");
    }
    w.write_u16(frame.len() as u16).await?;
    w.write_all(frame).await?;
    w.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncReadExt + Unpin>(r: &mut R) -> Result<Vec<u8>> {
    let len = r.read_u16().await? as usize;
    if len == 0 || len > MAX_TCP_FRAME {
        bail!("bad frame length {len}");
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf).await?;
    Ok(buf)
}

/// CRYPTO-REVIEW: seal a JSON value as an Uplink frame under the session.
pub fn seal_json<T: Serialize>(session: &mut Session, value: &T, now: u64) -> Result<Vec<u8>> {
    let pt = serde_json::to_vec(value)?;
    let mut out = vec![0u8; HEADER_LEN + pt.len() + TAG_LEN];
    let n = session
        .seal(FrameType::Uplink, &pt, &mut out, now)
        .map_err(|e| anyhow::anyhow!("seal: {e}"))?;
    out.truncate(n);
    Ok(out)
}

/// CRYPTO-REVIEW: open an Uplink frame. `open` is either `Session::open` or
/// `PeerSessions::open`.
pub fn open_json<T: for<'de> Deserialize<'de>>(
    frame: &[u8],
    open: impl FnOnce(&[u8], &mut [u8]) -> crypto_sharecrate::Result<usize>,
) -> Result<T> {
    let mut buf = vec![0u8; frame.len()];
    let n = open(frame, &mut buf).map_err(|e| anyhow::anyhow!("open: {e}"))?;
    serde_json::from_slice(&buf[..n]).context("uplink json")
}

/// Hex helpers for public keys in JSON and the database.
pub fn key_to_hex(k: &[u8; 32]) -> String {
    hex::encode(k)
}

pub fn key_from_hex(s: &str) -> Result<[u8; 32]> {
    let v = hex::decode(s)?;
    v.try_into()
        .map_err(|_| anyhow::anyhow!("key must be 32 bytes"))
}

pub fn fingerprint_hex(k: &[u8; 32]) -> String {
    let fp = crypto_sharecrate::crypto::identity::fingerprint_str(k);
    String::from_utf8_lossy(&fp).into_owned()
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Monotonic seconds since the process started (for session ages, so a
/// clock change from NTP can't make a session look younger).
pub fn mono_now() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs()
}

/// Reads or creates a 32-byte secret file with 0600 permissions.
///
/// CRYPTO-REVIEW: used for the PC daemon's identity seed. The OS RNG fills
/// new seeds.
pub fn load_or_create_secret(path: &std::path::Path) -> Result<[u8; 32]> {
    if let Ok(s) = std::fs::read_to_string(path) {
        return key_from_hex(s.trim());
    }
    let mut seed = [0u8; 32];
    getrandom(&mut seed)?;
    write_secret(path, &hex::encode(seed))?;
    Ok(seed)
}

pub fn write_secret(path: &std::path::Path, contents: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(contents.as_bytes())?;
    }
    #[cfg(not(unix))]
    std::fs::write(path, contents)?;
    Ok(())
}

/// CRYPTO-REVIEW: OS CSPRNG (getrandom / /dev/urandom / BCryptGenRandom).
pub fn getrandom(buf: &mut [u8]) -> Result<()> {
    use rand_core::{OsRng, RngCore};
    OsRng
        .try_fill_bytes(buf)
        .map_err(|e| anyhow::anyhow!("os rng: {e}"))
}

pub use rand_core;

//! The phone app's link to the hub, in Rust so the phone runs the exact same
//! protocol and crypto code (crypto-sharecrate) as the nodes, hub and PC.
//! The React Native app calls it through uniffi (see mobile/native).
//!
//! The JavaScript side only ever sees JSON requests and replies. Everything
//! between (pairing, the signed handshake, ChaCha20-Poly1305, rekeying every
//! 10 messages or 1 hour, retries over UDP) happens here.
//!
//! ```text
//! JS:   client.request('{"type":"channels"}')  ->  '{"channels":[...],"next":null,...}'
//! Rust: HELLO/HELLO-ACK when needed -> seal {"id":n,"req":...} -> UDP -> open reply
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use crypto_sharecrate::crypto::identity::Identity;
use crypto_sharecrate::frame::FrameType;
use crypto_sharecrate::node::{NodeCore, NodeEvent, Pairing};
use serde_json::{Value, json};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
use uplink::app::{MAX_APP_FRAME, MAX_APP_PLAINTEXT};

uniffi::setup_scaffolding!();

/// How long to wait for each reply before sending again.
const REPLY_TIMEOUT: Duration = Duration::from_millis(2500);
/// Sends per request (or per handshake) before giving up.
const TRIES: usize = 3;
/// How often a pairing request is repeated while waiting for approval.
const PAIR_RETRY: Duration = Duration::from_secs(3);

#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum MobileError {
    #[error("{0}")]
    Invalid(String),
    #[error("can't reach the hub: {0}")]
    Network(String),
    #[error("the hub didn't answer (is the phone on the farm Wi-Fi?)")]
    Timeout,
    #[error("this phone isn't paired with a hub yet")]
    NotPaired,
    /// The hub understood the request and said no (the message says why).
    #[error("{0}")]
    Hub(String),
    #[error("pairing failed: {0}")]
    Pairing(String),
}

fn invalid(e: impl std::fmt::Display) -> MobileError {
    MobileError::Invalid(e.to_string())
}

fn net(e: impl std::fmt::Display) -> MobileError {
    MobileError::Network(e.to_string())
}

/// What the app saves after pairing.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PhonePairing {
    pub phone_id: u16,
    /// CRYPTO-REVIEW: the hub's Ed25519 key (hex), pinned from now on.
    pub hub_public_key: String,
}

fn seed_from_hex(seed_hex: &str) -> Result<[u8; 32], MobileError> {
    uplink::key_from_hex(seed_hex.trim())
        .map_err(|_| invalid("identity seed must be 64 hex characters"))
}

/// Makes a new identity for this phone (64 hex characters). The app keeps it
/// in the phone's secure storage (iOS Keychain / Android Keystore).
///
/// CRYPTO-REVIEW: 32 bytes from the phone's OS CSPRNG. The seed passes
/// through JavaScript once, to be saved; it's the phone's long-term secret.
#[uniffi::export]
pub fn new_identity_seed() -> Result<String, MobileError> {
    let mut seed = [0u8; 32];
    uplink::getrandom(&mut seed).map_err(invalid)?;
    Ok(uplink::key_to_hex(&seed))
}

/// The fingerprint the desktop app shows for this phone's pairing request.
#[uniffi::export]
pub fn fingerprint_of_seed(seed_hex: String) -> Result<String, MobileError> {
    let id = Identity::from_seed(&seed_from_hex(&seed_hex)?);
    Ok(uplink::fingerprint_hex(&id.public_key()))
}

/// Fingerprint of a public key (hex), e.g. the hub's after pairing.
#[uniffi::export]
pub fn fingerprint_of_key(public_key_hex: String) -> Result<String, MobileError> {
    let k = uplink::key_from_hex(public_key_hex.trim()).map_err(invalid)?;
    Ok(uplink::fingerprint_hex(&k))
}

struct Inner {
    core: NodeCore,
    sock: Option<UdpSocket>,
    next_id: u64,
}

/// One connection to one hub. Requests are sent one at a time (callers that
/// overlap just wait their turn).
#[derive(uniffi::Object)]
pub struct HubClient {
    hub_addr: String,
    start: Instant,
    inner: Mutex<Inner>,
}

impl HubClient {
    fn now(&self) -> u64 {
        self.start.elapsed().as_secs()
    }
}

#[uniffi::export(async_runtime = "tokio")]
impl HubClient {
    /// `hub_addr` is "ip:port", e.g. "192.168.1.20:7301". `pairing` is what
    /// [`HubClient::pair`] returned last time (None before the first pairing).
    ///
    /// CRYPTO-REVIEW: `expected_hub_key` (hex), if given, is the only hub
    /// key pairing will accept, like FFA_HUB_KEY on the nodes. Without it the
    /// phone trusts the first hub that answers (check the fingerprint).
    #[uniffi::constructor]
    pub fn new(
        seed_hex: String,
        hub_addr: String,
        pairing: Option<PhonePairing>,
        expected_hub_key: Option<String>,
    ) -> Result<Arc<Self>, MobileError> {
        let seed = seed_from_hex(&seed_hex)?;
        let pairing = match pairing {
            Some(p) => Some(Pairing {
                node_id: p.phone_id,
                peer_pub: uplink::key_from_hex(&p.hub_public_key).map_err(invalid)?,
            }),
            None => None,
        };
        let mut core = NodeCore::new(&seed, pairing);
        if let Some(k) = expected_hub_key.filter(|k| !k.trim().is_empty()) {
            core = core.with_expected_peer(uplink::key_from_hex(k.trim()).map_err(invalid)?);
        }
        Ok(Arc::new(Self {
            hub_addr,
            start: Instant::now(),
            inner: Mutex::new(Inner {
                core,
                sock: None,
                next_id: 1,
            }),
        }))
    }

    pub async fn fingerprint(&self) -> String {
        let g = self.inner.lock().await;
        String::from_utf8_lossy(&g.core.fingerprint()).into_owned()
    }

    pub async fn pairing(&self) -> Option<PhonePairing> {
        let g = self.inner.lock().await;
        g.core.pairing().map(|p| PhonePairing {
            phone_id: p.node_id,
            hub_public_key: uplink::key_to_hex(&p.peer_pub),
        })
    }

    /// Asks the hub to pair, repeating until someone approves this phone in
    /// the desktop app or `timeout_secs` runs out.
    pub async fn pair(&self, timeout_secs: u32) -> Result<PhonePairing, MobileError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs.into());
        let mut guard = self.inner.lock().await;
        let g = &mut *guard;
        if let Some(p) = g.core.pairing() {
            return Ok(PhonePairing {
                phone_id: p.node_id,
                hub_public_key: uplink::key_to_hex(&p.peer_pub),
            });
        }
        let req = g.core.pair_request();
        self.ensure_socket(g).await?;
        let sock = g.sock.as_ref().expect("just made");
        let mut buf = vec![0u8; MAX_APP_FRAME + 1];
        while tokio::time::Instant::now() < deadline {
            sock.send(&req).await.map_err(net)?;
            let wait = (tokio::time::Instant::now() + PAIR_RETRY).min(deadline);
            while let Ok(r) = tokio::time::timeout_at(wait, sock.recv(&mut buf)).await {
                let n = r.map_err(net)?;
                if buf.first() != Some(&(FrameType::PairAccept as u8)) {
                    continue;
                }
                match g.core.handle_frame(&buf[..n], self.now()) {
                    Ok(NodeEvent::Paired(p)) => {
                        return Ok(PhonePairing {
                            phone_id: p.node_id,
                            hub_public_key: uplink::key_to_hex(&p.peer_pub),
                        });
                    }
                    Err(crypto_sharecrate::Error::WrongPeer) => {
                        return Err(MobileError::Pairing(
                            "the hub that answered isn't the one whose key you entered".into(),
                        ));
                    }
                    // Not for us, or forged: keep waiting.
                    _ => {}
                }
            }
        }
        Err(MobileError::Pairing(
            "not approved in time; approve this phone on the desktop app's Pairing page".into(),
        ))
    }

    /// Sends one request (JSON, e.g. `{"type":"channels","offset":0}`) and
    /// returns the hub's answer as JSON. A refusal from the hub comes back as
    /// `MobileError::Hub` with its reason.
    pub async fn request(&self, request_json: String) -> Result<String, MobileError> {
        let req: Value = serde_json::from_str(&request_json).map_err(invalid)?;
        let mut guard = self.inner.lock().await;
        let g = &mut *guard;
        if !g.core.is_paired() {
            return Err(MobileError::NotPaired);
        }
        let mut buf = vec![0u8; MAX_APP_FRAME + 1];
        for _ in 0..TRIES {
            self.ensure_session(g, &mut buf).await?;
            let id = g.next_id;
            g.next_id += 1;
            let body = json!({ "id": id, "req": req });
            if serde_json::to_vec(&body).map_err(invalid)?.len() > MAX_APP_PLAINTEXT {
                return Err(invalid("request is too large"));
            }
            let now = self.now();
            let session = g.core.session_mut().ok_or(MobileError::NotPaired)?;
            // CRYPTO-REVIEW: sealed under the current session; this counts
            // toward its 10 messages.
            let frame = uplink::seal_json(session, &body, now).map_err(invalid)?;
            g.sock
                .as_ref()
                .expect("made by ensure_session")
                .send(&frame)
                .await
                .map_err(net)?;
            if let Some(reply) = self.wait_reply(g, id, &mut buf).await? {
                if let Some(e) = reply.get("err") {
                    return Err(MobileError::Hub(
                        e.as_str().unwrap_or("refused").to_string(),
                    ));
                }
                return Ok(reply.get("ok").cloned().unwrap_or(Value::Null).to_string());
            }
            // No reply: send it again under a fresh counter (the hub's replay
            // check drops nothing we need, and every request is safe to repeat).
        }
        Err(MobileError::Timeout)
    }
}

impl HubClient {
    async fn ensure_socket(&self, g: &mut Inner) -> Result<(), MobileError> {
        if g.sock.is_none() {
            let s = UdpSocket::bind("0.0.0.0:0").await.map_err(net)?;
            // connect() also makes the OS drop datagrams from anyone but the hub.
            s.connect(&self.hub_addr).await.map_err(net)?;
            g.sock = Some(s);
        }
        Ok(())
    }

    /// CRYPTO-REVIEW: runs the signed X25519 handshake when there's no
    /// session or the current one has used its 10 messages / 1 hour.
    async fn ensure_session(&self, g: &mut Inner, buf: &mut [u8]) -> Result<(), MobileError> {
        self.ensure_socket(g).await?;
        // Look at the session itself: NodeCore::needs_handshake says false
        // while an unanswered HELLO is under 5 s old, even if the current
        // session is used up (e.g. right after a request timed out).
        let now = self.now();
        if g.core.session().is_some_and(|s| !s.needs_rekey(now)) {
            return Ok(());
        }
        for _ in 0..TRIES {
            let mut rand = [0u8; 48];
            uplink::getrandom(&mut rand).map_err(invalid)?;
            let hello = g
                .core
                .start_handshake(&rand, self.now())
                .map_err(|_| MobileError::NotPaired)?;
            let sock = g.sock.as_ref().expect("made above");
            sock.send(&hello).await.map_err(net)?;
            let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
            loop {
                let Ok(r) = tokio::time::timeout_at(deadline, sock.recv(buf)).await else {
                    break;
                };
                let n = r.map_err(net)?;
                if buf[0] != FrameType::HelloAck as u8 {
                    continue; // a late reply from before; drop it
                }
                if let Ok(NodeEvent::SessionUp { .. }) = g.core.handle_frame(&buf[..n], self.now())
                {
                    return Ok(());
                }
            }
        }
        Err(MobileError::Timeout)
    }

    /// The reply to request `id`, or None after REPLY_TIMEOUT.
    async fn wait_reply(
        &self,
        g: &mut Inner,
        id: u64,
        buf: &mut [u8],
    ) -> Result<Option<Value>, MobileError> {
        let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
        loop {
            let sock = g.sock.as_ref().expect("socket made before sending");
            let Ok(r) = tokio::time::timeout_at(deadline, sock.recv(buf)).await else {
                return Ok(None);
            };
            let n = r.map_err(net)?;
            if buf[0] != FrameType::Uplink as u8 {
                continue;
            }
            let now = self.now();
            let Some(session) = g.core.session_mut() else {
                return Ok(None);
            };
            // CRYPTO-REVIEW: AEAD + replay check. Anything that fails is dropped.
            let Ok(v) = uplink::open_json::<Value>(&buf[..n], |f, out| {
                session.open(f, out, now).map(|(_, pt)| pt.len())
            }) else {
                continue;
            };
            if v["id"].as_u64() == Some(id) {
                return Ok(Some(v));
            }
            // A late reply to an earlier try: ignore it and keep waiting.
        }
    }
}

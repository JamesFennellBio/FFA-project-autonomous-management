//! The phone app's link to the hub, in Rust so the phone runs the exact same
//! protocol and crypto code (crypto-sharecrate) as the hub and PC.
//! The Tauri app (app/src-tauri/src/phone.rs) calls it on iOS and Android.
//!
//! The app's UI only ever sees JSON requests and replies. Everything in
//! between happens here:
//! - pairing, and the v2 hybrid handshake (Ed25519 + ML-DSA-65 signatures,
//!   X25519 + ML-KEM-768 keys)
//! - ChaCha20-Poly1305, with new keys every 1000 messages or 1 hour
//! - the TCP connection and retries
//!
//! ```text
//! App:  client.request('{"type":"channels"}')  ->  '{"channels":[...],"next":null,...}'
//! Rust: HELLO2/HELLO-ACK2 when needed -> seal {"id":n,"req":...} -> TCP -> open reply
//! ```

use std::sync::Arc;
use std::time::{Duration, Instant};

use crypto_sharecrate::crypto::v2::identity::{self, HybridIdentity, KEY_ID_LEN, KeyId};
use crypto_sharecrate::crypto::v2::link::{LinkCore, LinkEvent, LinkPairing};
use crypto_sharecrate::frame::FrameType;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use uplink::app::MAX_APP_PLAINTEXT;
use uplink::{read_frame, write_frame};

/// How long to wait for each reply before sending again.
const REPLY_TIMEOUT: Duration = Duration::from_millis(2500);
/// How long to wait for the TCP connection to the hub.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Sends per request (or per handshake) before giving up.
const TRIES: usize = 3;
/// How often a pairing request is repeated while waiting for approval.
const PAIR_RETRY: Duration = Duration::from_secs(3);

#[derive(Debug, thiserror::Error)]
pub enum MobileError {
    #[error("{0}")]
    Invalid(String),
    #[error("can't reach the hub: {0}")]
    Network(String),
    #[error("the hub didn't answer (is the phone on the same Wi-Fi as the hub?)")]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonePairing {
    pub phone_id: u16,
    /// CRYPTO-REVIEW: the hub's hybrid key (Ed25519 + ML-DSA-65, 3968 hex
    /// characters), pinned from now on.
    pub hub_public_key: String,
}

fn seed_from_hex(seed_hex: &str) -> Result<[u8; 32], MobileError> {
    uplink::key_from_hex(seed_hex.trim())
        .map_err(|_| invalid("identity seed must be 64 hex characters"))
}

/// A hub key given by the person: its KeyId (64 hex) or its full hybrid key.
fn key_id_from_hex(s: &str) -> Result<KeyId, MobileError> {
    let s = s.trim();
    if s.len() == KEY_ID_LEN * 2 {
        return uplink::key_from_hex(s).map_err(invalid);
    }
    let k = uplink::v2::public_from_hex(s).map_err(invalid)?;
    Ok(identity::key_id(&k))
}

fn to_pairing(p: &LinkPairing) -> PhonePairing {
    PhonePairing {
        phone_id: p.node_id,
        hub_public_key: uplink::v2::public_to_hex(&p.peer_pub),
    }
}

/// Makes a new identity for this phone (64 hex characters). The app saves it
/// in its private data folder (app/src-tauri/src/phone.rs).
///
/// CRYPTO-REVIEW: 32 bytes from the phone's OS CSPRNG. It's the phone's
/// long-term secret and stays in Rust; the web view never sees it.
pub fn new_identity_seed() -> Result<String, MobileError> {
    let mut seed = [0u8; 32];
    uplink::getrandom(&mut seed).map_err(invalid)?;
    Ok(uplink::key_to_hex(&seed))
}

/// The fingerprint the desktop app shows for this phone's pairing request.
pub fn fingerprint_of_seed(seed_hex: String) -> Result<String, MobileError> {
    let id = HybridIdentity::from_seed(&seed_from_hex(&seed_hex)?);
    Ok(uplink::v2::fingerprint(id.public_key()))
}

/// Fingerprint of a hub key (its KeyId or its full key, hex), e.g. after pairing.
pub fn fingerprint_of_key(public_key_hex: String) -> Result<String, MobileError> {
    Ok(uplink::v2::fingerprint_of_key_id(&key_id_from_hex(
        &public_key_hex,
    )?))
}

/// What a phone pairing QR code says, for showing before pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingQr {
    /// "ip:port" to pass to [`HubClient::new`].
    pub hub_addr: String,
    /// CRYPTO-REVIEW: the hub's key ID (hex). Pass it to [`HubClient::new`] as
    /// `expected_hub_key`, so only that hub is accepted.
    pub hub_key_id: String,
    pub hub_fingerprint: String,
    /// Unix time the code stops working (the hub enforces it).
    pub expires: i64,
}

/// Reads a phone pairing QR code (`farmlab://pair?...`).
pub fn parse_pairing_qr(uri: String) -> Result<PairingQr, MobileError> {
    let qr = uplink::pairlink::PhoneQr::parse(&uri).map_err(invalid)?;
    Ok(PairingQr {
        hub_fingerprint: uplink::v2::fingerprint_of_key_id(&qr.hub_key_id),
        hub_key_id: hex::encode(qr.hub_key_id),
        hub_addr: qr.addr,
        expires: qr.expires,
    })
}

struct Inner {
    core: LinkCore,
    conn: Option<TcpStream>,
    next_id: u64,
}

/// One connection to one hub. Requests are sent one at a time (callers that
/// overlap just wait their turn).
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

impl HubClient {
    /// `hub_addr` is "ip:port", e.g. "192.168.1.20:7302". `pairing` is what
    /// [`HubClient::pair`] returned last time (None before the first pairing).
    ///
    /// CRYPTO-REVIEW: `expected_hub_key` (the hub's key ID or full key, hex),
    /// if given, is the only hub pairing will accept. Without it the phone
    /// trusts the first hub that answers (check the fingerprint). Phase 3's QR
    /// code always supplies it.
    pub fn new(
        seed_hex: String,
        hub_addr: String,
        pairing: Option<PhonePairing>,
        expected_hub_key: Option<String>,
    ) -> Result<Arc<Self>, MobileError> {
        let seed = seed_from_hex(&seed_hex)?;
        // A pairing saved before v2 (32-byte hub key) doesn't load: pair again.
        let pairing = pairing.and_then(|p| {
            uplink::v2::public_from_hex(&p.hub_public_key)
                .ok()
                .map(|k| LinkPairing {
                    node_id: p.phone_id,
                    peer_pub: *k,
                })
        });
        let mut core = LinkCore::new(&seed, pairing);
        if let Some(k) = expected_hub_key.filter(|k| !k.trim().is_empty()) {
            core = core.with_expected_peer(key_id_from_hex(&k)?);
        }
        Ok(Arc::new(Self {
            hub_addr,
            start: Instant::now(),
            inner: Mutex::new(Inner {
                core,
                conn: None,
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
        g.core.pairing().as_ref().map(to_pairing)
    }

    /// Asks the hub to pair, repeating until someone approves this phone in
    /// the desktop app or `timeout_secs` runs out.
    pub async fn pair(&self, timeout_secs: u32) -> Result<PhonePairing, MobileError> {
        let mut guard = self.inner.lock().await;
        let g = &mut *guard;
        if let Some(p) = g.core.pairing() {
            return Ok(to_pairing(&p));
        }
        let req = g.core.pair_request(None);
        self.pair_loop(
            g,
            &req,
            timeout_secs,
            "not approved in time; approve this phone on the desktop app's Pairing page",
        )
        .await
    }

    /// Pairs using the QR code from the desktop app (Pairing > Add phone). No
    /// approval click is needed: the code proves a person at the desktop
    /// wanted this phone added.
    ///
    /// CRYPTO-REVIEW: make this client from the same QR code first
    /// (`parse_pairing_qr` -> `HubClient::new(seed, qr.hub_addr, None,
    /// Some(qr.hub_key_id))`), so the hub's key is pinned before anything is
    /// sent. This refuses a client made any other way.
    pub async fn pair_with_qr(
        &self,
        qr_uri: String,
        timeout_secs: u32,
    ) -> Result<PhonePairing, MobileError> {
        use crypto_sharecrate::crypto::v2::pairing::Ticket;
        let qr = uplink::pairlink::PhoneQr::parse(&qr_uri).map_err(invalid)?;
        if qr.addr != self.hub_addr {
            return Err(invalid(format!(
                "this QR code is for the hub at {}, not {}",
                qr.addr, self.hub_addr
            )));
        }
        let mut guard = self.inner.lock().await;
        let g = &mut *guard;
        if g.core.expected_peer() != Some(qr.hub_key_id) {
            return Err(invalid(
                "make the hub client with the QR code's hub key (see parse_pairing_qr)",
            ));
        }
        if let Some(p) = g.core.pairing() {
            return Ok(to_pairing(&p));
        }
        let req = g.core.pair_request(Some(Ticket {
            token: &qr.token,
            responder_key_id: &qr.hub_key_id,
        }));
        self.pair_loop(
            g,
            &req,
            timeout_secs,
            "the hub didn't accept this QR code (it may have expired or been used); \
             show a new one on the desktop app",
        )
        .await
    }
}

impl HubClient {
    /// Sends `req` every few seconds until the hub's PAIR_ACCEPT2 arrives.
    async fn pair_loop(
        &self,
        g: &mut Inner,
        req: &[u8],
        timeout_secs: u32,
        not_accepted: &str,
    ) -> Result<PhonePairing, MobileError> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(timeout_secs.into());
        while tokio::time::Instant::now() < deadline {
            let wait = (tokio::time::Instant::now() + PAIR_RETRY).min(deadline);
            if let Err(e) = self.send(g, req).await {
                // The hub may be restarting; keep trying until the deadline.
                if tokio::time::Instant::now() + PAIR_RETRY >= deadline {
                    return Err(e);
                }
                tokio::time::sleep_until(wait).await;
                continue;
            }
            while let Some(f) = self.recv_until(g, wait).await? {
                if f.first() != Some(&(FrameType::PairAccept2 as u8)) {
                    continue;
                }
                match g.core.handle_frame(&f, self.now()) {
                    Ok(LinkEvent::Paired(p)) => return Ok(to_pairing(&p)),
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
        Err(MobileError::Pairing(not_accepted.into()))
    }
}

impl HubClient {
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
        let mut last_err = MobileError::Timeout;
        for _ in 0..TRIES {
            // A failed handshake already used its own retries.
            self.ensure_session(g).await?;
            let id = g.next_id;
            g.next_id += 1;
            let body = json!({ "id": id, "req": req });
            if serde_json::to_vec(&body).map_err(invalid)?.len() > MAX_APP_PLAINTEXT {
                return Err(invalid("request is too large"));
            }
            let now = self.now();
            let session = g.core.session_mut().ok_or(MobileError::NotPaired)?;
            // CRYPTO-REVIEW: sealed under the current session; this counts
            // toward its 1000 messages.
            let frame = uplink::seal_json(session, &body, now).map_err(invalid)?;
            if let Err(e) = self.send(g, &frame).await {
                last_err = e;
                continue;
            }
            match self.wait_reply(g, id).await {
                Ok(Some(reply)) => {
                    if let Some(e) = reply.get("err") {
                        return Err(MobileError::Hub(
                            e.as_str().unwrap_or("refused").to_string(),
                        ));
                    }
                    return Ok(reply.get("ok").cloned().unwrap_or(Value::Null).to_string());
                }
                Ok(None) => last_err = MobileError::Timeout,
                Err(e) => last_err = e,
            }
            // No reply: send it again under a fresh counter (the hub's replay
            // check drops nothing we need, and every request is safe to repeat).
        }
        Err(last_err)
    }
}

impl HubClient {
    async fn ensure_conn(&self, g: &mut Inner) -> Result<(), MobileError> {
        if g.conn.is_none() {
            let s = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&self.hub_addr))
                .await
                .map_err(|_| MobileError::Timeout)?
                .map_err(net)?;
            s.set_nodelay(true).map_err(net)?;
            g.conn = Some(s);
        }
        Ok(())
    }

    /// Writes one frame, connecting first if needed. A broken connection is
    /// dropped so the next call reconnects.
    async fn send(&self, g: &mut Inner, frame: &[u8]) -> Result<(), MobileError> {
        self.ensure_conn(g).await?;
        let conn = g.conn.as_mut().expect("connected above");
        if let Err(e) = write_frame(conn, frame).await {
            g.conn = None;
            return Err(net(e));
        }
        Ok(())
    }

    /// The next frame from the hub, or None once `deadline` passes.
    async fn recv_until(
        &self,
        g: &mut Inner,
        deadline: tokio::time::Instant,
    ) -> Result<Option<Vec<u8>>, MobileError> {
        let Some(conn) = g.conn.as_mut() else {
            return Ok(None);
        };
        match tokio::time::timeout_at(deadline, read_frame(conn)).await {
            Err(_) => Ok(None),
            Ok(Ok(f)) => Ok(Some(f)),
            Ok(Err(e)) => {
                g.conn = None;
                Err(net(e))
            }
        }
    }

    /// CRYPTO-REVIEW: runs the v2 hybrid handshake when there's no session or
    /// the current one has used its 1000 messages / 1 hour.
    async fn ensure_session(&self, g: &mut Inner) -> Result<(), MobileError> {
        // Look at the session itself: LinkCore::needs_handshake says false
        // while an unanswered HELLO2 is pending, even if the current session
        // is used up (e.g. right after a request timed out).
        let now = self.now();
        if g.conn.is_some() && g.core.session().is_some_and(|s| !s.needs_rekey(now)) {
            return Ok(());
        }
        let mut last_err = MobileError::Timeout;
        for _ in 0..TRIES {
            let rand = uplink::v2::hs_random().map_err(invalid)?;
            let hello = g
                .core
                .start_handshake(&rand, self.now())
                .map_err(|_| MobileError::NotPaired)?;
            if let Err(e) = self.send(g, &hello).await {
                last_err = e;
                continue;
            }
            let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
            loop {
                let f = match self.recv_until(g, deadline).await {
                    Ok(Some(f)) => f,
                    Ok(None) => break,
                    Err(e) => {
                        last_err = e;
                        break;
                    }
                };
                if f.first() != Some(&(FrameType::HelloAck2 as u8)) {
                    continue; // a late reply from before; drop it
                }
                if let Ok(LinkEvent::SessionUp { .. }) = g.core.handle_frame(&f, self.now()) {
                    return Ok(());
                }
            }
        }
        Err(last_err)
    }

    /// The reply to request `id`, or None after REPLY_TIMEOUT.
    async fn wait_reply(&self, g: &mut Inner, id: u64) -> Result<Option<Value>, MobileError> {
        let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
        loop {
            let Some(f) = self.recv_until(g, deadline).await? else {
                return Ok(None);
            };
            if f.first() != Some(&(FrameType::Uplink as u8)) {
                continue;
            }
            let now = self.now();
            let Some(session) = g.core.session_mut() else {
                return Ok(None);
            };
            // CRYPTO-REVIEW: AEAD + replay check. Anything that fails is dropped.
            let Ok(v) = uplink::open_json::<Value>(&f, |f, out| {
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

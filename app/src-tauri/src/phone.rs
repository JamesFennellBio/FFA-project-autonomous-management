//! Phone backend: the hub over the v2 encrypted TCP link, through
//! farmlab-mobile-core (the same Rust core the Expo app used).
//!
//! CRYPTO-REVIEW: the phone's identity seed and its pinned hub key are kept
//! in `phone.json` in the app's private data folder, written 0600 (Android
//! and iOS app sandboxes keep other apps out). The plan's Keychain / Keystore
//! plugin is not done yet; see docs/CRYPTO_REVIEW.md "Known weaknesses".

use std::path::PathBuf;
use std::sync::Arc;

use farmlab_mobile_core::{
    HubClient, PhonePairing, fingerprint_of_key, fingerprint_of_seed, new_identity_seed,
    parse_pairing_qr,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::Mutex;

/// What survives a restart.
#[derive(Serialize, Deserialize, Default, Clone)]
struct Saved {
    /// CRYPTO-REVIEW: the phone's long-term secret (64 hex).
    seed: String,
    hub_addr: Option<String>,
    /// The hub's key ID, pinned from the QR code (or typed in).
    hub_key: Option<String>,
    phone_id: Option<u16>,
    /// CRYPTO-REVIEW: the hub's full hybrid key, pinned at pairing.
    hub_public_key: Option<String>,
}

impl Saved {
    fn pairing(&self) -> Option<PhonePairing> {
        Some(PhonePairing {
            phone_id: self.phone_id?,
            hub_public_key: self.hub_public_key.clone()?,
        })
    }
}

pub struct Phone {
    path: PathBuf,
    inner: Mutex<(Saved, Option<Arc<HubClient>>)>,
}

#[derive(Serialize)]
pub struct PhoneState {
    paired: bool,
    hub_addr: Option<String>,
    hub_fingerprint: Option<String>,
    phone_fingerprint: String,
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// "192.168.1.20" -> "192.168.1.20:7302".
pub fn with_port(addr: &str) -> String {
    let a = addr.trim();
    let has_port = a
        .rsplit_once(':')
        .is_some_and(|(h, p)| !h.is_empty() && !h.ends_with(':') && p.parse::<u16>().is_ok());
    if has_port || a.starts_with('[') && a.contains("]:") {
        a.to_string()
    } else if a.contains(':') {
        format!("[{a}]:{}", uplink::app::APP_PORT) // bare IPv6
    } else {
        format!("{a}:{}", uplink::app::APP_PORT)
    }
}

impl Phone {
    pub fn load(dir: PathBuf) -> anyhow::Result<Self> {
        let path = dir.join("phone.json");
        let mut saved: Saved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if saved.seed.is_empty() {
            saved.seed = new_identity_seed()?;
            uplink::write_secret(&path, &serde_json::to_string(&saved)?)?;
        }
        let client = Self::make_client(&saved);
        Ok(Self {
            path,
            inner: Mutex::new((saved, client)),
        })
    }

    fn make_client(s: &Saved) -> Option<Arc<HubClient>> {
        let addr = s.hub_addr.clone()?;
        s.pairing()?;
        HubClient::new(s.seed.clone(), addr, s.pairing(), s.hub_key.clone()).ok()
    }

    fn save(&self, s: &Saved) -> Result<(), String> {
        uplink::write_secret(&self.path, &serde_json::to_string(s).map_err(err)?).map_err(err)
    }

    fn state(s: &Saved) -> PhoneState {
        PhoneState {
            paired: s.pairing().is_some(),
            hub_addr: s.hub_addr.clone(),
            hub_fingerprint: s
                .hub_public_key
                .clone()
                .and_then(|k| fingerprint_of_key(k).ok()),
            phone_fingerprint: fingerprint_of_seed(s.seed.clone()).unwrap_or_default(),
        }
    }

    async fn finish(
        &self,
        client: Arc<HubClient>,
        addr: String,
        key: Option<String>,
        p: PhonePairing,
    ) -> Result<PhoneState, String> {
        let mut g = self.inner.lock().await;
        g.0.hub_addr = Some(addr);
        g.0.hub_key = key;
        g.0.phone_id = Some(p.phone_id);
        g.0.hub_public_key = Some(p.hub_public_key);
        self.save(&g.0)?;
        g.1 = Some(client);
        Ok(Self::state(&g.0))
    }
}

type St<'a> = tauri::State<'a, Phone>;

#[tauri::command(rename_all = "snake_case")]
pub async fn phone_state(phone: St<'_>) -> Result<PhoneState, String> {
    Ok(Phone::state(&phone.inner.lock().await.0))
}

/// Pairs with the desktop app's QR code. The client is made from the QR
/// itself, so the hub's key ID is pinned before anything is sent.
#[tauri::command(rename_all = "snake_case")]
pub async fn phone_pair_qr(phone: St<'_>, uri: String) -> Result<PhoneState, String> {
    let qr = parse_pairing_qr(uri.clone()).map_err(err)?;
    let seed = phone.inner.lock().await.0.seed.clone();
    let client = HubClient::new(seed, qr.hub_addr.clone(), None, Some(qr.hub_key_id.clone()))
        .map_err(err)?;
    let p = client.pair_with_qr(uri, 30).await.map_err(err)?;
    phone
        .finish(client, qr.hub_addr, Some(qr.hub_key_id), p)
        .await
}

/// Pairs by address; someone approves the phone in the desktop app.
#[tauri::command(rename_all = "snake_case")]
pub async fn phone_pair_manual(
    phone: St<'_>,
    addr: String,
    hub_key: Option<String>,
) -> Result<PhoneState, String> {
    let addr = with_port(&addr);
    let key = hub_key.filter(|k| !k.trim().is_empty());
    let seed = phone.inner.lock().await.0.seed.clone();
    let client = HubClient::new(seed, addr.clone(), None, key.clone()).map_err(err)?;
    let p = client.pair(120).await.map_err(err)?;
    phone.finish(client, addr, key, p).await
}

#[tauri::command(rename_all = "snake_case")]
pub async fn phone_forget(phone: St<'_>) -> Result<PhoneState, String> {
    let mut g = phone.inner.lock().await;
    g.0 = Saved {
        seed: g.0.seed.clone(),
        ..Saved::default()
    };
    phone.save(&g.0)?;
    g.1 = None;
    Ok(Phone::state(&g.0))
}

/// One request to the hub (hub-node/src/hub/app.rs); returns its `ok` part.
#[tauri::command(rename_all = "snake_case")]
pub async fn hub_request(phone: St<'_>, req: Value) -> Result<Value, String> {
    let client = phone
        .inner
        .lock()
        .await
        .1
        .clone()
        .ok_or("This phone isn't paired with a hub yet (Settings).")?;
    let reply = client.request(req.to_string()).await.map_err(err)?;
    serde_json::from_str(&reply).map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_port_is_added() {
        assert_eq!(with_port("192.168.1.20"), "192.168.1.20:7302");
        assert_eq!(with_port(" 192.168.1.20:9000 "), "192.168.1.20:9000");
        assert_eq!(with_port("farmhub.local"), "farmhub.local:7302");
        assert_eq!(with_port("fe80::1"), "[fe80::1]:7302");
        assert_eq!(with_port("[fe80::1]:7302"), "[fe80::1]:7302");
    }

    #[tokio::test]
    async fn identity_is_made_once_and_kept() {
        let dir = std::env::temp_dir().join(format!("farmlab-phone-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = Phone::load(dir.clone()).unwrap();
        let fp = Phone::state(&a.inner.lock().await.0).phone_fingerprint;
        let b = Phone::load(dir.clone()).unwrap();
        assert_eq!(Phone::state(&b.inner.lock().await.0).phone_fingerprint, fp);
        assert!(!Phone::state(&b.inner.lock().await.0).paired);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

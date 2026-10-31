//! The one data layer every screen uses. Three backends:
//! - **Desktop** (Tauri): `daemon_api` -> pc-daemon's REST API. The Tauri side
//!   adds the token; this WASM code never sees it.
//! - **Phone** (Tauri): `hub_request` -> the hub over the v2 encrypted link
//!   (mobile/core). Replies are converted into the desktop's shapes here, so
//!   screens don't care which one they talk to.
//! - **Demo**: no Tauri (plain browser, `trunk serve`): sample data in memory,
//!   for working on the look. Add `?phone` to the URL for the phone layout.

use std::cell::RefCell;
use std::sync::OnceLock;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

use crate::format;
use crate::model::*;
use crate::prefs::Prefs;

pub type Res<T> = Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Desktop,
    Phone,
    Demo { phone: bool },
}

impl Backend {
    pub fn is_phone(self) -> bool {
        matches!(self, Backend::Phone | Backend::Demo { phone: true })
    }
    /// Only the desktop (pc-daemon) keeps acknowledged alerts, phones, names.
    pub fn is_desktop(self) -> bool {
        !self.is_phone()
    }
}

static BACKEND: OnceLock<Backend> = OnceLock::new();

/// Set once at startup from the Tauri side's answer (or Demo without Tauri).
pub fn set_backend(b: Backend) {
    let _ = BACKEND.set(b);
}

pub fn backend() -> Backend {
    *BACKEND.get().unwrap_or(&Backend::Demo { phone: false })
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "__TAURI__", "core"], js_name = invoke)]
    async fn tauri_invoke(cmd: &str, args: JsValue) -> Result<JsValue, JsValue>;
}

/// True inside the Tauri app (`app.withGlobalTauri` in tauri.conf.json).
pub fn has_tauri() -> bool {
    web_sys::window()
        .and_then(|w| js_sys::Reflect::get(&w, &"__TAURI__".into()).ok())
        .is_some_and(|v| !v.is_undefined())
}

fn js_err(e: JsValue) -> String {
    e.as_string()
        .or_else(|| js_sys::JSON::stringify(&e).ok().and_then(|s| s.as_string()))
        .unwrap_or_else(|| "unknown error".into())
}

/// Calls a Tauri command. Arguments are snake_case (see `rename_all` on the
/// Rust commands).
pub async fn invoke<T: DeserializeOwned>(cmd: &str, args: Value) -> Res<T> {
    let ser = serde_wasm_bindgen::Serializer::json_compatible();
    let args = args.serialize(&ser).map_err(|e| e.to_string())?;
    let v = tauri_invoke(cmd, args).await.map_err(js_err)?;
    serde_wasm_bindgen::from_value(v).map_err(|e| format!("unexpected reply from {cmd}: {e}"))
}

// ---- desktop: pc-daemon REST ----

#[derive(Deserialize)]
struct ApiReply {
    ok: bool,
    status: u16,
    data: Value,
}

async fn daemon<T: DeserializeOwned>(method: &str, path: &str, body: Option<Value>) -> Res<T> {
    let r: ApiReply = invoke(
        "daemon_api",
        json!({ "method": method, "path": path, "body": body }),
    )
    .await?;
    if !r.ok {
        return Err(r.data["error"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| format!("HTTP {}", r.status)));
    }
    serde_json::from_value(r.data).map_err(|e| format!("unexpected reply from {path}: {e}"))
}

// ---- phone: the hub's request/reply protocol (hub-node/src/hub/app.rs) ----

async fn hub<T: DeserializeOwned>(req: Value) -> Res<T> {
    let v: Value = invoke("hub_request", json!({ "req": req })).await?;
    serde_json::from_value(v).map_err(|e| format!("unexpected reply from the hub: {e}"))
}

/// Every page of a paged hub list.
async fn hub_all(ty: &str, key: &str) -> Res<Vec<Value>> {
    let mut out = Vec::new();
    let mut offset = 0u64;
    for _ in 0..50 {
        let r: Value = hub(json!({ "type": ty, "offset": offset })).await?;
        out.extend(r[key].as_array().cloned().unwrap_or_default());
        match r["next"].as_u64() {
            Some(n) => offset = n,
            None => break,
        }
    }
    Ok(out)
}

#[derive(Deserialize)]
struct HubInfo {
    hub_fingerprint: String,
    #[serde(default)]
    max_recipes: usize,
    time: i64,
}

#[derive(Deserialize)]
struct HubNode {
    id: u16,
    name: String,
    last_seen: Option<i64>,
}

#[derive(Deserialize)]
struct HubChannel {
    node: u16,
    channel: u8,
    kind: ChannelKind,
    name: String,
    value: Option<i64>,
    ts: Option<i64>,
}

async fn phone_nodes() -> Res<Vec<NodeInfo>> {
    let nodes = hub_all("nodes", "nodes").await?;
    let chans: Vec<HubChannel> = hub_all("channels", "channels")
        .await?
        .into_iter()
        .filter_map(|v| serde_json::from_value(v).ok())
        .collect();
    Ok(nodes
        .into_iter()
        .filter_map(|v| serde_json::from_value::<HubNode>(v).ok())
        .map(|n| NodeInfo {
            channels: chans
                .iter()
                .filter(|c| c.node == n.id)
                .map(|c| Channel {
                    channel: c.channel,
                    kind: c.kind,
                    name: c.name.clone(),
                    last_value: c.value,
                    last_ts: c.ts,
                })
                .collect(),
            id: n.id,
            name: n.name,
            fingerprint: String::new(),
            paired_at: 0,
            last_seen: n.last_seen,
            transport: None,
            sessions: 0,
        })
        .collect())
}

/// A hub event (phone alert list) as an [`Alert`].
fn event_alert(id: i64, e: &Value, nodes: &[NodeInfo]) -> Alert {
    let node = e["node_id"].as_i64();
    let ch = e["channel"].as_i64();
    let node_name = node.and_then(|n| {
        nodes
            .iter()
            .find(|x| i64::from(x.id) == n)
            .map(|x| x.name.clone())
    });
    let ch_name = || {
        nodes
            .iter()
            .find(|x| Some(i64::from(x.id)) == node)
            .and_then(|n| n.channels.iter().find(|c| Some(i64::from(c.channel)) == ch))
            .map_or_else(
                || format!("channel {}", ch.unwrap_or(0)),
                |c| c.name.clone(),
            )
    };
    let message = match e["type"].as_str() {
        Some("alert") => format!(
            "{}: {}",
            format::alert_code(e["code"].as_i64().unwrap_or(0)),
            ch_name()
        ),
        Some("recipe_alert") => format!(
            "{} ({})",
            e["text"].as_str().unwrap_or(""),
            e["name"].as_str().unwrap_or("")
        ),
        Some("command_dropped") => {
            format!("Command dropped, the node's queue was full: {}", ch_name())
        }
        _ => "Alert".into(),
    };
    Alert {
        id,
        node_id: node,
        node_name,
        channel: ch,
        code: e["code"].as_i64().unwrap_or(0),
        value: e["value"].as_i64(),
        message,
        ts: e["ts"].as_i64().unwrap_or(0),
        acked: false,
    }
}

// ---- the API every screen uses ----

pub async fn status() -> Res<Status> {
    match backend() {
        Backend::Desktop => daemon("GET", "/api/status", None).await,
        Backend::Phone => {
            let info: HubInfo = hub(json!({ "type": "info" })).await?;
            let me: PhoneState = invoke("phone_state", json!({})).await?;
            Ok(Status {
                pc_fingerprint: me.phone_fingerprint,
                pc_key_id: String::new(),
                time: info.time,
                hub: HubLive {
                    paired: true,
                    fingerprint: Some(info.hub_fingerprint),
                    connected: true,
                    last_seen: Some(info.time),
                    backlog: 0,
                    sessions: 0,
                },
            })
        }
        Backend::Demo { .. } => Ok(demo(|d| d.status())),
    }
}

pub async fn nodes() -> Res<Vec<NodeInfo>> {
    match backend() {
        Backend::Desktop => daemon("GET", "/api/nodes", None).await,
        Backend::Phone => phone_nodes().await,
        Backend::Demo { .. } => Ok(demo(|d| d.nodes())),
    }
}

/// History since `since`, oldest first. The phone gets at most 180 points
/// per request (one hub reply); the hub keeps 30 days.
pub async fn readings(node: u16, channel: u8, since: i64, limit: u32) -> Res<Vec<Reading>> {
    match backend() {
        Backend::Desktop => {
            daemon(
                "GET",
                &format!(
                    "/api/readings?node_id={node}&channel={channel}&since={since}&limit={limit}"
                ),
                None,
            )
            .await
        }
        Backend::Phone => {
            let r: Value = hub(json!({ "type": "history", "node": node, "channel": channel, "since": since, "points": limit.min(180) })).await?;
            Ok(r["points"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some(Reading {
                        ts: p[0].as_i64()?,
                        value: p[1].as_i64()?,
                    })
                })
                .collect())
        }
        Backend::Demo { .. } => Ok(demo(|d| d.readings(node, channel, since))),
    }
}

/// Alerts, newest first. The phone shows the hub's last 30 days and can't
/// acknowledge (that's kept on the PC).
pub async fn alerts(unacked_only: bool) -> Res<Vec<Alert>> {
    match backend() {
        Backend::Desktop => {
            daemon("GET", &format!("/api/alerts?unacked={unacked_only}"), None).await
        }
        Backend::Phone => {
            let nodes = phone_nodes().await?;
            let mut out = Vec::new();
            let mut before: Option<i64> = None;
            for _ in 0..4 {
                let r: Value = hub(json!({ "type": "events", "before": before })).await?;
                for e in r["events"].as_array().into_iter().flatten() {
                    out.push(event_alert(
                        e["id"].as_i64().unwrap_or(0),
                        &e["event"],
                        &nodes,
                    ));
                }
                match r["next"].as_i64() {
                    Some(n) => before = Some(n),
                    None => break,
                }
            }
            Ok(out)
        }
        Backend::Demo { .. } => Ok(demo(|d| {
            d.alerts
                .iter()
                .filter(|a| !unacked_only || !a.acked)
                .cloned()
                .collect()
        })),
    }
}

pub async fn ack_alert(id: i64) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>("POST", &format!("/api/alerts/{id}/ack"), None)
            .await
            .map(drop),
        Backend::Phone => Err("Acknowledge alerts in the desktop app.".into()),
        Backend::Demo { .. } => {
            demo(|d| {
                d.alerts
                    .iter_mut()
                    .filter(|a| a.id == id)
                    .for_each(|a| a.acked = true)
            });
            Ok(())
        }
    }
}

/// Sets an output. Ok(Some(note)) when it was only queued.
pub async fn command(node: u16, channel: u8, value: i64) -> Res<Option<String>> {
    match backend() {
        Backend::Desktop => daemon::<Value>(
            "POST",
            &format!("/api/nodes/{node}/channels/{channel}/command"),
            Some(json!({ "value": value })),
        )
        .await
        .map(|_| None),
        Backend::Phone => {
            let r: Value =
                hub(json!({ "type": "command", "node": node, "channel": channel, "value": value }))
                    .await?;
            Ok((r["node_online"] == false)
                .then(|| "Queued: the node gets it when it next reports.".to_string()))
        }
        Backend::Demo { .. } => {
            demo(|d| d.set_value(node, channel, value));
            Ok(None)
        }
    }
}

pub async fn recipes() -> Res<RecipeList> {
    match backend() {
        Backend::Desktop => daemon("GET", "/api/recipes", None).await,
        Backend::Phone => {
            let info: HubInfo = hub(json!({ "type": "info" })).await?;
            Ok(RecipeList {
                recipes: hub_all("recipes", "recipes")
                    .await?
                    .into_iter()
                    .filter_map(|v| serde_json::from_value(v).ok())
                    .collect(),
                max_recipes: info.max_recipes,
            })
        }
        Backend::Demo { .. } => Ok(demo(|d| RecipeList {
            recipes: d
                .recipes
                .iter()
                .map(|r| RecipeSummary {
                    id: r.id,
                    name: r.name.clone(),
                    enabled: r.enabled,
                })
                .collect(),
            max_recipes: 32,
        })),
    }
}

#[derive(Deserialize)]
struct RecipeReply {
    recipe: Recipe,
}

pub async fn recipe(id: u32) -> Res<Recipe> {
    match backend() {
        Backend::Desktop => daemon::<RecipeReply>("GET", &format!("/api/recipes/{id}"), None)
            .await
            .map(|r| r.recipe),
        Backend::Phone => hub::<RecipeReply>(json!({ "type": "get_recipe", "id": id }))
            .await
            .map(|r| r.recipe),
        Backend::Demo { .. } => {
            demo(|d| d.recipes.iter().find(|r| r.id == id).cloned()).ok_or("no such recipe".into())
        }
    }
}

#[derive(Deserialize)]
struct IdReply {
    id: u32,
}

/// Saves a recipe (`id` 0 = new) and returns its id.
pub async fn save_recipe(r: &Recipe) -> Res<u32> {
    let body = serde_json::to_value(r).map_err(|e| e.to_string())?;
    match backend() {
        Backend::Desktop if r.id == 0 => daemon::<IdReply>("POST", "/api/recipes", Some(body))
            .await
            .map(|r| r.id),
        Backend::Desktop => daemon::<IdReply>("PUT", &format!("/api/recipes/{}", r.id), Some(body))
            .await
            .map(|r| r.id),
        Backend::Phone => hub::<IdReply>(json!({ "type": "save_recipe", "recipe": body }))
            .await
            .map(|r| r.id),
        Backend::Demo { .. } => Ok(demo(|d| d.save_recipe(r.clone()))),
    }
}

pub async fn delete_recipe(id: u32) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>("DELETE", &format!("/api/recipes/{id}"), None)
            .await
            .map(drop),
        Backend::Phone => hub::<Value>(json!({ "type": "delete_recipe", "id": id }))
            .await
            .map(drop),
        Backend::Demo { .. } => {
            demo(|d| d.recipes.retain(|r| r.id != id));
            Ok(())
        }
    }
}

pub async fn set_recipe_enabled(id: u32, enabled: bool) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>(
            "POST",
            &format!("/api/recipes/{id}/enabled"),
            Some(json!({ "enabled": enabled })),
        )
        .await
        .map(drop),
        Backend::Phone => {
            hub::<Value>(json!({ "type": "set_recipe_enabled", "id": id, "enabled": enabled }))
                .await
                .map(drop)
        }
        Backend::Demo { .. } => {
            demo(|d| {
                d.recipes
                    .iter_mut()
                    .filter(|r| r.id == id)
                    .for_each(|r| r.enabled = enabled)
            });
            Ok(())
        }
    }
}

/// Devices asking to pair. On the phone: nodes only.
pub async fn pending() -> Res<Vec<PendingPair>> {
    match backend() {
        Backend::Desktop => daemon("GET", "/api/pairing/requests", None).await,
        Backend::Phone => Ok(hub_all("pair_requests", "requests")
            .await?
            .into_iter()
            .filter_map(|v| {
                Some(PendingPair {
                    kind: PairKind::Node,
                    public_key: v["public_key"].as_str()?.into(),
                    fingerprint: v["fingerprint"].as_str()?.into(),
                    transport: v["transport"].as_str().map(str::to_string),
                    seen_at: v["seen_at"].as_i64().unwrap_or(0),
                })
            })
            .collect()),
        Backend::Demo { .. } => Ok(demo(|d| d.pending.clone())),
    }
}

pub async fn approve(key: &str, name: Option<String>) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>(
            "POST",
            &format!("/api/pairing/requests/{key}/approve"),
            Some(json!({ "name": name })),
        )
        .await
        .map(drop),
        Backend::Phone => {
            hub::<Value>(json!({ "type": "approve_node", "public_key": key, "name": name }))
                .await
                .map(drop)
        }
        Backend::Demo { .. } => {
            demo(|d| d.approve(key, name));
            Ok(())
        }
    }
}

pub async fn reject(key: &str) -> Res<()> {
    match backend() {
        Backend::Desktop => {
            daemon::<Value>("POST", &format!("/api/pairing/requests/{key}/reject"), None)
                .await
                .map(drop)
        }
        _ => {
            demo(|d| d.pending.retain(|p| p.public_key != key));
            Ok(())
        }
    }
}

// ---- desktop only ----

pub async fn phones() -> Res<Vec<Phone>> {
    match backend() {
        Backend::Desktop => daemon("GET", "/api/phones", None).await,
        _ => Ok(demo(|d| d.phones.clone())),
    }
}

pub async fn unpair_phone(id: u16) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>("DELETE", &format!("/api/phones/{id}"), None)
            .await
            .map(drop),
        _ => {
            demo(|d| d.phones.retain(|p| p.id != id));
            Ok(())
        }
    }
}

pub async fn rename_node(id: u16, name: &str) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>(
            "PATCH",
            &format!("/api/nodes/{id}"),
            Some(json!({ "name": name })),
        )
        .await
        .map(drop),
        _ => {
            demo(|d| {
                d.nodes
                    .iter_mut()
                    .filter(|n| n.id == id)
                    .for_each(|n| n.name = name.into())
            });
            Ok(())
        }
    }
}

pub async fn rename_channel(id: u16, ch: u8, name: &str) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>(
            "PATCH",
            &format!("/api/nodes/{id}/channels/{ch}"),
            Some(json!({ "name": name })),
        )
        .await
        .map(drop),
        _ => {
            demo(|d| {
                d.nodes
                    .iter_mut()
                    .filter(|n| n.id == id)
                    .flat_map(|n| n.channels.iter_mut())
                    .filter(|c| c.channel == ch)
                    .for_each(|c| c.name = name.into())
            });
            Ok(())
        }
    }
}

pub async fn unpair_node(id: u16) -> Res<()> {
    match backend() {
        Backend::Desktop => daemon::<Value>("DELETE", &format!("/api/nodes/{id}"), None)
            .await
            .map(drop),
        _ => {
            demo(|d| d.nodes.retain(|n| n.id != id));
            Ok(())
        }
    }
}

#[derive(Deserialize)]
struct FpReply {
    fingerprint: String,
}

/// Pairs the hub with the one-time code `hub-node pair` printed.
pub async fn pair_hub(address: &str, code: &str) -> Res<String> {
    match backend() {
        Backend::Desktop => daemon::<FpReply>(
            "POST",
            "/api/hubs/pair",
            Some(json!({ "address": address, "code": code })),
        )
        .await
        .map(|r| r.fingerprint),
        _ => {
            if code.chars().filter(char::is_ascii_digit).count() != 12 {
                return Err("wrong pairing code (2 tries left)".into());
            }
            Ok("E5A0-6B13-F8D2-4C71".into())
        }
    }
}

pub async fn phone_ticket() -> Res<PhoneTicket> {
    match backend() {
        Backend::Desktop => daemon("POST", "/api/phones/ticket", Some(json!({}))).await,
        _ => Ok(demo(|d| d.ticket())),
    }
}

// ---- phone app only ----

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct PhoneState {
    pub paired: bool,
    pub hub_addr: Option<String>,
    pub hub_fingerprint: Option<String>,
    pub phone_fingerprint: String,
}

pub async fn phone_state() -> Res<PhoneState> {
    match backend() {
        Backend::Phone => invoke("phone_state", json!({})).await,
        _ => Ok(demo(|d| d.phone.clone())),
    }
}

/// Pairs with the QR code from the desktop app (no approval needed).
pub async fn phone_pair_qr(uri: &str) -> Res<PhoneState> {
    match backend() {
        Backend::Phone => invoke("phone_pair_qr", json!({ "uri": uri })).await,
        _ => {
            if !uri.starts_with("farmlab://pair?") {
                return Err("not a Farm & Lab pairing code".into());
            }
            Ok(demo(|d| d.pair_phone()))
        }
    }
}

/// Pairs by address; someone approves the phone on the desktop.
pub async fn phone_pair_manual(addr: &str, hub_key: Option<String>) -> Res<PhoneState> {
    match backend() {
        Backend::Phone => {
            invoke(
                "phone_pair_manual",
                json!({ "addr": addr, "hub_key": hub_key }),
            )
            .await
        }
        _ => Ok(demo(|d| d.pair_phone())),
    }
}

pub async fn phone_forget() -> Res<PhoneState> {
    match backend() {
        Backend::Phone => invoke("phone_forget", json!({})).await,
        _ => Ok(demo(|d| {
            d.phone.paired = false;
            d.phone.clone()
        })),
    }
}

/// Opens the camera and returns the QR code's text (Android / iOS only).
pub async fn scan_qr() -> Res<String> {
    #[derive(Deserialize)]
    struct Scanned {
        content: String,
    }
    let r: Scanned = invoke(
        "plugin:barcode-scanner|scan",
        json!({ "windowed": false, "formats": ["QR_CODE"] }),
    )
    .await?;
    Ok(r.content)
}

/// Whether this build has a camera scanner (only the Android / iOS builds).
pub async fn can_scan() -> bool {
    backend() == Backend::Phone
        && invoke::<bool>("has_camera_scanner", json!({}))
            .await
            .unwrap_or(false)
}

// ---- prefs, system, limits (Tauri side; browser storage in the demo) ----

#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct SystemInfo {
    pub total_ram_mb: u64,
    pub cpus: usize,
    pub os: String,
}

/// What the Tauri side could enforce for the chosen RAM / CPU limits.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct LimitsReport {
    /// "hard" (the OS enforces it) or "soft" (the app holds back by itself).
    pub kind: String,
    pub detail: String,
}

const PREFS_KEY: &str = "farmlab.prefs";

fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok()?
}

pub async fn load_prefs() -> Res<Option<Prefs>> {
    if has_tauri() {
        invoke("prefs_get", json!({})).await
    } else {
        Ok(local_storage()
            .and_then(|s| s.get_item(PREFS_KEY).ok().flatten())
            .and_then(|s| serde_json::from_str(&s).ok()))
    }
}

pub async fn save_prefs(p: &Prefs) -> Res<LimitsReport> {
    if has_tauri() {
        invoke("prefs_set", json!({ "prefs": p })).await
    } else {
        if let Some(s) = local_storage() {
            let _ = s.set_item(PREFS_KEY, &serde_json::to_string(p).unwrap_or_default());
        }
        Ok(LimitsReport {
            kind: "soft".into(),
            detail: "Preview in a browser: limits apply in the app.".into(),
        })
    }
}

pub async fn system_info() -> SystemInfo {
    if has_tauri() {
        invoke("system_info", json!({})).await.unwrap_or_default()
    } else {
        SystemInfo {
            total_ram_mb: 16_384,
            cpus: 8,
            os: "browser".into(),
        }
    }
}

// ---- demo data (browser preview) ----

thread_local! {
    static DEMO: RefCell<Demo> = RefCell::new(Demo::new());
}

fn demo<T>(f: impl FnOnce(&mut Demo) -> T) -> T {
    DEMO.with(|d| f(&mut d.borrow_mut()))
}

struct Demo {
    nodes: Vec<NodeInfo>,
    alerts: Vec<Alert>,
    recipes: Vec<Recipe>,
    pending: Vec<PendingPair>,
    phones: Vec<Phone>,
    phone: PhoneState,
    seed: u32,
}

impl Demo {
    fn new() -> Self {
        let t = format::now();
        let ch = |channel, kind, name: &str, v| Channel {
            channel,
            kind,
            name: name.into(),
            last_value: Some(v),
            last_ts: Some(t),
        };
        use ChannelKind::*;
        let node =
            |id, name: &str, fp: &str, ago: i64, transport: &str, sessions, channels| NodeInfo {
                id,
                name: name.into(),
                fingerprint: fp.into(),
                paired_at: t - 86_400 * 9,
                last_seen: Some(t - ago),
                transport: Some(transport.into()),
                sessions,
                channels,
            };
        let cmp = |node, channel, op, value| Cond::Compare {
            node,
            channel,
            op,
            value,
        };
        let set = |node, channel, value| Action::Set {
            node,
            channel,
            value,
        };
        Self {
            nodes: vec![
                node(
                    1,
                    "North barn",
                    "7F3A-91C2-0B4E-5D10",
                    0,
                    "wifi",
                    41,
                    vec![
                        ch(0, Temperature, "Barn temp", 24_600),
                        ch(1, AnalogIn, "Soil moisture", 1_820),
                        ch(2, DigitalIn, "Door", 0),
                        ch(3, DigitalOut, "Fans", 0),
                        ch(4, DigitalOut, "Water valve", 1),
                    ],
                ),
                node(
                    2,
                    "Greenhouse",
                    "C410-5D7E-A2F9-1180",
                    20,
                    "lora",
                    12,
                    vec![
                        ch(0, Temperature, "Air temp", 29_100),
                        ch(1, AnalogIn, "Light", 2_640),
                        ch(2, DigitalOut, "Siren", 0),
                    ],
                ),
                node(
                    3,
                    "Field pump",
                    "08BD-E61F-33C7-9A02",
                    3600,
                    "lora",
                    3,
                    vec![ch(0, Temperature, "Water temp", 14_300)],
                ),
            ],
            alerts: vec![
                Alert {
                    id: 1,
                    node_id: Some(2),
                    node_name: Some("Greenhouse".into()),
                    channel: Some(0),
                    code: 2,
                    value: Some(31_200),
                    message: "Air temp above 30 °C".into(),
                    ts: t - 600,
                    acked: false,
                },
                Alert {
                    id: 2,
                    node_id: Some(3),
                    node_name: Some("Field pump".into()),
                    channel: None,
                    code: 1,
                    value: None,
                    message: "Water temp sensor fault".into(),
                    ts: t - 3000,
                    acked: false,
                },
                Alert {
                    id: 3,
                    node_id: Some(1),
                    node_name: Some("North barn".into()),
                    channel: Some(2),
                    code: 3,
                    value: Some(1),
                    message: "Node restarted".into(),
                    ts: t - 86_400,
                    acked: true,
                },
            ],
            recipes: vec![
                Recipe {
                    id: 1,
                    name: "Cool the barn".into(),
                    enabled: true,
                    body: RecipeBody {
                        when: cmp(1, 0, Op::Gt, 30_000),
                        then: vec![set(1, 3, 1)],
                        otherwise: vec![set(1, 3, 0)],
                    },
                },
                Recipe {
                    id: 2,
                    name: "Greenhouse guard".into(),
                    enabled: true,
                    body: RecipeBody {
                        when: Cond::And {
                            all: vec![
                                cmp(2, 0, Op::Gt, 32_000),
                                Cond::Not {
                                    cond: Box::new(cmp(1, 2, Op::Eq, 1)),
                                },
                            ],
                        },
                        then: vec![
                            set(2, 2, 1),
                            Action::Alert {
                                text: "Greenhouse over 32 °C".into(),
                            },
                        ],
                        otherwise: vec![set(2, 2, 0)],
                    },
                },
                Recipe {
                    id: 3,
                    name: "Dry soil".into(),
                    enabled: false,
                    body: RecipeBody {
                        when: cmp(1, 1, Op::Lt, 1_200),
                        then: vec![set(1, 4, 1)],
                        otherwise: vec![set(1, 4, 0)],
                    },
                },
            ],
            pending: vec![PendingPair {
                kind: PairKind::Node,
                public_key: "a1b2c3".into(),
                fingerprint: "5E2D-77A0-C9B1-0F4E".into(),
                transport: Some("wifi".into()),
                seen_at: t - 15,
            }],
            phones: vec![Phone {
                id: 1,
                fingerprint: "B7C1-0E98-4FA2-6D3B".into(),
                name: "Sam's phone".into(),
                paired_at: t - 86_400 * 3,
            }],
            phone: PhoneState {
                paired: true,
                hub_addr: Some("192.168.1.20:7302".into()),
                hub_fingerprint: Some("E5A0-6B13-F8D2-4C71".into()),
                phone_fingerprint: "B7C1-0E98-4FA2-6D3B".into(),
            },
            seed: 0x9E37_79B9,
        }
    }

    /// Small pseudo-random numbers in -1..1 (xorshift), for the random walk.
    fn noise(&mut self) -> f64 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        f64::from(self.seed) / f64::from(u32::MAX) * 2.0 - 1.0
    }

    fn status(&mut self) -> Status {
        let t = format::now();
        Status {
            pc_fingerprint: "3C9E-1A42-D07B-88E1".into(),
            pc_key_id: "3c9e1a42d07b88e1".repeat(4),
            time: t,
            hub: HubLive {
                paired: true,
                fingerprint: Some("E5A0-6B13-F8D2-4C71".into()),
                connected: true,
                last_seen: Some(t),
                backlog: 0,
                sessions: 56,
            },
        }
    }

    /// Moves the live values a little each time, like the real thing.
    fn nodes(&mut self) -> Vec<NodeInfo> {
        let t = format::now();
        for i in 0..self.nodes.len() {
            if self.nodes[i].id == 3 {
                continue;
            }
            self.nodes[i].last_seen = Some(t);
            for j in 0..self.nodes[i].channels.len() {
                let kind = self.nodes[i].channels[j].kind;
                if matches!(kind, ChannelKind::Temperature | ChannelKind::AnalogIn) {
                    let step = (self.noise() * 150.0) as i64;
                    let c = &mut self.nodes[i].channels[j];
                    c.last_value = c.last_value.map(|v| v + step);
                    c.last_ts = Some(t);
                }
            }
        }
        self.nodes.clone()
    }

    fn set_value(&mut self, node: u16, channel: u8, value: i64) {
        for c in self
            .nodes
            .iter_mut()
            .filter(|n| n.id == node)
            .flat_map(|n| n.channels.iter_mut())
        {
            if c.channel == channel {
                c.last_value = Some(value);
            }
        }
    }

    fn readings(&mut self, node: u16, channel: u8, since: i64) -> Vec<Reading> {
        let t = format::now();
        let ch = self
            .nodes
            .iter()
            .find(|n| n.id == node)
            .and_then(|n| n.channels.iter().find(|c| c.channel == channel))
            .cloned();
        let base = ch.as_ref().and_then(|c| c.last_value).unwrap_or(0) as f64;
        let digital = ch.as_ref().is_some_and(|c| c.kind.is_digital());
        let span = (t - since).max(60);
        let wave = |ts: i64| ((ts % 86_400) as f64 / 86_400.0 * std::f64::consts::TAU).sin();
        // A daily swing that ends at today's live value.
        let now_wave = wave(t);
        (0..=160)
            .map(|i| {
                let ts = since + span * i / 160;
                let value = if digital {
                    i64::from(wave(ts) > 0.3)
                } else {
                    let noise = if i == 160 {
                        0.0
                    } else {
                        self.noise() * base * 0.004
                    };
                    (base + (wave(ts) - now_wave) * base * 0.12 + noise) as i64
                };
                Reading { ts, value }
            })
            .collect()
    }

    fn save_recipe(&mut self, mut r: Recipe) -> u32 {
        if r.id == 0 {
            r.id = self.recipes.iter().map(|r| r.id).max().unwrap_or(0) + 1;
            let id = r.id;
            self.recipes.push(r);
            id
        } else {
            let id = r.id;
            if let Some(old) = self.recipes.iter_mut().find(|x| x.id == id) {
                *old = r;
            }
            id
        }
    }

    fn approve(&mut self, key: &str, name: Option<String>) {
        if let Some(i) = self.pending.iter().position(|p| p.public_key == key) {
            let p = self.pending.remove(i);
            let id = self.nodes.iter().map(|n| n.id).max().unwrap_or(0) + 1;
            self.nodes.push(NodeInfo {
                id,
                name: name.unwrap_or_else(|| format!("Node {id}")),
                fingerprint: p.fingerprint,
                paired_at: format::now(),
                last_seen: None,
                transport: p.transport,
                sessions: 0,
                channels: Vec::new(),
            });
        }
    }

    fn ticket(&mut self) -> PhoneTicket {
        let id = self.phones.iter().map(|p| p.id).max().unwrap_or(0) + 1;
        // A fixed sample: a real QR code comes from the daemon.
        let mut svg = String::from(
            r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 25 25" shape-rendering="crispEdges"><rect width="25" height="25" fill="#fff"/>"##,
        );
        for (x, y) in [(0, 0), (18, 0), (0, 18)] {
            svg += &format!(
                r##"<path d="M{x} {y}h7v7h-7zM{a} {b}h5v5h-5z" fill="#000" fill-rule="evenodd"/><rect x="{c}" y="{d}" width="3" height="3"/>"##,
                a = x + 1,
                b = y + 1,
                c = x + 2,
                d = y + 2
            );
        }
        for i in 0..90u32 {
            let (x, y) = (8 + (i * 7) % 10, (i * 13) % 25);
            if (x > 7 && x < 17) || (y > 7 && y < 17) {
                svg += &format!(r#"<rect x="{x}" y="{y}" width="1" height="1"/>"#);
            }
        }
        svg += "</svg>";
        PhoneTicket {
            phone_id: id,
            uri: "farmlab://pair?v=2&a=192.168.1.20:7302&k=e5a06b13f8d24c71&t=00&e=0".into(),
            svg,
            expires: format::now() + 300,
            hub_fingerprint: "E5A0-6B13-F8D2-4C71".into(),
        }
    }

    fn pair_phone(&mut self) -> PhoneState {
        self.phone.paired = true;
        self.phone.clone()
    }
}

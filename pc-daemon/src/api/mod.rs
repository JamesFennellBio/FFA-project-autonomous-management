//! REST API for the Electron app. JSON only, every route needs the bearer token.
//!
//! | Method | Path                                          | What it does |
//! |--------|-----------------------------------------------|--------------|
//! | GET    | /api/status                                   | hub link + PC fingerprint |
//! | GET    | /api/pairing/requests                         | devices waiting for approval |
//! | POST   | /api/pairing/requests/{key}/approve           | approve (body: `{"name": "..."}`) |
//! | POST   | /api/pairing/requests/{key}/reject            | dismiss |
//! | GET    | /api/nodes                                    | nodes with their channels and last values |
//! | PATCH  | /api/nodes/{id}                               | rename (`{"name"}`) |
//! | DELETE | /api/nodes/{id}                               | unpair |
//! | PATCH  | /api/nodes/{id}/channels/{ch}                 | rename a channel |
//! | POST   | /api/nodes/{id}/channels/{ch}/command         | set an output (`{"value": 1}`) |
//! | GET    | /api/readings?node_id&channel&since&until&limit | history |
//! | GET    | /api/alerts?unacked=true                      | alerts |
//! | POST   | /api/alerts/{id}/ack                          | acknowledge |
//! | GET    | /api/phones                                   | phones paired with the hub |
//! | DELETE | /api/phones/{id}                              | unpair a phone |
//! | GET    | /api/recipes · POST /api/recipes              | hub recipes (Blockly); passed to the hub |
//! | GET    | /api/recipes/{id} · PUT · DELETE              | |
//! | POST   | /api/recipes/{id}/enabled                     | `{"enabled": true}` |

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router, middleware};
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::{Value, json};
use uplink::{DeviceKind, PcMsg, RecipeOp, unix_now};

use crate::state::{AppState, PairKind};
use crate::{auth, db};

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/status", get(status))
        .route("/api/pairing/requests", get(list_pending))
        .route("/api/pairing/requests/{key}/approve", post(approve))
        .route("/api/pairing/requests/{key}/reject", post(reject))
        .route("/api/nodes", get(list_nodes))
        .route("/api/nodes/{id}", patch(rename_node).delete(delete_node))
        .route("/api/nodes/{id}/channels/{ch}", patch(rename_channel))
        .route("/api/nodes/{id}/channels/{ch}/command", post(command))
        .route("/api/phones", get(list_phones))
        .route("/api/phones/{id}", delete(delete_phone))
        .route("/api/readings", get(readings))
        .route("/api/alerts", get(alerts))
        .route("/api/alerts/{id}/ack", post(ack_alert))
        .route("/api/recipes", get(list_recipes).post(create_recipe))
        .route(
            "/api/recipes/{id}",
            get(get_recipe).put(update_recipe).delete(delete_recipe),
        )
        .route("/api/recipes/{id}/enabled", post(set_recipe_enabled))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .with_state(state)
}

#[derive(Debug)]
pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, format!("{:#}", e.into()))
    }
}

impl ApiError {
    pub fn message(&self) -> &str {
        &self.1
    }
}

fn err(code: StatusCode, msg: &str) -> ApiError {
    ApiError(code, msg.to_string())
}

type ApiResult<T = Json<Value>> = Result<T, ApiError>;
type St = State<Arc<AppState>>;

async fn status(State(s): St) -> ApiResult {
    let paired = db::paired_hub(&s.db.lock().unwrap())?;
    let hub = s.hub.lock().unwrap();
    Ok(Json(json!({
        "pc_fingerprint": uplink::v2::fingerprint(s.identity.public_key()),
        // Goes in hub.toml as pc_key_id.
        "pc_key_id": hex::encode(s.identity.key_id()),
        "hub": {
            "paired": paired.is_some(),
            "fingerprint": paired.map(|(_, k)| uplink::v2::fingerprint(&k)),
            "connected": hub.connected,
            "last_seen": hub.last_seen,
            "backlog": hub.backlog,
            "sessions": hub.sessions,
        },
        "time": unix_now(),
    })))
}

async fn list_pending(State(s): St) -> ApiResult {
    Ok(Json(json!(s.pending_list(unix_now()))))
}

#[derive(Deserialize, Default)]
struct NameBody {
    name: Option<String>,
}

/// CRYPTO-REVIEW: the human approval step of pairing. A device's key is only
/// trusted after someone clicks Approve on its fingerprint here.
async fn approve(State(s): St, Path(key): Path<String>, body: Option<Json<NameBody>>) -> ApiResult {
    let name = body.and_then(|b| b.0.name);
    approve_key(&s, &key, name, false).map(Json)
}

/// Approves a waiting device. `nodes_only` is set when a paired phone asked
/// (through the hub): a phone may approve nodes, but never a hub or another phone.
///
/// CRYPTO-REVIEW: only keys that are in the pending list (they sent a signed
/// pairing request in the last few minutes) can be approved.
pub(crate) fn approve_key(
    s: &AppState,
    key: &str,
    name: Option<String>,
    nodes_only: bool,
) -> Result<Value, ApiError> {
    let name = name.filter(|n| !n.trim().is_empty());
    let mut pending = s.pending.lock().unwrap();
    if nodes_only && pending.get(key).is_some_and(|p| p.kind != PairKind::Node) {
        return Err(err(StatusCode::FORBIDDEN, "phones can only approve nodes"));
    }
    let p = pending.remove(key).ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "no such pairing request (it may have expired)",
        )
    })?;
    drop(pending);
    match p.kind {
        PairKind::Phone => {
            let phone_id = {
                let conn = s.db.lock().unwrap();
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM phones WHERE public_key = ?1",
                        [&p.public_key],
                        |r| r.get(0),
                    )
                    .optional()?;
                match existing {
                    Some(id) => id as u16,
                    None => {
                        let id = db::next_phone_id(&conn)?;
                        conn.execute(
                            "INSERT INTO phones (id, public_key, name, paired_at) VALUES (?1, ?2, ?3, ?4)",
                            params![id, p.public_key, name.unwrap_or_else(|| format!("Phone {id}")), unix_now()],
                        )?;
                        id
                    }
                }
            };
            if !s.send_to_hub(PcMsg::ApprovePair {
                public_key: p.public_key.clone(),
                node_id: phone_id,
                kind: DeviceKind::Phone,
            }) {
                s.add_pending(p);
                return Err(err(StatusCode::SERVICE_UNAVAILABLE, "hub is offline"));
            }
            Ok(json!({ "ok": true, "kind": "phone", "phone_id": phone_id }))
        }
        PairKind::Node => {
            let node_id = {
                let conn = s.db.lock().unwrap();
                let existing: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM nodes WHERE public_key = ?1",
                        [&p.public_key],
                        |r| r.get(0),
                    )
                    .optional()?;
                match existing {
                    Some(id) => id as u16,
                    None => {
                        let id = db::next_node_id(&conn)?;
                        conn.execute(
                            "INSERT INTO nodes (id, public_key, name, paired_at) VALUES (?1, ?2, ?3, ?4)",
                            params![id, p.public_key, name.unwrap_or_else(|| format!("Node {id}")), unix_now()],
                        )?;
                        id
                    }
                }
            };
            if !s.send_to_hub(PcMsg::ApprovePair {
                public_key: p.public_key.clone(),
                node_id,
                kind: DeviceKind::Node,
            }) {
                s.add_pending(p);
                return Err(err(StatusCode::SERVICE_UNAVAILABLE, "hub is offline"));
            }
            send_labels(s);
            Ok(json!({ "ok": true, "kind": "node", "node_id": node_id }))
        }
    }
}

/// Sends node/channel names to the hub for the phone app (skipped if the hub
/// is offline; it gets them again when it connects).
fn send_labels(s: &AppState) {
    let labels = db::labels(&s.db.lock().unwrap());
    match labels {
        Ok(m) => {
            s.send_to_hub(m);
        }
        Err(e) => tracing::warn!("reading labels: {e:#}"),
    }
}

async fn list_phones(State(s): St) -> ApiResult {
    let conn = s.db.lock().unwrap();
    let phones: Vec<Value> = conn
        .prepare("SELECT id, public_key, name, paired_at FROM phones ORDER BY id")?
        .query_map([], |r| {
            let pk: String = r.get(1)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                // Phones are stored by KeyId (v2).
                "fingerprint": uplink::key_from_hex(&pk).map(|k| uplink::v2::fingerprint_of_key_id(&k)).unwrap_or_default(),
                "name": r.get::<_, String>(2)?,
                "paired_at": r.get::<_, i64>(3)?,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Json(json!(phones)))
}

/// Unpairs a phone. Like nodes, the hub must hear about it, so this refuses
/// while the hub is offline (a lost phone must really stop working).
async fn delete_phone(State(s): St, Path(id): Path<u16>) -> ApiResult {
    if !s.send_to_hub(PcMsg::Unpair {
        node_id: id,
        kind: DeviceKind::Phone,
    }) {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "hub is offline; it must be connected to unpair a phone",
        ));
    }
    s.db.lock()
        .unwrap()
        .execute("DELETE FROM phones WHERE id = ?1", [id])?;
    Ok(Json(json!({ "ok": true })))
}

async fn reject(State(s): St, Path(key): Path<String>) -> ApiResult {
    s.pending.lock().unwrap().remove(&key);
    Ok(Json(json!({ "ok": true })))
}

async fn list_nodes(State(s): St) -> ApiResult {
    let conn = s.db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, public_key, name, paired_at, last_seen, transport, sessions FROM nodes ORDER BY id",
    )?;
    let mut nodes: Vec<Value> = stmt
        .query_map([], |r| {
            let pk: String = r.get(1)?;
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "fingerprint": uplink::key_from_hex(&pk).map(|k| uplink::fingerprint_hex(&k)).unwrap_or_default(),
                "name": r.get::<_, String>(2)?,
                "paired_at": r.get::<_, i64>(3)?,
                "last_seen": r.get::<_, Option<i64>>(4)?,
                "transport": r.get::<_, Option<String>>(5)?,
                "sessions": r.get::<_, i64>(6)?,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut cstmt = conn.prepare(
        "SELECT channel, kind, name, last_value, last_ts FROM channels WHERE node_id = ?1 ORDER BY channel",
    )?;
    for n in nodes.iter_mut() {
        let id = n["id"].as_i64().unwrap_or(0);
        let chans: Vec<Value> = cstmt
            .query_map([id], |r| {
                let kind: i64 = r.get(1)?;
                Ok(json!({
                    "channel": r.get::<_, i64>(0)?,
                    "kind": db::kind_name(kind),
                    "name": r.get::<_, String>(2)?,
                    "last_value": r.get::<_, Option<i64>>(3)?,
                    "last_ts": r.get::<_, Option<i64>>(4)?,
                }))
            })?
            .collect::<rusqlite::Result<_>>()?;
        n["channels"] = json!(chans);
    }
    Ok(Json(json!(nodes)))
}

async fn rename_node(State(s): St, Path(id): Path<u16>, Json(b): Json<NameBody>) -> ApiResult {
    let name = b
        .name
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "name required"))?;
    let n = s.db.lock().unwrap().execute(
        "UPDATE nodes SET name = ?2 WHERE id = ?1",
        params![id, name],
    )?;
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such node"));
    }
    send_labels(&s);
    Ok(Json(json!({ "ok": true })))
}

/// Unpairs a node. The hub has to hear about it, or it would keep trusting
/// the node's key, so this refuses while the hub is offline. Its id is never
/// handed out again (see db::next_node_id), so nothing that still names it
/// can act on another device (finding 16).
async fn delete_node(State(s): St, Path(id): Path<u16>) -> ApiResult {
    if !s.send_to_hub(PcMsg::Unpair {
        node_id: id,
        kind: DeviceKind::Node,
    }) {
        return Err(err(
            StatusCode::SERVICE_UNAVAILABLE,
            "hub is offline; it must be connected to unpair a node",
        ));
    }
    let conn = s.db.lock().unwrap();
    conn.execute("DELETE FROM nodes WHERE id = ?1", [id])?;
    Ok(Json(json!({ "ok": true })))
}

async fn rename_channel(
    State(s): St,
    Path((id, ch)): Path<(u16, u8)>,
    Json(b): Json<NameBody>,
) -> ApiResult {
    let name = b
        .name
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "name required"))?;
    s.db.lock().unwrap().execute(
        "UPDATE channels SET name = ?3 WHERE node_id = ?1 AND channel = ?2",
        params![id, ch, name],
    )?;
    send_labels(&s);
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
struct CommandBody {
    value: i32,
}

async fn command(
    State(s): St,
    Path((id, ch)): Path<(u16, u8)>,
    Json(b): Json<CommandBody>,
) -> ApiResult {
    {
        let conn = s.db.lock().unwrap();
        let kind: Option<i64> = conn
            .query_row(
                "SELECT kind FROM channels WHERE node_id = ?1 AND channel = ?2",
                params![id, ch],
                |r| r.get(0),
            )
            .optional()?;
        match kind {
            None => return Err(err(StatusCode::NOT_FOUND, "no such channel")),
            Some(1) => {}
            Some(_) => return Err(err(StatusCode::BAD_REQUEST, "channel is not an output")),
        }
        db::insert_command(&conn, id, ch, b.value, "app", unix_now())?;
    }
    if !s.send_to_hub(PcMsg::Command {
        node_id: id,
        channel: ch,
        value: b.value,
    }) {
        return Err(err(StatusCode::SERVICE_UNAVAILABLE, "hub is offline"));
    }
    Ok(Json(json!({ "ok": true })))
}

/// How long an API call waits for the hub to answer a recipe request. The hub
/// sends to the PC once a second.
const RECIPE_TIMEOUT: Duration = Duration::from_secs(10);
/// Hub recipes are at most ~1.1 KB of JSON; anything far bigger is refused
/// here so it can't make an uplink frame too big.
const MAX_RECIPE_BODY: usize = 4096;

/// Recipes live on the hub (hub-node/src/recipes.rs). The PC only passes
/// requests on and returns the hub's answer, so the phone and desktop apps
/// always see the same recipes.
async fn ask_hub(s: &AppState, op: RecipeOp) -> ApiResult {
    let rx = s
        .send_recipe(op)
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "hub is offline"))?;
    match tokio::time::timeout(RECIPE_TIMEOUT, rx).await {
        Ok(Ok(Ok(v))) => Ok(Json(v)),
        Ok(Ok(Err(why))) => Err(err(StatusCode::BAD_REQUEST, &why)),
        _ => Err(err(StatusCode::GATEWAY_TIMEOUT, "the hub didn't answer")),
    }
}

fn recipe_body(mut recipe: Value, id: u32) -> Result<RecipeOp, ApiError> {
    let obj = recipe
        .as_object_mut()
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "recipe must be an object"))?;
    obj.insert("id".into(), json!(id));
    if serde_json::to_vec(&recipe).map_or(usize::MAX, |v| v.len()) > MAX_RECIPE_BODY {
        return Err(err(StatusCode::PAYLOAD_TOO_LARGE, "recipe is too big"));
    }
    Ok(RecipeOp::Save { recipe })
}

async fn list_recipes(State(s): St) -> ApiResult {
    ask_hub(&s, RecipeOp::List).await
}

async fn get_recipe(State(s): St, Path(id): Path<u32>) -> ApiResult {
    ask_hub(&s, RecipeOp::Get { id }).await
}

async fn create_recipe(State(s): St, Json(recipe): Json<Value>) -> ApiResult {
    ask_hub(&s, recipe_body(recipe, 0)?).await
}

async fn update_recipe(State(s): St, Path(id): Path<u32>, Json(recipe): Json<Value>) -> ApiResult {
    if id == 0 {
        return Err(err(StatusCode::BAD_REQUEST, "no such recipe"));
    }
    ask_hub(&s, recipe_body(recipe, id)?).await
}

async fn delete_recipe(State(s): St, Path(id): Path<u32>) -> ApiResult {
    ask_hub(&s, RecipeOp::Delete { id }).await
}

#[derive(Deserialize)]
struct EnabledBody {
    enabled: bool,
}

async fn set_recipe_enabled(
    State(s): St,
    Path(id): Path<u32>,
    Json(b): Json<EnabledBody>,
) -> ApiResult {
    ask_hub(
        &s,
        RecipeOp::SetEnabled {
            id,
            enabled: b.enabled,
        },
    )
    .await
}

#[derive(Deserialize)]
struct ReadingsQuery {
    node_id: u16,
    channel: u8,
    since: Option<i64>,
    until: Option<i64>,
    limit: Option<i64>,
}

async fn readings(State(s): St, Query(q): Query<ReadingsQuery>) -> ApiResult {
    let now = unix_now();
    let since = q.since.unwrap_or(now - 86_400);
    let until = q.until.unwrap_or(now + 60);
    let limit = q.limit.unwrap_or(5_000).clamp(1, 50_000);
    let conn = s.db.lock().unwrap();
    // Newest `limit` rows in the window, returned oldest first for charts.
    let mut stmt = conn.prepare(
        "SELECT ts, value FROM (
            SELECT ts, value FROM readings
            WHERE node_id = ?1 AND channel = ?2 AND ts >= ?3 AND ts <= ?4
            ORDER BY ts DESC LIMIT ?5
         ) ORDER BY ts ASC",
    )?;
    let rows: Vec<Value> = stmt
        .query_map(params![q.node_id, q.channel, since, until, limit], |r| {
            Ok(json!({ "ts": r.get::<_, i64>(0)?, "value": r.get::<_, i64>(1)? }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Json(json!(rows)))
}

#[derive(Deserialize)]
struct AlertsQuery {
    unacked: Option<bool>,
    limit: Option<i64>,
}

async fn alerts(State(s): St, Query(q): Query<AlertsQuery>) -> ApiResult {
    let conn = s.db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT a.id, a.node_id, n.name, a.channel, a.code, a.value, a.message, a.ts, a.acked
         FROM alerts a LEFT JOIN nodes n ON n.id = a.node_id
         WHERE (?1 = 0 OR a.acked = 0) ORDER BY a.ts DESC LIMIT ?2",
    )?;
    let rows: Vec<Value> = stmt
        .query_map(
            params![
                q.unacked.unwrap_or(false) as i64,
                q.limit.unwrap_or(200).clamp(1, 5000)
            ],
            |r| {
                Ok(json!({
                    "id": r.get::<_, i64>(0)?,
                    "node_id": r.get::<_, Option<i64>>(1)?,
                    "node_name": r.get::<_, Option<String>>(2)?,
                    "channel": r.get::<_, Option<i64>>(3)?,
                    "code": r.get::<_, i64>(4)?,
                    "value": r.get::<_, Option<i64>>(5)?,
                    "message": r.get::<_, String>(6)?,
                    "ts": r.get::<_, i64>(7)?,
                    "acked": r.get::<_, i64>(8)? != 0,
                }))
            },
        )?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Json(json!(rows)))
}

async fn ack_alert(State(s): St, Path(id): Path<i64>) -> ApiResult {
    s.db.lock()
        .unwrap()
        .execute("UPDATE alerts SET acked = 1 WHERE id = ?1", [id])?;
    Ok(Json(json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    fn app() -> Router {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        router(Arc::new(AppState::new(conn, &[9u8; 32], "t".repeat(64))))
    }

    #[tokio::test]
    async fn rejects_missing_token() {
        let r = app()
            .oneshot(Request::get("/api/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn status_with_token() {
        let app = app();
        let auth = format!("Bearer {}", "t".repeat(64));
        let r = app
            .clone()
            .oneshot(
                Request::get("/api/status")
                    .header("authorization", &auth)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        let body = r.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["hub"]["paired"], false);
    }

    #[test]
    fn phones_pair_with_their_own_ids_and_can_only_approve_nodes() {
        use crate::state::PendingPair;
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let state = AppState::new(conn, &[9u8; 32], "t".repeat(64));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        state.hub.lock().unwrap().tx = Some(tx);
        let pending = |kind, key: &str| PendingPair {
            kind,
            public_key: key.to_string(),
            fingerprint: String::new(),
            transport: None,
            seen_at: unix_now(),
        };
        let phone_key = "11".repeat(32);
        let node_key = "22".repeat(32);
        state.add_pending(pending(PairKind::Phone, &phone_key));
        state.add_pending(pending(PairKind::Node, &node_key));

        // A phone (through the hub) can't approve another phone...
        let e = approve_key(&state, &phone_key, None, true).unwrap_err();
        assert_eq!(e.0, StatusCode::FORBIDDEN);
        // ...but the desktop app can, and phone ids are separate from node ids.
        let v = approve_key(&state, &phone_key, Some("Pixel".into()), false).unwrap();
        assert_eq!(v["phone_id"], 1);
        assert_eq!(
            rx.try_recv().unwrap(),
            PcMsg::ApprovePair {
                public_key: phone_key,
                node_id: 1,
                kind: DeviceKind::Phone
            }
        );
        // A phone can approve a waiting node.
        let v = approve_key(&state, &node_key, Some("Barn".into()), true).unwrap();
        assert_eq!(v["node_id"], 1);
        assert!(matches!(
            rx.try_recv().unwrap(),
            PcMsg::ApprovePair {
                kind: DeviceKind::Node,
                ..
            }
        ));
        assert!(
            matches!(rx.try_recv().unwrap(), PcMsg::Labels { nodes, .. } if nodes == vec![(1, "Barn".to_string())])
        );
        // Nothing left to approve.
        assert!(approve_key(&state, &node_key, None, true).is_err());
    }

    #[tokio::test]
    async fn recipe_requests_go_to_the_hub_and_wait_for_its_reply() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        db::init(&conn).unwrap();
        let state = Arc::new(AppState::new(conn, &[9u8; 32], "t".repeat(64)));

        // Hub offline: refused at once.
        let e = ask_hub(&state, RecipeOp::List).await.unwrap_err();
        assert_eq!(e.0, StatusCode::SERVICE_UNAVAILABLE);

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        state.hub.lock().unwrap().tx = Some(tx);
        // Plays the hub: answers the first request, refuses the second.
        let hub = {
            let state = state.clone();
            tokio::spawn(async move {
                for answer in [
                    Ok(json!({ "id": 3 })),
                    Err("node 9 isn't paired".to_string()),
                ] {
                    let Some(PcMsg::Recipe { req, op }) = rx.recv().await else {
                        panic!("no request")
                    };
                    assert!(matches!(op, RecipeOp::Save { ref recipe } if recipe["id"] == 0));
                    state.recipe_reply(req, answer);
                }
            })
        };
        let body = json!({ "name": "x", "when": {}, "then": [] });
        let Json(v) = ask_hub(&state, recipe_body(body.clone(), 0).unwrap())
            .await
            .unwrap();
        assert_eq!(v["id"], 3);
        let e = ask_hub(&state, recipe_body(body, 0).unwrap())
            .await
            .unwrap_err();
        assert_eq!(
            (e.0, e.message()),
            (StatusCode::BAD_REQUEST, "node 9 isn't paired")
        );
        hub.await.unwrap();

        // Too big to send over the uplink.
        let huge = json!({ "name": "x".repeat(MAX_RECIPE_BODY) });
        assert_eq!(
            recipe_body(huge, 0).unwrap_err().0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
}

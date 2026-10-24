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
//! | GET    | /api/rules  · POST /api/rules                 | automation rules |
//! | PUT    | /api/rules/{id} · DELETE /api/rules/{id}      | |

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post, put};
use axum::{Json, Router, middleware};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uplink::{PcMsg, unix_now};

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
        .route("/api/readings", get(readings))
        .route("/api/alerts", get(alerts))
        .route("/api/alerts/{id}/ack", post(ack_alert))
        .route("/api/rules", get(list_rules).post(create_rule))
        .route("/api/rules/{id}", put(update_rule).delete(delete_rule))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_token,
        ))
        .with_state(state)
}

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

fn err(code: StatusCode, msg: &str) -> ApiError {
    ApiError(code, msg.to_string())
}

type ApiResult<T = Json<Value>> = Result<T, ApiError>;
type St = State<Arc<AppState>>;

async fn status(State(s): St) -> ApiResult {
    let paired = db::paired_hub(&s.db.lock().unwrap())?;
    let hub = s.hub.lock().unwrap();
    Ok(Json(json!({
        "pc_fingerprint": uplink::fingerprint_hex(&s.identity.public_key()),
        "hub": {
            "paired": paired.is_some(),
            "fingerprint": paired.map(|(_, k)| uplink::fingerprint_hex(&k)),
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
    let p = s.pending.lock().unwrap().remove(&key).ok_or_else(|| {
        err(
            StatusCode::NOT_FOUND,
            "no such pairing request (it may have expired)",
        )
    })?;
    let pk = uplink::key_from_hex(&p.public_key)?;
    match p.kind {
        PairKind::Hub => {
            *s.approved_hub.lock().unwrap() = Some(pk);
            Ok(Json(json!({ "ok": true, "kind": "hub" })))
        }
        PairKind::Node => {
            let name = body.and_then(|b| b.0.name).filter(|n| !n.trim().is_empty());
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
            }) {
                s.add_pending(p);
                return Err(err(StatusCode::SERVICE_UNAVAILABLE, "hub is offline"));
            }
            Ok(Json(
                json!({ "ok": true, "kind": "node", "node_id": node_id }),
            ))
        }
    }
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
    Ok(Json(json!({ "ok": true })))
}

async fn delete_node(State(s): St, Path(id): Path<u16>) -> ApiResult {
    s.db.lock()
        .unwrap()
        .execute("DELETE FROM nodes WHERE id = ?1", [id])?;
    s.send_to_hub(PcMsg::Unpair { node_id: id });
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

#[derive(Deserialize, Serialize)]
struct RuleBody {
    name: String,
    #[serde(default = "yes")]
    enabled: bool,
    node_id: u16,
    channel: u8,
    op: String,
    threshold: i32,
    target_node: u16,
    target_channel: u8,
    #[serde(default = "one")]
    on_value: i32,
    off_value: Option<i32>,
}

fn yes() -> bool {
    true
}
fn one() -> i32 {
    1
}

impl RuleBody {
    fn check(&self) -> Result<(), ApiError> {
        if self.op != "gt" && self.op != "lt" {
            return Err(err(StatusCode::BAD_REQUEST, "op must be \"gt\" or \"lt\""));
        }
        if self.name.trim().is_empty() {
            return Err(err(StatusCode::BAD_REQUEST, "name required"));
        }
        Ok(())
    }
}

async fn list_rules(State(s): St) -> ApiResult {
    let conn = s.db.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT id, name, enabled, node_id, channel, op, threshold, target_node, target_channel, on_value, off_value, active
         FROM rules ORDER BY id",
    )?;
    let rows: Vec<Value> = stmt
        .query_map([], |r| {
            Ok(json!({
                "id": r.get::<_, i64>(0)?,
                "name": r.get::<_, String>(1)?,
                "enabled": r.get::<_, i64>(2)? != 0,
                "node_id": r.get::<_, i64>(3)?,
                "channel": r.get::<_, i64>(4)?,
                "op": r.get::<_, String>(5)?,
                "threshold": r.get::<_, i64>(6)?,
                "target_node": r.get::<_, i64>(7)?,
                "target_channel": r.get::<_, i64>(8)?,
                "on_value": r.get::<_, i64>(9)?,
                "off_value": r.get::<_, Option<i64>>(10)?,
                "active": r.get::<_, i64>(11)? != 0,
            }))
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(Json(json!(rows)))
}

async fn create_rule(State(s): St, Json(b): Json<RuleBody>) -> ApiResult {
    b.check()?;
    let conn = s.db.lock().unwrap();
    conn.execute(
        "INSERT INTO rules (name, enabled, node_id, channel, op, threshold, target_node, target_channel, on_value, off_value)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![b.name, b.enabled, b.node_id, b.channel, b.op, b.threshold, b.target_node, b.target_channel, b.on_value, b.off_value],
    )?;
    Ok(Json(json!({ "ok": true, "id": conn.last_insert_rowid() })))
}

async fn update_rule(State(s): St, Path(id): Path<i64>, Json(b): Json<RuleBody>) -> ApiResult {
    b.check()?;
    let n = s.db.lock().unwrap().execute(
        "UPDATE rules SET name=?2, enabled=?3, node_id=?4, channel=?5, op=?6, threshold=?7,
            target_node=?8, target_channel=?9, on_value=?10, off_value=?11, active=0 WHERE id=?1",
        params![
            id,
            b.name,
            b.enabled,
            b.node_id,
            b.channel,
            b.op,
            b.threshold,
            b.target_node,
            b.target_channel,
            b.on_value,
            b.off_value
        ],
    )?;
    if n == 0 {
        return Err(err(StatusCode::NOT_FOUND, "no such rule"));
    }
    Ok(Json(json!({ "ok": true })))
}

async fn delete_rule(State(s): St, Path(id): Path<i64>) -> ApiResult {
    s.db.lock()
        .unwrap()
        .execute("DELETE FROM rules WHERE id = ?1", [id])?;
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
    async fn status_and_rules_with_token() {
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

        let rule = json!({"name":"hot","node_id":1,"channel":0,"op":"gt","threshold":30000,"target_node":1,"target_channel":2});
        let r = app
            .clone()
            .oneshot(
                Request::post("/api/rules")
                    .header("authorization", &auth)
                    .header("content-type", "application/json")
                    .body(Body::from(rule.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }
}

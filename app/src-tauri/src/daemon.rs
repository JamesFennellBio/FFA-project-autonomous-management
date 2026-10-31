//! Desktop backend: the pc-daemon REST API on 127.0.0.1:7401.
//!
//! Port of the Electron main process (frontend/electron/main.ts): the API
//! token is read here and added to each request, so the web page (WASM)
//! never sees it, and only `/api/...` paths with known methods are passed on.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};

fn api_url() -> String {
    std::env::var("FARMLAB_API_URL").unwrap_or_else(|_| "http://127.0.0.1:7401".into())
}

/// The same file pc-daemon writes: the `directories` crate with
/// ("org", "farmlab", "farmlab-daemon"), so it matches on every OS.
pub fn token_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("FARMLAB_TOKEN_FILE") {
        return Some(p.into());
    }
    directories::ProjectDirs::from("org", "farmlab", "farmlab-daemon")
        .map(|d| d.config_dir().join("api-token"))
}

fn read_token() -> Option<String> {
    let t = std::fs::read_to_string(token_path()?).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_string())
}

fn client() -> &'static reqwest::Client {
    static C: OnceLock<reqwest::Client> = OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("http client")
    })
}

#[derive(Serialize)]
pub struct ApiReply {
    ok: bool,
    status: u16,
    data: Value,
}

fn fail(status: u16, msg: String) -> ApiReply {
    ApiReply {
        ok: false,
        status,
        data: json!({ "error": msg }),
    }
}

/// Only ever our own API, whatever the page asks for.
pub fn allowed(method: &str, path: &str) -> bool {
    matches!(method, "GET" | "POST" | "PATCH" | "PUT" | "DELETE")
        && path.starts_with("/api/")
        && !path.contains("..")
        && !path.contains('#')
        && !path.contains("://")
}

#[tauri::command(rename_all = "snake_case")]
pub async fn daemon_api(method: String, path: String, body: Option<Value>) -> ApiReply {
    if !allowed(&method, &path) {
        return fail(400, "bad request".into());
    }
    let Some(token) = read_token() else {
        let where_ = token_path().map_or_else(|| "?".into(), |p| p.display().to_string());
        return fail(
            0,
            format!("The daemon's token isn't at {where_}. Is pc-daemon running?"),
        );
    };
    let m = reqwest::Method::from_bytes(method.as_bytes()).unwrap_or(reqwest::Method::GET);
    let mut req = client()
        .request(m, format!("{}{path}", api_url()))
        .bearer_auth(token);
    if let Some(b) = body {
        req = req.json(&b);
    }
    match req.send().await {
        Ok(res) => {
            let status = res.status();
            let text = res.text().await.unwrap_or_default();
            let data = if text.is_empty() {
                Value::Null
            } else {
                serde_json::from_str(&text).unwrap_or_else(|_| json!({ "error": text }))
            };
            ApiReply {
                ok: status.is_success(),
                status: status.as_u16(),
                data,
            }
        }
        Err(e) => fail(0, format!("Can't reach the daemon at {} ({e})", api_url())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_api_is_passed_on() {
        assert!(allowed("GET", "/api/status"));
        assert!(allowed("POST", "/api/phones/ticket"));
        assert!(!allowed("GET", "/other"));
        assert!(!allowed("TRACE", "/api/status"));
        assert!(!allowed("GET", "/api/../secret"));
        assert!(!allowed("GET", "/api/x?u=http://evil"));
    }
}

//! Bearer-token check for the REST API.
//!
//! The API only listens on 127.0.0.1, but other programs on the same PC could
//! still call it, so it also needs a token. The token is a random file in the
//! config dir that only this user can read; the Electron main process reads it.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;

use crate::state::AppState;

/// CRYPTO-REVIEW: 256-bit random token from the OS RNG, stored with mode 0600.
pub fn load_or_create_token(path: &Path) -> Result<String> {
    if let Ok(s) = std::fs::read_to_string(path) {
        let s = s.trim().to_string();
        if s.len() >= 32 {
            return Ok(s);
        }
    }
    let mut b = [0u8; 32];
    uplink::getrandom(&mut b)?;
    let t = hex::encode(b);
    uplink::write_secret(path, &t)?;
    Ok(t)
}

/// CRYPTO-REVIEW: constant-time compare, so response timing doesn't leak how
/// much of the token was right.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub async fn require_token(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let ok = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| ct_eq(t.as_bytes(), state.token.as_bytes()));
    if ok {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

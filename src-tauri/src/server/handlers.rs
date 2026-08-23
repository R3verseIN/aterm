//! handlers.rs — Per-tab scoped HTTP handlers (port is the id).
//!
//! Each handler is a closure factory `make_*` that captures the tab `id` it is
//! scoped to. The router is built per share in `share.rs` with `id.clone()`
//! so `GET /output` means `GET /output` for that tab's `OUTPUTS` ring,
//! without needing a `:id` path param. No auth — localhost-only isolation
//! is the capability (port is the secret).

use axum::{extract::Query, http::StatusCode, response::IntoResponse, Json};
use serde::{Deserialize, Serialize};

use crate::pty;

/// Query for `GET /output?since=0&limit=32768` polling.
/// `since` is byte offset into the 512KB `OUTPUTS` ring, `next_offset` is next cursor.
#[derive(Deserialize)]
pub struct OutputQuery {
    pub since: Option<usize>,
    pub limit: Option<usize>,
}

/// Retained for documentation — the per-tab `GET /output` currently builds
/// JSON inline via `json!({"data", "next_offset", "total", "truncated", "id"})`
/// so this struct is unused but kept to document the intended shape.
#[allow(dead_code)]
#[derive(Serialize)]
struct OutputResp {
    data: String,
    next_offset: usize,
    total: usize,
    truncated: bool,
}

#[derive(Deserialize)]
pub struct WriteReq {
    pub data: String,
}

#[derive(Deserialize)]
pub struct ResizeReq {
    pub cols: u16,
    pub rows: u16,
}

/// GET /health — version + session count + this tab's id (for agent to confirm).
pub async fn health_scoped(id: String) -> impl IntoResponse {
    let alive = crate::pty::session_exists(&id);
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "id": id,
        "alive": alive,
        "pid": pty::list_sessions().into_iter().find(|m| m.id == id).and_then(|m| m.pid),
    }))
}

/// GET /output?since=0&limit=32768 — live poll for this tab's PTY output.
/// Lossy UTF-8 string, next_offset to use for next poll.
pub async fn get_output_scoped(Query(q): Query<OutputQuery>, id: String) -> impl IntoResponse {
    let since = q.since.unwrap_or(0);
    let limit = q.limit.unwrap_or(32 * 1024).min(256 * 1024);
    match pty::get_output_since(&id, since, limit) {
        Ok((bytes, next, total)) => {
            let data = String::from_utf8_lossy(&bytes).to_string();
            Json(serde_json::json!({
                "data": data,
                "next_offset": next,
                "total": total,
                "truncated": next < total,
                "id": id,
            }))
            .into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// POST /input {"data":"ls\n"} — write into the PTY master (same as xterm onData).
pub async fn post_input_scoped(Json(req): Json<WriteReq>, id: String) -> impl IntoResponse {
    match pty::write_to_session(&id, &req.data) {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// POST /resize {"cols":80,"rows":24} — SIGWINCH to the shell.
pub async fn post_resize_scoped(Json(req): Json<ResizeReq>, id: String) -> impl IntoResponse {
    match pty::resize_session(&id, req.cols, req.rows) {
        Ok(_) => Json(serde_json::json!({"ok": true})).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// GET /cwd — live working directory via /proc/<pid>/cwd.
pub async fn get_cwd_scoped(id: String) -> impl IntoResponse {
    match pty::get_cwd(&id) {
        Ok(cwd) => Json(serde_json::json!({"cwd": cwd, "id": id})).into_response(),
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

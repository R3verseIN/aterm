//! handlers.rs — Per-tab scoped HTTP handlers (port is the id).
//!
//! Each handler is a closure factory `make_*` that captures the tab `id` it is
//! scoped to. The router is built per share in `share.rs` with `id.clone()`
//! so `GET /output` means `GET /output` for that tab's `OUTPUTS` ring,
//! without needing a `:id` path param. No auth — localhost-only isolation
//! is the capability (port is the secret).

use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::pty;

/// Query for `GET /output?since=0&limit=32768` polling.
/// `since` is byte offset into the 512KB `OUTPUTS` ring, `next_offset` is next cursor.
#[derive(Deserialize)]
pub struct OutputQuery {
    pub since: Option<usize>,
    pub limit: Option<usize>,
}

/// Typed response for `GET /output?since=0&limit=…` — used by `get_output_scoped`.
///
/// This struct documents and enforces the wire shape. Previously built via
/// `json!` inline, it was flagged `dead_code` by the IDE/cargo. Now it is
/// constructed directly so `#[warn(dead_code)]` stays clean without an allow.
#[derive(Serialize)]
struct OutputResp {
    data: String,
    next_offset: usize,
    total: usize,
    truncated: bool,
    id: String,
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
            // Use the typed OutputResp struct so the shape is checked at compile time
            // and the IDE/cargo `dead_code` lint stays green without `#[allow]`.
            Json(OutputResp {
                data,
                next_offset: next,
                total,
                truncated: next < total,
                id,
            })
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

/// Query for `GET /screenshot?format=png|base64` — if `format=base64`, returns
/// JSON `{"image":"data:image/png;base64,...","id"}` for LLM data-URI vision.
/// Otherwise returns raw `image/png` bytes (single cross-compatible way).
#[derive(Deserialize)]
pub struct ScreenshotQuery {
    pub format: Option<String>,
}

/// GET /screenshot — per-tab PNG of that tab's terminal, even when not focused.
///
/// The frontend captures each tab's xterm DOM via `html2canvas` (on a cloned
/// offscreen div for hidden tabs) and pushes PNG base64 via `store_screenshot`.
/// This handler serves the cached PNG for the `id` it is scoped to (port is the
/// tab id). No auth, localhost-only; port secrecy is the capability.
/// Returns `404` if tab not shared, `503` if no screenshot cached yet (frontend
/// hasn't completed its first capture loop for that tab).
pub async fn get_screenshot_scoped(Query(q): Query<ScreenshotQuery>, id: String) -> impl IntoResponse {
    if !crate::pty::session_exists(&id) {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "session not found"}))).into_response();
    }
    // Hold-till-capture: no dummy, hold the HTTP connection until the frontend's
    // real `html2canvas` push arrives (up to 10 s as you requested). This gives
    // full stability — the agent gets the actual pixels, not a 1×1 placeholder.
    // The frontend captures via `onclone` (fix for `opacity:0` hidden tabs) every
    // ~900 ms after a 1500 ms initial delay (HMR guard). If still empty after 10 s,
    // return debug logs so the agent can see why (tainted canvas, zero size, etc.).
    let png = if let Some(b) = crate::server::state::get_screenshot(&id) {
        b
    } else if let Some(b) = crate::server::state::wait_for_screenshot(id.clone(), 10).await {
        b
    } else {
        // 10 s hold expired — return debug payload for AI to diagnose
        let last_err = crate::server::state::get_last_error(&id);
        let session_exists = crate::pty::session_exists(&id);
        let share_exists = crate::server::state::get_share(&id).is_some();
        let cache_empty = crate::server::state::get_screenshot(&id).is_none();
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "screenshot not yet available after 10s hold — frontend has not pushed a capture for this tab",
                "id": id,
                "logs": {
                    "session_exists": session_exists,
                    "share_exists": share_exists,
                    "cache_empty": cache_empty,
                    "frontend_last_error": last_err.unwrap_or_else(|| "no error reported — frontend capture loop may not have started (check is-hidden onclone, wrapper size 0, or Tauri IPC blocked)".to_string()),
                    "hint": "html2canvas onclone must set opacity:1 for [data-terminal-id] (hidden tabs use opacity:0 !important); ensure wrapper has non-zero size and store_screenshot IPC succeeds",
                    "elapsed_ms": 10000,
                    "port_is_tab_id": true
                },
                "issues": [
                    "check browser console for [screenshot] html2canvas failed",
                    "check Rust log for [screenshot] store for <id>",
                    "hidden tabs are opacity:0 — onclone fix required (see TerminalView.tsx)",
                    "wrapper size 0 — ResizeObserver not yet fitted",
                    "Tauri IPC blocked — check capabilities for store_screenshot/report_screenshot_error"
                ]
            })),
        )
            .into_response();
    };

    // `?format=base64` for LLM data-URI vision (JSON), otherwise raw PNG binary.
    let fmt = q.format.as_deref().unwrap_or("png");
    if fmt == "base64" {
        // Manual base64 encode without crate (RFC 4648). Keep zero-dep.
        const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::with_capacity(png.len() * 4 / 3 + 4);
        let mut i = 0;
        while i < png.len() {
            let b0 = png[i] as u32;
            let b1 = if i + 1 < png.len() { png[i + 1] as u32 } else { 0 };
            let b2 = if i + 2 < png.len() { png[i + 2] as u32 } else { 0 };
            let n = (b0 << 16) | (b1 << 8) | b2;
            out.push(ALPH[((n >> 18) & 63) as usize] as char);
            out.push(ALPH[((n >> 12) & 63) as usize] as char);
            out.push(if i + 1 < png.len() { ALPH[((n >> 6) & 63) as usize] as char } else { '=' });
            out.push(if i + 2 < png.len() { ALPH[(n & 63) as usize] as char } else { '=' });
            i += 3;
        }
        return Json(serde_json::json!({
            "image": format!("data:image/png;base64,{}", out),
            "id": id,
            "width": 0,  // reserved for future viewport size
            "height": 0,
        }))
        .into_response();
    }

    // Raw PNG binary — single cross-compatible way for `curl -o term.png` and vision.
    // Use a tuple response so Axum sets headers correctly without needing Response::builder.
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        png,
    )
        .into_response()
}

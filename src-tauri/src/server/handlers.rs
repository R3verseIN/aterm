//! handlers.rs — Per-tab scoped HTTP handlers (port is the id).
//!
//! Simple no-cache design: every GET is fresh, dump-all `GET /output` returns whole ring.
//! - `GET /output` dumps 512KB ring (no cursor, `since` ignored).
//! - `GET /screenshot` always waits for fresh html2canvas push, no cache.
//! - All GET responses send `no-store` headers.

use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};

use crate::pty;

/// Helper: headers that disable all caching (browsers, proxies, CDNs).
fn no_store_headers() -> [(header::HeaderName, &'static str); 3] {
    [
        (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate, proxy-revalidate"),
        (header::PRAGMA, "no-cache"),
        (header::EXPIRES, "0"),
    ]
}

/// Query for `GET /output?wait=ms&since=version` — hold-till-output.
/// `since` is the last version the client saw (from previous `version` field).
/// `wait` is hold time in seconds (0 = immediate dump, max 30). Holds until version changes.
#[derive(Deserialize)]
pub struct OutputQuery {
    pub since: Option<u64>,
    pub wait: Option<u64>,
}

/// Typed response for `GET /output` — simple dump-all with version for hold.
#[derive(Serialize)]
struct OutputResp {
    data: String,
    total: usize,
    version: u64,
    id: String,
}

#[derive(Deserialize)]
pub struct WriteReq {
    pub data: String,
    pub wait: Option<u64>,
}

#[derive(Deserialize)]
pub struct ResizeReq {
    pub cols: u16,
    pub rows: u16,
}

/// GET /health — version + session count + this tab's id (for agent to confirm).
pub async fn health_scoped(id: String) -> impl IntoResponse {
    let alive = crate::pty::session_exists(&id);
    let body = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "id": id,
        "alive": alive,
        "pid": pty::list_sessions().into_iter().find(|m| m.id == id).and_then(|m| m.pid),
    });
    (StatusCode::OK, no_store_headers(), Json(body)).into_response()
}

/// GET /output — simple dump-all for the tab's PTY output.
/// Returns the whole 512KB ring (no cursor). `?wait=seconds&since=version` holds until new output.
/// Holds via oneshot waiters (like screenshot) — no polling needed for agents.
pub async fn get_output_scoped(Query(q): Query<OutputQuery>, id: String) -> impl IntoResponse {
    let wait_secs = q.wait.unwrap_or(0).min(30);
    if let Some(since) = q.since {
        if wait_secs > 0 {
            // Hold-till-output: wait until version > since
            if let Some((bytes, total, ver)) =
                crate::server::state::wait_for_output(id.clone(), since, wait_secs).await
            {
                let data = String::from_utf8_lossy(&bytes).to_string();
                let body = OutputResp { data, total, version: ver, id };
                return (StatusCode::OK, no_store_headers(), Json(body)).into_response();
            } else {
                // Timeout: return current with timedOut flag so agent can retry without error log spam
                if let Ok((bytes, total, ver)) = pty::get_output(&id) {
                    let data = String::from_utf8_lossy(&bytes).to_string();
                    return (
                        StatusCode::OK,
                        no_store_headers(),
                        Json(serde_json::json!({"data": data, "total": total, "version": ver, "id": id, "timedOut": true})),
                    )
                        .into_response();
                }
            }
        }
    }
    match pty::get_output(&id) {
        Ok((bytes, total, version)) => {
            let data = String::from_utf8_lossy(&bytes).to_string();
            let body = OutputResp { data, total, version, id };
            (StatusCode::OK, no_store_headers(), Json(body)).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// POST /input {"data":"ls\n","wait":2} — one-way exec: write + hold till output, simple.
/// Auto-normalizes \n→\r and auto-appends \r if missing (ghost fix). Holds till version changes.
pub async fn post_input_scoped(Json(req): Json<WriteReq>, id: String) -> impl IntoResponse {
    if !crate::pty::session_exists(&id) {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": format!("session not found: {}", id)}))).into_response();
    }
    let wait_secs = req.wait.unwrap_or(2).min(10);
    let before = crate::pty::get_version(&id);
    let mut data = req.data.clone();
    if data.ends_with("\r\n") {
        data.pop();
    } else if data.ends_with('\n') {
        data.pop();
        data.push('\r');
    } else if !data.ends_with('\r') && !data.ends_with('\x03') && !data.ends_with('\x04') && !data.ends_with('\x1a') && !data.is_empty() {
        data.push('\r');
    }
    if let Err(e) = crate::pty::write_to_session(&id, &data) {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response();
    }
    if wait_secs > 0 {
        if let Some((bytes, total, ver)) = crate::server::state::wait_for_output(id.clone(), before, wait_secs).await {
            let data_str = String::from_utf8_lossy(&bytes).to_string();
            return (StatusCode::OK, no_store_headers(), Json(serde_json::json!({"data": data_str, "total": total, "version": ver, "id": id}))).into_response();
        } else if let Ok((bytes, total, ver)) = pty::get_output(&id) {
            let data_str = String::from_utf8_lossy(&bytes).to_string();
            return (StatusCode::OK, no_store_headers(), Json(serde_json::json!({"data": data_str, "total": total, "version": ver, "id": id, "timedOut": true}))).into_response();
        }
    }
    match pty::get_output(&id) {
        Ok((bytes, total, version)) => {
            let data_str = String::from_utf8_lossy(&bytes).to_string();
            (StatusCode::OK, no_store_headers(), Json(serde_json::json!({"data": data_str, "total": total, "version": version, "id": id}))).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// GET /history — simple dump-all current history (no hold), alias for GET /output without wait.
pub async fn get_history_scoped(id: String) -> impl IntoResponse {
    match pty::get_output(&id) {
        Ok((bytes, total, version)) => {
            let data = String::from_utf8_lossy(&bytes).to_string();
            (StatusCode::OK, no_store_headers(), Json(serde_json::json!({"data": data, "total": total, "version": version, "id": id}))).into_response()
        }
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

/// POST /clear — clear terminal output history for this tab. Simple: drain OUTPUTS ring
/// and invalidate screenshot waiters. No stale logs after this.
pub async fn clear_output_scoped(id: String) -> impl IntoResponse {
    match pty::clear_output(&id) {
        Ok(_) => {
            let body = serde_json::json!({"ok": true, "id": id, "cleared": true});
            (StatusCode::OK, no_store_headers(), Json(body)).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// GET /cwd — live working directory via /proc/<pid>/cwd.
pub async fn get_cwd_scoped(id: String) -> impl IntoResponse {
    match pty::get_cwd(&id) {
        Ok(cwd) => {
            let body = serde_json::json!({"cwd": cwd, "id": id});
            (StatusCode::OK, no_store_headers(), Json(body)).into_response()
        }
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

/// GET /screenshot/current — immediate cached screenshot (no hold), for history view.
pub async fn get_screenshot_current_scoped(Query(q): Query<ScreenshotQuery>, id: String) -> impl IntoResponse {
    if !crate::pty::session_exists(&id) {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "session not found"}))).into_response();
    }
    if let Some(png) = crate::server::state::get_screenshot(&id) {
        let fmt = q.format.as_deref().unwrap_or("png");
        if fmt == "base64" {
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
            return (StatusCode::OK, no_store_headers(), Json(serde_json::json!({"image": format!("data:image/png;base64,{}", out), "id": id}))).into_response();
        }
        return (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "image/png"),
                (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate, proxy-revalidate"),
                (header::PRAGMA, "no-cache"),
                (header::EXPIRES, "0"),
            ],
            png,
        )
            .into_response();
    }
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error": "no cached screenshot yet — run GET /screenshot first to trigger capture", "id": id})),
    )
        .into_response()
}

/// GET /screenshot — per-tab PNG of that tab's terminal, even when not focused.
///
/// Fresh-only, holds till capture: every request triggers `request-screenshot:{id}` in
/// `share.rs` then holds up to 10s for the frontend's `html2canvas` push.
/// No stale fast-path — if the frontend doesn't push in time, returns 503.
pub async fn get_screenshot_scoped(Query(q): Query<ScreenshotQuery>, id: String) -> impl IntoResponse {
    if !crate::pty::session_exists(&id) {
        return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error": "session not found"}))).into_response();
    }
    // Fresh-only: always wait for next capture, never serve stale cache
    let png = match crate::server::state::wait_for_screenshot(id.clone(), 10).await {
        Some(b) => b,
        None => {
            let last_err = crate::server::state::get_last_error(&id);
            let session_exists = crate::pty::session_exists(&id);
            let share_exists = crate::server::state::get_share(&id).is_some();
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                no_store_headers(),
                Json(serde_json::json!({
                    "error": "screenshot not yet available after 10s hold — frontend has not pushed a fresh capture for this tab",
                    "id": id,
                    "logs": {
                        "session_exists": session_exists,
                        "share_exists": share_exists,
                        "cache_empty": true,
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
        }
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
        return (StatusCode::OK, no_store_headers(), Json(serde_json::json!({
            "image": format!("data:image/png;base64,{}", out),
            "id": id,
            "width": 0,
            "height": 0,
        }))).into_response();
    }

    // Raw PNG binary — single cross-compatible way for `curl -o term.png` and vision.
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate, proxy-revalidate"),
            (header::PRAGMA, "no-cache"),
            (header::EXPIRES, "0"),
        ],
        png,
    )
        .into_response()
}

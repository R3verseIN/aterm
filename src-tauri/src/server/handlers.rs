//! handlers.rs — Per-tab scoped HTTP handlers (port is the id).
//!
//! Minimal no-cache surface: `GET /output` (dump-all ring), `POST /input`
//! (write + invisible OSC sentinel hold), `GET /clear`, `GET /screenshot`, `GET /health`.
//! - `GET /output` dumps 512KB ring (no cursor, no hold).
//! - `POST /input` injects invisible OSC `ESC]633;E;__ATERM_DONE_<uuid>__:code BEL`
//!   via `printf '\033]633;E;…\007'` — xterm consumes OSC (screenshot clean),
//!   PTY ring keeps raw `0x1b]633;E;…` for `\x1b` detection (avoids typed `; printf` false positive).
//! - All GET responses send `no-store` headers.

use std::time::{Duration, Instant};

use axum::{
    extract::Query,
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use uuid::Uuid;

use crate::pty;

/// Helper: headers that disable all caching (browsers, proxies, CDNs).
fn no_store_headers() -> [(header::HeaderName, &'static str); 3] {
    [
        (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate, proxy-revalidate"),
        (header::PRAGMA, "no-cache"),
        (header::EXPIRES, "0"),
    ]
}

/// Strip ANSI CSI / OSC via `strip-ansi-escapes` crate + `regex` for CR/BEL.
/// `strip-ansi-escapes` removes `\x1b[...`, `\x1b]...\x07` etc. (proper VT parser).
fn strip_ansi_osc(s: &str) -> String {
    let stripped = strip_ansi_escapes::strip(s.as_bytes());
    let mut out = String::from_utf8_lossy(&stripped).to_string();
    out = regex::Regex::new(r"\r").unwrap().replace_all(&out, "").to_string();
    out = regex::Regex::new(r"\x07").unwrap().replace_all(&out, "").to_string();
    out
}

#[derive(Deserialize)]
pub struct WriteReq {
    pub data: String,
}

/// GET /health — appVersion + session count + this tab's id (for agent to confirm).
pub async fn health_scoped(id: String) -> impl IntoResponse {
    let alive = crate::pty::session_exists(&id);
    let body = serde_json::json!({
        "appVersion": env!("CARGO_PKG_VERSION"),
        "id": id,
        "alive": alive,
        "pid": pty::list_sessions().into_iter().find(|m| m.id == id).and_then(|m| m.pid),
    });
    (StatusCode::OK, no_store_headers(), Json(body)).into_response()
}

/// POST /input {"data":"ls\n"} — one-way exec with invisible OSC sentinel hold.
///
/// Transparent: client sends plain `{"data":"…\n"}`.
/// Server rewrites to `…; printf '\033]633;E;__ATERM_DONE_<uuid>__:%s\007' "$?"\r`
/// (unless empty/Ctrl-C/D/Z). Shell's printf emits `ESC]633;E;__MARKER__:code BEL`
/// — xterm swallows OSC (screenshot stays clean), but PTY ring keeps raw `0x1b]633;E;…`
/// for detection. Typed `; printf '\033…'` is literal `\033` chars, not `0x1b`,
/// so `\x1b]633;E;` matches only the shell output, not the echo. Holds 300s,
/// strips marker, returns `exitCode`, or `timedOut:true`.
pub async fn post_input_scoped(Json(req): Json<WriteReq>, id: String) -> impl IntoResponse {
    if !crate::pty::session_exists(&id) {
        return (
            StatusCode::NOT_FOUND,
            no_store_headers(),
            Json(serde_json::json!({"error": format!("session not found: {}", id)})),
        )
            .into_response();
    }

    let raw = req.data.clone();

    // Control chars / empty: no sentinel, just write normalized data and hold for any output.
    let is_ctrl = raw.is_empty()
        || raw.ends_with('\x03')
        || raw.ends_with('\x04')
        || raw.ends_with('\x1a');

    if is_ctrl {
        let before = crate::pty::get_version(&id);
        let mut data = raw;
        if data.ends_with("\r\n") {
            data.pop();
        } else if data.ends_with('\n') {
            data.pop();
            data.push('\r');
        } else if !data.ends_with('\r')
            && !data.ends_with('\x03')
            && !data.ends_with('\x04')
            && !data.ends_with('\x1a')
            && !data.is_empty()
        {
            data.push('\r');
        }
        if let Err(e) = crate::pty::write_to_session(&id, &data) {
            return (
                StatusCode::NOT_FOUND,
                no_store_headers(),
                Json(serde_json::json!({"error": e})),
            )
                .into_response();
        }
        // Hold up to 300s for any output (version bump) — sentinel-free path.
        if let Some((bytes, total, ver)) =
            crate::server::state::wait_for_output(id.clone(), before, 300).await
        {
            let data_str = String::from_utf8_lossy(&bytes).to_string();
            return (
                StatusCode::OK,
                no_store_headers(),
                Json(serde_json::json!({"data": data_str, "total": total, "version": ver, "id": id})),
            )
                .into_response();
        }
        if let Ok((bytes, total, ver)) = pty::get_output(&id) {
            let data_str = String::from_utf8_lossy(&bytes).to_string();
            return (
                StatusCode::OK,
                no_store_headers(),
                Json(serde_json::json!({"data": data_str, "total": total, "version": ver, "id": id, "timedOut": true})),
            )
                .into_response();
        }
        return (
            StatusCode::NOT_FOUND,
            no_store_headers(),
            Json(serde_json::json!({"error": "no output"})),
        )
            .into_response();
    }

    // Stable path: if shell integration active (bash/zsh via PROMPT_COMMAND precmd),
    // the shell auto-emits invisible `ESC]633;D;code BEL` at each prompt — no per-command injection,
    // works for any syntax (`for`, `&&`, `|`, TUI) and keeps input line clean (screenshot).
    if crate::pty::has_integration(&id) {
        let mut base = raw;
        if base.ends_with("\r\n") {
            base.truncate(base.len() - 2);
        } else if base.ends_with('\n') || base.ends_with('\r') {
            base.pop();
        }
        let to_write = format!("{}\r", base);
        let before = crate::pty::get_version(&id);
        let before_s = pty::get_output(&id)
            .map(|(b, _, _)| String::from_utf8_lossy(&b).to_string())
            .unwrap_or_default();
        let before_len = before_s.len();
        let needle_d = "\x1b]633;D;";
        let needle_d_plain = "\x1b]633;D";
        let before_count = before_s.matches(needle_d_plain).count();
        let start = Instant::now();
        let deadline = start + Duration::from_secs(300);
        if let Err(e) = crate::pty::write_to_session(&id, &to_write) {
            return (
                StatusCode::NOT_FOUND,
                no_store_headers(),
                Json(serde_json::json!({"error": e})),
            )
                .into_response();
        }
        let mut since = before;
        loop {
            let now = Instant::now();
            if now >= deadline {
                if let Ok((bytes, _, ver)) = pty::get_output(&id) {
                    let s = String::from_utf8_lossy(&bytes).to_string();
                    let raw_delta = &s[before_len.min(s.len())..];
                    let clean = strip_ansi_osc(raw_delta);
                    return (
                        StatusCode::OK,
                        no_store_headers(),
                        Json(serde_json::json!({
                            "data": clean, "total": clean.len(), "version": ver, "id": id,
                            "timedOut": true, "elapsedMs": start.elapsed().as_millis()
                        })),
                    )
                        .into_response();
                }
                return (
                    StatusCode::NOT_FOUND,
                    no_store_headers(),
                    Json(serde_json::json!({"error": "timeout and no output"})),
                )
                    .into_response();
            }
            let remaining = deadline - now;
            let secs = remaining.as_secs().max(1).min(300);
            let waited = crate::server::state::wait_for_output(id.clone(), since, secs).await;
            let (bytes, _total, ver) = match waited {
                Some(v) => v,
                None => continue,
            };
            let s = String::from_utf8_lossy(&bytes).to_string();
            let cur_count = s.matches(needle_d_plain).count();
            if cur_count > before_count {
                let last_idx = s.rfind(needle_d).or_else(|| s.rfind(needle_d_plain)).unwrap_or(s.len());
                let raw_delta = &s[before_len.min(last_idx)..last_idx];
                let clean = strip_ansi_osc(raw_delta);
                let after = &s[last_idx + needle_d_plain.len().min(s.len() - last_idx)..];
                let code_str = after
                    .trim_start_matches(';')
                    .trim_start_matches(|c| c == '\x07' || c == '\r' || c == '\n')
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>();
                let exit_code: i32 = code_str.parse().unwrap_or(0);
                return (
                    StatusCode::OK,
                    no_store_headers(),
                    Json(serde_json::json!({
                        "data": clean,
                        "total": clean.len(),
                        "version": ver,
                        "id": id,
                        "exitCode": exit_code,
                        "elapsedMs": start.elapsed().as_millis()
                    })),
                )
                    .into_response();
            }
            since = ver;
        }
    }

    // Fallback: per-request invisible OSC sentinel (for sh/fish or when integration not active)
    let sentinel = format!("__ATERM_DONE_{}__", Uuid::new_v4().simple());
    let needle = format!("\x1b]633;E;{}:", sentinel);
    let needle_fallback = format!("\x1b]633;E;{}", sentinel);

    let mut base = raw;
    if base.ends_with("\r\n") {
        base.truncate(base.len() - 2);
    } else if base.ends_with('\n') || base.ends_with('\r') {
        base.pop();
    }
    let to_write = format!("{}; printf '\\033]633;E;{}:%s\\007' \"$?\"\r", base, sentinel);

    let before = crate::pty::get_version(&id);
    let before_s = pty::get_output(&id)
        .map(|(b, _, _)| String::from_utf8_lossy(&b).to_string())
        .unwrap_or_default();
    let before_len = before_s.len();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(300);

    if let Err(e) = crate::pty::write_to_session(&id, &to_write) {
        return (
            StatusCode::NOT_FOUND,
            no_store_headers(),
            Json(serde_json::json!({"error": e})),
        )
            .into_response();
    }

    let mut since = before;
    loop {
        let now = Instant::now();
        if now >= deadline {
            if let Ok((bytes, total, ver)) = pty::get_output(&id) {
                let data_str = String::from_utf8_lossy(&bytes).to_string();
                // Strip any partial sentinel attempt for cleanliness (unlikely on timeout)
                return (
                    StatusCode::OK,
                    no_store_headers(),
                    Json(serde_json::json!({
                        "data": data_str, "total": total, "version": ver, "id": id,
                        "timedOut": true, "sentinel": sentinel, "elapsedMs": start.elapsed().as_millis()
                    })),
                )
                    .into_response();
            }
            return (
                StatusCode::NOT_FOUND,
                no_store_headers(),
                Json(serde_json::json!({"error": "timeout and no output"})),
            )
                .into_response();
        }
        let remaining = deadline - now;
        // wait_for_output takes u64 secs; ensure at least 1s granularity, cap at 300
        let secs = remaining.as_secs().max(1).min(300);

        let waited = crate::server::state::wait_for_output(id.clone(), since, secs).await;
        let (bytes, _total, ver) = match waited {
            Some(v) => v,
            None => {
                // No output within remaining slice — treat as timeout slice, re-loop to check deadline
                // Also handle fast-path where version already == since but no new output
                if Instant::now() >= deadline {
                    continue;
                }
                // No new data, continue waiting
                continue;
            }
        };

        let s = String::from_utf8_lossy(&bytes).to_string();
        if let Some(idx) = s.find(&needle).or_else(|| s.find(&needle_fallback)) {
            let raw_delta = &s[before_len.min(idx)..idx];
            let clean = strip_ansi_osc(raw_delta);
            let after = &s[idx + needle_fallback.len().min(s.len() - idx)..];
            let code_str = after
                .trim_start_matches(':')
                .trim_start_matches(|c| c == '\x07' || c == '\r' || c == '\n')
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            let exit_code: i32 = if !code_str.is_empty() {
                code_str.parse().unwrap_or(0)
            } else {
                s[idx + needle.len().min(s.len() - idx)..]
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse()
                    .unwrap_or(0)
            };
            return (
                StatusCode::OK,
                no_store_headers(),
                Json(serde_json::json!({
                    "data": clean,
                    "total": clean.len(),
                    "version": ver,
                    "id": id,
                    "exitCode": exit_code,
                    "sentinel": sentinel,
                    "elapsedMs": start.elapsed().as_millis()
                })),
            )
                .into_response();
        }

        // No sentinel yet — update since and continue holding (covers `sleep 10` echo → wait for hello)
        since = ver;
        // Loop again until marker or deadline
    }
}

/// GET /output — dump-all current output ring (no hold), max 512KB.
pub async fn get_output_scoped(id: String) -> impl IntoResponse {
    match pty::get_output(&id) {
        Ok((bytes, total, version)) => {
            let data = String::from_utf8_lossy(&bytes).to_string();
            (
                StatusCode::OK,
                no_store_headers(),
                Json(serde_json::json!({"data": data, "total": total, "version": version, "id": id})),
            )
                .into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, no_store_headers(), Json(serde_json::json!({"error": e}))).into_response(),
    }
}

/// GET /clear — clear terminal via shell exec (`clear\r`/`cls\r`). Shell emits ESC[2J
/// and append_output auto-wipes the ring so HTTP and xterm stay in sync.
pub async fn clear_output_scoped(id: String) -> impl IntoResponse {
    match pty::clear_output(&id) {
        Ok(_) => {
            let body = serde_json::json!({"ok": true, "id": id, "cleared": true});
            (StatusCode::OK, no_store_headers(), Json(body)).into_response()
        }
        Err(e) => (StatusCode::NOT_FOUND, no_store_headers(), Json(serde_json::json!({"error": e}))).into_response(),
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
/// Fresh-only, holds till capture: every request triggers `request-screenshot:{id}` in
/// `share.rs` then holds up to 10s for the frontend's `html2canvas` push.
/// No stale fast-path — if the frontend doesn't push in time, returns 503.
pub async fn get_screenshot_scoped(Query(q): Query<ScreenshotQuery>, id: String) -> impl IntoResponse {
    if !crate::pty::session_exists(&id) {
        return (StatusCode::NOT_FOUND, no_store_headers(), Json(serde_json::json!({"error": "session not found"}))).into_response();
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

    let fmt = q.format.as_deref().unwrap_or("png");
    if fmt == "base64" {
        use base64::{engine::general_purpose::STANDARD, Engine as _};
        let out = STANDARD.encode(&png);
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

#[cfg(test)]
mod tests {
    use super::strip_ansi_osc;

    #[test]
    fn strips_csi_and_osc() {
        assert_eq!(strip_ansi_osc("\x1b[31mred\x1b[0m"), "red");
        assert_eq!(strip_ansi_osc("\x1b]0;title\x07hello"), "hello");
        assert_eq!(strip_ansi_osc("a\r\nb\x07c"), "a\nbc");
    }

    #[test]
    fn keeps_utf8() {
        assert_eq!(strip_ansi_osc("café \x1b[2J"), "café ");
    }
}

//! state.rs — Shared per-tab HTTP server registry.
//!
//! Each right-click Share spawns a dedicated Axum listener on a random high port
//! (127.0.0.1:0). This module tracks live shares so `share_tab` is idempotent,
//! `unshare_tab` aborts the task, and `close_session` auto-cleans its port.
//! Discovery files are also written here for external agents to find the URL.

use std::{collections::HashMap, sync::{Mutex, OnceLock}, time::Duration};
use tokio::{sync::oneshot, task::JoinHandle, time::timeout};

/// Info returned to the frontend after a successful share.
/// `port` is the random high port (ephemeral) and also the capability —
/// the URL `http://127.0.0.1:{port}` is scoped to a single tab `id`
/// so no `:id` path param is needed.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SharedInfo {
    /// The tab's original PTY id (UUID) this server is scoped to.
    pub id: String,
    /// Random high port assigned by the OS (bind 127.0.0.1:0).
    pub port: u16,
    /// Full base URL, e.g. `http://127.0.0.1:42817`.
    pub url: String,
}

/// Internal handle for a live per-tab server.
pub struct SharedServer {
    pub port: u16,
    pub handle: JoinHandle<()>,
}

static SHARES: OnceLock<Mutex<HashMap<String, SharedServer>>> = OnceLock::new();

/// Accessor for the global per-tab shares map.
pub fn shares() -> &'static Mutex<HashMap<String, SharedServer>> {
    SHARES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// List all currently shared tabs (cloned info for HTTP callers).
pub fn list_shares() -> Vec<SharedInfo> {
    let map = shares().lock().unwrap_or_else(|e| e.into_inner());
    map.iter()
        .map(|(id, s)| SharedInfo {
            id: id.clone(),
            port: s.port,
            url: format!("http://127.0.0.1:{}", s.port),
        })
        .collect()
}

/// Get the share info for a specific tab id, if shared.
pub fn get_share(id: &str) -> Option<SharedInfo> {
    let map = shares().lock().unwrap_or_else(|e| e.into_inner());
    map.get(id).map(|s| SharedInfo {
        id: id.to_string(),
        port: s.port,
        url: format!("http://127.0.0.1:{}", s.port),
    })
}

// ---------------------------------------------------------------------------
// Screenshot — fresh-only, no cache. Every GET /screenshot triggers a
// fresh html2canvas capture via `request-screenshot:{id}` and holds the
// HTTP connection until the frontend pushes via `store_screenshot`.
// No persistent cache: stale images are never served. We just rendezvous
// via oneshot waiters. Straightforward, no instant stale path.
// ---------------------------------------------------------------------------

/// Store a PNG screenshot for a tab (called from Tauri command `store_screenshot`).
/// Decodes base64 and wakes any `GET /screenshot` waiters holding the connection.
/// No persistent cache — purely a rendezvous to keep logic fresh-only.
pub fn store_screenshot(id: &str, png_b64: &str) -> Result<(), String> {
    let bytes = base64_decode(png_b64)?;
    println!("[screenshot] store for {}: {} bytes b64 -> {} bytes png", id, png_b64.len(), bytes.len());
    // Wake any `GET /screenshot` handlers holding a wait for this id (10 s hold)
    if let Some(waiters_vec) = waiters()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id)
    {
        for tx in waiters_vec {
            let _ = tx.send(bytes.clone());
        }
    }
    // Clear last error on success (capture now works)
    screenshot_errors()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    Ok(())
}

/// Remove a tab's pending screenshot waiters/errors (on clear/unshare/close).
/// No persistent image cache to clear — just waiters and error logs.
pub fn remove_screenshot(id: &str) {
    // Also clear last error and pending waiters for this tab
    screenshot_errors()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
    waiters()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(id);
}

// ---------------------------------------------------------------------------
// Screenshot waiters + error logs for stable hold-till-capture (10 s max)
// ---------------------------------------------------------------------------

/// Waiters: `id -> Vec<Sender>` — each `GET /screenshot` that finds cache empty
/// registers a oneshot and holds the connection until `store_screenshot` wakes it.
/// This makes `GET /screenshot` hold indefinitely (up to 10 s) for real capture
/// instead of returning a dummy 1×1 or immediate 503.
static WAITERS: OnceLock<Mutex<HashMap<String, Vec<oneshot::Sender<Vec<u8>>>>>> = OnceLock::new();

fn waiters() -> &'static Mutex<HashMap<String, Vec<oneshot::Sender<Vec<u8>>>>> {
    WAITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Last frontend error per tab — pushed via `report_screenshot_error` from
/// `TerminalView.tsx` when `html2canvas` throws. Returned in 503 debug payload
/// after the 10 s hold so agents can see why capture failed (tainted canvas,
/// zero size, etc.) without needing browser console.
static SCREENSHOT_ERRORS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn screenshot_errors() -> &'static Mutex<HashMap<String, String>> {
    SCREENSHOT_ERRORS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Record a frontend capture error for a tab (from `report_screenshot_error`).
pub fn report_screenshot_error(id: &str, err: String) {
    let mut map = screenshot_errors().lock().unwrap_or_else(|e| e.into_inner());
    map.insert(id.to_string(), err);
}

/// Get the last frontend error for a tab, if any.
pub fn get_last_error(id: &str) -> Option<String> {
    let map = screenshot_errors().lock().unwrap_or_else(|e| e.into_inner());
    map.get(id).cloned()
}

/// Hold until a real screenshot is pushed, up to `max_secs` (10 s).
/// Fresh-only: never returns a cached image, always waits for the next
/// `store_screenshot` push triggered by `request-screenshot:{id}`.
/// Returns `Some(png)` if a fresh capture arrived, `None` on timeout.
pub async fn wait_for_screenshot(id: String, max_secs: u64) -> Option<Vec<u8>> {
    // Register a oneshot waiter for this id — no cache fast path
    let (tx, rx) = oneshot::channel();
    {
        let mut map = waiters().lock().unwrap_or_else(|e| e.into_inner());
        map.entry(id.clone()).or_default().push(tx);
    }
    // Hold the connection — wakes when `store_screenshot` drains WAITERS[&id]
    match timeout(Duration::from_secs(max_secs), rx).await {
        Ok(Ok(png)) => Some(png),
        Ok(Err(_)) => None,
        Err(_) => None,
    }
}

/// Minimal base64 decode without extra crate (alphabet + padding, no wrap).
/// We avoid pulling `base64` crate for a single use site to keep deps lean.
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    // Use the `base64` crate if present via transitive `tauri` deps, but to stay
    // cross-compatible and zero-dep, implement a tiny decoder here. For
    // simplicity and speed we delegate to the `base64` crate if the feature is
    // available via `serde`'s transitive dep — fallback to manual if not.
    // Since `html2canvas` emits standard base64 (RFC 4648, `=` padding), we
    // implement the straightforward variant.
    const ALPH: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut table = [255u8; 256];
    for (i, &c) in ALPH.iter().enumerate() {
        table[c as usize] = i as u8;
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u8 = 0;
    for &b in input.as_bytes() {
        if b == b'=' { break; }
        if b == b'\n' || b == b'\r' || b == b' ' { continue; }
        let v = table[b as usize];
        if v == 255 { return Err(format!("invalid base64 char: {}", b as char)); }
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Ok(out)
}

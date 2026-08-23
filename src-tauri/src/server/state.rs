//! state.rs — Shared per-tab HTTP server registry.
//!
//! Each right-click Share spawns a dedicated Axum listener on a random high port
//! (127.0.0.1:0). This module tracks live shares so `share_tab` is idempotent,
//! `unshare_tab` aborts the task, and `close_session` auto-cleans its port.
//! Discovery files are also written here for external agents to find the URL.

use std::{collections::HashMap, sync::{Mutex, OnceLock}};
use tokio::task::JoinHandle;

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

//! state.rs — Shared per-tab HTTP server registry with explicit crates, no duct-tape.
//!
//! `dashmap` sharded, `event-listener` for TOCTOU-free wait, `base64` crate.

use std::{sync::OnceLock, time::Duration};
use dashmap::DashMap;
use event_listener::Event;
use std::sync::Arc;
use tokio::{task::JoinHandle, time::timeout};

/// Info returned to the frontend after a successful share.
#[derive(Clone, Debug, serde::Serialize)]
pub struct SharedInfo {
    pub id: String,
    pub port: u16,
    pub url: String,
}

pub struct SharedServer {
    pub port: u16,
    pub handle: JoinHandle<()>,
}

static SHARES: OnceLock<DashMap<String, SharedServer>> = OnceLock::new();
pub fn shares() -> &'static DashMap<String, SharedServer> {
    SHARES.get_or_init(DashMap::new)
}

pub fn list_shares() -> Vec<SharedInfo> {
    shares()
        .iter()
        .map(|e| SharedInfo {
            id: e.key().clone(),
            port: e.value().port,
            url: format!("http://127.0.0.1:{}", e.value().port),
        })
        .collect()
}

pub fn get_share(id: &str) -> Option<SharedInfo> {
    shares().get(id).map(|e| SharedInfo {
        id: e.key().clone(),
        port: e.value().port,
        url: format!("http://127.0.0.1:{}", e.value().port),
    })
}

// ---------------------------------------------------------------------------
// Screenshot — fresh-only rendezvous via `dashmap` + `event-listener`
// ---------------------------------------------------------------------------

static SCREENSHOT_DATA: OnceLock<DashMap<String, Vec<u8>>> = OnceLock::new();
fn screenshot_data() -> &'static DashMap<String, Vec<u8>> {
    SCREENSHOT_DATA.get_or_init(DashMap::new)
}
static SCREENSHOT_WAITERS: OnceLock<DashMap<String, Arc<Event>>> = OnceLock::new();
fn screenshot_waiters() -> &'static DashMap<String, Arc<Event>> {
    SCREENSHOT_WAITERS.get_or_init(DashMap::new)
}
static SCREENSHOT_ERRORS: OnceLock<DashMap<String, String>> = OnceLock::new();
fn screenshot_errors() -> &'static DashMap<String, String> {
    SCREENSHOT_ERRORS.get_or_init(DashMap::new)
}

pub fn store_screenshot(id: &str, png_b64: &str) -> Result<(), String> {
    let bytes = base64_decode(png_b64)?;
    println!("[screenshot] store for {}: {} bytes b64 -> {} bytes png", id, png_b64.len(), bytes.len());
    screenshot_data().insert(id.to_string(), bytes);
    if let Some(entry) = screenshot_waiters().get(id) {
        entry.notify(usize::MAX);
    }
    screenshot_errors().remove(id);
    Ok(())
}

pub fn remove_screenshot(id: &str) {
    screenshot_errors().remove(id);
    screenshot_data().remove(id);
    // keep waiter Event for reuse; no need to remove
}

pub fn report_screenshot_error(id: &str, err: String) {
    screenshot_errors().insert(id.to_string(), err);
}

pub fn get_last_error(id: &str) -> Option<String> {
    screenshot_errors().get(id).map(|v| v.clone())
}

pub async fn wait_for_screenshot(id: String, max_secs: u64) -> Option<Vec<u8>> {
    let event = screenshot_waiters()
        .entry(id.clone())
        .or_insert_with(|| Arc::new(Event::new()))
        .value()
        .clone();
    let listener = event.listen();
    // Fresh-only: never return stale, always wait for next store
    match timeout(Duration::from_secs(max_secs), listener).await {
        Ok(_) => screenshot_data().remove(&id).map(|(_, v)| v),
        Err(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Output waiters for hold-till-output (POST /input)
// ---------------------------------------------------------------------------

static OUTPUT_WAITERS: OnceLock<DashMap<String, Arc<Event>>> = OnceLock::new();
fn output_waiters() -> &'static DashMap<String, Arc<Event>> {
    OUTPUT_WAITERS.get_or_init(DashMap::new)
}

pub fn notify_output_waiters(id: &str, _version: u64) {
    if let Some(entry) = output_waiters().get(id) {
        entry.notify(usize::MAX);
    }
}

pub fn remove_output_waiters(id: &str) {
    // Keep Event for reuse; optionally notify to unblock
    if let Some(entry) = output_waiters().get(id) {
        entry.notify(usize::MAX);
    }
}

pub async fn wait_for_output(id: String, since: u64, max_secs: u64) -> Option<(Vec<u8>, usize, u64)> {
    // Subscribe first, then re-check version to fix TOCTOU gap
    let event = output_waiters()
        .entry(id.clone())
        .or_insert_with(|| Arc::new(Event::new()))
        .value()
        .clone();
    let listener = event.listen();
    // Re-check after subscribing
    if let Ok((bytes, total, ver)) = crate::pty::get_output(&id) {
        if ver != since {
            return Some((bytes, total, ver));
        }
    } else {
        return None;
    }
    match timeout(Duration::from_secs(max_secs), listener).await {
        Ok(_) => crate::pty::get_output(&id).ok(),
        Err(_) => None,
    }
}

/// Base64 decode via `base64` crate (RFC 4648)
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    STANDARD.decode(cleaned).map_err(|e| format!("invalid base64: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pty;

    #[tokio::test]
    async fn wait_for_output_wakes_on_notify() {
        let id = "test-wake";
        pty::outputs().insert(id.to_string(), b"hi".to_vec());
        pty::output_versions().insert(id.to_string(), 1);
        let h = tokio::spawn(wait_for_output(id.to_string(), 1, 2));
        // bump version and notify
        pty::output_versions().insert(id.to_string(), 2);
        notify_output_waiters(id, 2);
        let res = h.await.unwrap();
        assert!(res.is_some());
        pty::cleanup_state(id);
    }

    #[tokio::test]
    async fn wait_for_output_no_toctou() {
        let id = "test-toctou";
        pty::outputs().insert(id.to_string(), Vec::new());
        pty::output_versions().insert(id.to_string(), 0);
        // Pre-bump before wait — fast-path should return immediately
        pty::output_versions().insert(id.to_string(), 1);
        let res = wait_for_output(id.to_string(), 0, 1).await;
        assert!(res.is_some());
        pty::cleanup_state(id);
    }
}

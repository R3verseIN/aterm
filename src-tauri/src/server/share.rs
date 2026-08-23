//! share.rs — Spawn/teardown of per-tab random-port HTTP servers.
//!
//! `share_tab(id)` binds `127.0.0.1:0` (OS picks random high free port 32768-60999),
//! builds a router closed over `id`, spawns `axum::serve`, stores the JoinHandle in
//! `SHARES`, writes discovery files (`~/.config/aterm/shares/{id}.json` + `/tmp/aterm-{short}.port`),
//! and returns the URL. No global single config port — each tab gets its own listener,
//! so sharing multiple tabs doesn't collide.

use axum::{routing::{get, post}, Router};
use tauri::{AppHandle, Emitter};
use tower_http::cors::{Any, CorsLayer};

use super::handlers::{get_cwd_scoped, get_output_scoped, get_screenshot_scoped, health_scoped, post_input_scoped, post_resize_scoped};
use super::state::{shares, SharedInfo, SharedServer};

/// Share a tab's PTY on a new random high port. Idempotent — if already shared, returns existing port.
pub async fn share_tab(app: AppHandle, id: String) -> Result<SharedInfo, String> {
    // Validate session exists before binding a port
    if !crate::pty::session_exists(&id) {
        return Err(format!("session not found: {}", id));
    }

    // Idempotent: already shared -> return existing info (fast path, before bind)
    if let Some(existing) = super::state::get_share(&id) {
        return Ok(existing);
    }

    // Bind to 0 => OS picks a random high free port, race-free (we keep the listener)
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("failed to bind random port: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| e.to_string())?
        .port();
    let url = format!("http://127.0.0.1:{}", port);

    // Build a router scoped to this id — no :id param needed, port IS the id
    let id_clone = id.clone();
    let router = Router::new()
        .route("/health", get({
            let id = id_clone.clone();
            move || health_scoped(id.clone())
        }))
        .route("/output", get({
            let id = id_clone.clone();
            move |q: axum::extract::Query<super::handlers::OutputQuery>| get_output_scoped(q, id.clone())
        }))
        .route("/input", post({
            let id = id_clone.clone();
            move |b: axum::Json<super::handlers::WriteReq>| post_input_scoped(b, id.clone())
        }))
        .route("/cwd", get({
            let id = id_clone.clone();
            move || get_cwd_scoped(id.clone())
        }))
        .route("/resize", post({
            let id = id_clone.clone();
            move |b: axum::Json<super::handlers::ResizeReq>| post_resize_scoped(b, id.clone())
        }))
        .route("/screenshot", get({
            let id = id_clone.clone();
            let app = app.clone();
            move |q: axum::extract::Query<super::handlers::ScreenshotQuery>| {
                let id2 = id.clone();
                let app2 = app.clone();
                async move {
                    // On-demand: tell the frontend to capture this tab now
                    let _ = app2.emit(&format!("request-screenshot:{}", id2), ());
                    get_screenshot_scoped(q, id2).await
                }
            }
        }))
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any));

    // Spawn the per-tab server
    let handle = tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("[aterm share {}:{}] server error: {}", id_clone, port, e);
        }
    });

    // Store handle — handle TOCTOU race: if another concurrent share_tab already
    // inserted for this id while we were binding, abort the old listener and keep the new one.
    // Use poison recovery (into_inner) so a poisoned mutex doesn't permanently break sharing.
    {
        let mut map = shares().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = map.insert(id.clone(), SharedServer { port, handle }) {
            old.handle.abort();
            // Also clean old discovery files before overwriting (port changed)
            if let Some(dir) = dirs::config_dir().map(|d| d.join("aterm").join("shares")) {
                let _ = std::fs::remove_file(dir.join(format!("{}.json", id)));
            }
            eprintln!("[aterm share] race: replaced existing share for {} (old port {})", id, old.port);
        }
        // Double-check idempotency after insert: if session disappeared between pre-check and insert,
        // validate again and rollback if needed.
        if !crate::pty::session_exists(&id) {
            if let Some(entry) = map.remove(&id) {
                entry.handle.abort();
            }
            return Err(format!("session not found (race): {}", id));
        }
    }

    // Discovery files for agents: per-tab JSON + tmp port file for `curl $(cat /tmp/aterm-*.port)/health`
    // Use full id for JSON, and both full and short short-name for /tmp for convenience.
    if let Some(dir) = dirs::config_dir().map(|d| d.join("aterm").join("shares")) {
        let _ = std::fs::create_dir_all(&dir);
        let info = serde_json::json!({
            "id": id,
            "port": port,
            "url": url,
            "pid": crate::pty::list_sessions().into_iter().find(|m| m.id == id).and_then(|m| m.pid),
            "cwd": crate::pty::get_cwd(&id).ok(),
        });
        let _ = std::fs::write(dir.join(format!("{}.json", id)), serde_json::to_string_pretty(&info).unwrap_or_default());
    }
    // Write both full-id and short 8-char tmp files: full is collision-free, short is ergonomic.
    let short = &id[..8.min(id.len())];
    let _ = std::fs::write(format!("/tmp/aterm-{}.port", id), port.to_string());
    let _ = std::fs::write(format!("/tmp/aterm-{}.url", id), url.clone());
    let _ = std::fs::write(format!("/tmp/aterm-{}.port", short), port.to_string());
    let _ = std::fs::write(format!("/tmp/aterm-{}.url", short), url.clone());

    // Notify frontend for toast/clipboard (optional listener)
    let _ = app.emit(&format!("share:started:{}", id), port);
    println!("[aterm share] tab {} -> {} (no auth, localhost only)", id, url);

    Ok(SharedInfo { id, port, url })
}

/// Unshare a tab — abort its dedicated server and clean discovery files.
/// Idempotent.
pub fn unshare_tab(id: &str) -> Result<(), String> {
    let mut map = shares().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = map.remove(id) {
        entry.handle.abort();
        // Clean per-tab discovery files
        if let Some(dir) = dirs::config_dir().map(|d| d.join("aterm").join("shares")) {
            let _ = std::fs::remove_file(dir.join(format!("{}.json", id)));
        }
        let short = &id[..8.min(id.len())];
        let _ = std::fs::remove_file(format!("/tmp/aterm-{}.port", id));
        let _ = std::fs::remove_file(format!("/tmp/aterm-{}.url", id));
        let _ = std::fs::remove_file(format!("/tmp/aterm-{}.port", short));
        let _ = std::fs::remove_file(format!("/tmp/aterm-{}.url", short));
        println!("[aterm share] tab {} unshared (port {})", id, entry.port);
    }
    // Evict screenshot cache (per-tab PNG) — keeps memory bounded
    super::state::remove_screenshot(id);
    Ok(())
}

/// Get share info if exists (for frontend badge / copy URL).
pub fn get_share_info(id: &str) -> Option<SharedInfo> {
    super::state::get_share(id)
}

/// List all current shares (for debugging/health).
pub fn list_shares() -> Vec<SharedInfo> {
    super::state::list_shares()
}

/// Cleanup all shares (called on app exit or startup to purge stale files).
pub fn cleanup_all() {
    let ids: Vec<String> = shares()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .keys()
        .cloned()
        .collect();
    for id in ids {
        let _ = unshare_tab(&id);
    }
    // Also remove stale share dir files that may survive a crash
    if let Some(dir) = dirs::config_dir().map(|d| d.join("aterm").join("shares")) {
        if dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for e in entries.flatten() {
                    let path = e.path();
                    if path.is_file() {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
    }
    // Clean stale /tmp files — iterate /tmp and remove aterm-* ports (glob was broken before)
    if let Ok(entries) = std::fs::read_dir("/tmp") {
        for e in entries.flatten() {
            if let Some(name) = e.file_name().to_str() {
                if name.starts_with("aterm-") && (name.ends_with(".port") || name.ends_with(".url")) {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    }
}

//! mod.rs — HTTP API entry point (split from monolithic server.rs).
//!
//! Previously a single global server on 127.0.0.1:37241 with `check_auth` and routes
//! `/sessions/:id/*`. Now per-tab random-port sharing: each tab that is right-click
//! Shared gets its own listener on a random high port (bind 127.0.0.1:0). The port IS the id,
//! so routes are simple `GET /output`, `POST /input` scoped to that tab's `OUTPUTS` ring.
//!
//! Structure:
//! - state.rs: registry of live per-tab servers (port + JoinHandle) + SharedInfo
//! - handlers.rs: per-tab scoped Axum handlers (health_scoped, get_output_scoped, ...)
//! - share.rs: spawn/teardown logic (share_tab, unshare_tab, list_shares, cleanup_all)
//!
//! The legacy global server is kept as `start_legacy` for backward compat if config
//! `serverEnabled:true` is still enabled, but per your request it is no longer required —
//! per-tab share works without any global port/config.

pub mod handlers;
pub mod share;
pub mod state;

// Re-export the per-tab public API for `lib.rs` and `pty.rs`
pub use share::{cleanup_all, get_share_info, list_shares, share_tab, unshare_tab};
pub use state::SharedInfo;

// Keep legacy global server support optionally via config `serverEnabled`.
// If no legacy needed, this function just does nothing.
use tauri::AppHandle;

/// Legacy global server entry point (kept for compat, not required for per-tab share).
/// Called from `lib.rs` setup. If `config.serverEnabled` is false and no env `ATERM_ENABLE=1`,
/// it simply returns and only per-tab sharing is available.
pub async fn start(app: AppHandle) {
    // Reuse the original global server logic if user still has serverEnabled:true.
    // Import the old logic inline to avoid code duplication — we delegate to share.rs's
    // per-tab concept but keep a single global listener for backwards compat.
    let cfg = crate::config::load();
    let enabled = std::env::var("ATERM_ENABLE")
        .map(|v| v == "1" || v.to_lowercase() == "true")
        .unwrap_or(cfg.server_enabled);
    if !enabled {
        println!("[aterm http] global server disabled (use right-click Share for per-tab ports)");
        return;
    }

    // For backward compat, spawn a NOT-per-tab server on the configured host:port
    // that multiplexes all sessions via /sessions/:id/* (the old API).
    // We keep a minimal version here delegating to handlers with explicit id param
    // but also exposing the per-tab ports list.
    let host = std::env::var("ATERM_HOST").unwrap_or(cfg.server_host.clone());
    let port: u16 = std::env::var("ATERM_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(cfg.server_port);

    let addr = format!("{}:{}", host, port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[aterm http] legacy global bind failed {}: {}", addr, e);
            return;
        }
    };
    let local = listener.local_addr().ok();
    println!(
        "[aterm http] legacy global listening on http://{} (for old clients)",
        local.map(|a| a.to_string()).unwrap_or(addr)
    );

    // Build a tiny legacy router that just lists and proxies to existing handlers
    // (kept minimal — per-tab share is the recommended path).
    let app_state = std::sync::Arc::new(crate::server::handlers_legacy::LegacyState { app: app.clone() });
    let router = crate::server::handlers_legacy::legacy_router(app_state);

    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("[aterm http] legacy server error: {}", e);
    }
}

/// Legacy handlers module for the single global server (kept for `GET /sessions` listing).
/// This is intentionally separate from per-tab `handlers.rs` to avoid import cycles and bloat.
pub mod handlers_legacy {
    use axum::{extract::{Path, Query, State}, http::{HeaderMap, StatusCode}, response::IntoResponse, routing::{get, post}, Json, Router};
    use serde::{Deserialize, Serialize};
    use std::{collections::HashMap, sync::Arc};
    use tauri::AppHandle;
    use tower_http::cors::{Any, CorsLayer};

    use crate::{config, pty};

    #[derive(Clone)]
    pub struct LegacyState { pub app: AppHandle }

    #[derive(Serialize)] struct Health { version: String, sessions: usize }
    #[derive(Serialize)] struct SessionInfo { id: String, pid: Option<u32>, cwd: Option<String>, alive: bool }
    #[derive(Serialize)] struct CreateSessionResp { id: String }
    #[derive(Deserialize)] struct CreateSessionReq { cols: Option<u16>, rows: Option<u16>, cwd: Option<String> }
    #[derive(Deserialize)] struct ResizeReq { cols: u16, rows: u16 }
    #[derive(Deserialize)] struct WriteReq { data: String }
    #[derive(Deserialize)] struct OutputQuery { since: Option<usize>, limit: Option<usize> }

    fn check_auth(headers: &HeaderMap, token: &str, query: &HashMap<String, String>) -> bool {
        if token.is_empty() { return true; }
        if let Some(v) = headers.get("authorization") { if let Ok(s)=v.to_str() { if s==format!("Bearer {}",token) { return true; } } }
        if let Some(t)=query.get("token") { if t==token { return true; } }
        false
    }

    async fn health(State(_s): State<Arc<LegacyState>>) -> impl IntoResponse {
        let list = pty::list_sessions();
        Json(Health{ version: env!("CARGO_PKG_VERSION").to_string(), sessions: list.len() })
    }
    async fn list_sessions(State(_s): State<Arc<LegacyState>>, headers: HeaderMap, Query(q): Query<HashMap<String,String>>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&q){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        let v:Vec<SessionInfo>=pty::list_sessions().into_iter().map(|m| SessionInfo{id:m.id.clone(),pid:m.pid,cwd:m.cwd,alive:pty::session_exists(&m.id)}).collect();
        Json(v).into_response()
    }
    async fn create_session(State(state): State<Arc<LegacyState>>, headers: HeaderMap, Query(q): Query<HashMap<String,String>>, Json(req): Json<CreateSessionReq>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&q){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        let cols=req.cols.unwrap_or(80); let rows=req.rows.unwrap_or(24);
        match pty::create_session(state.app.clone(),cols,rows,req.cwd){ Ok(id)=>Json(CreateSessionResp{id}).into_response(), Err(e)=> (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"error":e}))).into_response(),}
    }
    async fn get_session(State(_s): State<Arc<LegacyState>>, headers: HeaderMap, Query(q): Query<HashMap<String,String>>, Path(id): Path<String>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&q){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        if !pty::session_exists(&id){ return (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":"session not found"}))).into_response();}
        let meta=pty::list_sessions().into_iter().find(|m| m.id==id);
        let cwd=pty::get_cwd(&id).ok().or_else(|| meta.as_ref().and_then(|m| m.cwd.clone()));
        Json(SessionInfo{id:id.clone(), pid:meta.as_ref().and_then(|m| m.pid), cwd, alive:true}).into_response()
    }
    async fn get_output(State(_s): State<Arc<LegacyState>>, headers: HeaderMap, Query(q): Query<OutputQuery>, Path(id): Path<String>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&HashMap::new()){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        let since=q.since.unwrap_or(0); let limit=q.limit.unwrap_or(32*1024).min(256*1024);
        match pty::get_output_since(&id,since,limit){ Ok((bytes,next,total))=>{ let data=String::from_utf8_lossy(&bytes).to_string(); Json(serde_json::json!({"data":data,"next_offset":next,"total":total,"truncated":next<total})).into_response()}, Err(e)=> (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":e}))).into_response(),}
    }
    async fn post_input(State(_s): State<Arc<LegacyState>>, headers: HeaderMap, Query(auth_q): Query<HashMap<String,String>>, Path(id): Path<String>, Json(req): Json<WriteReq>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&auth_q){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        match pty::write_to_session(&id,&req.data){ Ok(_)=>Json(serde_json::json!({"ok":true})).into_response(), Err(e)=> (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":e}))).into_response(),}
    }
    async fn post_resize(State(_s): State<Arc<LegacyState>>, headers: HeaderMap, Query(auth_q): Query<HashMap<String,String>>, Path(id): Path<String>, Json(req): Json<ResizeReq>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&auth_q){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        match pty::resize_session(&id,req.cols,req.rows){ Ok(_)=>Json(serde_json::json!({"ok":true})).into_response(), Err(e)=> (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":e}))).into_response(),}
    }
    async fn delete_session(State(_s): State<Arc<LegacyState>>, headers: HeaderMap, Query(auth_q): Query<HashMap<String,String>>, Path(id): Path<String>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&auth_q){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        match pty::close_session(&id){ Ok(_)=>{ let _=crate::server::unshare_tab(&id); Json(serde_json::json!({"ok":true})).into_response()}, Err(e)=> (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":e}))).into_response(),}
    }
    async fn get_cwd(State(_s): State<Arc<LegacyState>>, headers: HeaderMap, Query(auth_q): Query<HashMap<String,String>>, Path(id): Path<String>) -> impl IntoResponse {
        let cfg=config::load(); if !check_auth(&headers,&cfg.server_token,&auth_q){ return (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"error":"unauthorized"}))).into_response();}
        match pty::get_cwd(&id){ Ok(cwd)=> Json(serde_json::json!({"cwd":cwd})).into_response(), Err(e)=> (StatusCode::NOT_FOUND, Json(serde_json::json!({"error":e}))).into_response(),}
    }
    async fn list_shares_handler() -> impl IntoResponse {
        Json(crate::server::list_shares())
    }
    pub fn legacy_router(state: Arc<LegacyState>) -> Router {
        let cors=CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any);
        Router::new()
            .route("/health", get(health))
            .route("/sessions", get(list_sessions).post(create_session))
            .route("/sessions/:id", get(get_session).delete(delete_session))
            .route("/sessions/:id/output", get(get_output))
            .route("/sessions/:id/input", post(post_input))
            .route("/sessions/:id/resize", post(post_resize))
            .route("/sessions/:id/cwd", get(get_cwd))
            .route("/shares", get(list_shares_handler))
            .layer(cors).with_state(state)
    }
}

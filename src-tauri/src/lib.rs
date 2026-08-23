mod config;
mod pty;
mod server;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/

/// Simple greeting command — kept from the Tauri template for sanity checks.
/// Not used by the App but useful for `invoke("greet")` smoke tests.
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

/// Create a new PTY session and return its UUID.
/// - cols/rows: terminal grid size derived from window size and font metrics (App.handleNewTab)
/// - cwd: optional working directory to spawn the shell in. If Some(dir) and the path
///   exists and is a directory, CommandBuilder::cwd(dir) is used; otherwise the shell
///   inherits the parent process cwd (digital fallback). Frontend passes the active tab's
///   cwd via `get_cwd` so new tabs open "here".
#[tauri::command]
fn create_session(app: tauri::AppHandle, cols: u16, rows: u16, cwd: Option<String>) -> Result<String, String> {
    pty::create_session(app, cols, rows, cwd)
}

/// Return the current working directory of the given PTY session by reading
/// /proc/<pid>/cwd (Linux). Used by App.handleNewTab to inherit cwd for new tabs.
#[tauri::command]
fn get_cwd(id: String) -> Result<String, String> {
    pty::get_cwd(&id)
}

/// Write user input (keystrokes, paste) into the PTY master for the given session.
/// The PTY forwards this to the shell's stdin; output comes back via `pty:data:{id}` events.
#[tauri::command]
fn write_to_session(id: String, data: String) -> Result<(), String> {
    pty::write_to_session(&id, &data)
}

/// Resize the PTY master to match the xterm's new cols/rows after a window resize,
/// font zoom, or tab activation. The shell receives SIGWINCH and reflows.
#[tauri::command]
fn resize_session(id: String, cols: u16, rows: u16) -> Result<(), String> {
    pty::resize_session(&id, cols, rows)
}

/// Close and clean up a PTY session. Removes it from the global SESSIONS map,
/// kills the child shell if still running, and drops the PTY master (closing the fd).
/// The frontend also handles `pty:exit:{id}` for shell-initiated exits.
/// Also unshares the per-tab HTTP server if this tab was shared (right-click Share).
#[tauri::command]
fn close_session(id: String) -> Result<(), String> {
    let r = pty::close_session(&id);
    // Best-effort unshare — ignore error if not shared
    let _ = server::unshare_tab(&id);
    r
}

/// Share a terminal tab on a random high free port (localhost only, no auth).
/// The port IS the capability — `http://127.0.0.1:{port}/input` controls that tab.
/// Right-click Share in the tab bar calls this; it spawns a dedicated Axum listener.
#[tauri::command]
async fn share_tab(app: tauri::AppHandle, id: String) -> Result<server::SharedInfo, String> {
    server::share_tab(app, id).await
}

/// Stop sharing a terminal tab — aborts its dedicated server and removes discovery files.
#[tauri::command]
fn unshare_tab(id: String) -> Result<(), String> {
    server::unshare_tab(&id)
}

/// Get share info for a tab if it is currently shared.
#[tauri::command]
fn get_share_info(id: String) -> Option<server::SharedInfo> {
    server::get_share_info(&id)
}

/// List all currently shared tabs (for debugging).
#[tauri::command]
fn list_shares() -> Vec<server::SharedInfo> {
    server::list_shares()
}

/// Store a per-tab PNG screenshot captured by the frontend via `html2canvas`.
///
/// The frontend captures each tab's xterm DOM (via `onclone` fixing `opacity:0`
/// for hidden tabs) and pushes PNG base64 via this command. The Rust side
/// caches it in `server::state::SCREENSHOTS` so `GET /screenshot` on that
/// tab's dedicated port serves it, holding the connection up to 10 s until the
/// real capture arrives (no dummy). This is the single cross-compatible way.
#[tauri::command]
fn store_screenshot(id: String, png_b64: String) -> Result<(), String> {
    server::state::store_screenshot(&id, &png_b64)
}

/// Report a frontend `html2canvas` capture error for debug logs.
///
/// When `GET /screenshot` times out after 10 s, the handler returns `logs`
/// including `frontend_last_error` so the agent can see why capture failed
/// (tainted canvas, zero size, etc.) without needing browser console.
#[tauri::command]
fn report_screenshot_error(id: String, error: String) -> Result<(), String> {
    server::state::report_screenshot_error(&id, error);
    Ok(())
}

/// Load the persisted user config from `~/.config/aterm/config.json`.
/// Never fails — returns Config::default() on missing/corrupt file.
#[tauri::command]
fn get_config() -> config::Config {
    config::load()
}

/// Persist the user config to disk. The frontend validates via Zod (ConfigSchema)
/// before invoking this, so the payload should always be well-formed.
#[tauri::command]
fn save_config(config: config::Config) -> Result<(), String> {
    config::save(config)
}

/// Tauri application entry point — called from main.rs.
/// - On Linux: disables WebKit DMABUF renderer (fixes blank window on some Wayland/NVIDIA)
///   and ignores SIGINT so Ctrl+C in the embedded terminal does not kill the host process.
/// - Registers the `tauri-plugin-opener` (for opener:default) and all IPC handlers.
/// - The `windows` config (decorations:false, devUrl, frontendDist) comes from tauri.conf.json,
///   and window capability permissions come from capabilities/default.json.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        unsafe {
            libc::signal(libc::SIGINT, libc::SIG_IGN);
        }
    }

    tauri::Builder::default()
        .setup(|app| {
            // Clean stale share discovery files from a previous crash.
            // Remove stale JSONs and /tmp aterm-* files (handles unclean shutdown).
            server::cleanup_all();
            // Recreate shares dir for this run (cleanup_all removes stale files but keeps dir)
            if let Some(dir) = dirs::config_dir().map(|d| d.join("aterm").join("shares")) {
                let _ = std::fs::create_dir_all(&dir);
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                server::start(handle).await;
            });
            Ok(())
        })
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            greet,
            create_session,
            write_to_session,
            resize_session,
            close_session,
            share_tab,
            unshare_tab,
            get_share_info,
            list_shares,
            store_screenshot,
            report_screenshot_error,
            get_cwd,
            get_config,
            save_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

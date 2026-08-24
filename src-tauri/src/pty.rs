use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{AppHandle, Emitter};

// Type aliases for trait objects to keep Session readable.
type ChildBox = Box<dyn portable_pty::Child + Send + Sync>;
type MasterBox = Box<dyn portable_pty::MasterPty + Send>;
type WriterBox = Box<dyn std::io::Write + Send>;

/// Session — holds the PTY master, a shared writer, and the shell child process.
/// - master: the PTY master side; used for resize and try_clone_reader. Dropping it closes the PTY.
/// - writer: Arc<Mutex<WriterBox>> — single-use writer obtained via `master.take_writer()`. Wrapped in
///   Arc<Mutex> so write_to_session can lock and write from any thread. take_writer is single-use per
///   portable-pty's API, so we store it once at creation.
/// - child: the spawned shell (e.g., bash). Wrapped in Arc<Mutex> so get_cwd can read its pid and
///   close_session can kill it. The pid is needed for `get_cwd` (/proc/<pid>/cwd).
struct Session {
    master: MasterBox,
    writer: Arc<Mutex<WriterBox>>,
    child: Arc<Mutex<ChildBox>>,
}

/// Global session registry — lazily initialized HashMap from UUID string to Session.
/// OnceLock ensures single initialization; Mutex guards concurrent access from Tauri command
/// handlers (which run on the async runtime) and the reader threads. The map is never cleared
/// except via close_session or pty exit.
static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();

/// Per-session output history for HTTP API polling.
/// Stores raw bytes emitted by the reader thread, capped to ~512KB per session (ring).
/// Simple dump-all: `GET /output` returns the whole buffer (no cursor).
static OUTPUTS: OnceLock<Mutex<HashMap<String, Vec<u8>>>> = OnceLock::new();

/// Accessor for the global SESSIONS map, initializing it on first call.
fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn outputs() -> &'static Mutex<HashMap<String, Vec<u8>>> {
    OUTPUTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Narrow auto-clear: only real clear sequences (`clear`/`reset`/`Ctrl-L`).
/// Matches `\x0c`, `ESC c`, `ESC[2J`, `ESC[3J` — NOT `ESC[J` which appears in normal prompts (kali).
fn contains_clear_sequence(data: &[u8]) -> bool {
    if data.contains(&0x0c) {
        return true;
    }
    if data.windows(2).any(|w| w == [0x1b, b'c']) {
        return true;
    }
    if data.windows(4).any(|w| w == [0x1b, b'[', b'2', b'J'] || w == [0x1b, b'[', b'3', b'J']) {
        return true;
    }
    false
}

/// Append data to the history ring for a session, truncating to 512KB.
/// Syncs HTTP with xterm: if data contains a clear-screen sequence (`clear` typed),
/// wipe the ring so `GET /output` (dump-all) matches what the user sees.
fn append_output(id: &str, data: &[u8]) {
    if contains_clear_sequence(data) {
        if let Ok(mut map) = outputs().lock() {
            if let Some(buf) = map.get_mut(id) {
                buf.clear();
            }
        }
        crate::server::state::remove_screenshot(id);
    }
    let mut map = match outputs().lock() {
        Ok(m) => m,
        Err(_) => return,
    };
    let buf = map.entry(id.to_string()).or_default();
    buf.extend_from_slice(data);
    const MAX: usize = 512 * 1024;
    if buf.len() > MAX {
        let drain = buf.len() - MAX;
        buf.drain(0..drain);
    }
}

/// Explicit clear of a session's output history (called via POST /clear or Tauri clear_terminal).
/// Simple: drain the ring and invalidate screenshot. No generation tracking.
pub fn clear_output(id: &str) -> Result<(), String> {
    if !session_exists(id) {
        return Err(format!("session not found: {}", id));
    }
    if let Ok(mut map) = outputs().lock() {
        if let Some(buf) = map.get_mut(id) {
            buf.clear();
        } else {
            map.insert(id.to_string(), Vec::new());
        }
    }
    crate::server::state::remove_screenshot(id);
    Ok(())
}

/// Centralized cleanup: remove all state for a session id.
pub fn cleanup_state(id: &str) {
    {
        let mut sessions_map = sessions().lock().unwrap_or_else(|e| e.into_inner());
        sessions_map.remove(id);
    }
    {
        let mut out = outputs().lock().unwrap_or_else(|e| e.into_inner());
        out.remove(id);
    }
    crate::server::state::remove_screenshot(id);
}

/// Returns the current working directory of the given session's shell process.
///
/// On Linux this reads the symlink `/proc/<pid>/cwd` where pid is the shell's process id
/// (`Child::process_id()`). The symlink is maintained by the kernel and updates live as the
/// shell does `cd`, so App.handleNewTab can query the active tab's cwd and pass it to
/// `create_session` to open new tabs "here". Errors if the session doesn't exist, has no pid,
/// or the symlink cannot be read (e.g., shell already exited).
/// On non-Linux platforms this returns an error (unsupported — future: use `proc_pidinfo` on macOS).
pub fn get_cwd(id: &str) -> Result<String, String> {
    // Poison recovery: a prior panic while holding SESSIONS shouldn't permanently brick get_cwd.
    let map = sessions().lock().unwrap_or_else(|e| e.into_inner());
    let session = map.get(id).ok_or_else(|| format!("session not found: {}", id))?;
    let pid = session
        .child
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .process_id()
        .ok_or_else(|| "no pid for session".to_string())?;
    #[cfg(target_os = "linux")]
    {
        let path = std::fs::read_link(format!("/proc/{}/cwd", pid))
            .map_err(|e| format!("read_link failed: {}", e))?;
        Ok(path.to_string_lossy().to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        Err("get_cwd only supported on Linux".to_string())
    }
}

/// Create a new PTY session with the given grid size and optional working directory.
///
/// Steps:
/// 1. Resolve the shell binary: $SHELL -> /bin/bash -> /bin/sh fallback. Checks existence so a
///    misconfigured $SHELL (e.g., deleted binary) doesn't fail spawn.
/// 2. Normalize cols/rows (0 -> 80/24) to avoid portable-pty errors.
/// 3. Open a new PTY pair (master + slave) with the requested size via NativePtySystem.
/// 4. Build a CommandBuilder for the shell, setting TERM=xterm-256color and COLORTERM=truecolor
///    so colors and truecolor work in the shell. If `cwd` is Some(dir) and is a directory, set it
///    via `CommandBuilder::cwd(dir)` so the shell starts in that folder (used for "new tab here").
/// 5. Spawn the shell on the slave side, generating a UUID id for this session.
/// 6. Take the PTY master writer (single-use) and clone a reader for the output thread.
/// 7. Spawn a reader thread that loops `reader.read(buf)` and emits `pty:data:{id}` events with
///    the raw output (converted via from_utf8_lossy). Handles EIO/5 (PTY closed) as exit, and
///    WouldBlock with a brief sleep to avoid busy loops. On exit, emits `pty:exit:{id}` so the
///    frontend can auto-close the tab.
/// 8. Insert the Session into the global SESSIONS map and return the id.
///
/// The caller (Tauri command `create_session`) forwards the id to the frontend, which creates
/// a new Tab and mounts a TerminalView bound to that id's events.
pub fn create_session(app: AppHandle, cols: u16, rows: u16, cwd: Option<String>) -> Result<String, String> {
    // Resolve shell binary with existence checks
    let raw_shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let shell = if std::path::Path::new(&raw_shell).exists() {
        raw_shell
    } else if std::path::Path::new("/bin/bash").exists() {
        "/bin/bash".to_string()
    } else {
        "/bin/sh".to_string()
    };

    // Guard against 0 dimensions which some apps treat as invalid
    let cols = if cols == 0 { 80 } else { cols };
    let rows = if rows == 0 { 24 } else { rows };

    // Create the native PTY pair (master for control, slave for the shell)
    let pty_system = NativePtySystem::default();
    let size = PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    };

    let pair = pty_system
        .openpty(size)
        .map_err(|e| format!("failed to open pty: {}", e))?;

    // Build the shell command with env and optional cwd
    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(dir) = cwd {
        if !dir.is_empty() && std::path::Path::new(&dir).is_dir() {
            cmd.cwd(dir);
        }
    }
    let child = pair
        .slave
        .spawn_command(cmd)
        .map_err(|e| format!("failed to spawn shell: {}", e))?;

    let id = uuid::Uuid::new_v4().to_string();

    let child_arc: Arc<Mutex<ChildBox>> = Arc::new(Mutex::new(child));

    // Take writer once and store it (take_writer is single-use per MasterPty)
    let writer: WriterBox = pair.master.take_writer().expect("failed to take writer");
    let writer_arc = Arc::new(Mutex::new(writer));

    // Reader thread: forwards PTY output to the frontend via Tauri events
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("failed to clone reader");
    let app_clone = app.clone();
    let id_clone = id.clone();

    // Init empty history for this session
    outputs().lock().unwrap().insert(id.clone(), Vec::new());

    std::thread::spawn(move || {
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    let raw = &buf[..n];
                    let data = String::from_utf8_lossy(raw).to_string();
                    // Save to HTTP history before emit
                    append_output(&id_clone, raw);
                    let event_name = format!("pty:data:{}", id_clone);
                    // ignore emit errors (e.g. window closed)
                    let _ = app_clone.emit(&event_name, data);
                }
                Err(e) => {
                    // EIO or closed indicates pty exited
                    if e.kind() == std::io::ErrorKind::Other
                        || e.raw_os_error() == Some(5)
                    {
                        break;
                    }
                    // brief sleep to avoid busy loop on transient errors
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    // if error persists, break on next iteration if no data
                    // but continue for now
                    // Treat WouldBlock as continue
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        continue;
                    }
                    break;
                }
            }
        }
        // Emit exit event to frontend so the tab can auto-close
        let exit_event = format!("pty:exit:{}", id_clone);
        let _ = app_clone.emit(&exit_event, ());
        // Auto-cleanup via centralized helper (covers SESSIONS, OUTPUTS, screenshot)
        cleanup_state(&id_clone);
        let _ = crate::server::unshare_tab(&id_clone);
    });

    let session = Session {
        master: pair.master,
        writer: writer_arc,
        child: child_arc,
    };

    sessions()
        .lock()
        .unwrap()
        .insert(id.clone(), session);

    Ok(id)
}

/// Write data into the PTY for the given session (e.g., keystrokes from xterm onData).
/// Locks the SESSIONS map, finds the session, locks its writer, and does write_all + flush.
/// The shell receives this on its stdin.
/// Uses poison recovery (`into_inner`) — if a thread panicked while holding the
/// SESSIONS/Writer mutex, we recover the inner value instead of permanently
/// failing all future writes (common pitfall that would brick the terminal).
pub fn write_to_session(id: &str, data: &str) -> Result<(), String> {
    let map = sessions().lock().unwrap_or_else(|e| e.into_inner());
    let session = map.get(id).ok_or_else(|| format!("session not found: {}", id))?;
    let mut writer = session.writer.lock().unwrap_or_else(|e| e.into_inner());
    use std::io::Write;
    writer.write_all(data.as_bytes()).map_err(|e| e.to_string())?;
    writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Resize the PTY master for the given session to new cols/rows.
/// Called from TerminalView's ResizeObserver and activation effect whenever the
/// wrapper size, font size, or visibility changes. The kernel sends SIGWINCH to
/// the shell's foreground group so apps (vim, less) can reflow.
/// Poison recovery via `into_inner` keeps resizing alive after a panic.
pub fn resize_session(id: &str, cols: u16, rows: u16) -> Result<(), String> {
    let mut map = sessions().lock().unwrap_or_else(|e| e.into_inner());
    let session = map.get_mut(id).ok_or_else(|| format!("session not found: {}", id))?;
    session
        .master
        .resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|e| e.to_string())
}

/// Close and remove the session with the given id.
/// Removes it from the map, tries to kill the child shell if still alive, then drops
/// the Session (closing the PTY file descriptors). If the id is not found, returns an error.
/// Also called implicitly when the shell exits and the reader thread emits pty:exit.
pub fn close_session(id: &str) -> Result<(), String> {
    let session_opt = {
        let mut map = sessions().lock().unwrap_or_else(|e| e.into_inner());
        map.remove(id)
    };
    if let Some(session) = session_opt {
        {
            let mut child = session.child.lock().unwrap_or_else(|e| e.into_inner());
            let _ = child.kill();
        }
        drop(session);
        // Centralized cleanup for OUTPUTS + screenshot
        cleanup_state(id);
        let _ = crate::server::unshare_tab(id);
        Ok(())
    } else {
        // Covers race where reader thread already removed SESSIONS
        cleanup_state(id);
        let _ = crate::server::unshare_tab(id);
        Err(format!("session not found: {}", id))
    }
}

/// Snapshot metadata for listing sessions via HTTP.
#[derive(Clone)]
pub struct SessionMeta {
    pub id: String,
    pub pid: Option<u32>,
}

/// List all active sessions (ids + pid + cwd attempt).
/// Uses poison recovery (`into_inner`) so a prior panic doesn't permanently
/// hide all sessions from `GET /sessions` and per-tab health checks.
pub fn list_sessions() -> Vec<SessionMeta> {
    let map = sessions().lock().unwrap_or_else(|e| e.into_inner());
    map.iter()
        .map(|(id, sess)| {
            let pid = sess.child.lock().ok().and_then(|c| c.process_id());
            SessionMeta {
                id: id.clone(),
                pid,
            }
        })
        .collect()
}

/// Whether a session id exists
/// Poison recovery keeps existence checks alive after a lock poisoning event.
pub fn session_exists(id: &str) -> bool {
    sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(id)
}

/// Get output snapshot for HTTP: returns (data_bytes, total_len).
/// Simple dump-all: returns the whole 512KB ring (no cursor). Poison recovery.
pub fn get_output(id: &str) -> Result<(Vec<u8>, usize), String> {
    let map = outputs().lock().unwrap_or_else(|e| e.into_inner());
    let buf = map.get(id).ok_or_else(|| format!("session not found: {}", id))?;
    Ok((buf.clone(), buf.len()))
}

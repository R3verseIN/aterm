use aho_corasick::AhoCorasick;
use dashmap::DashMap;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::LazyLock;
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
    tmp_rc: Option<String>,
}

/// Global session registry — Mutex still needed because Session contains `dyn MasterPty + Send` (not Sync, so DashMap fails)
static SESSIONS: OnceLock<std::sync::Mutex<std::collections::HashMap<String, Session>>> = OnceLock::new();

/// Per-session output history for HTTP API polling — DashMap sharded, no poisoning
static OUTPUTS: OnceLock<DashMap<String, Vec<u8>>> = OnceLock::new();

/// Monotonic version per session
static OUTPUT_VERSIONS: OnceLock<DashMap<String, u64>> = OnceLock::new();

fn sessions() -> &'static std::sync::Mutex<std::collections::HashMap<String, Session>> {
    SESSIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub(crate) fn outputs() -> &'static DashMap<String, Vec<u8>> {
    OUTPUTS.get_or_init(DashMap::new)
}

pub(crate) fn output_versions() -> &'static DashMap<String, u64> {
    OUTPUT_VERSIONS.get_or_init(DashMap::new)
}

fn bump_version(id: &str) -> u64 {
    let mut entry = output_versions().entry(id.to_string()).or_insert(0);
    *entry = entry.wrapping_add(1);
    *entry
}

pub fn get_version(id: &str) -> u64 {
    output_versions().get(id).map(|v| *v).unwrap_or(0)
}

/// Narrow auto-clear: only real clear sequences (`clear`/`reset`/`Ctrl-L`).
/// Matches `\x0c`, `ESC c`, `ESC[2J`, `ESC[3J` — NOT `ESC[J` which appears in normal prompts (kali).
/// Uses `aho-corasick` single-pass on &[u8] (no windows overlap bug).
static CLEAR_AC: LazyLock<AhoCorasick> = LazyLock::new(|| {
    AhoCorasick::new(["\x0c", "\x1b\x63", "\x1b[2J", "\x1b[3J"]).unwrap()
});
fn contains_clear_sequence(data: &[u8]) -> bool {
    CLEAR_AC.is_match(data)
}

/// Append data to the history ring for a session, truncating to 512KB.
fn append_output(id: &str, data: &[u8]) {
    if contains_clear_sequence(data) {
        if let Some(mut buf) = outputs().get_mut(id) {
            buf.clear();
        }
        crate::server::state::remove_screenshot(id);
    }
    {
        let mut buf = outputs().entry(id.to_string()).or_default();
        buf.extend_from_slice(data);
        const MAX: usize = 512 * 1024;
        if buf.len() > MAX {
            let drain = buf.len() - MAX;
            buf.drain(0..drain);
        }
    }
    let ver = bump_version(id);
    crate::server::state::notify_output_waiters(id, ver);
}

/// Explicit clear — simple: just execute `clear` (or `cls` on Windows) in the shell.
/// No manual buffer manipulation — let the shell emit ESC[2J and let append_output's
/// contains_clear_sequence auto-wipe the 512KB ring so HTTP and xterm stay in sync.
pub fn clear_output(id: &str) -> Result<(), String> {
    if !session_exists(id) {
        return Err(format!("session not found: {}", id));
    }
    let data = if cfg!(windows) { "cls\r" } else { "clear\r" };
    write_to_session(id, data)?;
    crate::server::state::remove_screenshot(id);
    Ok(())
}

/// Centralized cleanup: remove all state for a session id.
pub fn cleanup_state(id: &str) {
    let tmp_rc_opt = {
        let mut map = sessions().lock().unwrap_or_else(|e| e.into_inner());
        map.remove(id).and_then(|s| s.tmp_rc)
    };
    if let Some(p) = tmp_rc_opt {
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir_all(&p);
        let _ = std::fs::remove_file(format!("{}/.zshrc", p));
    }
    outputs().remove(id);
    output_versions().remove(id);
    crate::server::state::remove_screenshot(id);
    crate::server::state::remove_output_waiters(id);
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
    // Shell integration: for bash, use a temp rcfile that sources ~/.bashrc then
    // installs invisible OSC 633;D prompt marker (stable for any command, no per-request `; printf`).
    // This keeps `POST /input` hold prompt-based, screenshot-clean, and syntax-agnostic.
    let mut _tmp_rc_path: Option<String> = None;
    let mut cmd = CommandBuilder::new(&shell);
    if shell.contains("bash") {
        let rc_path = format!("/tmp/aterm-{}-bashrc", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let rc_content = "[ -f ~/.bashrc ] && source ~/.bashrc\n__aterm_precmd() { printf '\\033]633;D;%s\\007' \"$?\"; printf '\\033]633;A\\007'; }\nPROMPT_COMMAND=\"__aterm_precmd;${PROMPT_COMMAND:+$PROMPT_COMMAND; }\"\n";
        if std::fs::write(&rc_path, rc_content).is_ok() {
            cmd.args(["--rcfile", &rc_path, "-i"]);
            _tmp_rc_path = Some(rc_path);
        }
    } else if shell.contains("zsh") {
        let zdot = format!("/tmp/aterm-{}-zdot", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let _ = std::fs::create_dir_all(&zdot);
        let zrc = format!("{}/.zshrc", zdot);
        let zcontent = "[ -f ~/.zshrc ] && source ~/.zshrc\n__aterm_precmd() { printf '\\033]633;D;%s\\007' \"$?\"; printf '\\033]633;A\\007'; }\nif (( $+functions[precmd_functions] )); then precmd_functions+=(__aterm_precmd); else precmd() { __aterm_precmd; }; fi\n";
        if std::fs::write(&zrc, zcontent).is_ok() {
            cmd.env("ZDOTDIR", zdot.clone());
            _tmp_rc_path = Some(zdot);
        }
    }
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    // Inherit aterm marker so handlers can know integration is active (optional)
    cmd.env("ATERM_SHELL_INTEGRATION", "1");
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
    outputs().insert(id.clone(), Vec::new());
    output_versions().insert(id.clone(), 0);

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
        tmp_rc: _tmp_rc_path.clone(),
    };

    sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
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
        if let Some(ref p) = session.tmp_rc {
            let _ = std::fs::remove_file(p);
            let _ = std::fs::remove_dir_all(p);
            let _ = std::fs::remove_file(format!("{}/.zshrc", p));
        }
        {
            let mut child = session.child.lock().unwrap_or_else(|e| e.into_inner());
            let _ = child.kill();
        }
        drop(session);
        outputs().remove(id);
        output_versions().remove(id);
        crate::server::state::remove_screenshot(id);
        crate::server::state::remove_output_waiters(id);
        let _ = crate::server::unshare_tab(id);
        Ok(())
    } else {
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

pub fn session_exists(id: &str) -> bool {
    sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(id)
}

pub fn has_integration(id: &str) -> bool {
    sessions()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(id)
        .and_then(|s| s.tmp_rc.clone())
        .is_some()
}

pub fn get_output(id: &str) -> Result<(Vec<u8>, usize, u64), String> {
    let entry = outputs().get(id).ok_or_else(|| format!("session not found: {}", id))?;
    let (cloned, len) = (entry.clone(), entry.len());
    drop(entry);
    let ver = get_version(id);
    Ok((cloned, len, ver))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clear_seq_aho_detects_esc2j() {
        assert!(contains_clear_sequence(b"\x1b[2J"));
        assert!(contains_clear_sequence(b"\x1b[3J"));
        assert!(contains_clear_sequence(b"\x0c"));
        assert!(contains_clear_sequence(b"\x1b\x63"));
        assert!(!contains_clear_sequence(b"\x1b[J"));
        assert!(!contains_clear_sequence(b"hello"));
    }

    #[test]
    fn version_bump_wraps() {
        let id = "test-ver";
        output_versions().insert(id.to_string(), u64::MAX);
        let v = bump_version(id);
        assert_eq!(v, 0);
        output_versions().remove(id);
    }

    #[test]
    fn append_output_truncates_512k() {
        let id = "test-trunc";
        outputs().insert(id.to_string(), Vec::new());
        output_versions().insert(id.to_string(), 0);
        let big = vec![b'x'; 600 * 1024];
        append_output(id, &big);
        let (buf, len, _) = get_output(id).unwrap();
        assert!(len <= 512 * 1024);
        assert_eq!(buf.len(), len);
        cleanup_state(id);
    }
}

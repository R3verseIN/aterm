use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Default value helpers — used by serde `default = "..."` attributes.
// Each returns the canonical default for that field, matching the Zod defaults
// in src/schemas/configSchema.ts (theme/fontSize/fontFamily/shell/s себеск).
// ---------------------------------------------------------------------------

/// Default theme — must match ConfigSchema.enum default ("aterm-dark").
fn default_theme() -> String {
    "aterm-dark".to_string()
}

/// Default font family — monospaced stack optimized for terminals.
fn default_font_family() -> String {
    "JetBrains Mono, monospace".to_string()
}

/// Default font size in pixels — 12px balances readability and fit density.
/// Zod schema clamps 8..32; Rust u8 allows 0..255 but we default to 12.
fn default_font_size() -> u8 {
    12
}

/// Default scrollback lines — 1500 lines kept in history buffer.
/// Matches Zod min 100 max 10000 default 1500.
fn default_scrollback() -> u32 {
    1500
}

/// Default for HTTP API server enabled flag — disabled by default for security.
fn default_server_enabled() -> bool {
    false
}

/// Default HTTP API port — 37241 is unassigned, not conflicting with vite:1420.
fn default_server_port() -> u16 {
    37241
}

/// Default server host — localhost only, never 0.0.0.0 by default.
fn default_server_host() -> String {
    "127.0.0.1".to_string()
}

/// User configuration persisted at `~/.config/aterm/config.json` (via `dirs` crate).
///
/// Serialization:
/// - `rename_all = "camelCase"` emits camelCase JSON (fontSize, fontFamily) to match
///   the frontend ConfigType. Aliases (`alias = "font_family"`) accept old snake_case
///   files written before the Tauri migration, ensuring backward compatibility.
/// - `default = "..."` on each field ensures that if a key is missing (e.g., old file
///   without `scrollback` or a frontend that omits a field), deserialization still
///   succeeds with the canonical default rather than failing the whole load.
/// - `Default` impl provides the same defaults for `load()` fallback when the file
///   is missing or corrupted.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Theme name — one of "aterm-dark" | "aterm-light" | "nord".
    #[serde(default = "default_theme")]
    pub theme: String,
    /// CSS font-family for the terminal, e.g. "JetBrains Mono, monospace".
    #[serde(default = "default_font_family", alias = "font_family")]
    pub font_family: String,
    /// Font size in pixels, clamped 8..32 on the frontend.
    #[serde(default = "default_font_size", alias = "font_size")]
    pub font_size: u8,
    /// Shell binary path — empty means use $SHELL fallback chain in pty.rs.
    #[serde(default)]
    pub shell: String,
    /// Scrollback buffer size in lines.
    #[serde(default = "default_scrollback", alias = "scrollback")]
    pub scrollback: u32,
    /// Whether the built-in HTTP API server is enabled (localhost only).
    #[serde(default = "default_server_enabled")]
    pub server_enabled: bool,
    /// Host for HTTP API — default 127.0.0.1. Only change to 0.0.0.0 if you understand the risk.
    #[serde(default = "default_server_host")]
    pub server_host: String,
    /// Port for HTTP API — 37241 default, 0 = random available.
    #[serde(default = "default_server_port")]
    pub server_port: u16,
    /// Bearer token for HTTP API — empty means no auth (localhost-only still safe). Set to require Authorization: Bearer <token>.
    #[serde(default)]
    pub server_token: String,
}

impl Default for Config {
    /// Returns the canonical default config — used when no config file exists
    /// or when `load()` encounters a read/parse error and falls back.
    fn default() -> Self {
        Self {
            theme: "aterm-dark".to_string(),
            font_family: "JetBrains Mono, monospace".to_string(),
            font_size: 12,
            shell: String::new(),
            scrollback: 1500,
            server_enabled: false,
            server_host: "127.0.0.1".to_string(),
            server_port: 37241,
            server_token: String::new(),
        }
    }
}

/// Resolve the config file path.
/// Primary: `dirs::config_dir()` (XDG_CONFIG_HOME or ~/.config on Linux).
/// Fallback: $HOME/.config/aterm/config.json for edge cases where config_dir is None
/// (e.g., minimal containers without XDG env).
fn config_path() -> PathBuf {
    if let Some(dir) = dirs::config_dir() {
        dir.join("aterm").join("config.json")
    } else {
        // fallback to $HOME/.config/aterm/config.json
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".config")
            .join("aterm")
            .join("config.json")
    }
}

/// Load the config from disk. On any I/O or JSON parse error (missing file,
/// corrupted JSON, unknown fields), silently returns `Config::default()` so the
/// app always starts with a usable config. The caller (App.tsx get_config) then
/// validates via parseConfig and shows defaults.
pub fn load() -> Config {
    let path = config_path();
    if let Ok(data) = fs::read_to_string(&path) {
        if let Ok(cfg) = serde_json::from_str::<Config>(&data) {
            return cfg;
        }
    }
    Config::default()
}

/// Persist the config to disk, creating parent directories as needed.
/// The config is serialized with `to_string_pretty` for human-editable JSON and
/// uses camelCase keys due to `rename_all`. Any I/O or serialization error is
/// returned as a String for the Tauri command to surface to the frontend.
pub fn save(cfg: Config) -> Result<(), String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let data = serde_json::to_string_pretty(&cfg).map_err(|e| e.to_string())?;
    fs::write(&path, data).map_err(|e| e.to_string())
}

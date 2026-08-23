use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

fn default_theme() -> String {
    "aterm-dark".to_string()
}
fn default_font_family() -> String {
    "JetBrains Mono, monospace".to_string()
}
fn default_font_size() -> u8 {
    12
}
fn default_scrollback() -> u32 {
    1500
}

/// User config at `~/.config/aterm/config.json` — per-tab share needs no server fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_font_family", alias = "font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size", alias = "font_size")]
    pub font_size: u8,
    #[serde(default)]
    pub shell: String,
    #[serde(default = "default_scrollback", alias = "scrollback")]
    pub scrollback: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "aterm-dark".to_string(),
            font_family: "JetBrains Mono, monospace".to_string(),
            font_size: 12,
            shell: String::new(),
            scrollback: 1500,
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

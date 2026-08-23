//! main.rs — Binary entry point for aterm.
//! On Windows this hides the extra console window in release builds
//! (the `windows_subsystem = "windows"` attribute). On all platforms it
//! simply delegates to `aterm_lib::run()` which builds the Tauri app,
//! spawns the optional HTTP API (if enabled), and runs the event loop.

// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Binary entry — Tauri's `run()` blocks until the window closes.
fn main() {
    aterm_lib::run()
}

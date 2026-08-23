//! build.rs — Tauri codegen. This runs at `cargo build` time to
//! generate `src-tauri/gen/` (capabilities, ACL manifests, schemas) from
//! `tauri.conf.json` + `capabilities/*.json`. Keep it minimal; `tauri_build::build()`
//! already handles context generation and `cargo:rerun-if-changed` hints.
fn main() {
    tauri_build::build()
}

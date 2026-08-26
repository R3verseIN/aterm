//! mod.rs — HTTP API entry point for per-tab random-port sharing.
//!
//! Each right-click Share binds `127.0.0.1:0` (random high port). The port is
//! the tab id, so routes are `POST /input`, `GET /output`, `GET /clear`,
//! `GET /screenshot`, `GET /health` scoped to that tab.

pub mod handlers;
pub mod share;
pub mod state;

pub use share::{cleanup_all, get_share_info, list_shares, share_tab, unshare_tab};
pub use state::SharedInfo;

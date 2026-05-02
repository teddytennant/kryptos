//! SigVim — vim-first Signal desktop client.
//!
//! Top-level crate root. Modules are split by responsibility:
//!
//! - [`config`] — parse and watch `~/.config/sigvim/config.toml`.
//! - [`core`]   — shared error type, logging.
//! - [`ui`]     — gtk4 + libadwaita view layer.

pub mod config;
pub mod core;
pub mod ui;

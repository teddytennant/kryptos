//! Kryptos — vim-first Signal desktop client.
//!
//! Top-level crate root. Modules are split by responsibility:
//!
//! - [`config`] — parse and watch `~/.config/kryptos/config.toml`.
//! - [`core`]   — shared error type, logging.
//! - [`dbus`]   — type-safe zbus proxy for signal-cli.
//! - [`ui`]     — gtk4 + libadwaita view layer.
//! - [`vim`]    — modal state machine and keymap evaluator.

pub mod config;
pub mod core;
pub mod dbus;
pub mod ui;
pub mod vim;

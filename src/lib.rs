//! SigVim — vim-first Signal desktop client.
//!
//! Top-level crate root. Modules are split by responsibility:
//!
//! - [`core`] — shared error type, logging.
//! - [`ui`]   — gtk4 + libadwaita view layer.

pub mod core;
pub mod ui;

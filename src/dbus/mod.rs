//! signal-cli D-Bus client.
//!
//! signal-cli exposes its API on the user session bus under the
//! well-known name `org.asamk.Signal`. Two interfaces are involved:
//!
//! - `org.asamk.SignalControl` at `/org/asamk/Signal` — registration,
//!   linking, listing accounts.
//! - `org.asamk.Signal` at `/org/asamk/Signal` (single-account mode) or
//!   `/org/asamk/Signal/_<digits>` (multi-account daemon mode) —
//!   per-account messaging.
//!
//! See <https://github.com/AsamK/signal-cli/blob/master/man/signal-cli-dbus.5.adoc>.

pub mod client;
pub mod proxy;
pub mod stream;

pub use client::SignalClient;

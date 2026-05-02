# Backlog

## Future

- Add Telegram support — extend beyond Signal-only. Likely a separate transport
  module alongside `dbus/` (signal-cli) so the vim engine and UI stay
  protocol-agnostic. Candidates: `tdlib` (official, C++), `grammers` (Rust,
  MTProto), or `tdlib-rs` bindings.

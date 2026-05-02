# Backlog

## Future

### Telegram support (secondary messenger)

Extend Kryptos beyond Signal-only. Goal: one client, multiple accounts across
protocols, vim engine and UI stay protocol-agnostic.

**Backend choice — leaning `grammers`:**

- `grammers` (pure Rust MTProto, async, Tokio-native) — best stack fit, no C++
  dep, drops into the existing crate. Trade-off: no secret chats, no voice/video
  calls.
- `rust-tdlib` (bindings to official C++ TDLib) — full feature parity (secret
  chats, calls, all of Telegram). Trade-off: ~50MB native dep, slower builds,
  more Nix packaging work.
- `tdl` daemon over JSON-RPC — mirrors the signal-cli/D-Bus pattern but adds an
  out-of-process hop for no real gain in Rust.

Start with `grammers`. Swap to `rust-tdlib` later if calls/secret chats become
must-haves.

**Architecture:**

- Introduce a `messenger/` module with a `MessengerBackend` trait:
  `list_chats`, `send_message`, `subscribe_events`, `fetch_history`,
  `mark_read`, `typing`, `attachments`. Async, returns a stream of normalized
  events.
- Move current `dbus/` (signal-cli) behind a `signal::SignalBackend` impl of
  the trait. Keep zbus as the IPC layer for that one backend.
- Add `telegram::TelegramBackend` using `grammers-client`. Login via phone +
  code, store session in `~/.local/share/kryptos/telegram.session` (encrypted
  via the existing keyring path used for Signal).
- Normalize across protocols: `ChatId(Backend, NativeId)`, `Message`,
  `Attachment`, `Reaction`, `Presence`. Backend-specific extras live in an
  enum tail (`BackendExtras::Telegram { ... }`) so the UI can render reply
  threads, polls, reactions, etc. without leaking protocol details into core.
- Multiplex backends in a `MessengerHub` that owns each backend's task and
  fans events into the existing event bus.

**Config (`config.toml`):**

```toml
[backends.signal]
enabled = true
account = "+1..."

[backends.telegram]
enabled = true
api_id = 12345        # from my.telegram.org
api_hash = "..."
session_path = "~/.local/share/kryptos/telegram.session"
```

Hot-reload should toggle backends on/off without restart.

**UI:**

- Chat list groups by backend with a small badge (S / T) — or unified, sorted
  by recency. Decide once both backends exist.
- Account switcher (`<leader>a`) cycles backend+account pairs.
- Status line shows which backend the focused chat belongs to.

**Phasing:**

1. Refactor `dbus/` into `messenger/signal/` behind the trait. No behavior
   change, just plumbing. Tests still green.
2. Add `messenger/telegram/` skeleton + login flow (CLI prompt for code, then
   persist session). No UI yet — verify via integration test that we can
   list dialogs.
3. Wire Telegram into `MessengerHub`, surface in chat list, send/receive
   text only.
4. Attachments (photo, file, voice note playback).
5. Telegram-specific niceties: replies, reactions, edits, pinned messages.
6. Multi-account per backend.

**Open questions:**

- 2FA password handling on login — prompt in TUI overlay vs. `pass` lookup.
- Where to draw the line on Telegram-only features (channels, bots,
  stickers). Probably: render, don't author, in v1.
- Notification routing: should `notify-rust` actions reply via the
  originating backend? (Yes — `MessengerHub::reply(NotificationId, text)`.)
- License: `grammers` is MIT, fine. `rust-tdlib` pulls TDLib (Boost
  Software License) — also fine if we go that route later.

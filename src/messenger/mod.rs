//! Protocol-agnostic messenger backend abstraction.
//!
//! Kryptos started as a signal-cli wrapper. To grow beyond Signal we
//! split the data layer into:
//!
//! - [`MessengerBackend`] — async trait every backend implements.
//! - [`SignalBackend`](signal::SignalBackend) — wraps [`crate::dbus::SignalClient`].
//! - [`TelegramBackend`](telegram::TelegramBackend) — `grammers-client` (stub
//!   today; activated once the dependency is wired in).
//! - [`MessengerHub`] — fans every backend's event stream
//!   into a single channel and routes outbound calls by [`Backend`].
//!
//! All chat IDs travel as [`ChatId`] (`backend + native id`) so the UI can
//! mix backends without losing track of where a conversation came from.

pub mod hub;
pub mod signal;
pub mod telegram;

use std::fmt;
use std::path::PathBuf;

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::core::Result;

pub use hub::MessengerHub;

/// Which protocol a chat / message belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Backend {
    Signal,
    Telegram,
}

impl Backend {
    /// Stable short tag used in serialised IDs and UI badges.
    pub fn as_tag(self) -> &'static str {
        match self {
            Backend::Signal => "signal",
            Backend::Telegram => "telegram",
        }
    }

    pub fn parse_tag(tag: &str) -> Option<Self> {
        match tag {
            "signal" => Some(Backend::Signal),
            "telegram" => Some(Backend::Telegram),
            _ => None,
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_tag())
    }
}

/// Protocol-tagged conversation identifier.
///
/// `native` is whatever the underlying protocol uses to address a chat —
/// for Signal this is an E.164 number, UUID, or hex group id; for
/// Telegram it's the dialog's id (`peer.id` rendered as decimal).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChatId {
    pub backend: Backend,
    pub native: String,
}

impl ChatId {
    pub fn new(backend: Backend, native: impl Into<String>) -> Self {
        Self {
            backend,
            native: native.into(),
        }
    }

    /// `signal:+14155552671` style round-trippable string.
    pub fn to_wire(&self) -> String {
        format!("{}:{}", self.backend.as_tag(), self.native)
    }

    /// Parse the inverse of [`Self::to_wire`]. Returns `None` for any
    /// malformed input (missing colon, unknown tag, empty native).
    pub fn from_wire(s: &str) -> Option<Self> {
        let (tag, native) = s.split_once(':')?;
        if native.is_empty() {
            return None;
        }
        Some(Self::new(Backend::parse_tag(tag)?, native))
    }
}

impl fmt::Display for ChatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_wire())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAttachment {
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    /// Local path when the backend has already downloaded the blob.
    pub local_path: Option<PathBuf>,
    pub size: Option<i64>,
}

/// Backend-specific tail kept tiny in v1 — protocol features (replies,
/// reactions, polls, voice notes) bolt on here without churning the
/// core type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendExtras {
    Signal {
        /// Group id (raw bytes) when the message belongs to a group.
        group_id: Option<Vec<u8>>,
    },
    Telegram {
        reply_to_msg_id: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedMessage {
    /// The conversation this message belongs to.
    pub id: ChatId,
    pub ts_ms: i64,
    pub sender: String,
    pub body: Option<String>,
    pub attachments: Vec<NormalizedAttachment>,
    pub backend_extras: BackendExtras,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    MessageReceived(NormalizedMessage),
    Edited {
        id: ChatId,
        ts: i64,
        new_body: String,
    },
    Deleted {
        id: ChatId,
        ts: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: ChatId,
    pub title: String,
    pub last_message_ts: Option<i64>,
    pub unread: u32,
}

/// Common surface every messenger backend exposes to the rest of the app.
///
/// All methods are async; backends are expected to be cheaply cloneable
/// (typically wrapped in `Arc`) and to keep their long-running work
/// inside a tokio task spawned at construction time.
#[async_trait]
pub trait MessengerBackend: Send + Sync {
    fn backend(&self) -> Backend;

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>>;

    async fn fetch_history(
        &self,
        id: &ChatId,
        limit: u32,
        before_ts: Option<i64>,
    ) -> Result<Vec<NormalizedMessage>>;

    /// Send a message. Returns the timestamp the backend assigned (used
    /// downstream as the message's stable id alongside `sender`).
    async fn send_message(&self, id: &ChatId, body: &str, attachments: &[PathBuf]) -> Result<i64>;

    async fn mark_read(&self, id: &ChatId) -> Result<()>;

    async fn typing(&self, id: &ChatId, on: bool) -> Result<()>;

    /// Subscribe to incoming events. Each call returns a fresh receiver;
    /// the backend is expected to broadcast internally so multiple
    /// subscribers can coexist without dropping events.
    async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<Event>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_tag_round_trips() {
        for b in [Backend::Signal, Backend::Telegram] {
            assert_eq!(Backend::parse_tag(b.as_tag()), Some(b));
        }
        assert_eq!(Backend::parse_tag("imessage"), None);
    }

    #[test]
    fn chat_id_wire_round_trip() {
        let id = ChatId::new(Backend::Signal, "+14155552671");
        assert_eq!(id.to_wire(), "signal:+14155552671");
        assert_eq!(ChatId::from_wire("signal:+14155552671"), Some(id.clone()));
        assert_eq!(id.to_string(), "signal:+14155552671");

        let tg = ChatId::new(Backend::Telegram, "100200300");
        assert_eq!(tg.to_wire(), "telegram:100200300");
        assert_eq!(ChatId::from_wire(&tg.to_wire()), Some(tg));
    }

    #[test]
    fn chat_id_native_can_contain_colons() {
        // A telegram id will never contain a colon, but a signal group
        // hex blob conceivably could be wrapped in something that does;
        // make sure split_once(':') keeps anything after the first colon.
        let parsed = ChatId::from_wire("telegram:abc:def").unwrap();
        assert_eq!(parsed.backend, Backend::Telegram);
        assert_eq!(parsed.native, "abc:def");
    }

    #[test]
    fn chat_id_rejects_malformed() {
        assert_eq!(ChatId::from_wire(""), None);
        assert_eq!(ChatId::from_wire("signal"), None, "no colon");
        assert_eq!(ChatId::from_wire("signal:"), None, "empty native");
        assert_eq!(ChatId::from_wire("imessage:foo"), None, "unknown tag");
    }
}

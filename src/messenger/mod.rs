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
    /// Friendly sender label when the backend resolved one (Telegram
    /// `first_name [last_name]`, signal-cli envelope `sourceName` /
    /// cached contact). Falls back to `sender` for the UI when
    /// `None`.
    pub sender_display: Option<String>,
    pub body: Option<String>,
    pub attachments: Vec<NormalizedAttachment>,
    pub backend_extras: BackendExtras,
}

impl NormalizedMessage {
    /// Best-effort display name for the message author. Returns the
    /// resolved label when present, otherwise the raw `sender`.
    pub fn sender_label(&self) -> &str {
        self.sender_display
            .as_deref()
            .unwrap_or(self.sender.as_str())
    }
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
    /// Raw fallback title (the backend's native id, used when nothing
    /// better is available). Kept around so the UI can still render
    /// something for an unknown peer.
    pub title: String,
    /// Friendly name resolved from the backend's contact / chat
    /// metadata (Telegram `first_name [last_name]` for users, group
    /// title for chats; signal-cli `getContactName` for E.164 peers).
    /// `None` means "we couldn't resolve a name; fall back to `title`".
    pub display_name: Option<String>,
    pub last_message_ts: Option<i64>,
    /// Body of the most recent message in this conversation, used as
    /// the sidebar preview line under the contact name. `None` when
    /// the backend hasn't surfaced one (live `list_conversations`
    /// from signal-cli / grammers carries no bodies — only the
    /// cache-driven path fills this).
    pub preview: Option<String>,
    pub unread: u32,
}

impl ConversationSummary {
    /// Best-effort label for the chat list. Prefers the resolved
    /// display name, falls back to the raw `title`.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .unwrap_or(self.title.as_str())
    }
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

    /// Identifier the backend is logged in as — Signal's E.164 number,
    /// Telegram's user_id rendered as a string. Returns `None` when the
    /// backend hasn't (yet) resolved its own identity (no account
    /// configured + zero local accounts; Telegram pre-login). The UI
    /// uses this to decide whether an incoming message's `sender`
    /// belongs to the local user.
    fn self_account(&self) -> Option<&str> {
        None
    }
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

    #[test]
    fn conversation_summary_label_prefers_display_name() {
        let with_name = ConversationSummary {
            id: ChatId::new(Backend::Signal, "+14155552671"),
            title: "+14155552671".into(),
            display_name: Some("Alice Smith".into()),
            last_message_ts: None,
            preview: None,
            unread: 0,
        };
        assert_eq!(with_name.label(), "Alice Smith");

        let without_name = ConversationSummary {
            id: ChatId::new(Backend::Telegram, "12345"),
            title: "12345".into(),
            display_name: None,
            last_message_ts: None,
            preview: None,
            unread: 0,
        };
        assert_eq!(
            without_name.label(),
            "12345",
            "fallback to title when display_name is None"
        );
    }

    #[test]
    fn normalized_message_sender_label_falls_back_to_sender() {
        let resolved = NormalizedMessage {
            id: ChatId::new(Backend::Telegram, "9"),
            ts_ms: 1,
            sender: "9".into(),
            sender_display: Some("Carol Danvers".into()),
            body: Some("hi".into()),
            attachments: Vec::new(),
            backend_extras: BackendExtras::Telegram {
                reply_to_msg_id: None,
            },
        };
        assert_eq!(resolved.sender_label(), "Carol Danvers");

        let unresolved = NormalizedMessage {
            sender_display: None,
            ..resolved
        };
        assert_eq!(
            unresolved.sender_label(),
            "9",
            "fall back to raw sender when None"
        );
    }
}

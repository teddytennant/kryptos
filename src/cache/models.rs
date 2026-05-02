//! Plain data types mirroring the D-Bus shape from signal-cli.
//!
//! Timestamps are unix milliseconds (`i64`) — kept raw to avoid a chrono/time dep.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conversation {
    pub id: String,
    pub name: Option<String>,
    pub group_id: Option<Vec<u8>>,
    pub last_message_ts: Option<i64>,
    pub unread_count: i32,
    pub archived: bool,
    pub muted_until: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// SQLite rowid; 0 means "not yet inserted".
    pub id: i64,
    pub conversation_id: String,
    pub ts: i64,
    pub sender: String,
    pub body: Option<String>,
    pub quote_ts: Option<i64>,
    pub quote_sender: Option<String>,
    pub edited_ts: Option<i64>,
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contact {
    pub number: String,
    pub name: Option<String>,
    pub profile_name: Option<String>,
    pub blocked: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: i64,
    pub message_id: i64,
    pub mime_type: String,
    pub file_name: Option<String>,
    pub path: Option<String>,
    pub size: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reaction {
    pub message_id: i64,
    pub sender: String,
    pub emoji: String,
    pub ts: i64,
}

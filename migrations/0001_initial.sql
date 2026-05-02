-- Conversations: 1:1 or group threads keyed by signal-cli's conversation id.
CREATE TABLE conversations (
    id               TEXT PRIMARY KEY,
    name             TEXT,
    group_id         BLOB,
    last_message_ts  INTEGER,
    unread_count     INTEGER NOT NULL DEFAULT 0,
    archived         INTEGER NOT NULL DEFAULT 0,
    muted_until      INTEGER
);

-- Messages: rowid is our internal id; ts is unix millis from signal-cli.
CREATE TABLE messages (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id  TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    ts               INTEGER NOT NULL,
    sender           TEXT NOT NULL,
    body             TEXT,
    quote_ts         INTEGER,
    quote_sender     TEXT,
    edited_ts        INTEGER,
    deleted          INTEGER NOT NULL DEFAULT 0
);

-- Hot path: list_messages paginates DESC by (conversation_id, ts).
CREATE INDEX idx_messages_conv_ts ON messages (conversation_id, ts DESC);

CREATE TABLE contacts (
    number        TEXT PRIMARY KEY,
    name          TEXT,
    profile_name  TEXT,
    blocked       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE attachments (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    mime_type   TEXT NOT NULL,
    file_name   TEXT,
    path        TEXT,
    size        INTEGER
);

CREATE INDEX idx_attachments_message ON attachments (message_id);

-- Reactions are idempotent per (message, sender): a sender's latest reaction wins.
CREATE TABLE reactions (
    message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    sender      TEXT NOT NULL,
    emoji       TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    PRIMARY KEY (message_id, sender)
);

CREATE INDEX idx_reactions_message ON reactions (message_id);

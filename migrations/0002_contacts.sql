-- Generic per-backend contact directory.
--
-- The legacy `contacts` table from migration 0001 is keyed only on
-- `number` (E.164) and was modelled around signal-cli. The Telegram
-- backend identifies peers by numeric user_id (and groups by hex
-- PackedChat blobs), so we need a second table that's tagged with
-- the backend so signal:+1… and telegram:12345 can coexist without
-- key collisions.
--
-- `display_name` is whatever the backend hands us (signal-cli's
-- contact name, Telegram's `first_name [last_name]` for users, the
-- group title for chats/channels). The UI prefers this string when
-- rendering the chat list, content header, and per-message sender;
-- callers fall back to the raw native id when the row is missing.
--
-- `updated_at` is unix milliseconds; we keep it so a future "refresh
-- if older than X" job can avoid hammering the network on every
-- launch.
CREATE TABLE messenger_contacts (
    backend       TEXT    NOT NULL,
    native_id     TEXT    NOT NULL,
    display_name  TEXT    NOT NULL,
    updated_at    INTEGER NOT NULL,
    PRIMARY KEY (backend, native_id)
);

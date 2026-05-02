//! Repository-style methods on [`Cache`].
//!
//! Each method is a single SQL operation (or short transaction) against the
//! pool. Returns are owned plain values from [`super::models`] — no sqlx types leak.

use sqlx::Row;

use super::models::{Attachment, Contact, Conversation, Message, Reaction};
use super::Cache;
use crate::core::Result;

impl Cache {
    pub async fn upsert_conversation(&self, c: &Conversation) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO conversations
                (id, name, group_id, last_message_ts, unread_count, archived, muted_until)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name            = excluded.name,
                group_id        = excluded.group_id,
                last_message_ts = excluded.last_message_ts,
                unread_count    = excluded.unread_count,
                archived        = excluded.archived,
                muted_until     = excluded.muted_until
            "#,
        )
        .bind(&c.id)
        .bind(&c.name)
        .bind(c.group_id.as_deref())
        .bind(c.last_message_ts)
        .bind(c.unread_count)
        .bind(c.archived)
        .bind(c.muted_until)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn list_conversations(&self) -> Result<Vec<Conversation>> {
        // NULLS LAST: convs without messages sink below active threads.
        let rows = sqlx::query(
            r#"
            SELECT id, name, group_id, last_message_ts, unread_count, archived, muted_until
            FROM conversations
            ORDER BY last_message_ts IS NULL, last_message_ts DESC
            "#,
        )
        .fetch_all(self.pool())
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(Conversation {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                group_id: row.try_get::<Option<Vec<u8>>, _>("group_id")?,
                last_message_ts: row.try_get("last_message_ts")?,
                unread_count: row.try_get("unread_count")?,
                archived: row.try_get("archived")?,
                muted_until: row.try_get("muted_until")?,
            });
        }
        Ok(out)
    }

    /// Inserts a message and bumps the conversation's `last_message_ts` if
    /// this message is newer than what's stored.
    pub async fn insert_message(&self, m: &Message) -> Result<i64> {
        let mut tx = self.pool().begin().await?;

        let row = sqlx::query(
            r#"
            INSERT INTO messages
                (conversation_id, ts, sender, body, quote_ts, quote_sender, edited_ts, deleted)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            RETURNING id
            "#,
        )
        .bind(&m.conversation_id)
        .bind(m.ts)
        .bind(&m.sender)
        .bind(&m.body)
        .bind(m.quote_ts)
        .bind(&m.quote_sender)
        .bind(m.edited_ts)
        .bind(m.deleted)
        .fetch_one(&mut *tx)
        .await?;
        let id: i64 = row.try_get("id")?;

        sqlx::query(
            r#"
            UPDATE conversations
            SET last_message_ts = ?
            WHERE id = ?
              AND (last_message_ts IS NULL OR last_message_ts < ?)
            "#,
        )
        .bind(m.ts)
        .bind(&m.conversation_id)
        .bind(m.ts)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(id)
    }

    /// `before_ts`-keyset pagination: caller passes the ts of the oldest row
    /// from the previous page to fetch the next-older `limit` rows.
    pub async fn list_messages(
        &self,
        conversation_id: &str,
        limit: i64,
        before_ts: Option<i64>,
    ) -> Result<Vec<Message>> {
        let rows = match before_ts {
            Some(ts) => {
                sqlx::query(
                    r#"
                    SELECT id, conversation_id, ts, sender, body,
                           quote_ts, quote_sender, edited_ts, deleted
                    FROM messages
                    WHERE conversation_id = ? AND ts < ?
                    ORDER BY ts DESC
                    LIMIT ?
                    "#,
                )
                .bind(conversation_id)
                .bind(ts)
                .bind(limit)
                .fetch_all(self.pool())
                .await?
            }
            None => {
                sqlx::query(
                    r#"
                    SELECT id, conversation_id, ts, sender, body,
                           quote_ts, quote_sender, edited_ts, deleted
                    FROM messages
                    WHERE conversation_id = ?
                    ORDER BY ts DESC
                    LIMIT ?
                    "#,
                )
                .bind(conversation_id)
                .bind(limit)
                .fetch_all(self.pool())
                .await?
            }
        };

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(Message {
                id: row.try_get("id")?,
                conversation_id: row.try_get("conversation_id")?,
                ts: row.try_get("ts")?,
                sender: row.try_get("sender")?,
                body: row.try_get("body")?,
                quote_ts: row.try_get("quote_ts")?,
                quote_sender: row.try_get("quote_sender")?,
                edited_ts: row.try_get("edited_ts")?,
                deleted: row.try_get("deleted")?,
            });
        }
        Ok(out)
    }

    pub async fn upsert_contact(&self, c: &Contact) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO contacts (number, name, profile_name, blocked)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(number) DO UPDATE SET
                name         = excluded.name,
                profile_name = excluded.profile_name,
                blocked      = excluded.blocked
            "#,
        )
        .bind(&c.number)
        .bind(&c.name)
        .bind(&c.profile_name)
        .bind(c.blocked)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn get_contact(&self, number: &str) -> Result<Option<Contact>> {
        let row = sqlx::query(
            r#"
            SELECT number, name, profile_name, blocked
            FROM contacts
            WHERE number = ?
            "#,
        )
        .bind(number)
        .fetch_optional(self.pool())
        .await?;

        Ok(match row {
            Some(row) => Some(Contact {
                number: row.try_get("number")?,
                name: row.try_get("name")?,
                profile_name: row.try_get("profile_name")?,
                blocked: row.try_get("blocked")?,
            }),
            None => None,
        })
    }

    pub async fn add_attachment(&self, a: &Attachment) -> Result<i64> {
        let row = sqlx::query(
            r#"
            INSERT INTO attachments (message_id, mime_type, file_name, path, size)
            VALUES (?, ?, ?, ?, ?)
            RETURNING id
            "#,
        )
        .bind(a.message_id)
        .bind(&a.mime_type)
        .bind(&a.file_name)
        .bind(&a.path)
        .bind(a.size)
        .fetch_one(self.pool())
        .await?;
        Ok(row.try_get("id")?)
    }

    pub async fn list_attachments(&self, message_id: i64) -> Result<Vec<Attachment>> {
        let rows = sqlx::query(
            r#"
            SELECT id, message_id, mime_type, file_name, path, size
            FROM attachments
            WHERE message_id = ?
            ORDER BY id ASC
            "#,
        )
        .bind(message_id)
        .fetch_all(self.pool())
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            out.push(Attachment {
                id: row.try_get("id")?,
                message_id: row.try_get("message_id")?,
                mime_type: row.try_get("mime_type")?,
                file_name: row.try_get("file_name")?,
                path: row.try_get("path")?,
                size: row.try_get("size")?,
            });
        }
        Ok(out)
    }

    /// Idempotent on (message_id, sender): a sender's latest reaction wins.
    pub async fn add_reaction(&self, r: &Reaction) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO reactions (message_id, sender, emoji, ts)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(message_id, sender) DO UPDATE SET
                emoji = excluded.emoji,
                ts    = excluded.ts
            "#,
        )
        .bind(r.message_id)
        .bind(&r.sender)
        .bind(&r.emoji)
        .bind(r.ts)
        .execute(self.pool())
        .await?;
        Ok(())
    }

    pub async fn mark_read(&self, conversation_id: &str) -> Result<()> {
        sqlx::query(
            r#"
            UPDATE conversations
            SET unread_count = 0
            WHERE id = ?
            "#,
        )
        .bind(conversation_id)
        .execute(self.pool())
        .await?;
        Ok(())
    }
}

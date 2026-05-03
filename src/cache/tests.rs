use super::models::{Attachment, Contact, Conversation, Message, MessengerContact, Reaction};
use super::Cache;

fn convo(id: &str, last_ts: Option<i64>) -> Conversation {
    Conversation {
        id: id.to_string(),
        name: Some(format!("conv-{id}")),
        group_id: None,
        last_message_ts: last_ts,
        unread_count: 0,
        archived: false,
        muted_until: None,
    }
}

fn msg(conv: &str, ts: i64, sender: &str, body: &str) -> Message {
    Message {
        id: 0,
        conversation_id: conv.to_string(),
        ts,
        sender: sender.to_string(),
        body: Some(body.to_string()),
        quote_ts: None,
        quote_sender: None,
        edited_ts: None,
        deleted: false,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn list_conversations_orders_by_last_message_ts_desc_nulls_last() {
    let cache = Cache::open_in_memory().await.unwrap();

    cache
        .upsert_conversation(&convo("a", Some(100)))
        .await
        .unwrap();
    cache
        .upsert_conversation(&convo("b", Some(300)))
        .await
        .unwrap();
    cache.upsert_conversation(&convo("c", None)).await.unwrap();
    cache
        .upsert_conversation(&convo("d", Some(200)))
        .await
        .unwrap();

    let got: Vec<_> = cache
        .list_conversations()
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(got, vec!["b", "d", "a", "c"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn insert_message_updates_conversation_last_message_ts() {
    let cache = Cache::open_in_memory().await.unwrap();
    cache.upsert_conversation(&convo("x", None)).await.unwrap();

    cache
        .insert_message(&msg("x", 1000, "+15551112222", "hi"))
        .await
        .unwrap();
    let listed = cache.list_conversations().await.unwrap();
    assert_eq!(listed[0].last_message_ts, Some(1000));

    // Newer message bumps it.
    cache
        .insert_message(&msg("x", 2000, "+15551112222", "hello"))
        .await
        .unwrap();
    let listed = cache.list_conversations().await.unwrap();
    assert_eq!(listed[0].last_message_ts, Some(2000));

    // Older message does NOT regress it.
    cache
        .insert_message(&msg("x", 500, "+15551112222", "old"))
        .await
        .unwrap();
    let listed = cache.list_conversations().await.unwrap();
    assert_eq!(listed[0].last_message_ts, Some(2000));
}

#[tokio::test(flavor = "multi_thread")]
async fn list_messages_paginates_with_before_ts() {
    let cache = Cache::open_in_memory().await.unwrap();
    cache.upsert_conversation(&convo("x", None)).await.unwrap();

    for ts in [100, 200, 300, 400, 500] {
        cache.insert_message(&msg("x", ts, "s", "b")).await.unwrap();
    }

    let page1 = cache.list_messages("x", 2, None).await.unwrap();
    let page1_ts: Vec<_> = page1.iter().map(|m| m.ts).collect();
    assert_eq!(page1_ts, vec![500, 400]);

    let page2 = cache.list_messages("x", 2, Some(400)).await.unwrap();
    let page2_ts: Vec<_> = page2.iter().map(|m| m.ts).collect();
    assert_eq!(page2_ts, vec![300, 200]);

    let page3 = cache.list_messages("x", 2, Some(200)).await.unwrap();
    let page3_ts: Vec<_> = page3.iter().map(|m| m.ts).collect();
    assert_eq!(page3_ts, vec![100]);
}

#[tokio::test(flavor = "multi_thread")]
async fn contact_roundtrip() {
    let cache = Cache::open_in_memory().await.unwrap();
    let c = Contact {
        number: "+15550000000".into(),
        name: Some("Alice".into()),
        profile_name: Some("a.l.i.c.e".into()),
        blocked: false,
    };
    cache.upsert_contact(&c).await.unwrap();
    assert_eq!(
        cache.get_contact("+15550000000").await.unwrap(),
        Some(c.clone())
    );

    let updated = Contact { blocked: true, ..c };
    cache.upsert_contact(&updated).await.unwrap();
    assert_eq!(
        cache.get_contact("+15550000000").await.unwrap(),
        Some(updated)
    );

    assert_eq!(cache.get_contact("+19999999999").await.unwrap(), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn attachments_roundtrip() {
    let cache = Cache::open_in_memory().await.unwrap();
    cache.upsert_conversation(&convo("x", None)).await.unwrap();
    let mid = cache.insert_message(&msg("x", 1, "s", "")).await.unwrap();

    let a1 = Attachment {
        id: 0,
        message_id: mid,
        mime_type: "image/png".into(),
        file_name: Some("cat.png".into()),
        path: Some("/tmp/cat.png".into()),
        size: Some(2048),
    };
    let id1 = cache.add_attachment(&a1).await.unwrap();
    let a2 = Attachment {
        id: 0,
        message_id: mid,
        mime_type: "audio/ogg".into(),
        file_name: None,
        path: None,
        size: None,
    };
    let id2 = cache.add_attachment(&a2).await.unwrap();
    assert!(id2 > id1);

    let listed = cache.list_attachments(mid).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].id, id1);
    assert_eq!(listed[0].mime_type, "image/png");
    assert_eq!(listed[1].id, id2);
    assert_eq!(listed[1].mime_type, "audio/ogg");
}

#[tokio::test(flavor = "multi_thread")]
async fn reactions_idempotent_per_sender() {
    let cache = Cache::open_in_memory().await.unwrap();
    cache.upsert_conversation(&convo("x", None)).await.unwrap();
    let mid = cache.insert_message(&msg("x", 1, "s", "")).await.unwrap();

    cache
        .add_reaction(&Reaction {
            message_id: mid,
            sender: "+1".into(),
            emoji: "heart".into(),
            ts: 1,
        })
        .await
        .unwrap();
    // Same sender updates rather than duplicates.
    cache
        .add_reaction(&Reaction {
            message_id: mid,
            sender: "+1".into(),
            emoji: "fire".into(),
            ts: 2,
        })
        .await
        .unwrap();
    cache
        .add_reaction(&Reaction {
            message_id: mid,
            sender: "+2".into(),
            emoji: "thumbs_up".into(),
            ts: 3,
        })
        .await
        .unwrap();

    let row = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM reactions WHERE message_id = ?")
        .bind(mid)
        .fetch_one(cache.pool())
        .await
        .unwrap();
    assert_eq!(row, 2);

    let emoji = sqlx::query_scalar::<_, String>(
        "SELECT emoji FROM reactions WHERE message_id = ? AND sender = ?",
    )
    .bind(mid)
    .bind("+1")
    .fetch_one(cache.pool())
    .await
    .unwrap();
    assert_eq!(emoji, "fire");
}

#[tokio::test(flavor = "multi_thread")]
async fn mark_read_zeroes_unread_count() {
    let cache = Cache::open_in_memory().await.unwrap();
    let mut c = convo("x", Some(1));
    c.unread_count = 7;
    cache.upsert_conversation(&c).await.unwrap();

    cache.mark_read("x").await.unwrap();
    let listed = cache.list_conversations().await.unwrap();
    assert_eq!(listed[0].unread_count, 0);
}

/// A syntactically broken raw query against an in-memory cache must
/// surface as [`crate::core::Error::Sqlx`] rather than panic. Locks in
/// the `From<sqlx::Error>` wiring on our `Error` enum.
#[tokio::test(flavor = "multi_thread")]
async fn bad_raw_query_returns_sqlx_error() {
    use crate::core::Error;
    let cache = Cache::open_in_memory().await.unwrap();
    let res: Result<(i64,), sqlx::Error> = sqlx::query_as("SELECT FROM not_a_table")
        .fetch_one(cache.pool())
        .await;
    let sqlx_err = res.expect_err("bad SQL must fail");
    let our_err: Error = sqlx_err.into();
    assert!(
        matches!(our_err, Error::Sqlx(_)),
        "want Error::Sqlx(_), got {our_err:?}"
    );
    // Round-trip the Display path too — many callers `format!("{e}")` it.
    assert!(format!("{our_err}").starts_with("sqlx:"));
}

/// `mark_read` interleaved with `insert_message` must be atomic at the
/// SQL level — the unread-count zero-out and the new-message bump
/// can't see partial state from each other. We don't strictly order
/// the two operations (the UI shouldn't either), but every `(insert,
/// mark_read)` pair must end with `unread_count = 0` and the inserted
/// message visible. We exercise this by interleaving N pairs against
/// a shared cache from multiple tasks.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mark_read_and_insert_message_interleave_safely() {
    let cache = std::sync::Arc::new(Cache::open_in_memory().await.unwrap());
    let mut c = convo("x", None);
    c.unread_count = 5;
    cache.upsert_conversation(&c).await.unwrap();

    let mut handles = Vec::new();
    for i in 0..10 {
        let cache_ins = cache.clone();
        handles.push(tokio::spawn(async move {
            cache_ins
                .insert_message(&msg("x", 1000 + i, "+15551112222", "hi"))
                .await
                .unwrap();
        }));
        let cache_mark = cache.clone();
        handles.push(tokio::spawn(async move {
            cache_mark.mark_read("x").await.unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }

    // All 10 messages are present.
    let listed = cache.list_messages("x", 100, None).await.unwrap();
    assert_eq!(listed.len(), 10, "lost messages under interleave");

    // After the dust settles a final mark_read must still zero unread.
    cache.mark_read("x").await.unwrap();
    let convs = cache.list_conversations().await.unwrap();
    assert_eq!(convs[0].unread_count, 0);

    // last_message_ts walked monotonically up; the highest insert ts
    // must be reflected even though mark_read writes ran concurrently
    // (mark_read does not touch last_message_ts).
    let max_ts = listed.iter().map(|m| m.ts).max().unwrap();
    assert_eq!(convs[0].last_message_ts, Some(max_ts));
}

/// Ties on `last_message_ts` are stable but unspecified at the SQL
/// level; the contract we lock down here is "ties don't crash and
/// don't drop rows." If the UI later wants a deterministic tie-break
/// (e.g. by id / name) we add an `ORDER BY last_message_ts DESC, id`
/// and tighten this test accordingly.
#[tokio::test(flavor = "multi_thread")]
async fn list_conversations_ties_keep_every_row() {
    let cache = Cache::open_in_memory().await.unwrap();
    cache
        .upsert_conversation(&convo("alpha", Some(500)))
        .await
        .unwrap();
    cache
        .upsert_conversation(&convo("bravo", Some(500)))
        .await
        .unwrap();
    cache
        .upsert_conversation(&convo("charlie", Some(500)))
        .await
        .unwrap();
    cache
        .upsert_conversation(&convo("delta", Some(400)))
        .await
        .unwrap();

    let listed = cache.list_conversations().await.unwrap();
    assert_eq!(listed.len(), 4);
    // First three (the 500-ts cluster) come before delta regardless of
    // intra-tie order.
    let ids: Vec<_> = listed.iter().map(|c| c.id.clone()).collect();
    let delta_pos = ids.iter().position(|i| i == "delta").unwrap();
    assert_eq!(delta_pos, 3, "delta with older ts must sort last");
    // Every input id is present.
    for want in ["alpha", "bravo", "charlie", "delta"] {
        assert!(ids.iter().any(|i| i == want), "missing {want}");
    }
}

/// `messenger_contacts` upsert + lookup round-trip. Confirms the
/// (backend, native_id) composite key, the `display_name` payload,
/// and that updates overwrite the previous name without spawning a
/// duplicate row.
#[tokio::test(flavor = "multi_thread")]
async fn messenger_contact_roundtrip() {
    let cache = Cache::open_in_memory().await.unwrap();

    cache
        .upsert_messenger_contact("signal", "+14155552671", "Alice")
        .await
        .unwrap();
    cache
        .upsert_messenger_contact("telegram", "12345", "Bob B")
        .await
        .unwrap();

    assert_eq!(
        cache
            .get_messenger_contact_name("signal", "+14155552671")
            .await
            .unwrap(),
        Some("Alice".into())
    );
    assert_eq!(
        cache
            .get_messenger_contact_name("telegram", "12345")
            .await
            .unwrap(),
        Some("Bob B".into())
    );

    // Upsert overwrites the display name in place rather than
    // inserting a second row.
    cache
        .upsert_messenger_contact("signal", "+14155552671", "Alice Smith")
        .await
        .unwrap();
    assert_eq!(
        cache
            .get_messenger_contact_name("signal", "+14155552671")
            .await
            .unwrap(),
        Some("Alice Smith".into())
    );

    // (backend, native_id) is composite — same native id under a
    // different backend is its own row.
    cache
        .upsert_messenger_contact("telegram", "+14155552671", "different person")
        .await
        .unwrap();
    assert_eq!(
        cache
            .get_messenger_contact_name("signal", "+14155552671")
            .await
            .unwrap(),
        Some("Alice Smith".into()),
        "telegram upsert must not stomp signal's row"
    );

    // Unknown peer returns None — caller falls back to native id.
    assert_eq!(
        cache
            .get_messenger_contact_name("signal", "+19999999999")
            .await
            .unwrap(),
        None
    );
}

/// `get_messenger_contact` (full row) returns a recent `updated_at`
/// (within a few seconds of "now"). We accept a wide window to dodge
/// CI clock skew, just enough to prove the upsert wrote a real
/// timestamp instead of leaving zero.
#[tokio::test(flavor = "multi_thread")]
async fn messenger_contact_records_updated_at() {
    let cache = Cache::open_in_memory().await.unwrap();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    cache
        .upsert_messenger_contact("signal", "+14155552671", "Alice")
        .await
        .unwrap();
    let row: MessengerContact = cache
        .get_messenger_contact("signal", "+14155552671")
        .await
        .unwrap()
        .expect("row must exist");
    assert_eq!(row.backend, "signal");
    assert_eq!(row.native_id, "+14155552671");
    assert_eq!(row.display_name, "Alice");
    // Allow a 60 second slack each side; we just want to lock down
    // that updated_at is "recent" (and definitely not zero).
    let after = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!(
        row.updated_at >= before - 60_000 && row.updated_at <= after + 60_000,
        "updated_at out of window: {} not in [{}, {}]",
        row.updated_at,
        before - 60_000,
        after + 60_000
    );
}

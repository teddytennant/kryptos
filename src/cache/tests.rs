use super::models::{Attachment, Contact, Conversation, Message, Reaction};
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

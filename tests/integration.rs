//! Black-box integration tests beyond `tests/smoke.rs`.
//!
//! These exercise the public surface of `kryptos` end-to-end:
//!
//! - `theme_round_trip_each_palette` — every advertised builtin
//!   palette parses and exposes the canonical token set.
//! - `cache_open_creates_dir_if_missing` — `Cache::open(path)` should
//!   create any missing parent directories.
//! - `messenger_hub_send_routes_correctly` — the hub forwards
//!   `send` calls to the backend whose tag matches the chat id.
//! - `cache_open_in_memory_then_bad_query_is_sqlx_error` — round-
//!   trips a sqlx error through `kryptos::core::Error`.

mod common;

use kryptos::cache::Cache;
use kryptos::core::Error;
use kryptos::messenger::{Backend, ChatId};
use kryptos::theme::builtin;

/// Every advertised palette must parse and expose the canonical
/// `kryptos_*` color tokens the GTK widget tree consumes. This is the
/// same contract the unit test in `src/theme/mod.rs` enforces, but
/// run here at the public-API level so a `cargo test --tests` smoke
/// catches a regression even if the unit tests are skipped.
#[test]
fn theme_round_trip_each_palette() {
    const REQUIRED_TOKENS: &[&str] = &[
        "bg", "mantle", "surface", "surface2", "overlay", "fg", "subtle", "accent", "blue",
        "green", "yellow", "red", "lavender",
    ];

    let names = builtin::known_names();
    assert!(
        names.len() >= 8,
        "expected ≥8 known theme names (system + builtins), got {}: {:?}",
        names.len(),
        names
    );
    for name in &names {
        if *name == "system" {
            // system is special — no embedded CSS, libadwaita drives it.
            assert!(builtin::lookup(name).is_none(), "system must not resolve");
            continue;
        }
        let resolved = builtin::lookup(name)
            .unwrap_or_else(|| panic!("advertised theme {name} did not resolve"));
        assert_eq!(resolved.canonical_name, *name);
        assert!(
            !resolved.css.trim().is_empty(),
            "{name} has empty embedded CSS"
        );
        for token in REQUIRED_TOKENS {
            let needle = format!("@define-color kryptos_{token}");
            assert!(
                resolved.css.contains(&needle),
                "{name} is missing token kryptos_{token}"
            );
        }
        // Every palette must style the modeline so an .normal mode
        // doesn't fall through to GTK defaults.
        assert!(
            resolved.css.contains(".modeline.normal"),
            "{name} missing .modeline.normal"
        );
    }
}

/// `Cache::open(path)` is documented as creating the parent directory
/// chain if it does not exist. Verify that on a fresh tempdir with a
/// nested target the cache file is created and reachable for queries.
#[tokio::test(flavor = "multi_thread")]
async fn cache_open_creates_dir_if_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // Two missing layers under the tempdir root.
    let nested = tmp.path().join("a").join("b").join("cache.db");
    assert!(!nested.parent().unwrap().exists(), "guard: parent missing");

    let cache = Cache::open(&nested)
        .await
        .expect("open must create parent dirs");

    assert!(nested.exists(), "cache file should now exist on disk");
    assert!(
        nested.parent().expect("has parent").is_dir(),
        "parent dir should now exist"
    );

    // And the migrated schema is reachable.
    let convos = cache.list_conversations().await.expect("query works");
    assert!(convos.is_empty());
}

/// The hub must forward `send` calls to the backend whose tag matches
/// the chat id. Uses the shared mock from `common.rs` to keep the
/// surface to the public API only.
#[tokio::test(flavor = "multi_thread")]
async fn messenger_hub_send_routes_correctly() {
    let (hub, s, t) = common::hub_with_signal_and_telegram();

    hub.send(&ChatId::new(Backend::Signal, "+15550001111"), "hi-s", &[])
        .await
        .expect("signal send");
    hub.send(&ChatId::new(Backend::Telegram, "9001"), "hi-t", &[])
        .await
        .expect("telegram send");

    let s_sent = s.sent();
    let t_sent = t.sent();
    assert_eq!(s_sent.len(), 1, "signal got exactly one message");
    assert_eq!(s_sent[0].0.backend, Backend::Signal);
    assert_eq!(s_sent[0].1, "hi-s");
    assert_eq!(t_sent.len(), 1, "telegram got exactly one message");
    assert_eq!(t_sent[0].0.backend, Backend::Telegram);
    assert_eq!(t_sent[0].1, "hi-t");
}

/// Sending to a backend tag the hub doesn't know about is an
/// `Error::Config`, not a panic, even on the public surface.
#[tokio::test(flavor = "multi_thread")]
async fn messenger_hub_send_to_unregistered_is_config_error() {
    let hub = kryptos::messenger::hub::MessengerHub::new();
    let err = hub
        .send(&ChatId::new(Backend::Signal, "+1"), "hi", &[])
        .await
        .expect_err("must fail without backends");
    assert!(
        matches!(err, Error::Config(_)),
        "want Error::Config(_), got {err:?}"
    );
    assert!(format!("{err}").contains("no backend"));
}

/// A bogus telegram session path must surface as `Error::Telegram(_)`
/// from the public API. We pass the tempdir itself as the session
/// "file" — `Session::load_file_or_create` sees an existing path,
/// tries to read it, and `read_to_end` returns `EISDIR`. The Telegram
/// backend wraps that into `Error::Telegram` *before* any network
/// call, so the test stays hermetic.
#[tokio::test(flavor = "multi_thread")]
async fn telegram_open_bogus_session_path_is_telegram_error() {
    use kryptos::messenger::telegram::TelegramBackend;
    let tmp = tempfile::tempdir().expect("tempdir");
    // TelegramBackend is not Debug, so we can't use expect_err here.
    match TelegramBackend::open(1, "deadbeef", tmp.path()).await {
        Ok(_) => panic!("opening with a dir session path must fail"),
        Err(e) => assert!(
            matches!(e, Error::Telegram(_)),
            "want Error::Telegram(_), got {e:?}"
        ),
    }
}

/// `Cache::open_in_memory` returning the same handle to multiple
/// queries — proves the in-memory pool's single-connection setup
/// actually shares state across awaits.
#[tokio::test(flavor = "multi_thread")]
async fn cache_open_in_memory_shares_state_across_queries() {
    use kryptos::cache::models::Conversation;
    let cache = Cache::open_in_memory().await.expect("open in-memory");
    cache
        .upsert_conversation(&Conversation {
            id: "x".into(),
            name: Some("X".into()),
            group_id: None,
            last_message_ts: Some(1),
            unread_count: 0,
            archived: false,
            muted_until: None,
        })
        .await
        .expect("upsert");

    let listed = cache.list_conversations().await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "x");
}

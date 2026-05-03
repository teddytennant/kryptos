//! Signal backend — wraps [`crate::dbus::SignalClient`] behind the
//! [`MessengerBackend`] trait.
//!
//! A single `SignalBackend` is bound to one signal-cli account (the
//! E.164 number passed at construction). Multi-account use means
//! constructing one `SignalBackend` per account and adding each to the
//! [`MessengerHub`](crate::messenger::MessengerHub).

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::{debug, warn};

use crate::cache::Cache;
use crate::core::{Error, Result};
use crate::dbus::stream::{self, Event as SignalEvent};
use crate::dbus::SignalClient;
use crate::messenger::{
    Backend, BackendExtras, ChatId, ConversationSummary, Event, MessengerBackend,
    NormalizedAttachment, NormalizedMessage,
};

/// Channel capacity for the internal broadcast bus. Picked to be large
/// enough that a momentarily slow subscriber doesn't drop messages
/// during a backfill burst, but small enough not to balloon memory.
const BROADCAST_CAPACITY: usize = 256;

pub struct SignalBackend {
    client: Arc<SignalClient>,
    account: String,
    /// Lazily-started forwarder from the per-account D-Bus signal stream
    /// into our internal broadcast channel.
    forwarder: Mutex<Option<broadcast::Sender<Event>>>,
    /// Optional shared cache. When set, the backend writes resolved
    /// contact names into `messenger_contacts` and reads them back as
    /// the cheap fallback when signal-cli's `getContactName` doesn't
    /// know the peer (or D-Bus is unreachable on a refresh).
    cache: Option<Arc<Cache>>,
}

impl SignalBackend {
    pub fn new(client: Arc<SignalClient>, account: impl Into<String>) -> Self {
        Self {
            client,
            account: account.into(),
            forwarder: Mutex::new(None),
            cache: None,
        }
    }

    /// Builder-style: attach a shared [`Cache`] so contact-name
    /// resolution writes through to `messenger_contacts` (and reads
    /// from it when signal-cli is offline / unaware of the peer).
    pub fn with_cache(mut self, cache: Arc<Cache>) -> Self {
        self.cache = Some(cache);
        self
    }

    /// Resolve the account from `(configured, available)`:
    ///
    /// - non-empty `configured` → use it verbatim.
    /// - empty + `available` has exactly one entry → use that.
    /// - empty + `available` has many → log + pick the first; the user
    ///   is expected to set `[backends.signal] account = "+…"` to
    ///   disambiguate.
    /// - empty + `available` is empty → return `None` (no signed-in
    ///   account; backend can't be built).
    ///
    /// Pure function so the resolver is unit-testable without a live
    /// signal-cli connection.
    pub fn resolve_account(configured: &str, available: &[String]) -> Option<String> {
        if !configured.is_empty() {
            return Some(configured.to_string());
        }
        match available.len() {
            0 => None,
            1 => Some(available[0].clone()),
            _ => {
                warn!(
                    accounts = ?available,
                    "multiple signal-cli accounts present and `[backends.signal] account` is empty; picking first"
                );
                Some(available[0].clone())
            }
        }
    }

    pub fn account(&self) -> &str {
        &self.account
    }

    /// Boot (or reuse) the broadcast forwarder. The first call subscribes
    /// to signal-cli's per-account stream and spawns a task that
    /// rebroadcasts normalized [`Event`]s; later calls return a fresh
    /// receiver against the same channel.
    async fn ensure_forwarder(&self) -> Result<broadcast::Receiver<Event>> {
        let mut guard = self.forwarder.lock().await;
        if let Some(tx) = guard.as_ref() {
            return Ok(tx.subscribe());
        }

        let mut signal_rx = stream::subscribe(&self.client, &self.account).await?;
        let (tx, rx) = broadcast::channel(BROADCAST_CAPACITY);
        let tx_task = tx.clone();
        let cache = self.cache.clone();

        tokio::spawn(async move {
            while let Some(ev) = signal_rx.recv().await {
                let mut normalized = match ev {
                    SignalEvent::MessageReceived {
                        ts,
                        sender,
                        group_id,
                        body,
                        attachments,
                    } => normalize_message_received(ts, sender, group_id, body, attachments),
                };
                // Lazy display-name population. signal-cli's
                // MessageReceived signal does NOT carry sourceName, so
                // we lean on the cache here: a previous
                // list_conversations / getContactName already may have
                // recorded a name for this peer. Whatever's stored
                // wins; otherwise the UI falls back to the raw E.164
                // sender. We deliberately don't issue a fresh
                // getContactName D-Bus call per message — that would
                // serialise behind the bus on every burst.
                if let Some(cache) = cache.as_ref() {
                    if let Ok(Some(name)) = cache
                        .get_messenger_contact_name(Backend::Signal.as_tag(), &normalized.sender)
                        .await
                    {
                        normalized.sender_display = Some(name);
                    }
                }
                if tx_task.send(Event::MessageReceived(normalized)).is_err() {
                    debug!("signal forwarder: no live subscribers, sleeping");
                    // broadcast::send returns Err only when *currently* nobody
                    // is subscribed; that's fine, we keep pumping in case a
                    // new subscriber arrives. Don't break.
                }
            }
            debug!("signal forwarder: upstream closed");
        });

        *guard = Some(tx);
        Ok(rx)
    }
}

/// Resolve a Signal peer's display name. Order of preference:
///
/// 1. signal-cli's `getContactName` over D-Bus (authoritative; the
///    name the user has saved locally).
/// 2. Whatever's in the cache from a previous resolve (covers the
///    "bus flaked on this refresh" case so we don't blank out names
///    we already knew).
/// 3. `None` — caller falls back to the raw E.164 / UUID.
///
/// Successful resolutions are written through to the cache so the
/// next launch / event has them ready without another bus round
/// trip.
pub(crate) async fn resolve_signal_display_name(
    client: &SignalClient,
    cache: Option<&Cache>,
    account: &str,
    recipient: &str,
) -> Option<String> {
    if let Ok(Some(name)) = client.get_contact_name(account, recipient).await {
        if let Some(cache) = cache {
            if let Err(e) = cache
                .upsert_messenger_contact(Backend::Signal.as_tag(), recipient, &name)
                .await
            {
                debug!(error = %e, "messenger_contacts upsert failed");
            }
        }
        return Some(name);
    }
    if let Some(cache) = cache {
        if let Ok(Some(name)) = cache
            .get_messenger_contact_name(Backend::Signal.as_tag(), recipient)
            .await
        {
            return Some(name);
        }
    }
    None
}

#[async_trait]
impl MessengerBackend for SignalBackend {
    fn backend(&self) -> Backend {
        Backend::Signal
    }

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        // Real conversation list: signal-cli's per-account `listNumbers`
        // (1:1 contacts) + `listGroups` (group threads). Self number is
        // filtered out — chatting with yourself is the dedicated
        // "Note to Self" feature, not a regular row.
        let numbers = self.client.list_numbers(&self.account).await.unwrap_or_else(|e| {
            warn!(error = %e, account = %self.account, "listNumbers failed; sidebar will skip 1:1 contacts");
            Vec::new()
        });
        let groups = self.client.list_groups(&self.account).await.unwrap_or_else(|e| {
            warn!(error = %e, account = %self.account, "listGroups failed; sidebar will skip groups");
            Vec::new()
        });

        let mut out = Vec::with_capacity(numbers.len() + groups.len());

        for n in numbers.into_iter().filter(|n| n != &self.account) {
            let display_name = resolve_signal_display_name(
                &self.client,
                self.cache.as_deref(),
                &self.account,
                &n,
            )
            .await;
            out.push(ConversationSummary {
                id: ChatId::new(Backend::Signal, n.clone()),
                title: n,
                display_name,
                last_message_ts: None,
                unread: 0,
            });
        }

        for (gid, name) in groups {
            let native = hex_encode(&gid);
            let display_name = if name.is_empty() { None } else { Some(name) };
            out.push(ConversationSummary {
                id: ChatId::new(Backend::Signal, native.clone()),
                title: native,
                display_name,
                last_message_ts: None,
                unread: 0,
            });
        }

        Ok(out)
    }

    async fn fetch_history(
        &self,
        _id: &ChatId,
        _limit: u32,
        _before_ts: Option<i64>,
    ) -> Result<Vec<NormalizedMessage>> {
        // signal-cli doesn't expose history over D-Bus; our own SQLite
        // cache will own this once the UI starts hitting it. For now an
        // empty backfill keeps the trait honest without lying about data.
        Ok(Vec::new())
    }

    async fn send_message(&self, id: &ChatId, body: &str, attachments: &[PathBuf]) -> Result<i64> {
        if id.backend != Backend::Signal {
            return Err(Error::Config(format!(
                "SignalBackend cannot send to {} chat",
                id.backend
            )));
        }
        if attachments.is_empty() {
            self.client.send_text(&self.account, &id.native, body).await
        } else {
            self.client
                .send_with_attachments(&self.account, &id.native, body, attachments)
                .await
        }
    }

    async fn mark_read(&self, _id: &ChatId) -> Result<()> {
        // signal-cli has sendReadReceipt but it requires the original
        // message ids; the cache layer will plumb that through later.
        Ok(())
    }

    async fn typing(&self, _id: &ChatId, _on: bool) -> Result<()> {
        // sendTyping isn't on the existing proxy yet. Stub for now —
        // returning Ok keeps the UI free to call this without surfacing
        // an error toast for an unimplemented courtesy feature.
        Ok(())
    }

    async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<Event>> {
        let mut bcast_rx = self.ensure_forwarder().await?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            loop {
                match bcast_rx.recv().await {
                    Ok(ev) => {
                        if tx.send(ev).is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "signal subscriber lagged, dropping events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(rx)
    }

    fn self_account(&self) -> Option<&str> {
        Some(&self.account)
    }
}

/// Pure conversion from a signal-cli `MessageReceived` payload into the
/// protocol-neutral [`NormalizedMessage`]. Pulled out so unit tests can
/// drive the mapping without spinning up a D-Bus connection.
pub(crate) fn normalize_message_received(
    ts: i64,
    sender: String,
    group_id: Option<Vec<u8>>,
    body: String,
    attachments: Vec<String>,
) -> NormalizedMessage {
    let conversation_native = match &group_id {
        Some(gid) => hex_encode(gid),
        None => sender.clone(),
    };
    let attachments = attachments
        .into_iter()
        .map(|path| NormalizedAttachment {
            mime_type: None,
            file_name: PathBuf::from(&path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned()),
            local_path: Some(PathBuf::from(path)),
            size: None,
        })
        .collect();
    NormalizedMessage {
        id: ChatId::new(Backend::Signal, conversation_native),
        ts_ms: ts,
        sender,
        // signal-cli's MessageReceived signal doesn't carry a
        // sourceName today; the broadcast forwarder enriches this
        // from the cache before relaying to subscribers (see
        // `ensure_forwarder`). Until that runs we leave it None.
        sender_display: None,
        body: if body.is_empty() { None } else { Some(body) },
        attachments,
        backend_extras: BackendExtras::Signal { group_id },
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(&mut s, "{:02x}", b);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn one_to_one_message_routes_by_sender() {
        let msg = normalize_message_received(
            1_700_000_000_000,
            "+14155552671".into(),
            None,
            "hi".into(),
            Vec::new(),
        );
        assert_eq!(msg.id.backend, Backend::Signal);
        assert_eq!(msg.id.native, "+14155552671");
        assert_eq!(msg.sender, "+14155552671");
        assert_eq!(msg.body.as_deref(), Some("hi"));
        assert!(msg.attachments.is_empty());
        assert!(matches!(
            msg.backend_extras,
            BackendExtras::Signal { group_id: None }
        ));
        // signal-cli envelopes don't carry sourceName today, so the
        // bare normalize path leaves sender_display unset; the
        // broadcast forwarder enriches it from the cache when one
        // exists. Lock the default in so tests catch a regression.
        assert_eq!(msg.sender_display, None);
    }

    #[test]
    fn group_message_routes_by_hex_group_id() {
        let gid = vec![0xde, 0xad, 0xbe, 0xef];
        let msg = normalize_message_received(
            1_700_000_001_000,
            "+14155552672".into(),
            Some(gid.clone()),
            "yo".into(),
            Vec::new(),
        );
        assert_eq!(msg.id.native, "deadbeef");
        assert_eq!(msg.sender, "+14155552672");
        match msg.backend_extras {
            BackendExtras::Signal { group_id } => assert_eq!(group_id, Some(gid)),
            other => panic!("expected Signal extras, got {other:?}"),
        }
    }

    #[test]
    fn empty_body_normalizes_to_none() {
        let msg = normalize_message_received(
            42,
            "+14155552671".into(),
            None,
            String::new(),
            vec!["/tmp/img.jpg".into()],
        );
        assert!(msg.body.is_none());
        assert_eq!(msg.attachments.len(), 1);
        assert_eq!(msg.attachments[0].file_name.as_deref(), Some("img.jpg"));
        assert_eq!(
            msg.attachments[0].local_path,
            Some(PathBuf::from("/tmp/img.jpg"))
        );
    }

    #[test]
    fn very_long_body_passes_through_intact() {
        // No length cap on bodies — Signal accepts ~64KiB messages and the
        // cache layer enforces its own limits. Make sure normalization
        // doesn't truncate or panic on a multi-kilobyte payload.
        let body: String = "ok ".repeat(10_000);
        let msg =
            normalize_message_received(1, "+14155552671".into(), None, body.clone(), Vec::new());
        assert_eq!(msg.body.as_deref(), Some(body.as_str()));
    }

    #[test]
    fn unicode_body_is_preserved_codepoint_perfect() {
        // RTL, emoji, ZWJ family — anything Signal sends, we pass through.
        let body = "Привет 👨‍👩‍👧‍👦 العربية 𝕜𝕣𝕪𝕡𝕥𝕠𝕤".to_string();
        let msg =
            normalize_message_received(1, "+14155552671".into(), None, body.clone(), Vec::new());
        assert_eq!(msg.body.as_deref(), Some(body.as_str()));
    }

    #[test]
    fn duplicate_attachment_paths_are_kept_distinct_entries() {
        // Signal is allowed to attach the same file twice; we don't
        // dedupe — each is a distinct attachment row in the cache.
        let msg = normalize_message_received(
            1,
            "+14155552671".into(),
            None,
            "look".into(),
            vec!["/tmp/dup.jpg".into(), "/tmp/dup.jpg".into()],
        );
        assert_eq!(msg.attachments.len(), 2);
        assert_eq!(msg.attachments[0].local_path, msg.attachments[1].local_path);
        assert_eq!(msg.attachments[0].file_name, msg.attachments[1].file_name);
    }

    #[test]
    fn self_account_resolves_from_config_or_first_listed() {
        // 1. explicit account in config → returned verbatim.
        let resolved = SignalBackend::resolve_account("+14155552671", &[]);
        assert_eq!(resolved.as_deref(), Some("+14155552671"));

        // 2. empty config + exactly one listed → that one.
        let resolved = SignalBackend::resolve_account("", &["+14155552672".into()]);
        assert_eq!(resolved.as_deref(), Some("+14155552672"));

        // 3. empty config + zero listed → None (no account at all).
        let resolved = SignalBackend::resolve_account("", &[]);
        assert_eq!(resolved, None);

        // 4. empty config + multiple listed → first entry (with a warn).
        let resolved =
            SignalBackend::resolve_account("", &["+14155552673".into(), "+14155552674".into()]);
        assert_eq!(resolved.as_deref(), Some("+14155552673"));

        // 5. explicit beats listed even when listed has a different value.
        let resolved =
            SignalBackend::resolve_account("+14155552675", &["+14155552676".into()]);
        assert_eq!(resolved.as_deref(), Some("+14155552675"));
    }

    #[test]
    fn attachment_without_filename_segment_yields_none_filename() {
        // A pathological "/" path has no file_name component; the
        // helper should leave file_name=None rather than panic.
        let msg = normalize_message_received(
            1,
            "+14155552671".into(),
            None,
            String::new(),
            vec!["/".into()],
        );
        assert_eq!(msg.attachments.len(), 1);
        assert!(msg.attachments[0].file_name.is_none());
    }
}

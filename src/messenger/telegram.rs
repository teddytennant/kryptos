//! Telegram backend, backed by the `grammers-client` crate.
//!
//! A single [`TelegramBackend`] owns one connected `grammers` `Client`
//! (and therefore one persisted session at `session_path`). Multiple
//! Telegram accounts means constructing one `TelegramBackend` per
//! account and adding each to the [`MessengerHub`].
//!
//! Login is a multi-step flow because Telegram speaks SMS / 2FA, not
//! just a username/password handshake:
//!
//! 1. [`TelegramBackend::open`] connects (creating or loading the
//!    session). At this point the client may already be authorised
//!    from a previous run — check with [`TelegramBackend::is_authorized`].
//! 2. If not authorised, [`TelegramBackend::request_login`] sends a login code
//!    to the user's phone. The grammers `LoginToken` is stashed
//!    internally so callers don't have to thread it back.
//! 3. [`TelegramBackend::submit_code`] hands the SMS code to grammers; the
//!    returned [`NeedsPassword`] tells the UI whether to additionally
//!    prompt for the user's 2FA password.
//! 4. [`TelegramBackend::submit_password`] (only if needed) finishes the
//!    handshake.
//! 5. [`TelegramBackend::save_session`] persists the session blob to disk.
//!
//! [`MessengerHub`]: crate::messenger::MessengerHub

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use grammers_client::session::{PackedChat, Session};
use grammers_client::{
    types::{InputMessage, LoginToken, Update},
    Client, Config as GClientConfig, InitParams, InvocationError, SignInError,
};
use grammers_tl_types as tl;
use tokio::sync::{broadcast, mpsc, Mutex};
use tokio::time::{sleep, Duration};
use tracing::{debug, warn};

use crate::core::{Error, Result};
use crate::messenger::{
    Backend, BackendExtras, ChatId, ConversationSummary, Event, MessengerBackend,
    NormalizedAttachment, NormalizedMessage,
};

const BROADCAST_CAPACITY: usize = 256;

/// Pure config for [`TelegramBackend::open`]. Mirrors
/// [`crate::config::schema::TelegramBackendConfig`] but with the
/// session path resolved into a real [`PathBuf`].
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub api_id: i32,
    pub api_hash: String,
    pub session_path: PathBuf,
}

/// Stage of the login handshake. We hold the grammers tokens here so
/// callers can drive the flow with three string-only methods
/// (`request_login` / `submit_code` / `submit_password`) instead of
/// having to round-trip opaque grammers types through the UI layer.
#[derive(Default)]
struct LoginState {
    /// Set after a successful [`Client::request_login_code`] call.
    /// Cleared on `submit_code` success.
    token: Option<LoginToken>,
    /// Set when `submit_code` returns [`SignInError::PasswordRequired`].
    /// Cleared on `submit_password` success.
    password_token: Option<grammers_client::types::PasswordToken>,
}

/// Whether [`TelegramBackend::submit_code`] still needs a password to
/// finish authenticating. Returned to the UI so it knows whether to
/// hide or surface a 2FA prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NeedsPassword(pub bool);

pub struct TelegramBackend {
    client: Client,
    session_path: PathBuf,
    /// Internal event bus. Gets wired up by [`Self::subscribe`] on the
    /// first call (lazy so we don't spawn an update task before the
    /// caller actually wants events).
    bus: broadcast::Sender<Event>,
    forwarder_started: Mutex<bool>,
    login: Arc<Mutex<LoginState>>,
    /// Cached own user_id (decimal string) populated after a successful
    /// authorisation check. `OnceLock` so `self_account()` stays a
    /// non-async `&str`-returning method on the trait.
    self_id: OnceLock<String>,
}

impl TelegramBackend {
    /// Connect to Telegram and load (or create) the session at
    /// `session_path`. No login is performed; callers should check
    /// [`Self::is_authorized`] and run the login flow if needed.
    ///
    /// `api_id` / `api_hash` are the developer credentials registered
    /// at <https://my.telegram.org/auth>.
    pub async fn open(api_id: i32, api_hash: &str, session_path: &Path) -> Result<Self> {
        let session = Session::load_file_or_create(session_path)
            .map_err(|e| Error::Telegram(format!("session load: {e}")))?;
        let client = Client::connect(GClientConfig {
            session,
            api_id,
            api_hash: api_hash.to_string(),
            params: InitParams::default(),
        })
        .await
        .map_err(|e| Error::Telegram(format!("connect: {e}")))?;

        let (bus, _) = broadcast::channel(BROADCAST_CAPACITY);
        Ok(Self {
            client,
            session_path: session_path.to_path_buf(),
            bus,
            forwarder_started: Mutex::new(false),
            login: Arc::new(Mutex::new(LoginState::default())),
            self_id: OnceLock::new(),
        })
    }

    /// `true` if the persisted session is already signed in. Hits the
    /// network the first time we observe an authorised session to cache
    /// the local user_id so [`Self::self_account`] can return it
    /// synchronously later.
    pub async fn is_authorized(&self) -> bool {
        let ok = self.client.is_authorized().await.unwrap_or(false);
        if ok && self.self_id.get().is_none() {
            match self.client.get_me().await {
                Ok(user) => {
                    let _ = self.self_id.set(user.id().to_string());
                }
                Err(e) => debug!(error = %e, "telegram get_me after authorized check failed"),
            }
        }
        ok
    }

    /// Step 1 of login. Asks Telegram to deliver a code to `phone`
    /// (E.164 with `+`). Stash the returned token internally so
    /// [`Self::submit_code`] can finish the handshake.
    pub async fn request_login(&self, phone: &str) -> Result<()> {
        let token = self
            .client
            .request_login_code(phone)
            .await
            .map_err(|e| Error::Telegram(format!("request_login_code: {e}")))?;
        let mut st = self.login.lock().await;
        st.token = Some(token);
        Ok(())
    }

    /// Step 2 of login. If `Ok(NeedsPassword(true))` the account has
    /// 2FA and the caller should follow up with
    /// [`Self::submit_password`]; if `Ok(NeedsPassword(false))` the
    /// session is now authorised.
    pub async fn submit_code(&self, code: &str) -> Result<NeedsPassword> {
        // grammers' `LoginToken` is `!Clone` and consumed by reference,
        // so hold the lock across the network call and decide what to
        // put back based on the outcome. The lock is per-backend so
        // serialising login attempts is acceptable.
        let mut st = self.login.lock().await;
        let token = st
            .token
            .as_ref()
            .ok_or_else(|| Error::Telegram("submit_code called before request_login".into()))?;
        match self.client.sign_in(token, code).await {
            Ok(user) => {
                st.token = None;
                st.password_token = None;
                let _ = self.self_id.set(user.id().to_string());
                Ok(NeedsPassword(false))
            }
            Err(SignInError::PasswordRequired(pwd_token)) => {
                st.token = None;
                st.password_token = Some(pwd_token);
                Ok(NeedsPassword(true))
            }
            Err(SignInError::InvalidCode) => Err(Error::Telegram("invalid code".into())),
            Err(SignInError::SignUpRequired { .. }) => Err(Error::Telegram(
                "sign-up required (use official client)".into(),
            )),
            Err(SignInError::InvalidPassword) => Err(Error::Telegram("invalid password".into())),
            Err(SignInError::Other(e)) => Err(Error::Telegram(format!("sign_in: {e}"))),
        }
    }

    /// Step 3 of login (only when [`Self::submit_code`] returned
    /// `NeedsPassword(true)`).
    pub async fn submit_password(&self, password: &str) -> Result<()> {
        let pwd_token = {
            let st = self.login.lock().await;
            st.password_token.clone().ok_or_else(|| {
                Error::Telegram("submit_password called before submit_code".into())
            })?
        };
        match self.client.check_password(pwd_token, password).await {
            Ok(user) => {
                let mut st = self.login.lock().await;
                st.password_token = None;
                let _ = self.self_id.set(user.id().to_string());
                Ok(())
            }
            Err(SignInError::InvalidPassword) => Err(Error::Telegram("invalid password".into())),
            Err(other) => Err(Error::Telegram(format!("check_password: {other}"))),
        }
    }

    /// Persist the live session to disk. Should be called after a
    /// successful login and periodically afterwards (grammers updates
    /// the in-memory session as messages flow).
    pub async fn save_session(&self) -> Result<()> {
        self.client
            .session()
            .save_to_file(&self.session_path)
            .map_err(|e| Error::Telegram(format!("session save: {e}")))?;
        Ok(())
    }
}

/// Cap on `list_conversations`. Telegram users routinely have
/// hundreds of dialogs; pulling 200 is enough to fill any sidebar
/// without paying for a full traversal on every refresh.
const DIALOG_LIMIT: usize = 200;

#[async_trait]
impl MessengerBackend for TelegramBackend {
    fn backend(&self) -> Backend {
        Backend::Telegram
    }

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        let mut iter = self.client.iter_dialogs().limit(DIALOG_LIMIT);
        let mut out = Vec::new();
        while let Some(dialog) = iter
            .next()
            .await
            .map_err(|e| Error::Telegram(format!("iter_dialogs: {e}")))?
        {
            let chat = dialog.chat();
            let last_message_ts = dialog
                .last_message
                .as_ref()
                .map(|m| conv::datetime_to_ms(m.date()));
            let unread = match &dialog.raw {
                tl::enums::Dialog::Dialog(d) => d.unread_count.max(0) as u32,
                tl::enums::Dialog::Folder(_) => 0,
            };
            out.push(ConversationSummary {
                id: ChatId::new(Backend::Telegram, conv::packed_to_native(chat.pack())),
                title: chat.name().to_string(),
                last_message_ts,
                unread,
            });
        }
        Ok(out)
    }

    async fn fetch_history(
        &self,
        id: &ChatId,
        limit: u32,
        before_ts: Option<i64>,
    ) -> Result<Vec<NormalizedMessage>> {
        if id.backend != Backend::Telegram {
            return Err(Error::Telegram(format!(
                "TelegramBackend cannot fetch from {} chat",
                id.backend
            )));
        }
        let packed = conv::native_to_packed(&id.native)?;
        let mut iter = self.client.iter_messages(packed).limit(limit as usize);
        let mut out = Vec::new();
        while let Some(msg) = iter
            .next()
            .await
            .map_err(|e| Error::Telegram(format!("iter_messages: {e}")))?
        {
            let ts_ms = conv::datetime_to_ms(msg.date());
            // `before_ts` walks backwards in time: skip anything newer
            // than the cursor. grammers' iterator already returns
            // newest-first, so we just filter as we go.
            if let Some(cutoff) = before_ts {
                if ts_ms >= cutoff {
                    continue;
                }
            }
            out.push(message_to_normalized(&msg, id));
        }
        Ok(out)
    }

    async fn send_message(&self, id: &ChatId, body: &str, attachments: &[PathBuf]) -> Result<i64> {
        if id.backend != Backend::Telegram {
            return Err(Error::Telegram(format!(
                "TelegramBackend cannot send to {} chat",
                id.backend
            )));
        }
        let packed = conv::native_to_packed(&id.native)?;

        // Build the InputMessage. v1: when there are attachments we
        // upload the first one and ship it with `body` as the
        // caption. Multi-attachment albums need `send_album` and a
        // separate code path that's deferred until the UI grows
        // album-style composing.
        let input = if let Some(path) = attachments.first() {
            let uploaded = self
                .client
                .upload_file(path)
                .await
                .map_err(|e| Error::Telegram(format!("upload_file: {e}")))?;
            InputMessage::text(body).file(uploaded)
        } else {
            InputMessage::text(body)
        };

        let sent = self
            .client
            .send_message(packed, input)
            .await
            .map_err(|e| Error::Telegram(format!("send_message: {e}")))?;
        Ok(conv::datetime_to_ms(sent.date()))
    }

    async fn mark_read(&self, id: &ChatId) -> Result<()> {
        if id.backend != Backend::Telegram {
            return Err(Error::Telegram(format!(
                "TelegramBackend cannot mark_read on {} chat",
                id.backend
            )));
        }
        let packed = conv::native_to_packed(&id.native)?;
        self.client
            .mark_as_read(packed)
            .await
            .map_err(|e| Error::Telegram(format!("mark_as_read: {e}")))?;
        Ok(())
    }

    async fn typing(&self, id: &ChatId, on: bool) -> Result<()> {
        if id.backend != Backend::Telegram {
            return Err(Error::Telegram(format!(
                "TelegramBackend cannot typing on {} chat",
                id.backend
            )));
        }
        let packed = conv::native_to_packed(&id.native)?;
        let action = self.client.action(packed);
        let result = if on {
            // One-shot typing notification. Telegram auto-expires it
            // after ~6s server-side, so for sustained typing the UI
            // would need to call us again — which is fine, vim users
            // are bursty.
            action
                .oneshot(tl::enums::SendMessageAction::SendMessageTypingAction)
                .await
        } else {
            action.cancel().await
        };
        result.map_err(|e| Error::Telegram(format!("set typing: {e}")))?;
        Ok(())
    }

    async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<Event>> {
        self.ensure_forwarder().await;
        let mut bcast_rx = self.bus.subscribe();
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
                        warn!(skipped = n, "telegram subscriber lagged, dropping events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(rx)
    }

    fn self_account(&self) -> Option<&str> {
        self.self_id.get().map(String::as_str)
    }
}

impl TelegramBackend {
    /// Lazily start the single update-pump task. The task loops on
    /// [`Client::next_update`] and rebroadcasts each
    /// [`Update`]-derived [`Event`] over the internal bus. Disconnects
    /// are retried with exponential backoff capped at 60s so a flaky
    /// network doesn't permanently silence the backend.
    async fn ensure_forwarder(&self) {
        let mut started = self.forwarder_started.lock().await;
        if *started {
            return;
        }
        let client = self.client.clone();
        let bus = self.bus.clone();
        tokio::spawn(async move {
            // Backoff timer used only for non-poison errors. A
            // disconnect bubbles up as `InvocationError::Read` (or
            // similar transport variant); we don't try to discriminate
            // — any error pauses, sleeps, then retries the next call.
            let mut backoff = Duration::from_secs(1);
            const MAX_BACKOFF: Duration = Duration::from_secs(60);

            loop {
                match client.next_update().await {
                    Ok(update) => {
                        backoff = Duration::from_secs(1);
                        if let Some(ev) = update_to_event(update) {
                            if bus.send(ev).is_err() {
                                debug!("telegram forwarder: no live subscribers");
                                // Don't break — keep pumping so a
                                // future subscriber catches up.
                            }
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "telegram next_update failed; backing off");
                        sleep(backoff).await;
                        backoff = (backoff * 2).min(MAX_BACKOFF);
                        // If the error was a hard close (e.g.
                        // `InvocationError::Dropped`), `next_update`
                        // will keep yielding the same — the backoff
                        // ceiling stops us from spinning hot.
                        if matches!(e, InvocationError::Dropped) {
                            debug!("telegram forwarder: client dropped, exiting");
                            break;
                        }
                    }
                }
            }
        });
        *started = true;
    }
}

/// Convert a grammers [`Message`](grammers_client::types::Message)
/// into the protocol-neutral [`NormalizedMessage`]. The `id` is the
/// `ChatId` of the conversation we fetched from; we copy it instead
/// of recomputing the chat hex per message because `iter_messages`
/// already addresses one chat at a time.
fn message_to_normalized(msg: &grammers_client::types::Message, id: &ChatId) -> NormalizedMessage {
    let sender = msg
        .sender()
        .map(|c| c.name().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    let reply_to_msg_id = match msg.reply_header() {
        Some(tl::enums::MessageReplyHeader::Header(h)) => h.reply_to_msg_id.map(i64::from),
        _ => None,
    };

    NormalizedMessage {
        id: id.clone(),
        ts_ms: conv::datetime_to_ms(msg.date()),
        sender,
        body: conv::text_to_body(msg.text()),
        // Attachments aren't decoded into local files for history
        // backfill — the message's media reference would still need
        // a download pass. Keep the slot honest by leaving it empty.
        attachments: Vec::<NormalizedAttachment>::new(),
        backend_extras: BackendExtras::Telegram { reply_to_msg_id },
    }
}

/// Translate a grammers [`Update`] into our protocol-neutral
/// [`Event`]. Returns `None` for variants we don't care about
/// (callback queries, inline events, raw passthroughs) so the
/// caller can simply `if let Some(ev) = update_to_event(u)` without
/// allocating a noisy enum branch for every TL update kind.
fn update_to_event(update: Update) -> Option<Event> {
    match update {
        Update::NewMessage(msg) => {
            let chat = msg.chat();
            let id = ChatId::new(Backend::Telegram, conv::packed_to_native(chat.pack()));
            Some(Event::MessageReceived(message_to_normalized(&msg, &id)))
        }
        Update::MessageEdited(msg) => {
            let chat = msg.chat();
            let id = ChatId::new(Backend::Telegram, conv::packed_to_native(chat.pack()));
            Some(Event::Edited {
                id,
                ts: conv::datetime_to_ms(msg.date()),
                new_body: msg.text().to_string(),
            })
        }
        Update::MessageDeleted(deletion) => {
            // grammers' MessageDeletion only carries the optional
            // channel_id and the message ids — it does not say which
            // peer the message belonged to for non-channel chats.
            // For channels we can synthesise a Channel ChatId; for
            // 1:1 / small-group deletes we have no peer to attach,
            // so skip rather than fabricate one.
            let channel_id = deletion.channel_id()?;
            let packed = PackedChat {
                ty: grammers_client::session::PackedType::Megagroup,
                id: channel_id,
                access_hash: None,
            };
            Some(Event::Deleted {
                id: ChatId::new(Backend::Telegram, conv::packed_to_native(packed)),
                ts: 0,
            })
        }
        // Bot / inline events aren't on the user-client critical
        // path; surface them as `None` and let the bus drop them.
        // grammers marks `Update` as `#[non_exhaustive]`, so the
        // wildcard arm catches future additions too.
        Update::CallbackQuery(_)
        | Update::InlineQuery(_)
        | Update::InlineSend(_)
        | Update::Raw(_) => None,
        _ => None,
    }
}

/// Resolve the configured [`TelegramConfig::session_path`] from the
/// raw schema string. Empty string means "use
/// `$XDG_DATA_HOME/kryptos/telegram.session`". The `directories` crate
/// is only consulted for the empty case; a non-empty config value is
/// trusted verbatim so users can pin a custom location.
pub fn resolve_session_path(raw: &str) -> PathBuf {
    if !raw.is_empty() {
        return PathBuf::from(raw);
    }
    if let Some(dirs) = directories::ProjectDirs::from("", "", "kryptos") {
        return dirs.data_dir().join("telegram.session");
    }
    // Last resort: cwd-relative. ProjectDirs only fails on truly
    // exotic environments (no $HOME on Linux), so this branch is
    // mostly defensive.
    PathBuf::from("telegram.session")
}

/// Pure helpers used by the trait impl + unit tests. Pulled out so
/// the conversion logic is reachable without spinning up grammers.
pub(crate) mod conv {
    use super::*;
    use chrono::{DateTime, Utc};

    /// `chrono::DateTime<Utc>` -> unix milliseconds. grammers stores
    /// message dates with second precision, but the rest of the app
    /// works in `i64` ms so we widen here for free.
    pub fn datetime_to_ms(dt: DateTime<Utc>) -> i64 {
        dt.timestamp() * 1000 + i64::from(dt.timestamp_subsec_millis())
    }

    /// Turn the body text out of a grammers message into the
    /// `Option<String>` shape used by [`NormalizedMessage`]: empty
    /// strings collapse to `None` (Telegram returns "" for service
    /// messages and pure-media posts).
    pub fn text_to_body(text: &str) -> Option<String> {
        if text.is_empty() {
            None
        } else {
            Some(text.to_string())
        }
    }

    /// Encode a `PackedChat` as a [`ChatId::native`] payload. We use
    /// grammers' own hex serialisation so it round-trips through any
    /// cache + restores the access_hash needed for outgoing requests.
    pub fn packed_to_native(packed: PackedChat) -> String {
        packed.to_hex()
    }

    /// Inverse of [`packed_to_native`]. Returns `Error::Telegram` on
    /// malformed hex.
    pub fn native_to_packed(native: &str) -> Result<PackedChat> {
        PackedChat::from_hex(native)
            .map_err(|_| Error::Telegram(format!("invalid telegram chat id: {native}")))
    }
}

#[cfg(test)]
pub(crate) mod test_helpers {
    use super::*;
    use grammers_client::session::PackedType;

    /// Build a deterministic `PackedChat` for unit tests. We pick
    /// `PackedType::User` and supply an `access_hash` so the round
    /// trip through `to_hex` / `from_hex` exercises the full payload.
    pub fn fake_user_chat(id: i64, access_hash: i64) -> PackedChat {
        PackedChat {
            ty: PackedType::User,
            id,
            access_hash: Some(access_hash),
        }
    }

    /// Build a no-op `Update::Raw` for unit tests of [`update_to_event`]'s
    /// fall-through arm. `UpdateAttachMenuBots` is a fieldless TL
    /// constructor so it costs nothing to materialise.
    pub fn fake_raw_update() -> Update {
        Update::Raw(tl::enums::Update::AttachMenuBots)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn datetime_to_ms_at_epoch_is_zero() {
        let dt = chrono::Utc.timestamp_opt(0, 0).single().unwrap();
        assert_eq!(conv::datetime_to_ms(dt), 0);
    }

    #[test]
    fn datetime_to_ms_round_seconds() {
        let dt = chrono::Utc
            .timestamp_opt(1_700_000_000, 0)
            .single()
            .unwrap();
        assert_eq!(conv::datetime_to_ms(dt), 1_700_000_000_000);
    }

    #[test]
    fn datetime_to_ms_includes_subsec_millis() {
        // 123 ms after the second mark.
        let dt = chrono::Utc
            .timestamp_opt(1_700_000_000, 123_000_000)
            .single()
            .unwrap();
        assert_eq!(conv::datetime_to_ms(dt), 1_700_000_000_123);
    }

    #[test]
    fn text_to_body_empty_collapses_to_none() {
        assert_eq!(conv::text_to_body(""), None);
        assert_eq!(conv::text_to_body("hi"), Some("hi".into()));
    }

    #[test]
    fn packed_chat_round_trips_through_native_string() {
        let original = test_helpers::fake_user_chat(42, 0xdead_beef);
        let native = conv::packed_to_native(original);
        let parsed = conv::native_to_packed(&native).expect("round trip");
        assert_eq!(parsed.id, 42);
        assert_eq!(parsed.access_hash, Some(0xdead_beef));
    }

    #[test]
    fn native_to_packed_rejects_garbage() {
        let err = conv::native_to_packed("not-hex").unwrap_err();
        assert!(format!("{err}").contains("invalid telegram chat id"));
    }

    #[test]
    fn resolve_session_path_uses_explicit_string() {
        let p = resolve_session_path("/explicit/path.session");
        assert_eq!(p, PathBuf::from("/explicit/path.session"));
    }

    #[test]
    fn update_to_event_drops_raw_passthroughs() {
        let raw = test_helpers::fake_raw_update();
        assert!(update_to_event(raw).is_none());
    }

    #[test]
    fn resolve_session_path_default_lands_under_kryptos() {
        let p = resolve_session_path("");
        // We can't pin the exact dir (varies by host), but it should
        // always end with the canonical filename and live under a
        // path that mentions our app name.
        assert_eq!(p.file_name().unwrap(), "telegram.session");
        assert!(
            p.to_string_lossy().contains("kryptos"),
            "expected kryptos in path, got {}",
            p.display()
        );
    }

    #[test]
    fn text_to_body_passes_unicode_flags_through_unchanged() {
        // Multi-codepoint flag emoji (regional indicator pairs). We
        // never decompose / re-encode the body so it must round-trip
        // byte-for-byte.
        let flag = "🇺🇸🇯🇵";
        assert_eq!(conv::text_to_body(flag), Some(flag.to_string()));
    }

    #[test]
    fn text_to_body_passes_combining_chars_through_unchanged() {
        // Composed vs decomposed (NFC vs NFD) matters for cache keys;
        // we deliberately don't normalize here so the backend's bytes
        // survive the round trip.
        let composed = "café"; // single codepoint é
        let decomposed = "cafe\u{0301}"; // e + combining acute
        assert_eq!(conv::text_to_body(composed), Some(composed.to_string()));
        assert_eq!(conv::text_to_body(decomposed), Some(decomposed.to_string()));
        // And confirm we treat them as distinct strings (no implicit
        // normalization slipped in).
        assert_ne!(composed, decomposed);
    }

    #[test]
    fn datetime_to_ms_handles_pre_epoch_dates() {
        // grammers stores timestamps as i64 seconds; nothing prevents
        // a server-side message from being before the epoch (a clock
        // skew or doctored date). We must not panic.
        let dt = chrono::Utc.timestamp_opt(-1, 0).single().unwrap();
        assert_eq!(conv::datetime_to_ms(dt), -1_000);
    }

    #[test]
    fn datetime_to_ms_high_subsec_precision_preserved() {
        // 999ms — boundary case where subsec_millis rolls into the
        // next second. Make sure we don't double-count.
        let dt = chrono::Utc
            .timestamp_opt(1_700_000_000, 999_000_000)
            .single()
            .unwrap();
        assert_eq!(conv::datetime_to_ms(dt), 1_700_000_000_999);
    }

    #[test]
    fn packed_chat_native_string_is_url_safe() {
        // The hex form ends up as the native part of a `signal:...` /
        // `telegram:...` wire string in caches and logs; assert it
        // contains no whitespace / control chars / backslashes that
        // would need escaping.
        let chat = test_helpers::fake_user_chat(123, 456);
        let native = conv::packed_to_native(chat);
        assert!(!native.is_empty());
        assert!(native.chars().all(|c| !c.is_whitespace()));
        assert!(!native.contains('\\'));
        assert!(!native.contains(':'), "colons would clash with ChatId wire");
    }

    #[test]
    fn native_to_packed_rejects_empty_string() {
        assert!(conv::native_to_packed("").is_err());
    }

    #[test]
    fn update_to_event_drops_callback_inline_passthroughs() {
        // We can't easily construct CallbackQuery / InlineQuery /
        // InlineSend without a live grammers session because their
        // constructors are private. The fall-through arm is what
        // protects us from `non_exhaustive` additions, so prove the
        // `Update::Raw` path returns None as a stand-in.
        let raw = test_helpers::fake_raw_update();
        assert!(update_to_event(raw).is_none());
    }

    /// `TelegramBackend::open` should bail with `Error::Telegram(_)` —
    /// not panic, not bubble up an opaque io error — when the session
    /// path is unusable. We exercise the path by handing it an
    /// existing *directory*: `Session::load_file_or_create` sees
    /// `path.exists() == true`, calls `load_file`, and `read_to_end`
    /// fails with `EISDIR`. The `?` chain converts that into our
    /// flattened `Error::Telegram` variant before any network I/O is
    /// attempted, so the test stays hermetic.
    #[tokio::test]
    async fn open_with_directory_session_path_returns_telegram_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Pass the directory itself as the "session file".
        // `TelegramBackend` is not `Debug` so we can't use `expect_err`
        // — match on Result directly.
        match TelegramBackend::open(1, "deadbeef", tmp.path()).await {
            Ok(_) => panic!("opening with a dir session path must fail"),
            Err(e) => assert!(
                matches!(e, Error::Telegram(_)),
                "want Error::Telegram(_), got {e:?}"
            ),
        }
    }
}

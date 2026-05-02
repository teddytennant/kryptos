//! Telegram backend, backed by [`grammers-client`].
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
//!    from a previous run — check with [`Self::is_authorized`].
//! 2. If not authorised, [`Self::request_login`] sends a login code
//!    to the user's phone. The grammers `LoginToken` is stashed
//!    internally so callers don't have to thread it back.
//! 3. [`Self::submit_code`] hands the SMS code to grammers; the
//!    returned [`NeedsPassword`] tells the UI whether to additionally
//!    prompt for the user's 2FA password.
//! 4. [`Self::submit_password`] (only if needed) finishes the
//!    handshake.
//! 5. [`Self::save_session`] persists the session blob to disk.
//!
//! [`MessengerHub`]: crate::messenger::MessengerHub

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use grammers_client::session::{PackedChat, Session};
use grammers_client::{
    types::LoginToken, Client, Config as GClientConfig, InitParams, SignInError,
};
use tokio::sync::{broadcast, mpsc, Mutex};

use crate::core::{Error, Result};
use crate::messenger::{
    Backend, ChatId, ConversationSummary, Event, MessengerBackend, NormalizedMessage,
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
        })
    }

    /// `true` if the persisted session is already signed in.
    pub async fn is_authorized(&self) -> bool {
        self.client.is_authorized().await.unwrap_or(false)
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
            Ok(_user) => {
                st.token = None;
                st.password_token = None;
                Ok(NeedsPassword(false))
            }
            Err(SignInError::PasswordRequired(pwd_token)) => {
                st.token = None;
                st.password_token = Some(pwd_token);
                Ok(NeedsPassword(true))
            }
            Err(SignInError::InvalidCode) => Err(Error::Telegram("invalid code".into())),
            Err(SignInError::SignUpRequired { .. }) => {
                Err(Error::Telegram("sign-up required (use official client)".into()))
            }
            Err(SignInError::InvalidPassword) => Err(Error::Telegram("invalid password".into())),
            Err(SignInError::Other(e)) => Err(Error::Telegram(format!("sign_in: {e}"))),
        }
    }

    /// Step 3 of login (only when [`Self::submit_code`] returned
    /// [`NeedsPassword(true)`]).
    pub async fn submit_password(&self, password: &str) -> Result<()> {
        let pwd_token = {
            let st = self.login.lock().await;
            st.password_token.clone().ok_or_else(|| {
                Error::Telegram("submit_password called before submit_code".into())
            })?
        };
        match self.client.check_password(pwd_token, password).await {
            Ok(_user) => {
                let mut st = self.login.lock().await;
                st.password_token = None;
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

#[async_trait]
impl MessengerBackend for TelegramBackend {
    fn backend(&self) -> Backend {
        Backend::Telegram
    }

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        Err(Error::Telegram("list_conversations: not yet implemented".into()))
    }

    async fn fetch_history(
        &self,
        _id: &ChatId,
        _limit: u32,
        _before_ts: Option<i64>,
    ) -> Result<Vec<NormalizedMessage>> {
        Err(Error::Telegram("fetch_history: not yet implemented".into()))
    }

    async fn send_message(
        &self,
        _id: &ChatId,
        _body: &str,
        _attachments: &[PathBuf],
    ) -> Result<i64> {
        Err(Error::Telegram("send_message: not yet implemented".into()))
    }

    async fn mark_read(&self, _id: &ChatId) -> Result<()> {
        Err(Error::Telegram("mark_read: not yet implemented".into()))
    }

    async fn typing(&self, _id: &ChatId, _on: bool) -> Result<()> {
        Err(Error::Telegram("typing: not yet implemented".into()))
    }

    async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<Event>> {
        // Forwarder isn't wired to grammers yet; hand back a live
        // (but empty) receiver so callers don't special-case telegram.
        let _ = self.forwarder_started.lock().await;
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
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Ok(rx)
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
        let dt = chrono::Utc.timestamp_opt(1_700_000_000, 0).single().unwrap();
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
}

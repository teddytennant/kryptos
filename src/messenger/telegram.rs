//! Telegram backend skeleton.
//!
//! The plan is `grammers-client` (pure-Rust MTProto). Pulling the
//! crate in means a non-trivial first compile and a swathe of new
//! transitive deps; we land the surface area first so the rest of the
//! app can program against it, then activate the real implementation
//! in a follow-up commit by replacing the bodies of these methods with
//! grammers calls.
//!
//! Activation checklist for the follow-up:
//!
//! 1. Add `grammers-client` and `grammers-session` to `Cargo.toml`.
//! 2. Replace [`TelegramBackend`]'s placeholder fields with the real
//!    `grammers_client::Client` plus a join handle for the update loop.
//! 3. Fill in [`TelegramBackend::login`] using
//!    `grammers_session::Session::load_file_or_create`.
//! 4. Wire [`TelegramBackend::request_code`] / [`submit_code`] into
//!    `client.bot_sign_in` / `client.sign_in` (with optional 2FA).
//! 5. Implement `list_conversations` via `client.iter_dialogs()`,
//!    `fetch_history` via `client.iter_messages(chat).limit(limit)`,
//!    `subscribe` by polling `client.next_update()` in a tokio task and
//!    forwarding into the broadcast channel below.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, Mutex};
use tracing::warn;

use crate::core::{Error, Result};
use crate::messenger::{
    Backend, ChatId, ConversationSummary, Event, MessengerBackend, NormalizedMessage,
};

const BROADCAST_CAPACITY: usize = 256;

#[derive(Debug, Clone)]
pub struct TelegramConfig {
    pub api_id: i32,
    pub api_hash: String,
    pub session_path: PathBuf,
}

/// In-progress sign-in handle. `grammers` returns a token from
/// `request_login_code` that you hand back to `sign_in`; we mirror that
/// by storing the request inside the backend so the UI can drive the
/// flow without holding library types directly.
#[derive(Debug, Default)]
struct LoginState {
    phone: Option<String>,
    code_requested: bool,
}

pub struct TelegramBackend {
    #[allow(dead_code)]
    config: TelegramConfig,
    /// Internal event bus. Empty until the real client is wired in;
    /// keeping it here means [`subscribe`] returns a live receiver
    /// today rather than erroring, which matches the trait contract.
    bus: broadcast::Sender<Event>,
    login: Arc<Mutex<LoginState>>,
}

impl TelegramBackend {
    /// Construct a backend without performing any network I/O. Use
    /// [`login`] when you want to authenticate against Telegram for the
    /// first time; otherwise this is enough for the hub to track the
    /// account and surface an "unconfigured" placeholder to the UI.
    pub fn new(config: TelegramConfig) -> Self {
        let (bus, _) = broadcast::channel(BROADCAST_CAPACITY);
        Self {
            config,
            bus,
            login: Arc::new(Mutex::new(LoginState::default())),
        }
    }

    /// Authenticate against Telegram and persist a session at
    /// `session_path`. Stub for now — returns [`Error::Config`] until
    /// `grammers-client` is wired in.
    pub async fn login(api_id: i32, api_hash: &str, session_path: &Path) -> Result<Self> {
        warn!(
            ?session_path,
            api_id,
            "TelegramBackend::login is not yet implemented (grammers-client pending)"
        );
        let _ = api_hash;
        Err(Error::Config(
            "telegram backend not yet implemented — pending grammers-client integration".into(),
        ))
    }

    /// Step 1 of the login flow: ask Telegram to SMS / push a code to
    /// the given phone number. UIs show a code-entry screen after this
    /// call returns.
    pub async fn request_code(&self, phone: &str) -> Result<()> {
        let mut st = self.login.lock().await;
        st.phone = Some(phone.to_string());
        st.code_requested = true;
        Err(Error::Config(
            "telegram backend not yet implemented — pending grammers-client integration".into(),
        ))
    }

    /// Step 2: hand the SMS code (and an optional 2FA password) back so
    /// `grammers` can finish the sign-in handshake.
    pub async fn submit_code(&self, code: &str, password: Option<&str>) -> Result<()> {
        let _ = (code, password);
        let st = self.login.lock().await;
        if !st.code_requested {
            return Err(Error::Config(
                "telegram: submit_code called before request_code".into(),
            ));
        }
        Err(Error::Config(
            "telegram backend not yet implemented — pending grammers-client integration".into(),
        ))
    }
}

#[async_trait]
impl MessengerBackend for TelegramBackend {
    fn backend(&self) -> Backend {
        Backend::Telegram
    }

    async fn list_conversations(&self) -> Result<Vec<ConversationSummary>> {
        Err(Error::Config(
            "telegram backend not yet implemented".into(),
        ))
    }

    async fn fetch_history(
        &self,
        _id: &ChatId,
        _limit: u32,
        _before_ts: Option<i64>,
    ) -> Result<Vec<NormalizedMessage>> {
        Err(Error::Config(
            "telegram backend not yet implemented".into(),
        ))
    }

    async fn send_message(
        &self,
        _id: &ChatId,
        _body: &str,
        _attachments: &[PathBuf],
    ) -> Result<i64> {
        Err(Error::Config(
            "telegram backend not yet implemented".into(),
        ))
    }

    async fn mark_read(&self, _id: &ChatId) -> Result<()> {
        Err(Error::Config(
            "telegram backend not yet implemented".into(),
        ))
    }

    async fn typing(&self, _id: &ChatId, _on: bool) -> Result<()> {
        Err(Error::Config(
            "telegram backend not yet implemented".into(),
        ))
    }

    async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<Event>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> TelegramConfig {
        TelegramConfig {
            api_id: 0,
            api_hash: String::new(),
            session_path: PathBuf::from("/tmp/kryptos-test.session"),
        }
    }

    #[tokio::test]
    async fn stub_returns_telegram_backend_kind() {
        let b = TelegramBackend::new(cfg());
        assert_eq!(b.backend(), Backend::Telegram);
    }

    #[tokio::test]
    async fn stub_send_errors_until_implemented() {
        let b = TelegramBackend::new(cfg());
        let err = b
            .send_message(
                &ChatId::new(Backend::Telegram, "1"),
                "hi",
                &[],
            )
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("not yet implemented"));
    }

    #[tokio::test]
    async fn submit_code_requires_request_code_first() {
        let b = TelegramBackend::new(cfg());
        let err = b.submit_code("12345", None).await.unwrap_err();
        // Either "before request_code" or "not yet implemented" — both
        // are acceptable today; we just need a hard error.
        let msg = format!("{err}");
        assert!(msg.contains("telegram"), "got: {msg}");
    }

    #[tokio::test]
    async fn subscribe_yields_an_open_receiver() {
        let b = TelegramBackend::new(cfg());
        let mut rx = b.subscribe().await.unwrap();
        // No events in the bus yet — try_recv should return Empty,
        // proving the channel is open and not closed.
        assert!(matches!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ));
    }
}

//! GTK4 + libadwaita view layer for Kryptos.
//!
//! Module layout:
//!
//! - `window`      — widget tree construction.
//! - `statusline`  — mode line + command/search bar widgets.
//! - `input`       — gdk → [`crate::vim::Key`] translation.
//! - `dispatcher`  — apply [`crate::vim::Action`]s to the widget tree.
//! - [`settings`]    — `adw::PreferencesWindow` over `~/.config/kryptos/config.toml`.
//! - [`onboarding`]  — first-run device-link flow (QR + signal-cli polling).

mod commands;
mod composer;
mod dispatcher;
mod input;
pub mod onboarding;
pub mod settings;
mod statusline;
mod window;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tokio::sync::mpsc as tmpsc;
use tracing::{debug, error, info, warn};

use crate::cache::models::{Conversation, Message};
use crate::cache::Cache;
use crate::config::{loader, Config, ConfigWatcher};
use crate::core::Result;
use crate::dbus::SignalClient;
use crate::messenger::{
    signal::SignalBackend, telegram::TelegramBackend, Backend, ChatId, ConversationSummary,
    Event as MEvent, MessengerHub, NormalizedMessage,
};
use std::collections::HashMap;
use crate::theme::ThemeManager;
use crate::vim::{Engine, KeySym, KeymapSet, Mode, Outcome};

use dispatcher::Dispatcher;
use window::WindowParts;

const APP_ID: &str = "dev.kryptos.Kryptos";

/// Run the libadwaita application loop. Returns the glib exit code.
pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();

    app.connect_activate(activate);
    app.run()
}

/// Async-side state shared between the worker thread and the GTK
/// thread. The hub + cache are `Arc`-wrapped so the worker can clone
/// handles freely; the runtime stays parked behind `Arc<Runtime>` so
/// we can `runtime.spawn(...)` from anywhere.
struct AsyncCtx {
    runtime: Arc<tokio::runtime::Runtime>,
    hub: Arc<MessengerHub>,
    cache: Arc<Cache>,
}

impl AsyncCtx {
    /// Spin up the runtime, build the cache and hub, attach all
    /// configured backends. Errors degrade to "no backends, no cache" —
    /// the UI will simply show its empty state.
    fn try_build(cfg: &Config) -> Option<Arc<Self>> {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(rt) => Arc::new(rt),
            Err(e) => {
                error!(error = %e, "tokio runtime build failed; sync layer disabled");
                return None;
            }
        };

        let cache = match runtime.block_on(open_cache()) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                error!(error = %e, "cache open failed; sync layer disabled");
                return None;
            }
        };

        let mut hub = MessengerHub::new();
        if cfg.backends.signal.enabled {
            match runtime
                .block_on(build_signal_backend(&cfg.backends.signal.account, cache.clone()))
            {
                Ok(Some(backend)) => hub.add(backend),
                Ok(None) => debug!("no Signal account configured; backend not added"),
                Err(e) => warn!(error = %e, "signal backend init failed"),
            }
        }
        // Telegram: attach only when the user has both flipped
        // `[backends.telegram].enabled = true` and completed the
        // interactive login (session file exists + grammers reports
        // is_authorized()). Otherwise log a hint pointing them at
        // `:telegram-login` so they're not stuck guessing.
        if cfg.backends.telegram.enabled {
            match runtime.block_on(build_telegram_backend(&cfg.backends.telegram, cache.clone())) {
                Ok(Some(backend)) => hub.add(backend),
                Ok(None) => info!(
                    "telegram backend enabled but not authorized; run :telegram-login"
                ),
                Err(e) => warn!(error = %e, "telegram backend init failed"),
            }
        }

        Some(Arc::new(Self {
            runtime,
            hub: Arc::new(hub),
            cache,
        }))
    }
}

async fn open_cache() -> Result<Cache> {
    let path = Cache::default_path()?;
    Cache::open(&path).await
}

async fn build_telegram_backend(
    cfg: &crate::config::schema::TelegramBackendConfig,
    cache: Arc<Cache>,
) -> Result<Option<Arc<TelegramBackend>>> {
    if cfg.api_id == 0 || cfg.api_hash.is_empty() {
        return Ok(None);
    }
    let session_path = crate::messenger::telegram::resolve_session_path(&cfg.session_path);
    let backend = TelegramBackend::open(cfg.api_id, &cfg.api_hash, &session_path).await?;
    if !backend.is_authorized().await {
        debug!("telegram session present but not authorized");
        return Ok(None);
    }
    Ok(Some(Arc::new(backend.with_cache(cache))))
}

async fn build_signal_backend(
    configured_account: &str,
    cache: Arc<Cache>,
) -> Result<Option<Arc<SignalBackend>>> {
    let client = match SignalClient::connect().await {
        Ok(c) => c,
        Err(e) => {
            warn!(error = %e, "signal-cli D-Bus connect failed; signal disabled");
            return Ok(None);
        }
    };
    if let Err(e) = crate::dbus::ensure_running(client.connection()).await {
        warn!(error = %e, "signal-cli daemon unreachable; signal disabled");
        return Ok(None);
    }
    let available = client.list_accounts().await.unwrap_or_default();
    let account = match SignalBackend::resolve_account(configured_account, &available) {
        Some(a) => a,
        None => {
            debug!("signal-cli has no accounts yet; backend will spin up after link");
            return Ok(None);
        }
    };
    let client = Arc::new(client);
    Ok(Some(Arc::new(
        SignalBackend::new(client, account).with_cache(cache),
    )))
}

fn activate(app: &adw::Application) {
    info!("activating main window");

    let config_path = match loader::default_path() {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "could not resolve config path");
            return;
        }
    };
    let config_existed = config_path.exists();
    let cfg = match loader::load_or_default(&config_path) {
        Ok(c) => c,
        Err(e) => {
            error!(error = %e, "config load failed; falling back to defaults");
            Config::default()
        }
    };

    let parts = window::build(app, &cfg);

    // Install theme stack against the default display and apply the
    // configured theme. Failures to apply log + fall through with the
    // built-in provider unloaded (libadwaita defaults).
    let theme = match gtk::gdk::Display::default() {
        Some(display) => {
            let mut tm = ThemeManager::install_for_display(&display);
            if let Err(e) = tm.apply(&cfg.general.theme) {
                warn!(error = %e, "initial theme apply failed");
            }
            tm
        }
        None => {
            error!("no default display; theme manager disabled");
            return;
        }
    };
    let theme = Rc::new(RefCell::new(theme));

    let engine = match KeymapSet::from_config(&cfg) {
        Ok(set) => Engine::new(set),
        Err(e) => {
            error!(error = %e, "keymap build failed; using empty keymaps");
            Engine::new(KeymapSet::default())
        }
    };
    let engine = Rc::new(RefCell::new(engine));
    let dispatcher = Dispatcher::new(&parts, theme.clone(), config_path.clone());

    // Live-reload: watch `config.toml` for saves and reapply theme in
    // place when the user edits `[general] theme = "..."`. The watcher's
    // worker thread broadcasts new `Config` snapshots through a
    // `tokio::sync::watch` channel; we poll it on the GTK main loop so
    // mutations to `theme` (a non-`Send` `Rc<RefCell<_>>`) stay on the
    // GTK thread.
    spawn_config_watcher(
        config_path.clone(),
        theme.clone(),
        parts.messages_box.clone(),
        cfg.appearance.message_bubbles,
    );

    let async_ctx = AsyncCtx::try_build(&cfg);

    // Active chat id for the composer + per-row history fetch. Owned by
    // the GTK thread and cloned into worker callbacks as needed.
    let active_chat: Rc<RefCell<Option<ChatId>>> = Rc::new(RefCell::new(None));

    // Per-backend "this is me" identifier — populated on startup once
    // backends are attached so `is_mine` doesn't have to ask the hub on
    // every render. Telegram entries land here only after a successful
    // login (the cache is keyed off `is_authorized`).
    let self_accounts: Rc<RefCell<HashMap<Backend, String>>> =
        Rc::new(RefCell::new(HashMap::new()));

    if let Some(ctx) = async_ctx.as_ref() {
        for backend in [Backend::Signal, Backend::Telegram] {
            if let Some(id) = ctx.hub.self_account_for(backend) {
                self_accounts.borrow_mut().insert(backend, id);
            }
        }
        prime_sidebar_from_cache(ctx.clone(), &parts);
        spawn_remote_refresh(ctx.clone(), &parts);
        spawn_event_subscription(
            ctx.clone(),
            &parts,
            active_chat.clone(),
            self_accounts.clone(),
        );
        wire_sidebar_selection(
            ctx.clone(),
            &parts,
            active_chat.clone(),
            self_accounts.clone(),
        );
    }

    wire_composer_send(
        async_ctx.clone(),
        &parts,
        active_chat.clone(),
        engine.clone(),
        self_accounts.clone(),
    );

    wire_command_bar(&parts, &dispatcher, engine.clone());
    wire_keys(&parts, &dispatcher, engine.clone());

    parts.mode_line.set_mode(engine.borrow().mode());
    parts.window.present();

    // First-run welcome experience. We show it whenever the config file
    // didn't exist yet (proxy for a brand-new install) OR the user has
    // never completed onboarding. The skip / completion paths both
    // persist `[onboarding] completed = true`.
    let needs_welcome = !cfg.onboarding.completed || !config_existed;
    if needs_welcome {
        let win = parts.window.clone();
        let path = config_path.clone();
        onboarding::present_welcome(&win, path, move || {
            // Once the welcome flow finishes, kick a refresh so any
            // newly-linked Signal account starts populating the
            // sidebar without requiring the user to restart.
            debug!("welcome finished; sync layer takes over");
        });
    } else {
        // Old behaviour: nudge the user toward the linker if signal-cli
        // reports zero accounts. Skipped when welcome runs because the
        // welcome flow already routes the user there.
        maybe_open_first_run_linker(&parts.window);
    }
}

/// Read the cached conversation list synchronously (well — block on the
/// runtime, the call is fast since SQLite is local) and push it to the
/// sidebar so the user sees immediate state on cold start.
///
/// Only conversations with at least one cached message land in the
/// sidebar — `list_active_conversations` filters out empty rows and
/// joins the latest message body in for the preview line.
fn prime_sidebar_from_cache(ctx: Arc<AsyncCtx>, parts: &WindowParts) {
    let pairs = match ctx.runtime.block_on(ctx.cache.list_active_conversations()) {
        Ok(rows) => rows,
        Err(e) => {
            warn!(error = %e, "cache list_active_conversations failed; sidebar starts empty");
            return;
        }
    };
    let summaries: Vec<ConversationSummary> = pairs
        .iter()
        .map(|(c, preview)| {
            let mut s = conv_row_to_summary(c);
            s.preview = preview.clone();
            s
        })
        .collect();
    parts.set_conversations(&summaries);
}

/// Refresh contact display names from the backend without touching
/// the conversations cache.  The sidebar is fed solely from
/// `list_active_conversations` (chats with at least one message),
/// so we deliberately don't push the backend's list of every known
/// number into either the cache or the sidebar — otherwise every
/// E.164 the daemon has ever heard of would clutter the chat list.
///
/// What this DOES do: pull the backend's resolved
/// `display_name`s for known peers and write them to
/// `messenger_contacts`. That way when an existing chat row gets
/// re-rendered, the friendly name shows up instead of the raw
/// E.164 / numeric id.
fn spawn_remote_refresh(ctx: Arc<AsyncCtx>, _parts: &WindowParts) {
    let hub = ctx.hub.clone();
    let cache = ctx.cache.clone();

    ctx.runtime.spawn(async move {
        let convs = hub.list_all_conversations().await;
        for c in &convs {
            if let Some(name) = c.display_name.as_deref() {
                let _ = cache
                    .upsert_messenger_contact(c.id.backend.as_tag(), &c.id.native, name)
                    .await;
            }
        }
    });
}

/// Subscribe to hub events on the runtime, cache write-through, and
/// push UI updates to the GTK thread.
fn spawn_event_subscription(
    ctx: Arc<AsyncCtx>,
    parts: &WindowParts,
    active_chat: Rc<RefCell<Option<ChatId>>>,
    self_accounts: Rc<RefCell<HashMap<Backend, String>>>,
) {
    let hub = ctx.hub.clone();
    let cache = ctx.cache.clone();

    let (events_tx, events_rx) = std::sync::mpsc::channel::<MEvent>();

    ctx.runtime.spawn(async move {
        let mut rx = match hub.subscribe_all().await {
            Ok(rx) => rx,
            Err(e) => {
                warn!(error = %e, "hub subscribe_all failed; live events disabled");
                return;
            }
        };
        while let Some(ev) = rx.recv().await {
            // Cache write-through. Failures get logged but don't drop
            // the event — we still surface it in the UI so the user
            // doesn't miss messages even if persistence is sad.
            if let Err(e) = persist_event(&cache, &ev).await {
                warn!(error = %e, "cache persist failed");
            }
            if events_tx.send(ev).is_err() {
                debug!("UI consumer dropped; teardown");
                break;
            }
        }
    });

    let messages_box = parts.messages_box.clone();
    let messages_scroller = parts.messages_scroller.clone();
    let sidebar_index = parts.sidebar_index.clone();
    let sidebar_list = parts.sidebar_list.clone();
    let sidebar_scroller = parts.sidebar_scroller.clone();
    let sidebar_empty = parts.sidebar_empty.clone();
    let content_title = parts.content_title.clone();

    glib::source::timeout_add_local(Duration::from_millis(120), move || {
        loop {
            match events_rx.try_recv() {
                Ok(MEvent::MessageReceived(msg)) => {
                    // Carry through any sender display name the
                    // backend resolved so the sidebar / header stop
                    // showing the raw E.164 / numeric id once we've
                    // seen the peer once.
                    let summary = ConversationSummary {
                        id: msg.id.clone(),
                        title: msg.id.native.clone(),
                        display_name: msg.sender_display.clone(),
                        last_message_ts: Some(msg.ts_ms),
                        preview: msg.body.clone(),
                        unread: 0,
                    };
                    let parts = WindowPartsLite {
                        sidebar_list: sidebar_list.clone(),
                        sidebar_scroller: sidebar_scroller.clone(),
                        sidebar_empty: sidebar_empty.clone(),
                        sidebar_index: sidebar_index.clone(),
                    };
                    parts.upsert_conversation(&summary);

                    // If the message belongs to the active chat, append
                    // it to the visible message box too.
                    if active_chat.borrow().as_ref() == Some(&msg.id) {
                        let own_id = self_accounts
                            .borrow()
                            .get(&msg.id.backend)
                            .cloned();
                        append_message_widget(
                            &messages_box,
                            &messages_scroller,
                            &msg,
                            own_id.as_deref(),
                        );
                    }
                    let _ = &content_title;
                }
                Ok(MEvent::Edited { id, ts, new_body }) => {
                    debug!(?id, ts, "edit event (UI rebuild deferred)");
                    let _ = new_body;
                }
                Ok(MEvent::Deleted { id, ts }) => {
                    debug!(?id, ts, "delete event (UI rebuild deferred)");
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

/// Write a hub event into the cache. Conversation upserts come first so
/// foreign-key refs from `messages` are valid.
async fn persist_event(cache: &Cache, ev: &MEvent) -> Result<()> {
    match ev {
        MEvent::MessageReceived(msg) => {
            // Promote whatever display name the backend resolved into
            // the conversation row so cold-start sees the friendly
            // label. Falls back to the native id when none.
            let conv_name = msg
                .sender_display
                .clone()
                .unwrap_or_else(|| msg.id.native.clone());
            cache
                .upsert_conversation(&Conversation {
                    id: msg.id.to_wire(),
                    name: Some(conv_name),
                    group_id: None,
                    last_message_ts: Some(msg.ts_ms),
                    unread_count: 0,
                    archived: false,
                    muted_until: None,
                })
                .await?;
            // Tuck the resolved name into the messenger_contacts
            // table for future Signal events that arrive without a
            // sourceName (lazy enrichment in the broadcast forwarder
            // reads it back).
            if let Some(name) = msg.sender_display.as_deref() {
                let _ = cache
                    .upsert_messenger_contact(
                        msg.id.backend.as_tag(),
                        &msg.sender,
                        name,
                    )
                    .await;
            }
            cache
                .insert_message(&Message {
                    id: 0,
                    conversation_id: msg.id.to_wire(),
                    ts: msg.ts_ms,
                    sender: msg.sender.clone(),
                    body: msg.body.clone(),
                    quote_ts: None,
                    quote_sender: None,
                    edited_ts: None,
                    deleted: false,
                })
                .await?;
        }
        MEvent::Edited { .. } | MEvent::Deleted { .. } => {
            // Cache schema doesn't currently expose update-by-(conv,ts)
            // so leave these as a TODO; the UI side already logs.
        }
    }
    Ok(())
}

/// Wire row-clicks: when a sidebar row is selected, show the messages
/// for that chat (cache first, then refresh from the hub).
fn wire_sidebar_selection(
    ctx: Arc<AsyncCtx>,
    parts: &WindowParts,
    active_chat: Rc<RefCell<Option<ChatId>>>,
    self_accounts: Rc<RefCell<HashMap<Backend, String>>>,
) {
    let sidebar_index = parts.sidebar_index.clone();
    let messages_box = parts.messages_box.clone();
    let messages_scroller = parts.messages_scroller.clone();
    let content_title = parts.content_title.clone();

    parts.sidebar_list.connect_row_selected(move |_, row| {
        let row = match row {
            Some(r) => r,
            None => return,
        };
        let id = sidebar_index
            .borrow()
            .iter()
            .find(|(_, r)| r == row)
            .map(|(id, _)| id.clone());
        let id = match id {
            Some(id) => id,
            None => return,
        };
        *active_chat.borrow_mut() = Some(id.clone());
        // Prefer the resolved contact / chat name when the cache has
        // one. Cheap synchronous block_on against SQLite — matches
        // how `prime_sidebar_from_cache` already runs.
        let title = ctx
            .runtime
            .block_on(
                ctx.cache
                    .get_messenger_contact_name(id.backend.as_tag(), &id.native),
            )
            .ok()
            .flatten()
            .unwrap_or_else(|| id.native.clone());
        content_title.set_title(&title);

        let own_id = self_accounts.borrow().get(&id.backend).cloned();

        // Cache-first read.
        let cache_msgs = ctx
            .runtime
            .block_on(ctx.cache.list_messages(&id.to_wire(), 200, None))
            .unwrap_or_default();
        // Sort oldest-first; cache returns DESC.
        let mut sorted = cache_msgs.clone();
        sorted.sort_by_key(|m| m.ts);
        rebuild_messages_box(&messages_box, &messages_scroller, &sorted, &id, own_id.as_deref());

        // Async refresh from hub.
        let id_clone = id.clone();
        let cache = ctx.cache.clone();
        let hub = ctx.hub.clone();
        let (tx, rx) = std::sync::mpsc::channel::<Vec<NormalizedMessage>>();
        ctx.runtime.spawn(async move {
            match hub
                .backends()
                .iter()
                .find(|b| b.backend() == id_clone.backend)
            {
                Some(backend) => match backend.fetch_history(&id_clone, 100, None).await {
                    Ok(msgs) => {
                        // Persist for next cold start.
                        for m in &msgs {
                            let _ = cache
                                .insert_message(&Message {
                                    id: 0,
                                    conversation_id: m.id.to_wire(),
                                    ts: m.ts_ms,
                                    sender: m.sender.clone(),
                                    body: m.body.clone(),
                                    quote_ts: None,
                                    quote_sender: None,
                                    edited_ts: None,
                                    deleted: false,
                                })
                                .await;
                        }
                        let _ = tx.send(msgs);
                    }
                    Err(e) => {
                        debug!(error = %e, "fetch_history failed");
                    }
                },
                None => debug!(?id_clone, "no backend registered for chat"),
            }
        });

        let messages_box_ui = messages_box.clone();
        let messages_scroller_ui = messages_scroller.clone();
        let id_for_ui = id.clone();
        let own_id_for_ui = own_id.clone();
        glib::source::timeout_add_local(Duration::from_millis(120), move || match rx.try_recv() {
            Ok(mut msgs) => {
                if !msgs.is_empty() {
                    msgs.sort_by_key(|m| m.ts_ms);
                    rebuild_messages_box_normalized(
                        &messages_box_ui,
                        &messages_scroller_ui,
                        &msgs,
                        &id_for_ui,
                        own_id_for_ui.as_deref(),
                    );
                }
                glib::ControlFlow::Break
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
    });
}

/// Composer Enter: send via the hub (off-thread) and optimistically
/// append the message to the UI + cache so the user sees their text
/// land instantly.
fn wire_composer_send(
    ctx: Option<Arc<AsyncCtx>>,
    parts: &WindowParts,
    active_chat: Rc<RefCell<Option<ChatId>>>,
    engine: Rc<RefCell<Engine>>,
    self_accounts: Rc<RefCell<HashMap<Backend, String>>>,
) {
    let mode_line = parts.mode_line.clone();
    let messages_box = parts.messages_box.clone();
    let messages_scroller = parts.messages_scroller.clone();
    let toast_overlay = parts.toast_overlay.clone();

    parts.composer.set_on_send(move |text| {
        // PII: never log message body. `len` is enough for activity tracing.
        info!(len = text.len(), "composer Enter");
        engine.borrow_mut().set_mode(Mode::Normal);
        mode_line.set_mode(Mode::Normal);

        let id = match active_chat.borrow().clone() {
            Some(id) => id,
            None => {
                let toast = adw::Toast::builder()
                    .title("Pick a chat first.")
                    .timeout(3)
                    .build();
                toast_overlay.add_toast(toast);
                return;
            }
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);

        // Optimistic UI: append a "mine" bubble immediately. We tag it
        // with the resolved self-account so it round-trips through
        // `is_mine` correctly; falling back to "me" keeps the bubble
        // visually correct when the backend hasn't reported a self id
        // yet (e.g. Telegram pre-login).
        let own_id = self_accounts.borrow().get(&id.backend).cloned();
        let optimistic_sender = own_id.clone().unwrap_or_else(|| "me".into());
        let optimistic = NormalizedMessage {
            id: id.clone(),
            ts_ms: now,
            sender: optimistic_sender.clone(),
            // Outgoing messages render as "You" in the UI so we
            // don't need a resolved display name here.
            sender_display: None,
            body: Some(text.clone()),
            attachments: Vec::new(),
            backend_extras: match id.backend {
                crate::messenger::Backend::Signal => {
                    crate::messenger::BackendExtras::Signal { group_id: None }
                }
                crate::messenger::Backend::Telegram => crate::messenger::BackendExtras::Telegram {
                    reply_to_msg_id: None,
                },
            },
        };
        append_message_widget(&messages_box, &messages_scroller, &optimistic, own_id.as_deref());

        if let Some(ctx) = ctx.clone() {
            let id_clone = id.clone();
            let body_clone = text.clone();
            let cache = ctx.cache.clone();
            let hub = ctx.hub.clone();
            let toast_overlay_for_err = toast_overlay.clone();
            let (err_tx, err_rx) = std::sync::mpsc::channel::<String>();
            ctx.runtime.spawn(async move {
                match hub.send(&id_clone, &body_clone, &[]).await {
                    Ok(ts) => {
                        let _ = cache
                            .insert_message(&Message {
                                id: 0,
                                conversation_id: id_clone.to_wire(),
                                ts,
                                sender: optimistic_sender.clone(),
                                body: Some(body_clone),
                                quote_ts: None,
                                quote_sender: None,
                                edited_ts: None,
                                deleted: false,
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = err_tx.send(format!("{e}"));
                    }
                }
            });
            glib::source::timeout_add_local(Duration::from_millis(120), move || {
                match err_rx.try_recv() {
                    Ok(msg) => {
                        let toast = adw::Toast::builder()
                            .title(format!("Send failed: {msg}"))
                            .timeout(5)
                            .priority(adw::ToastPriority::High)
                            .build();
                        toast_overlay_for_err.add_toast(toast);
                        glib::ControlFlow::Break
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
                }
            });
        }
    });
}

/// Spawn a `ConfigWatcher` for `config_path` and poll it from the GTK
/// main loop. When a fresh snapshot arrives whose `theme` differs from
/// the manager's currently-applied theme, reapply in place — same
/// `CssProvider`, no display re-registration.
///
/// The watcher (which owns a notify thread + worker thread) is moved
/// into the timeout closure so its lifetime matches the application.
/// Errors creating the watcher are logged and degrade gracefully: the
/// app keeps running, just without live reload.
fn spawn_config_watcher(
    config_path: std::path::PathBuf,
    theme: Rc<RefCell<ThemeManager>>,
    messages_box: gtk::Box,
    initial_bubbles: bool,
) {
    let watcher = match ConfigWatcher::new(config_path) {
        Ok(w) => w,
        Err(e) => {
            warn!(error = %e, "config watcher disabled; live-reload off");
            return;
        }
    };

    // `tokio::sync::watch::Receiver` is poll-friendly without a tokio
    // runtime: `has_changed()` and `borrow_and_update()` are sync. We
    // poll every 200ms which is well under save-to-visible perceptual
    // latency (the worker itself debounces ~150ms inside notify).
    let mut rx = watcher.rx.clone();
    // Move the watcher into the closure so its notify + worker threads
    // outlive `activate()`. Underscore-prefixed because we never read
    // it — but it must stay owned; dropping it would tear down the
    // background machinery.
    let _watcher_keepalive = watcher;
    let last_bubbles = Rc::new(Cell::new(initial_bubbles));

    glib::source::timeout_add_local(Duration::from_millis(200), move || {
        if !rx.has_changed().unwrap_or(false) {
            return glib::ControlFlow::Continue;
        }
        let new_cfg = rx.borrow_and_update().clone();
        let mut tm = theme.borrow_mut();
        let current = tm.current().unwrap_or("");
        if !new_cfg.general.theme.eq_ignore_ascii_case(current) {
            match tm.apply(&new_cfg.general.theme) {
                Ok(()) => info!(theme = %new_cfg.general.theme, "theme hot-reloaded"),
                Err(e) => warn!(error = %e, "live theme reload failed"),
            }
        }
        if new_cfg.appearance.message_bubbles != last_bubbles.get() {
            window::apply_bubble_class(&messages_box, new_cfg.appearance.message_bubbles);
            last_bubbles.set(new_cfg.appearance.message_bubbles);
            info!(
                bubbles = new_cfg.appearance.message_bubbles,
                "message-bubble setting hot-reloaded"
            );
        }
        // Anchor the watcher's lifetime to this closure.
        let _ = &_watcher_keepalive;
        glib::ControlFlow::Continue
    });
}

// ----- Local helpers -----

/// Lightweight handle bundle so we can share sidebar mutation logic
/// between the sync prime path and the async refresh callback. Mirrors
/// the subset of [`WindowParts`] those helpers need.
struct WindowPartsLite {
    sidebar_list: gtk::ListBox,
    sidebar_scroller: gtk::ScrolledWindow,
    sidebar_empty: gtk::Widget,
    sidebar_index: Rc<RefCell<Vec<(ChatId, gtk::ListBoxRow)>>>,
}

impl WindowPartsLite {
    fn upsert_conversation(&self, summary: &ConversationSummary) {
        let existing = {
            let idx = self.sidebar_index.borrow();
            idx.iter()
                .position(|(cid, _)| cid == &summary.id)
                .map(|i| (i, idx[i].1.clone()))
        };
        if let Some((i, row)) = existing {
            self.sidebar_list.remove(&row);
            self.sidebar_index.borrow_mut().remove(i);
        }
        let ts_label = summary
            .last_message_ts
            .map(window::format_clock_label)
            .unwrap_or_default();
        let preview = summary.preview.as_deref().unwrap_or("");
        let row = window::chat_row(summary.label(), preview, &ts_label);
        self.sidebar_list.prepend(&row);
        self.sidebar_index
            .borrow_mut()
            .insert(0, (summary.id.clone(), row));
        let has_rows = self.sidebar_list.row_at_index(0).is_some();
        self.sidebar_scroller.set_visible(has_rows);
        self.sidebar_empty.set_visible(!has_rows);
    }
}

fn rebuild_messages_box(
    b: &gtk::Box,
    scroller: &gtk::ScrolledWindow,
    msgs: &[Message],
    id: &ChatId,
    own_id: Option<&str>,
) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
    if msgs.is_empty() {
        b.append(&window::messages_empty_state());
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let _ = id; // retained for future per-chat colouring
    let rows: Vec<(bool, String, i64)> = msgs
        .iter()
        .map(|m| {
            (
                window::is_mine(&m.sender, own_id),
                m.body.clone().unwrap_or_default(),
                m.ts,
            )
        })
        .collect();
    window::populate_messages(b, &rows, now);
    // Fresh chat open / cache load: pin to the latest message.
    window::scroll_to_bottom_idle(scroller);
}

fn rebuild_messages_box_normalized(
    b: &gtk::Box,
    scroller: &gtk::ScrolledWindow,
    msgs: &[NormalizedMessage],
    id: &ChatId,
    own_id: Option<&str>,
) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
    if msgs.is_empty() {
        b.append(&window::messages_empty_state());
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let _ = id;
    let rows: Vec<(bool, String, i64)> = msgs
        .iter()
        .map(|m| {
            (
                window::is_mine(&m.sender, own_id),
                m.body.clone().unwrap_or_default(),
                m.ts_ms,
            )
        })
        .collect();
    window::populate_messages(b, &rows, now);
    // Hub refresh after a chat switch is still a "first render" of fresh
    // history — pin to the latest message so the user lands at the foot.
    window::scroll_to_bottom_idle(scroller);
}

/// Append a single new message to the visible chat. If the user was
/// already at (or within `NEAR_BOTTOM_PX` of) the foot of the scroller
/// when the message arrived, snap to the new bottom on the next idle
/// tick. If they had scrolled up to read history, leave the viewport
/// alone — yanking them back is the textbook "chat app loses my place"
/// regression and we explicitly don't want it.
fn append_message_widget(
    messages_box: &gtk::Box,
    scroller: &gtk::ScrolledWindow,
    msg: &NormalizedMessage,
    own_id: Option<&str>,
) {
    let was_near_bottom = window::is_near_bottom(scroller);
    if let Some(first) = messages_box.first_child() {
        if first.has_css_class("kryptos-empty-state") {
            messages_box.remove(&first);
        }
    }
    let mine = window::is_mine(&msg.sender, own_id);
    let label = window::format_clock_label(msg.ts_ms);
    let body = msg.body.clone().unwrap_or_default();
    messages_box.append(&window::message_row(mine, &body, &label, true, true));
    if was_near_bottom {
        window::scroll_to_bottom_idle(scroller);
    }
}

fn conv_row_to_summary(c: &Conversation) -> ConversationSummary {
    let id = ChatId::from_wire(&c.id).unwrap_or_else(|| ChatId {
        backend: crate::messenger::Backend::Signal,
        native: c.id.clone(),
    });
    // The cached `name` column doubles as a "human label or
    // backend-native id" — when it differs from the native id we
    // treat it as a resolved display name. (Pre-contact-name code
    // wrote the native id verbatim into `name`, so this heuristic
    // promotes new entries while leaving legacy rows visible.)
    let title_or_native = c.name.clone().unwrap_or_else(|| id.native.clone());
    let display_name = if title_or_native != id.native {
        Some(title_or_native.clone())
    } else {
        None
    };
    ConversationSummary {
        id,
        title: title_or_native,
        display_name,
        last_message_ts: c.last_message_ts,
        preview: None,
        unread: c.unread_count.max(0) as u32,
    }
}

// Avoid warnings for unused imports / channels in degraded modes.
#[allow(dead_code)]
fn _unused_marker(_: tmpsc::UnboundedReceiver<MEvent>) {}

/// Off-thread first-run check. If signal-cli is reachable and reports
/// zero accounts, we surface the linker non-modally on top of the
/// freshly-presented main window. Any error path (no bus, no daemon)
/// falls through to the main UI so the user is never trapped.
fn maybe_open_first_run_linker(window: &adw::ApplicationWindow) {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel::<bool>();
    std::thread::Builder::new()
        .name("kryptos-first-run".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .ok()
                    .map(|rt| {
                        rt.block_on(async {
                            match SignalClient::connect().await {
                                Ok(c) => {
                                    let _ = crate::dbus::ensure_running(c.connection()).await;
                                    onboarding::first_run_check_async(&c).await
                                }
                                Err(_) => false,
                            }
                        })
                    })
                    .unwrap_or(false)
            });
            let _ = tx.send(result.unwrap_or(false));
        })
        .expect("spawn first-run probe thread");

    let win = window.clone();
    glib::source::timeout_add_local(Duration::from_millis(150), move || match rx.try_recv() {
        Ok(true) => {
            onboarding::open_linker(&win);
            glib::ControlFlow::Break
        }
        Ok(false) => glib::ControlFlow::Break,
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

fn wire_command_bar(parts: &WindowParts, dispatcher: &Dispatcher, engine: Rc<RefCell<Engine>>) {
    // Activate (Enter) — commit and return to Normal.
    let entry = parts.command_bar.entry().clone();
    let bar = parts.command_bar.clone();
    let mode_line = parts.mode_line.clone();
    let dispatcher_act = dispatcher.clone();
    let engine_act = engine.clone();
    entry.connect_activate(move |entry| {
        let text = entry.text().to_string();
        let mode = engine_act.borrow().mode();
        match mode {
            Mode::Command => dispatcher_act.run_command(&text),
            Mode::Search => dispatcher_act.run_search(&text),
            _ => {}
        }
        bar.hide();
        engine_act.borrow_mut().set_mode(Mode::Normal);
        mode_line.set_mode(Mode::Normal);
    });

    // Esc inside the entry — cancel and return to Normal. In Search
    // mode we also drop the active filter so the sidebar is whole again.
    let esc = gtk::EventControllerKey::new();
    let bar_esc = parts.command_bar.clone();
    let mode_line_esc = parts.mode_line.clone();
    let engine_esc = engine.clone();
    let dispatcher_esc = dispatcher.clone();
    esc.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk::gdk::Key::Escape {
            let mode_before = engine_esc.borrow().mode();
            if mode_before == Mode::Search {
                dispatcher_esc.clear_search();
            }
            bar_esc.hide();
            engine_esc.borrow_mut().set_mode(Mode::Normal);
            mode_line_esc.set_mode(Mode::Normal);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    parts.command_bar.entry().add_controller(esc);
}

fn wire_keys(parts: &WindowParts, dispatcher: &Dispatcher, engine: Rc<RefCell<Engine>>) {
    let controller = gtk::EventControllerKey::new();

    let mode_line = parts.mode_line.clone();
    let command_bar = parts.command_bar.clone();
    let dispatcher = dispatcher.clone();

    controller.connect_key_pressed(move |_ctrl, keyval, _keycode, state| {
        // While the command bar is up, leave its entry alone — its own
        // controllers handle Enter / Esc.
        if command_bar.is_visible() {
            return glib::Propagation::Proceed;
        }

        let Some(key) = input::translate_gdk(keyval, state) else {
            return glib::Propagation::Proceed;
        };

        let current_mode = engine.borrow().mode();

        // In Insert, only forward Esc and Ctrl-modified keys to the
        // engine. Plain printable keys flow to the focused composer.
        if current_mode == Mode::Insert
            && !key.mods.ctrl
            && !matches!(&key.sym, KeySym::Named(n) if n == "Esc")
        {
            return glib::Propagation::Proceed;
        }

        let outcome = engine.borrow_mut().feed(key);
        match outcome {
            Outcome::Action(action) => {
                let mode_before = engine.borrow().mode();
                if let Some(new_mode) = dispatcher.dispatch(&action, mode_before) {
                    engine.borrow_mut().set_mode(new_mode);
                }
                mode_line.set_mode(engine.borrow().mode());
                glib::Propagation::Stop
            }
            Outcome::Pending => glib::Propagation::Stop,
            Outcome::Cancelled => {
                if engine.borrow().mode() == Mode::Insert {
                    glib::Propagation::Proceed
                } else {
                    glib::Propagation::Stop
                }
            }
        }
    });

    parts.window.add_controller(controller);
}

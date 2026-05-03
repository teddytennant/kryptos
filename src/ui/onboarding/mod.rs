//! Device-linking onboarding flow.
//!
//! Kryptos talks to `signal-cli` over D-Bus, but signal-cli only
//! responds usefully once it knows about an account. The user gets one
//! by linking Kryptos as a *secondary device* of an existing Signal
//! install on their phone:
//!
//! 1. Call `SignalControl.link(name)` — returns a `tsdevice://...` URI.
//! 2. Render that URI as a QR code.
//! 3. The user scans it from Signal → Settings → Linked devices.
//! 4. signal-cli completes the handshake and the new account appears
//!    in `list_accounts()`.
//!
//! This module wires that flow into a beautiful Adwaita window so the
//! user never has to drop into a terminal.
//!
//! Design tenets, in order:
//!   * The QR card is the hero — large, centered, pristine quiet zone.
//!   * Generous whitespace; no toolbar clutter while waiting.
//!   * Status copy reads as prose, not stack trace.
//!   * The window is non-modal: the user can keep using the rest of
//!     the app. We just present it on top of the main window.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tracing::{debug, error, info, warn};

use crate::core::{Error, Result};
use crate::dbus::SignalClient;

mod link_state;
pub mod telegram_login;
mod welcome;

pub use link_state::{detect_new_account, LinkOutcome};
pub use telegram_login::present as present_telegram_login;
pub use welcome::present as present_welcome;

/// How often we ask signal-cli "did the link finish yet?".
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// Hard cap for the polling task. Signal's flow normally finishes in
/// seconds; if it stalls past this the user has either walked away or
/// signal-cli isn't running. Either way: stop burning bus calls.
const POLL_TIMEOUT: Duration = Duration::from_secs(300);

/// Resolve true if the user has *no* linked Signal accounts yet, in
/// which case the main window should immediately surface the linker.
///
/// Errors degrade to `false` — we'd rather skip the autostart than
/// trap the user behind a broken first-run modal if signal-cli is
/// merely down.
pub async fn first_run_check_async(client: &SignalClient) -> bool {
    match client.list_accounts().await {
        Ok(accounts) => accounts.is_empty(),
        Err(e) => {
            warn!(error = %e, "first-run check: list_accounts failed; skipping autostart");
            false
        }
    }
}

/// Open the linker window, transient over `parent`.
///
/// Non-blocking — returns as soon as the window is presented. The
/// linker drives its own background work via a worker thread + glib
/// timeout (the same pattern as `settings::spawn_version_probe`).
pub fn open_linker(parent: &impl IsA<gtk::Window>) {
    LinkerWindow::build(parent.as_ref()).present();
}

/// One-stop backends panel.
///
/// Renders the two messengers with live "Connected as …" status, and
/// for already-connected backends offers a fork: keep using the local
/// cache as-is, or wipe it and re-sync from the server with the same
/// account.  The status probe is async (signal-cli D-Bus + a config
/// + session-file check for Telegram), so the rows render as
/// "Checking…" until the worker thread reports back.
pub fn present_backends_panel(parent: &impl IsA<gtk::Window>) {
    let win = adw::Window::builder()
        .transient_for(parent.as_ref())
        .modal(true)
        .default_width(520)
        .default_height(420)
        .title("Backends")
        .build();
    win.add_css_class("kryptos-backends");

    let header = adw::HeaderBar::builder().show_title(true).build();
    header.add_css_class("flat");

    let intro = gtk::Label::builder()
        .label("Manage your messaging accounts. Connected backends can keep their cached conversations, or start fresh with the same account.")
        .halign(gtk::Align::Start)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .build();
    intro.add_css_class("body");
    intro.add_css_class("dim-label");

    let listbox = gtk::ListBox::new();
    listbox.set_selection_mode(gtk::SelectionMode::None);
    listbox.add_css_class("boxed-list");

    let signal_row = adw::ActionRow::builder()
        .title("Signal")
        .subtitle("Checking…")
        .build();
    let signal_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    signal_actions.set_valign(gtk::Align::Center);
    signal_row.add_suffix(&signal_actions);

    let telegram_row = adw::ActionRow::builder()
        .title("Telegram")
        .subtitle("Checking…")
        .build();
    let telegram_actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    telegram_actions.set_valign(gtk::Align::Center);
    telegram_row.add_suffix(&telegram_actions);

    listbox.append(&signal_row);
    listbox.append(&telegram_row);

    let toast_overlay = adw::ToastOverlay::new();
    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .build();
    body.set_margin_start(28);
    body.set_margin_end(28);
    body.set_margin_top(20);
    body.set_margin_bottom(24);
    body.append(&intro);
    body.append(&listbox);
    toast_overlay.set_child(Some(&body));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toast_overlay));
    win.set_content(Some(&toolbar));
    win.present();

    spawn_backends_probe(BackendsPanelHandles {
        win: win.clone(),
        toast_overlay,
        signal_row,
        signal_actions,
        telegram_row,
        telegram_actions,
    });
}

struct BackendsPanelHandles {
    win: adw::Window,
    toast_overlay: adw::ToastOverlay,
    signal_row: adw::ActionRow,
    signal_actions: gtk::Box,
    telegram_row: adw::ActionRow,
    telegram_actions: gtk::Box,
}

#[derive(Debug, Clone)]
enum BackendStatus {
    /// signal-cli reachable + at least one account.
    Linked { account: String },
    /// signal-cli reachable but no account / Telegram creds set but
    /// no usable session yet.
    NotLinked,
    /// Backend isn't configured at all (Telegram only — signal-cli
    /// is always installed in this build).
    NotConfigured,
    /// Probe couldn't reach the backend (D-Bus down, etc.). Treat as
    /// "we don't know"; render with a "Set up" button as a neutral
    /// fallback.
    Unknown(String),
}

#[derive(Debug)]
struct ProbeResult {
    signal: BackendStatus,
    telegram: BackendStatus,
}

fn spawn_backends_probe(handles: BackendsPanelHandles) {
    let (tx, rx) = mpsc::channel::<ProbeResult>();

    std::thread::Builder::new()
        .name("kryptos-backends-probe".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run_backends_probe()
            }));
            let probe = result.unwrap_or_else(|_| ProbeResult {
                signal: BackendStatus::Unknown("probe panicked".into()),
                telegram: BackendStatus::Unknown("probe panicked".into()),
            });
            let _ = tx.send(probe);
        })
        .expect("spawn kryptos-backends-probe thread");

    let BackendsPanelHandles {
        win,
        toast_overlay,
        signal_row,
        signal_actions,
        telegram_row,
        telegram_actions,
    } = handles;

    glib::source::timeout_add_local(Duration::from_millis(120), move || match rx.try_recv() {
        Ok(result) => {
            apply_backend_status(
                &win,
                &toast_overlay,
                &signal_row,
                &signal_actions,
                "signal",
                &result.signal,
            );
            apply_backend_status(
                &win,
                &toast_overlay,
                &telegram_row,
                &telegram_actions,
                "telegram",
                &result.telegram,
            );
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

fn run_backends_probe() -> ProbeResult {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            return ProbeResult {
                signal: BackendStatus::Unknown(format!("tokio: {e}")),
                telegram: BackendStatus::Unknown(format!("tokio: {e}")),
            };
        }
    };
    rt.block_on(async {
        ProbeResult {
            signal: probe_signal().await,
            telegram: probe_telegram().await,
        }
    })
}

async fn probe_signal() -> BackendStatus {
    let client = match SignalClient::connect().await {
        Ok(c) => c,
        Err(e) => return BackendStatus::Unknown(format!("D-Bus: {e}")),
    };
    if let Err(e) = crate::dbus::ensure_running(client.connection()).await {
        return BackendStatus::Unknown(format!("signal-cli: {e}"));
    }
    match client.list_accounts().await {
        Ok(list) if !list.is_empty() => {
            let mut sorted = list;
            sorted.sort();
            BackendStatus::Linked {
                account: sorted.into_iter().next().unwrap_or_default(),
            }
        }
        Ok(_) => BackendStatus::NotLinked,
        Err(e) => BackendStatus::Unknown(format!("listAccounts: {e}")),
    }
}

async fn probe_telegram() -> BackendStatus {
    let cfg_path = match crate::config::loader::default_path() {
        Ok(p) => p,
        Err(e) => return BackendStatus::Unknown(format!("config path: {e}")),
    };
    let cfg = match crate::config::loader::load_or_default(&cfg_path) {
        Ok(c) => c,
        Err(e) => return BackendStatus::Unknown(format!("load config: {e}")),
    };
    if cfg.backends.telegram.api_id == 0 || cfg.backends.telegram.api_hash.is_empty() {
        return BackendStatus::NotConfigured;
    }
    let session_path =
        crate::messenger::telegram::resolve_session_path(&cfg.backends.telegram.session_path);
    if !session_path.exists() {
        return BackendStatus::NotLinked;
    }
    BackendStatus::Linked {
        account: "your account".into(),
    }
}

fn clear_box(b: &gtk::Box) {
    while let Some(child) = b.first_child() {
        b.remove(&child);
    }
}

fn apply_backend_status(
    panel_win: &adw::Window,
    toast: &adw::ToastOverlay,
    row: &adw::ActionRow,
    actions: &gtk::Box,
    tag: &'static str,
    status: &BackendStatus,
) {
    clear_box(actions);
    let label = backend_label(tag);
    match status {
        BackendStatus::Linked { account } => {
            row.set_subtitle(&format!("Connected as {account}"));
            row.add_css_class("kryptos-backend-connected");

            let restore = gtk::Button::with_label("Use existing");
            restore.set_valign(gtk::Align::Center);
            restore.add_css_class("flat");
            restore.set_tooltip_text(Some(&format!(
                "Keep cached {label} conversations and pull updates as they arrive."
            )));
            {
                let panel_win = panel_win.clone();
                let toast = toast.clone();
                let label = label.to_string();
                restore.connect_clicked(move |_| {
                    show_panel_toast(&toast, &format!("Restored — keeping cached {label} chats."));
                    let win = panel_win.clone();
                    glib::source::timeout_add_local_once(Duration::from_millis(700), move || {
                        win.close();
                    });
                });
            }

            let fresh = gtk::Button::with_label("Start fresh");
            fresh.set_valign(gtk::Align::Center);
            fresh.add_css_class("destructive-action");
            fresh.set_tooltip_text(Some(&format!(
                "Wipe local {label} cache. Same account; conversations repopulate from the server."
            )));
            {
                let panel_win = panel_win.clone();
                let toast = toast.clone();
                fresh.connect_clicked(move |_| {
                    confirm_start_fresh(&panel_win, &toast, tag);
                });
            }

            actions.append(&restore);
            actions.append(&fresh);
        }
        BackendStatus::NotLinked | BackendStatus::NotConfigured => {
            let subtitle = match (tag, status) {
                ("signal", _) => "Not linked yet — pair Kryptos with your phone.",
                ("telegram", BackendStatus::NotConfigured) => {
                    "Not set up yet — needs api_id / api_hash from my.telegram.org."
                }
                ("telegram", _) => "Credentials stored, but the session needs a sign-in.",
                _ => "Not configured yet.",
            };
            row.set_subtitle(subtitle);
            row.remove_css_class("kryptos-backend-connected");

            let setup = gtk::Button::with_label("Set up…");
            setup.set_valign(gtk::Align::Center);
            setup.add_css_class("suggested-action");
            let panel_win = panel_win.clone();
            setup.connect_clicked(move |_| match tag {
                "signal" => open_linker(&panel_win),
                "telegram" => present_telegram_login(&panel_win, None),
                _ => {}
            });
            actions.append(&setup);
        }
        BackendStatus::Unknown(reason) => {
            row.set_subtitle(&format!("Status unknown — {reason}"));
            row.remove_css_class("kryptos-backend-connected");

            let setup = gtk::Button::with_label("Set up…");
            setup.set_valign(gtk::Align::Center);
            setup.add_css_class("flat");
            let panel_win = panel_win.clone();
            setup.connect_clicked(move |_| match tag {
                "signal" => open_linker(&panel_win),
                "telegram" => present_telegram_login(&panel_win, None),
                _ => {}
            });
            actions.append(&setup);
        }
    }
}

fn backend_label(tag: &str) -> &'static str {
    match tag {
        "signal" => "Signal",
        "telegram" => "Telegram",
        _ => "this backend",
    }
}

fn show_panel_toast(overlay: &adw::ToastOverlay, msg: &str) {
    overlay.add_toast(adw::Toast::builder().title(msg).timeout(3).build());
}

fn confirm_start_fresh(parent: &adw::Window, toast: &adw::ToastOverlay, tag: &'static str) {
    let label = backend_label(tag);
    let dialog = adw::AlertDialog::builder()
        .heading(format!("Start fresh with {label}?"))
        .body(format!(
            "This wipes Kryptos's local copy of your {label} conversations, messages, and contacts. \
             Your account stays signed in; messages will repopulate from the server."
        ))
        .build();
    dialog.add_response("cancel", "Cancel");
    dialog.add_response("wipe", "Start fresh");
    dialog.set_response_appearance("wipe", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let toast = toast.clone();
    let parent_for_wipe = parent.clone();
    dialog.connect_response(None, move |dlg, response| {
        if response == "wipe" {
            spawn_backend_wipe(toast.clone(), parent_for_wipe.clone(), tag);
        }
        dlg.close();
    });
    dialog.present(Some(parent));
}

fn spawn_backend_wipe(toast: adw::ToastOverlay, parent: adw::Window, tag: &'static str) {
    let (tx, rx) = mpsc::channel::<std::result::Result<(), String>>();
    std::thread::Builder::new()
        .name("kryptos-backend-wipe".into())
        .spawn(move || {
            let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| format!("tokio: {e}"))?;
                rt.block_on(async {
                    let path = crate::cache::Cache::default_path()
                        .map_err(|e| format!("cache path: {e}"))?;
                    let cache = crate::cache::Cache::open(&path)
                        .await
                        .map_err(|e| format!("open cache: {e}"))?;
                    cache
                        .clear_backend(tag)
                        .await
                        .map_err(|e| format!("clear: {e}"))
                })
            }))
            .unwrap_or_else(|_| Err("wipe worker panicked".to_string()));
            let _ = tx.send(res);
        })
        .expect("spawn kryptos-backend-wipe thread");

    let label = backend_label(tag);
    glib::source::timeout_add_local(
        Duration::from_millis(120),
        move || match rx.try_recv() {
            Ok(Ok(())) => {
                show_panel_toast(
                    &toast,
                    &format!("Cleared local {label} cache. Restart Kryptos to re-sync."),
                );
                let win = parent.clone();
                glib::source::timeout_add_local_once(Duration::from_millis(900), move || {
                    win.close();
                });
                glib::ControlFlow::Break
            }
            Ok(Err(e)) => {
                error!(error = %e, backend = tag, "backend wipe failed");
                show_panel_toast(&toast, &format!("Couldn't clear {label}: {e}"));
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        },
    );
}

// ---------------------------------------------------------------------------
// Window construction
// ---------------------------------------------------------------------------

struct LinkerWindow;

impl LinkerWindow {
    fn build(parent: &gtk::Window) -> adw::Window {
        let win = adw::Window::builder()
            .transient_for(parent)
            .modal(false)
            .default_width(560)
            .default_height(720)
            .title("Link to Signal")
            .build();
        win.add_css_class("kryptos-linker");

        let header = adw::HeaderBar::builder().show_title(false).build();
        header.add_css_class("flat");

        // Hero copy column — 480px wide, left-aligned. The title and
        // body sit together; the QR card centres separately below.
        let title = gtk::Label::builder()
            .label("Link to Signal")
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .build();
        title.add_css_class("title-1");
        title.add_css_class("kryptos-linker-title");

        let body = gtk::Label::builder()
            .label(
                "Open Signal on your phone and head to \
                 Settings → Linked devices → Link a new device. \
                 Scan the code below to bring this conversation home.",
            )
            .halign(gtk::Align::Start)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .build();
        body.add_css_class("body");
        body.add_css_class("dim-label");
        body.add_css_class("kryptos-linker-body");

        let hero_column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Start)
            .width_request(480)
            .build();
        hero_column.add_css_class("kryptos-linker-hero");
        hero_column.append(&title);
        hero_column.append(&body);

        // QR hero card. Starts as a placeholder; Generate flips it.
        let qr_area = gtk::DrawingArea::builder()
            .content_width(QR_CARD_PX)
            .content_height(QR_CARD_PX)
            .halign(gtk::Align::Center)
            .build();
        qr_area.add_css_class("kryptos-qr-canvas");

        let qr_state: Rc<RefCell<QrState>> = Rc::new(RefCell::new(QrState::Idle));
        {
            let qr_state = qr_state.clone();
            qr_area.set_draw_func(move |_, cr, w, h| {
                draw_qr(cr, w, h, &qr_state.borrow());
            });
        }

        let qr_card = gtk::Frame::new(None);
        qr_card.set_child(Some(&qr_area));
        qr_card.set_halign(gtk::Align::Center);
        qr_card.add_css_class("kryptos-qr-card");

        // URI shown in monospace under the QR for verification / fallback.
        let uri_label = gtk::Label::builder()
            .label("")
            .halign(gtk::Align::Center)
            .selectable(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .build();
        uri_label.add_css_class("monospace");
        uri_label.add_css_class("dim-label");
        uri_label.add_css_class("caption");
        uri_label.add_css_class("kryptos-linker-uri");

        // Device name input + Generate button row. The Generate button is
        // a square 56×40 primary action — Swiss aesthetic, not a pill.
        // A "Copy URI" button appears next to the entry once we've got
        // a URI to copy.
        let name_entry = gtk::Entry::builder()
            .placeholder_text("Device name")
            .text(default_device_name())
            .hexpand(true)
            .build();
        name_entry.add_css_class("kryptos-linker-name");

        let copy_btn = gtk::Button::from_icon_name("edit-copy-symbolic");
        copy_btn.set_tooltip_text(Some("Copy URI"));
        copy_btn.add_css_class("flat");
        copy_btn.add_css_class("kryptos-linker-copy");
        copy_btn.set_visible(false);

        let generate_btn = gtk::Button::with_label("Generate code");
        generate_btn.add_css_class("suggested-action");
        generate_btn.add_css_class("kryptos-linker-generate");
        generate_btn.set_size_request(160, 40);

        let input_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::Fill)
            .build();
        input_row.append(&name_entry);
        input_row.append(&copy_btn);
        input_row.append(&generate_btn);

        // Status / spinner row.
        let spinner = gtk::Spinner::new();
        spinner.set_visible(false);
        let status_label = gtk::Label::builder()
            .label("Pick a name for this device, then generate a code.")
            .halign(gtk::Align::Start)
            .wrap(true)
            .xalign(0.0)
            .build();
        status_label.add_css_class("dim-label");
        status_label.add_css_class("kryptos-linker-status");

        let status_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::Start)
            .build();
        status_row.append(&spinner);
        status_row.append(&status_label);

        // Close button at the bottom-right; takes the user out of the flow.
        let close_btn = gtk::Button::with_label("Close");
        close_btn.add_css_class("flat");
        close_btn.add_css_class("pill");
        let close_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::End)
            .build();
        close_row.append(&close_btn);

        // Compose.
        let body_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            .build();
        body_box.set_margin_start(36);
        body_box.set_margin_end(36);
        body_box.set_margin_top(12);
        body_box.set_margin_bottom(28);
        body_box.append(&hero_column);
        body_box.append(&spacer(8));
        body_box.append(&qr_card);
        body_box.append(&uri_label);
        body_box.append(&spacer(4));
        body_box.append(&input_row);
        body_box.append(&status_row);
        body_box.append(&close_row);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&body_box));
        win.set_content(Some(&toolbar));

        install_linker_styles();

        // Wire actions.
        let win_for_close = win.clone();
        close_btn.connect_clicked(move |_| win_for_close.close());

        wire_generate(WireGenerateArgs {
            window: win.clone(),
            generate_btn: generate_btn.clone(),
            name_entry: name_entry.clone(),
            copy_btn: copy_btn.clone(),
            qr_area: qr_area.clone(),
            qr_state: qr_state.clone(),
            uri_label: uri_label.clone(),
            spinner: spinner.clone(),
            status_label: status_label.clone(),
        });

        win
    }
}

fn spacer(px: i32) -> gtk::Widget {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .height_request(px)
        .build()
        .upcast::<gtk::Widget>()
}

// ---------------------------------------------------------------------------
// Generate-button wiring
// ---------------------------------------------------------------------------

struct WireGenerateArgs {
    window: adw::Window,
    generate_btn: gtk::Button,
    name_entry: gtk::Entry,
    copy_btn: gtk::Button,
    qr_area: gtk::DrawingArea,
    qr_state: Rc<RefCell<QrState>>,
    uri_label: gtk::Label,
    spinner: gtk::Spinner,
    status_label: gtk::Label,
}

fn wire_generate(args: WireGenerateArgs) {
    let WireGenerateArgs {
        window,
        generate_btn,
        name_entry,
        copy_btn,
        qr_area,
        qr_state,
        uri_label,
        spinner,
        status_label,
    } = args;

    let btn = generate_btn.clone();
    let copy_for_click = copy_btn.clone();
    btn.connect_clicked(move |_| {
        let device_name = name_entry.text().to_string();
        if device_name.trim().is_empty() {
            status_label.set_text("Pick a name for this device first.");
            return;
        }

        // Lock the input row while we run.
        generate_btn.set_sensitive(false);
        name_entry.set_sensitive(false);
        spinner.set_visible(true);
        spinner.start();
        status_label.set_text("Asking signal-cli for a link code…");
        *qr_state.borrow_mut() = QrState::Idle;
        qr_area.queue_draw();
        uri_label.set_text("");
        copy_for_click.set_visible(false);

        spawn_link_flow(LinkFlowHandles {
            device_name,
            window: window.clone(),
            generate_btn: generate_btn.clone(),
            name_entry: name_entry.clone(),
            copy_btn: copy_for_click.clone(),
            qr_area: qr_area.clone(),
            qr_state: qr_state.clone(),
            uri_label: uri_label.clone(),
            spinner: spinner.clone(),
            status_label: status_label.clone(),
        });
    });
}

struct LinkFlowHandles {
    device_name: String,
    window: adw::Window,
    generate_btn: gtk::Button,
    name_entry: gtk::Entry,
    copy_btn: gtk::Button,
    qr_area: gtk::DrawingArea,
    qr_state: Rc<RefCell<QrState>>,
    uri_label: gtk::Label,
    spinner: gtk::Spinner,
    status_label: gtk::Label,
}

/// Drive the full link flow off the GTK main thread:
///
///   1. Snapshot accounts (so we can detect the new one).
///   2. Call `SignalClient::link(name)` to get the URI.
///   3. Marshal the URI back to GTK so we can render the QR.
///   4. Poll `list_accounts()` every 2s until the link completes or
///      we hit `POLL_TIMEOUT`.
///   5. Marshal the final outcome back to GTK and update the UI.
///
/// We use a dedicated thread + a fresh tokio current-thread runtime to
/// match the existing pattern in `settings::spawn_version_probe`.
fn spawn_link_flow(handles: LinkFlowHandles) {
    let LinkFlowHandles {
        device_name,
        window,
        generate_btn,
        name_entry,
        copy_btn,
        qr_area,
        qr_state,
        uri_label,
        spinner,
        status_label,
    } = handles;

    let (tx, rx) = mpsc::channel::<LinkEvent>();

    // Worker thread: owns the tokio runtime, drives the bus.
    {
        let device_name = device_name.clone();
        let tx = tx.clone();
        std::thread::Builder::new()
            .name("kryptos-linker".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_link_flow(&device_name, &tx)
                }));
                if result.is_err() {
                    let _ = tx.send(LinkEvent::Error(
                        "internal error in linker worker".to_string(),
                    ));
                }
            })
            .expect("spawn kryptos-linker thread");
    }

    // GTK side: consume worker events on a glib timeout.
    let qr_area_for_tick = qr_area.clone();
    let qr_state_for_tick = qr_state.clone();
    let uri_label_for_tick = uri_label.clone();
    let spinner_for_tick = spinner.clone();
    let status_label_for_tick = status_label.clone();
    let generate_btn_for_tick = generate_btn.clone();
    let name_entry_for_tick = name_entry.clone();
    let copy_btn_for_tick = copy_btn.clone();
    let window_for_tick = window.clone();
    glib::source::timeout_add_local(Duration::from_millis(120), move || {
        loop {
            match rx.try_recv() {
                Ok(LinkEvent::Uri(uri)) => {
                    info!("got tsdevice URI; rendering QR");
                    match build_qr(&uri) {
                        Ok(matrix) => {
                            *qr_state_for_tick.borrow_mut() = QrState::Ready(matrix);
                            qr_area_for_tick.queue_draw();
                            uri_label_for_tick.set_text(&shorten_uri(&uri));
                            uri_label_for_tick.set_tooltip_text(Some(&uri));
                            status_label_for_tick.set_text("Waiting for your phone to confirm…");
                            wire_copy_button(&copy_btn_for_tick, &uri, &status_label_for_tick);
                        }
                        Err(e) => {
                            error!(error = %e, "failed to encode QR");
                            *qr_state_for_tick.borrow_mut() = QrState::Idle;
                            qr_area_for_tick.queue_draw();
                            status_label_for_tick.set_text(&format!("Couldn't render QR: {e}",));
                            unlock_inputs(&generate_btn_for_tick, &name_entry_for_tick);
                            spinner_for_tick.stop();
                            spinner_for_tick.set_visible(false);
                        }
                    }
                }
                Ok(LinkEvent::Linked(account)) => {
                    info!(%account, "linked successfully");
                    spinner_for_tick.stop();
                    spinner_for_tick.set_visible(false);
                    status_label_for_tick.set_text(&format!("Linked! Welcome, {account}.",));
                    // Give the user a beat to read the success line.
                    let win = window_for_tick.clone();
                    glib::source::timeout_add_local_once(Duration::from_millis(900), move || {
                        win.close()
                    });
                    return glib::ControlFlow::Break;
                }
                Ok(LinkEvent::AlreadyLinked(account)) => {
                    info!(%account, "linker invoked but account already linked; short-circuit");
                    spinner_for_tick.stop();
                    spinner_for_tick.set_visible(false);
                    *qr_state_for_tick.borrow_mut() = QrState::Idle;
                    qr_area_for_tick.queue_draw();
                    uri_label_for_tick.set_text("");
                    copy_btn_for_tick.set_visible(false);
                    status_label_for_tick.set_text(&format!(
                        "Already linked as {account}. You're all set — close this window to use Kryptos.",
                    ));
                    unlock_inputs(&generate_btn_for_tick, &name_entry_for_tick);
                    return glib::ControlFlow::Break;
                }
                Ok(LinkEvent::TimedOut) => {
                    warn!("link flow timed out");
                    spinner_for_tick.stop();
                    spinner_for_tick.set_visible(false);
                    status_label_for_tick
                        .set_text("Timed out waiting for your phone. Try generating a new code.");
                    unlock_inputs(&generate_btn_for_tick, &name_entry_for_tick);
                    return glib::ControlFlow::Break;
                }
                Ok(LinkEvent::Error(e)) => {
                    error!(error = %e, "link flow failed");
                    spinner_for_tick.stop();
                    spinner_for_tick.set_visible(false);
                    status_label_for_tick.set_text(&format!("Couldn't link: {e}"));
                    unlock_inputs(&generate_btn_for_tick, &name_entry_for_tick);
                    return glib::ControlFlow::Break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    spinner_for_tick.stop();
                    spinner_for_tick.set_visible(false);
                    unlock_inputs(&generate_btn_for_tick, &name_entry_for_tick);
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}

fn unlock_inputs(generate_btn: &gtk::Button, name_entry: &gtk::Entry) {
    generate_btn.set_sensitive(true);
    name_entry.set_sensitive(true);
}

/// Make `copy_btn` visible and rebind it to copy `uri` to the display
/// clipboard. We disconnect previous handlers via a fresh signal handle
/// — `gtk::Button` doesn't expose handler IDs cheaply, so we rely on
/// `connect_clicked` being idempotent for our purposes (each generate
/// pass clears + reattaches before the URI lands).
fn wire_copy_button(copy_btn: &gtk::Button, uri: &str, status_label: &gtk::Label) {
    copy_btn.set_visible(true);
    let uri = uri.to_string();
    let status_label = status_label.clone();
    copy_btn.connect_clicked(move |btn| {
        let clipboard = btn.clipboard();
        clipboard.set_text(&uri);
        status_label.set_text("URI copied to clipboard.");
    });
}

// ---------------------------------------------------------------------------
// Worker-side: actual signal-cli interaction
// ---------------------------------------------------------------------------

enum LinkEvent {
    Uri(String),
    Linked(String),
    /// signal-cli already has at least one account; the linker doesn't
    /// need to run. Carries the existing account's E.164 number so the
    /// UI can tell the user what they're already linked as.
    AlreadyLinked(String),
    TimedOut,
    Error(String),
}

fn run_link_flow(device_name: &str, tx: &mpsc::Sender<LinkEvent>) {
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = tx.send(LinkEvent::Error(format!("tokio runtime: {e}")));
            return;
        }
    };

    rt.block_on(async {
        let client = match SignalClient::connect().await {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(LinkEvent::Error(format!(
                    "couldn't reach signal-cli on D-Bus: {e}"
                )));
                return;
            }
        };

        if let Err(e) = crate::dbus::ensure_running(client.connection()).await {
            let _ = tx.send(LinkEvent::Error(format!("{e}")));
            return;
        }

        let before: std::collections::HashSet<String> = match client.list_accounts().await {
            Ok(list) => list.into_iter().collect(),
            Err(e) => {
                debug!(error = %e, "pre-link list_accounts failed; assuming empty");
                std::collections::HashSet::new()
            }
        };

        // signal-cli's `link` registers a NEW account; it does nothing
        // for an account already known to the daemon. Without this
        // short-circuit the user sees "Waiting for your phone..." for
        // 5 minutes because `detect_new_account` can never observe a
        // delta — `before` and `now` both contain the same account.
        if !before.is_empty() {
            let mut sorted: Vec<String> = before.iter().cloned().collect();
            sorted.sort();
            let account = sorted.into_iter().next().unwrap_or_default();
            let _ = tx.send(LinkEvent::AlreadyLinked(account));
            return;
        }

        let uri = match client.link(device_name).await {
            Ok(u) => u,
            Err(e) => {
                let _ = tx.send(LinkEvent::Error(format!("{e}")));
                return;
            }
        };
        let _ = tx.send(LinkEvent::Uri(uri));

        // Poll for completion.
        let deadline = tokio::time::Instant::now() + POLL_TIMEOUT;
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            if tokio::time::Instant::now() >= deadline {
                let _ = tx.send(LinkEvent::TimedOut);
                return;
            }
            match client.list_accounts().await {
                Ok(now) => match detect_new_account(&before, &now) {
                    LinkOutcome::Pending => continue,
                    LinkOutcome::Linked(account) => {
                        let _ = tx.send(LinkEvent::Linked(account));
                        return;
                    }
                },
                Err(e) => {
                    debug!(error = %e, "list_accounts during poll failed; retrying");
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// QR rendering
// ---------------------------------------------------------------------------

const QR_CARD_PX: i32 = 480;
const QR_QUIET_MODULES: usize = 4;

#[derive(Clone)]
struct QrMatrix {
    /// Side length in modules (excluding quiet zone).
    width: usize,
    /// Row-major bitmap; `true` = dark module.
    bits: Vec<bool>,
}

enum QrState {
    Idle,
    Ready(QrMatrix),
}

fn build_qr(uri: &str) -> Result<QrMatrix> {
    let code = qrcode::QrCode::new(uri.as_bytes())
        .map_err(|e| Error::Config(format!("qr encode: {e}")))?;
    let bits = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Ok(QrMatrix {
        width: code.width(),
        bits,
    })
}

fn draw_qr(cr: &gtk::cairo::Context, width: i32, height: i32, state: &QrState) {
    let w = width as f64;
    let h = height as f64;

    // Always paint a white card body so the quiet zone is real white,
    // not whatever the parent's background happens to be.
    cr.set_source_rgb(1.0, 1.0, 1.0);
    cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();

    let matrix = match state {
        QrState::Ready(m) => m,
        QrState::Idle => {
            // Subtle placeholder: a thin neutral border so the card has shape
            // before the user generates a code.
            cr.set_source_rgba(0.0, 0.0, 0.0, 0.06);
            cr.set_line_width(1.0);
            cr.rectangle(0.5, 0.5, w - 1.0, h - 1.0);
            let _ = cr.stroke();
            return;
        }
    };

    let n = matrix.width;
    if n == 0 {
        return;
    }
    let total_modules = (n + 2 * QR_QUIET_MODULES) as f64;
    // Snap the module size to whole pixels so QR squares stay crisp.
    let raw_module = (w.min(h)) / total_modules;
    let module = raw_module.floor().max(1.0);
    let painted = module * total_modules;
    let offset_x = ((w - painted) / 2.0).floor();
    let offset_y = ((h - painted) / 2.0).floor();

    cr.set_source_rgb(0.0, 0.0, 0.0);
    for y in 0..n {
        for x in 0..n {
            if matrix.bits[y * n + x] {
                let px = offset_x + (QR_QUIET_MODULES + x) as f64 * module;
                let py = offset_y + (QR_QUIET_MODULES + y) as f64 * module;
                cr.rectangle(px, py, module, module);
            }
        }
    }
    let _ = cr.fill();
}

fn shorten_uri(uri: &str) -> String {
    // Pretty hint of the URI under the QR. Full string is in tooltip
    // and selectable, so users can copy-paste if needed.
    if uri.len() <= 64 {
        return uri.to_string();
    }
    format!("{}…{}", &uri[..40], &uri[uri.len() - 12..])
}

// ---------------------------------------------------------------------------
// Defaults / styling
// ---------------------------------------------------------------------------

fn default_device_name() -> String {
    let host = hostname().unwrap_or_else(|| "linux".to_string());
    format!("kryptos-{host}")
}

/// Best-effort hostname read; we don't pull in a crate just for this.
fn hostname() -> Option<String> {
    if let Ok(h) = std::env::var("HOSTNAME") {
        if !h.trim().is_empty() {
            return Some(sanitize_hostname(&h));
        }
    }
    if let Ok(bytes) = std::fs::read("/etc/hostname") {
        if let Ok(s) = std::str::from_utf8(&bytes) {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(sanitize_hostname(trimmed));
            }
        }
    }
    None
}

fn sanitize_hostname(raw: &str) -> String {
    // signal-cli imposes a 50-char cap; keep some headroom for the prefix.
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect()
}

fn install_linker_styles() {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }

    let provider = gtk::CssProvider::new();
    provider.load_from_string(LINKER_STYLES);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
}

/// Structural styles for the linker. Colour comes from the active
/// palette (`@kryptos_*` tokens); these rules just set sizes, paddings,
/// type weights so the linker reads as Swiss minimal: pristine QR card,
/// generous quiet zone, hero title in monospace, no decorative shadow.
const LINKER_STYLES: &str = r#"
.kryptos-linker-hero {
    /* Anchor the hero column to the left so the QR card can centre
       independently. Width clamped to 480px via builder. */
}
.kryptos-linker-title {
    font-size: 28px;
    font-weight: 600;
    letter-spacing: -0.01em;
    margin-top: 4px;
}
.kryptos-linker-body {
    font-size: 14px;
    line-height: 1.5;
}
.kryptos-qr-card {
    /* Generous quiet zone: 32px on all sides. The DrawingArea paints
       its own white plate inside, so this padding is the white halo. */
    padding: 32px;
}
.kryptos-qr-canvas {
    background-color: white;
}
.kryptos-linker-uri {
    font-size: 12px;
    letter-spacing: 0;
    margin-top: 4px;
}
.kryptos-linker-name {
    padding: 10px 12px;
    border-radius: 4px;
    font-size: 13px;
}
.kryptos-linker-status {
    font-size: 13px;
    line-height: 1.5;
}
.kryptos-linker-copy {
    min-width: 36px;
    min-height: 36px;
    padding: 0;
    border-radius: 4px;
    background: transparent;
    transition: background-color 100ms ease-out;
}
.kryptos-linker-copy:hover {
    background-color: alpha(currentColor, 0.06);
}
/* Square primary action — Swiss aesthetic, not a pill. Thin border,
   bold weight, snaps to a 56-wide / 40-tall block. */
button.suggested-action.kryptos-linker-generate {
    padding: 0 18px;
    border-radius: 4px;
    border: 1px solid alpha(currentColor, 0.20);
    font-weight: 700;
    letter-spacing: 0.04em;
    min-height: 40px;
}
button.suggested-action.kryptos-linker-generate:hover {
    background-color: alpha(currentColor, 0.06);
}
"#;

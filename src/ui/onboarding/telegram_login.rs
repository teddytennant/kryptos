//! Interactive Telegram login dialog.
//!
//! Walks the user through grammers' multi-step authentication:
//!
//!   1. **API credentials** — `api_id` (i32) + `api_hash` (string).
//!      Skipped when both are already non-default in `[backends.telegram]`.
//!   2. **Phone number**    — E.164 entry → `request_login`.
//!   3. **Verification code** — 6-digit entry → `submit_code`. Branches
//!      to step 4 only when grammers reports `NeedsPassword(true)`.
//!   4. **2FA password**    — password entry → `submit_password`.
//!   5. **Linked**          — confirmation panel; "Continue" closes.
//!
//! The dialog is a small modal `adw::Window` over the parent. Each step
//! lives on an `adw::NavigationView` page with a flat header and a
//! shared `adw::ToastOverlay` for inline errors. All grammers calls run
//! on a fresh single-thread tokio runtime, off the GTK main loop, with
//! results marshalled back via `glib::idle_add_local_once`.
//!
//! Persistence: when API credentials are entered we write them to
//! `~/.config/kryptos/config.toml` (and flip `[backends.telegram].enabled
//! = true`) before the network call so a crash leaves the user a config
//! they can reuse on the next run.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc;

use adw::prelude::*;
use gtk::glib;
use tracing::{error, info, warn};

use crate::config::loader;
use crate::messenger::telegram::{resolve_session_path, NeedsPassword, TelegramBackend};

/// Optional pre-existing backend handle. The welcome flow doesn't have
/// one (it's logging in for the first time), but a future "switch
/// account" entry point could pass an already-`open()`ed backend here.
pub type ExistingBackend = Option<std::sync::Arc<TelegramBackend>>;

/// Build and present the Telegram login dialog over `parent`.
///
/// Non-blocking: returns once the window is on screen. The flow
/// completes (or aborts) on the user's actions.
pub fn present(parent: &impl IsA<gtk::Window>, _existing: ExistingBackend) {
    install_styles();

    let cfg_path = match loader::default_path() {
        Ok(p) => p,
        Err(e) => {
            error!(error = %e, "telegram-login: cannot resolve config path");
            return;
        }
    };
    let cfg = loader::load_or_default(&cfg_path).unwrap_or_else(|e| {
        warn!(error = %e, "telegram-login: load failed; using defaults");
        crate::config::Config::default()
    });

    // The shared, mutable state every page handler reads / writes. We
    // funnel grammers calls + the eventual `Arc<TelegramBackend>` through
    // here so `dyn Fn` closures can drop into the same Rc<RefCell>.
    let state: Rc<RefCell<LoginState>> = Rc::new(RefCell::new(LoginState {
        api_id: cfg.backends.telegram.api_id,
        api_hash: cfg.backends.telegram.api_hash.clone(),
        session_path: resolve_session_path(&cfg.backends.telegram.session_path),
        backend: None,
    }));

    let win = adw::Window::builder()
        .transient_for(parent.as_ref())
        .modal(true)
        .default_width(520)
        .default_height(560)
        .title("Set up Telegram")
        .build();
    win.add_css_class("kryptos-telegram-login");

    let header = adw::HeaderBar::builder().show_title(false).build();
    header.add_css_class("flat");

    let nav = adw::NavigationView::new();
    nav.add_css_class("kryptos-telegram-login");

    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&nav));

    // Build pages in order.
    nav.add(&page_credentials(&nav, &toast_overlay, state.clone(), cfg_path.clone()));
    nav.add(&page_phone(&nav, &toast_overlay, state.clone()));
    nav.add(&page_code(&nav, &toast_overlay, state.clone()));
    nav.add(&page_password(&nav, &toast_overlay, state.clone()));
    nav.add(&page_done(&win));

    // Skip the credentials step when both id + hash are already set.
    let need_credentials = state.borrow().api_id == 0 || state.borrow().api_hash.is_empty();
    if !need_credentials {
        nav.replace_with_tags(&["phone"]);
    }

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&toast_overlay));
    win.set_content(Some(&toolbar));

    win.present();
}

/// Cross-page mutable state. Field names mirror the grammers vocabulary
/// so the wire trace lines up with the type names.
struct LoginState {
    api_id: i32,
    api_hash: String,
    session_path: PathBuf,
    backend: Option<std::sync::Arc<TelegramBackend>>,
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn page_credentials(
    nav: &adw::NavigationView,
    toast_overlay: &adw::ToastOverlay,
    state: Rc<RefCell<LoginState>>,
    cfg_path: PathBuf,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Telegram credentials");
    let body = hero_body(
        "Grab your API id + hash from \
         https://my.telegram.org/apps. We store them locally in \
         ~/.config/kryptos/config.toml.",
    );

    let id_entry = gtk::Entry::builder()
        .placeholder_text("api_id (number)")
        .hexpand(true)
        .input_purpose(gtk::InputPurpose::Digits)
        .build();
    id_entry.add_css_class("monospace");
    if state.borrow().api_id != 0 {
        id_entry.set_text(&state.borrow().api_id.to_string());
    }

    let hash_entry = gtk::Entry::builder()
        .placeholder_text("api_hash (32 hex characters)")
        .hexpand(true)
        .build();
    hash_entry.add_css_class("monospace");
    if !state.borrow().api_hash.is_empty() {
        hash_entry.set_text(&state.borrow().api_hash);
    }

    let field_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .halign(gtk::Align::Center)
        .build();
    field_box.set_size_request(360, -1);
    field_box.append(&id_entry);
    field_box.append(&hash_entry);

    let cta = primary_cta("Save and continue");
    {
        let nav = nav.clone();
        let state = state.clone();
        let id_entry = id_entry.clone();
        let hash_entry = hash_entry.clone();
        let toast_overlay = toast_overlay.clone();
        cta.connect_clicked(move |btn| {
            let raw_id = id_entry.text().to_string();
            let raw_hash = hash_entry.text().trim().to_string();
            let parsed_id: i32 = match raw_id.trim().parse() {
                Ok(n) if n > 0 => n,
                _ => {
                    toast(&toast_overlay, "api_id must be a positive integer");
                    return;
                }
            };
            if raw_hash.len() < 16 || !raw_hash.chars().all(|c| c.is_ascii_hexdigit()) {
                toast(&toast_overlay, "api_hash must be a hex string");
                return;
            }
            // Persist to disk before the network call so a crash here
            // leaves the user a working config.
            if let Err(e) = persist_credentials(&cfg_path, parsed_id, &raw_hash) {
                error!(error = %e, "telegram-login: persist credentials failed");
                toast(
                    &toast_overlay,
                    &format!("Couldn't save credentials: {e}"),
                );
                return;
            }
            {
                let mut st = state.borrow_mut();
                st.api_id = parsed_id;
                st.api_hash = raw_hash;
            }
            btn.set_sensitive(true);
            nav.push_by_tag("phone");
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&field_box);
    column.append(&cta);

    wrap_page("credentials", "Credentials", column.upcast())
}

fn page_phone(
    nav: &adw::NavigationView,
    toast_overlay: &adw::ToastOverlay,
    state: Rc<RefCell<LoginState>>,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Phone number");
    let body = hero_body(
        "We'll text a 6-digit code to this number. Use the international \
         format with a leading +.",
    );

    let entry = gtk::Entry::builder()
        .placeholder_text("+15551234567")
        .hexpand(true)
        .input_purpose(gtk::InputPurpose::Phone)
        .build();
    entry.add_css_class("monospace");
    entry.set_size_request(280, -1);
    entry.set_halign(gtk::Align::Center);

    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let cta = primary_cta("Send code");

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    row.append(&spinner);
    row.append(&cta);

    {
        let nav = nav.clone();
        let state = state.clone();
        let entry = entry.clone();
        let cta_inside = cta.clone();
        let spinner = spinner.clone();
        let toast_overlay = toast_overlay.clone();
        cta.connect_clicked(move |_| {
            let phone = entry.text().trim().to_string();
            if !phone.starts_with('+') || phone.len() < 7 {
                toast(&toast_overlay, "Use E.164 (e.g. +15551234567)");
                return;
            }
            cta_inside.set_sensitive(false);
            entry.set_sensitive(false);
            spinner.set_visible(true);
            spinner.start();

            let (api_id, api_hash, session_path) = {
                let st = state.borrow();
                (st.api_id, st.api_hash.clone(), st.session_path.clone())
            };

            run_async(move |tx| {
                let backend = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(async move {
                        match TelegramBackend::open(api_id, &api_hash, &session_path).await {
                            Ok(b) => {
                                let arc = std::sync::Arc::new(b);
                                if let Err(e) = arc.request_login(&phone).await {
                                    return Err(format!("{e}"));
                                }
                                Ok(arc)
                            }
                            Err(e) => Err(format!("{e}")),
                        }
                    }),
                    Err(e) => Err(format!("tokio: {e}")),
                };
                let _ = tx.send(backend.map(LoginStep::PhoneAccepted).map_err(|e| e));
            });

            // Glib-side consumer.
            let nav = nav.clone();
            let state = state.clone();
            let cta = cta_inside.clone();
            let entry = entry.clone();
            let spinner = spinner.clone();
            let toast_overlay = toast_overlay.clone();
            poll_step(move |result| {
                spinner.stop();
                spinner.set_visible(false);
                cta.set_sensitive(true);
                entry.set_sensitive(true);
                match result {
                    Ok(LoginStep::PhoneAccepted(backend)) => {
                        state.borrow_mut().backend = Some(backend);
                        nav.push_by_tag("code");
                    }
                    Ok(_) => warn!("phone step received non-PhoneAccepted result"),
                    Err(e) => toast(&toast_overlay, &format!("Couldn't send code: {e}")),
                }
            });
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&entry);
    column.append(&row);

    wrap_page("phone", "Phone", column.upcast())
}

fn page_code(
    nav: &adw::NavigationView,
    toast_overlay: &adw::ToastOverlay,
    state: Rc<RefCell<LoginState>>,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Verification code");
    let body = hero_body("Enter the 6-digit code Telegram just sent.");

    let entry = gtk::Entry::builder()
        .placeholder_text("123 456")
        .hexpand(true)
        .max_length(8)
        .input_purpose(gtk::InputPurpose::Digits)
        .build();
    entry.add_css_class("monospace");
    entry.set_size_request(220, -1);
    entry.set_halign(gtk::Align::Center);

    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let cta = primary_cta("Confirm");

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    row.append(&spinner);
    row.append(&cta);

    {
        let nav = nav.clone();
        let state = state.clone();
        let entry = entry.clone();
        let cta_inside = cta.clone();
        let spinner = spinner.clone();
        let toast_overlay = toast_overlay.clone();
        cta.connect_clicked(move |_| {
            let code: String = entry
                .text()
                .chars()
                .filter(char::is_ascii_digit)
                .collect();
            if code.is_empty() {
                toast(&toast_overlay, "Enter the code from your phone");
                return;
            }
            let backend = match state.borrow().backend.clone() {
                Some(b) => b,
                None => {
                    toast(&toast_overlay, "Login state lost. Restart the flow.");
                    return;
                }
            };
            cta_inside.set_sensitive(false);
            entry.set_sensitive(false);
            spinner.set_visible(true);
            spinner.start();

            run_async(move |tx| {
                let result = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(async move {
                        match backend.submit_code(&code).await {
                            Ok(NeedsPassword(true)) => Ok(LoginStep::CodeNeedsPassword),
                            Ok(NeedsPassword(false)) => match backend.save_session().await {
                                Ok(()) => Ok(LoginStep::CodeAcceptedNoPassword),
                                Err(e) => Err(format!("save_session: {e}")),
                            },
                            Err(e) => Err(format!("{e}")),
                        }
                    }),
                    Err(e) => Err(format!("tokio: {e}")),
                };
                let _ = tx.send(result);
            });

            let nav = nav.clone();
            let cta = cta_inside.clone();
            let entry = entry.clone();
            let spinner = spinner.clone();
            let toast_overlay = toast_overlay.clone();
            poll_step(move |result| {
                spinner.stop();
                spinner.set_visible(false);
                cta.set_sensitive(true);
                entry.set_sensitive(true);
                match result {
                    Ok(LoginStep::CodeNeedsPassword) => {
                        nav.push_by_tag("password");
                    }
                    Ok(LoginStep::CodeAcceptedNoPassword) => {
                        nav.push_by_tag("done");
                    }
                    Ok(_) => warn!("code step received unexpected result"),
                    Err(e) => toast(&toast_overlay, &format!("Couldn't verify code: {e}")),
                }
            });
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&entry);
    column.append(&row);

    wrap_page("code", "Code", column.upcast())
}

fn page_password(
    nav: &adw::NavigationView,
    toast_overlay: &adw::ToastOverlay,
    state: Rc<RefCell<LoginState>>,
) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Two-factor password");
    let body = hero_body(
        "Your account has 2FA enabled. Enter the cloud password you set in \
         Telegram → Settings → Privacy and Security.",
    );

    let entry = gtk::PasswordEntry::builder()
        .placeholder_text("Cloud password")
        .hexpand(true)
        .show_peek_icon(true)
        .build();
    entry.set_size_request(280, -1);
    entry.set_halign(gtk::Align::Center);

    let spinner = gtk::Spinner::new();
    spinner.set_visible(false);
    let cta = primary_cta("Sign in");

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    row.append(&spinner);
    row.append(&cta);

    {
        let nav = nav.clone();
        let state = state.clone();
        let entry = entry.clone();
        let cta_inside = cta.clone();
        let spinner = spinner.clone();
        let toast_overlay = toast_overlay.clone();
        cta.connect_clicked(move |_| {
            let password = entry.text().to_string();
            if password.is_empty() {
                toast(&toast_overlay, "Enter your 2FA password");
                return;
            }
            let backend = match state.borrow().backend.clone() {
                Some(b) => b,
                None => {
                    toast(&toast_overlay, "Login state lost. Restart the flow.");
                    return;
                }
            };
            cta_inside.set_sensitive(false);
            entry.set_sensitive(false);
            spinner.set_visible(true);
            spinner.start();

            run_async(move |tx| {
                let result = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt.block_on(async move {
                        match backend.submit_password(&password).await {
                            Ok(()) => match backend.save_session().await {
                                Ok(()) => Ok(LoginStep::PasswordAccepted),
                                Err(e) => Err(format!("save_session: {e}")),
                            },
                            Err(e) => Err(format!("{e}")),
                        }
                    }),
                    Err(e) => Err(format!("tokio: {e}")),
                };
                let _ = tx.send(result);
            });

            let nav = nav.clone();
            let cta = cta_inside.clone();
            let entry = entry.clone();
            let spinner = spinner.clone();
            let toast_overlay = toast_overlay.clone();
            poll_step(move |result| {
                spinner.stop();
                spinner.set_visible(false);
                cta.set_sensitive(true);
                entry.set_sensitive(true);
                match result {
                    Ok(LoginStep::PasswordAccepted) => {
                        nav.push_by_tag("done");
                    }
                    Ok(_) => warn!("password step received unexpected result"),
                    Err(e) => toast(&toast_overlay, &format!("Couldn't sign in: {e}")),
                }
            });
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&entry);
    column.append(&row);

    wrap_page("password", "Password", column.upcast())
}

fn page_done(window: &adw::Window) -> adw::NavigationPage {
    let column = column_with_breathing_room();

    let title = hero_title("Linked to Telegram.");
    let body = hero_body(
        "Your dialogs will start populating shortly. Restart Kryptos if \
         the sidebar stays empty.",
    );

    let cta = primary_cta("Continue");
    {
        let win = window.clone();
        cta.connect_clicked(move |_| {
            win.close();
        });
    }

    column.append(&title);
    column.append(&body);
    column.append(&spacer(8));
    column.append(&cta);

    wrap_page("done", "Done", column.upcast())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

enum LoginStep {
    PhoneAccepted(std::sync::Arc<TelegramBackend>),
    CodeNeedsPassword,
    CodeAcceptedNoPassword,
    PasswordAccepted,
}

/// Run `f` on a fresh worker thread. `f` gets a `mpsc::Sender` and is
/// expected to send exactly one `Result<LoginStep, String>` back.
fn run_async<F>(f: F)
where
    F: FnOnce(mpsc::Sender<Result<LoginStep, String>>) + Send + 'static,
{
    let tx = THREAD_LOCAL_RX.with(|cell| {
        let (tx, rx) = mpsc::channel();
        *cell.borrow_mut() = Some(rx);
        tx
    });
    std::thread::Builder::new()
        .name("kryptos-tg-login".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(tx.clone())));
            if result.is_err() {
                let _ = tx.send(Err("internal error in telegram login worker".into()));
            }
        })
        .expect("spawn telegram login worker");
}

/// Glib timeout that drains the worker channel handed to the most
/// recent `run_async`. Calls `done(result)` on the GTK thread when the
/// worker reports back.
fn poll_step<F>(done: F)
where
    F: Fn(Result<LoginStep, String>) + 'static,
{
    let rx = THREAD_LOCAL_RX
        .with(|cell| cell.borrow_mut().take())
        .expect("poll_step called without a matching run_async");
    let done = std::rc::Rc::new(done);
    glib::source::timeout_add_local(std::time::Duration::from_millis(120), move || {
        match rx.try_recv() {
            Ok(result) => {
                done(result);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

thread_local! {
    /// Hand-off slot between `run_async` and the immediately-following
    /// `poll_step` on the same UI handler. One pending receiver at a
    /// time per thread; pages always call them as a pair.
    static THREAD_LOCAL_RX: RefCell<Option<mpsc::Receiver<Result<LoginStep, String>>>> = const { RefCell::new(None) };
}

fn toast(overlay: &adw::ToastOverlay, msg: &str) {
    info!(message = %msg, "telegram-login: toast");
    let toast = adw::Toast::builder()
        .title(msg)
        .timeout(4)
        .priority(adw::ToastPriority::High)
        .build();
    overlay.add_toast(toast);
}

fn wrap_page(tag: &str, title: &str, content: gtk::Widget) -> adw::NavigationPage {
    let header = adw::HeaderBar::builder().show_title(false).build();
    header.add_css_class("flat");

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();
    scroller.set_child(Some(&content));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroller));

    adw::NavigationPage::builder()
        .title(title)
        .tag(tag)
        .child(&toolbar)
        .build()
}

fn column_with_breathing_room() -> gtk::Box {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(20)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    column.set_margin_start(48);
    column.set_margin_end(48);
    column.set_margin_top(64);
    column.set_margin_bottom(48);
    column
}

fn hero_title(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("kryptos-tg-title");
    l.set_halign(gtk::Align::Center);
    l.set_justify(gtk::Justification::Center);
    l
}

fn hero_body(text: &str) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .justify(gtk::Justification::Center)
        .halign(gtk::Align::Center)
        .max_width_chars(56)
        .build();
    l.add_css_class("kryptos-tg-body");
    l
}

fn primary_cta(text: &str) -> gtk::Button {
    let b = gtk::Button::with_label(text);
    b.add_css_class("suggested-action");
    b.add_css_class("kryptos-tg-cta");
    b.set_halign(gtk::Align::Center);
    b.set_size_request(160, 40);
    b
}

fn spacer(px: i32) -> gtk::Widget {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .height_request(px)
        .build()
        .upcast()
}

/// Persist credentials to the on-disk config and flip
/// `[backends.telegram].enabled = true` so future starts pick up the
/// session without manual TOML edits.
fn persist_credentials(path: &PathBuf, api_id: i32, api_hash: &str) -> crate::core::Result<()> {
    let mut cfg = loader::load_or_default(path)?;
    cfg.backends.telegram.enabled = true;
    cfg.backends.telegram.api_id = api_id;
    cfg.backends.telegram.api_hash = api_hash.to_string();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(&cfg)?;
    std::fs::write(path, body)?;
    Ok(())
}

fn install_styles() {
    use std::sync::OnceLock;
    static INSTALLED: OnceLock<()> = OnceLock::new();
    if INSTALLED.set(()).is_err() {
        return;
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLES);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
        );
    }
}

const STYLES: &str = r#"
.kryptos-tg-title {
    font-size: 28px;
    font-weight: 600;
    letter-spacing: -0.01em;
    font-feature-settings: "tnum";
}
.kryptos-tg-body {
    font-size: 14px;
    line-height: 1.55;
    opacity: 0.78;
}
button.suggested-action.kryptos-tg-cta {
    padding: 0 18px;
    border-radius: 4px;
    border: 1px solid alpha(currentColor, 0.20);
    font-weight: 700;
    letter-spacing: 0.04em;
}
"#;

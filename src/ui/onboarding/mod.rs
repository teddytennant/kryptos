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

pub use link_state::{detect_new_account, LinkOutcome};

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

        // Hero copy block.
        let title = gtk::Label::builder()
            .label("Link to Signal")
            .halign(gtk::Align::Start)
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

        // Device name input + Generate button row.
        let name_entry = gtk::Entry::builder()
            .placeholder_text("Device name")
            .text(default_device_name())
            .hexpand(true)
            .build();
        name_entry.add_css_class("kryptos-linker-name");

        let generate_btn = gtk::Button::with_label("Generate code");
        generate_btn.add_css_class("suggested-action");
        generate_btn.add_css_class("pill");

        let input_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::Fill)
            .build();
        input_row.append(&name_entry);
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
        body_box.append(&title);
        body_box.append(&body);
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
        qr_area,
        qr_state,
        uri_label,
        spinner,
        status_label,
    } = args;

    let btn = generate_btn.clone();
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

        spawn_link_flow(LinkFlowHandles {
            device_name,
            window: window.clone(),
            generate_btn: generate_btn.clone(),
            name_entry: name_entry.clone(),
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

// ---------------------------------------------------------------------------
// Worker-side: actual signal-cli interaction
// ---------------------------------------------------------------------------

enum LinkEvent {
    Uri(String),
    Linked(String),
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

const QR_CARD_PX: i32 = 320;
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
.kryptos-linker-title {
    font-size: 28px;
    font-weight: 600;
    letter-spacing: -0.01em;
    margin-top: 4px;
}
.kryptos-linker-body {
    font-size: 14px;
    line-height: 1.5;
    max-width: 460px;
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
button.suggested-action.pill {
    padding: 14px 32px;
    border-radius: 999px;
}
"#;

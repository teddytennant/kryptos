//! Construction of the main `adw::ApplicationWindow` and its layout.
//! The window is a vertical stack:
//!
//! ```text
//! ┌────────────────────────────────────────────┐
//! │  OverlaySplitView                          │
//! │  ┌─────────────┬────────────────────────┐  │
//! │  │  Sidebar    │  Content               │  │
//! │  │  (chats)    │  (messages + composer) │  │
//! │  └─────────────┴────────────────────────┘  │
//! ├────────────────────────────────────────────┤
//! │  command bar (hidden by default)           │
//! ├────────────────────────────────────────────┤
//! │  ▎ NORMAL                                  │
//! └────────────────────────────────────────────┘
//! ```

use adw::prelude::*;

use crate::config::Config;

use super::composer::Composer;
use super::onboarding;
use super::settings::Settings;
use super::statusline::{CommandBar, ModeLine};

/// Pieces of the window the rest of the UI layer needs to talk to.
pub struct WindowParts {
    pub window: adw::ApplicationWindow,
    pub sidebar_list: gtk::ListBox,
    pub composer: Composer,
    pub mode_line: ModeLine,
    pub command_bar: CommandBar,
    pub toast_overlay: adw::ToastOverlay,
}

/// Static placeholder rows: (display name, last-message preview, timestamp).
/// Real data comes from the cache once the conversation list is wired up;
/// this is purely for visual rhythm at design time.
const PLACEHOLDER_CHATS: &[(&str, &str, &str)] = &[
    ("Family", "On our way home now.", "12:42"),
    ("Work", "ack — pushing the patch", "11:08"),
    ("Linux Linux Linux", "btw arch is now self-hosting", "Mon"),
];
const PLACEHOLDER_MESSAGES: &[(bool, &str, &str)] = &[
    (false, "Hey, did you see the new Kryptos release?", "12:38"),
    (true, "Not yet — what's in it?", "12:39"),
    (false, "Vim modes everywhere. It's wonderful.", "12:40"),
    (true, "Of course it is.", "12:41"),
    (false, "Try `:help` once you boot it.", "12:42"),
];

pub fn build(app: &adw::Application, cfg: &Config) -> WindowParts {
    // Force JetBrains Mono Nerd Font as the default app face *before* any
    // widget gets sized. Pango caches font extents per widget, so doing
    // this after `build_*` would leave existing widgets at the old metric.
    apply_default_font();

    let sidebar_list = build_sidebar_list();
    let sidebar_search = build_sidebar_search();
    let sidebar = build_sidebar(&sidebar_list, &sidebar_search);
    let (content, composer, prefs_button, link_button) = build_content();

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar)
        .content(&content)
        .show_sidebar(true)
        .min_sidebar_width(280.0)
        .max_sidebar_width(360.0)
        .sidebar_width_fraction(0.26)
        .build();
    split.set_vexpand(true);

    let mode_line = ModeLine::new();
    let command_bar = CommandBar::new();

    // ToastOverlay wraps the split view so toasts hover over the chat
    // body without overlapping the command bar / mode line.
    let toast_overlay = adw::ToastOverlay::new();
    toast_overlay.set_child(Some(&split));
    toast_overlay.set_vexpand(true);

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    root.append(&toast_overlay);
    root.append(command_bar.widget());
    root.append(mode_line.widget());

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Kryptos")
        .default_width(1100)
        .default_height(720)
        .content(&root)
        .build();

    if cfg.general.start_maximized {
        window.set_maximized(true);
    }

    let win_for_prefs = window.clone();
    prefs_button.connect_clicked(move |_| Settings::open(&win_for_prefs));

    let win_for_link = window.clone();
    link_button.connect_clicked(move |_| onboarding::open_linker(&win_for_link));

    install_styles();

    WindowParts {
        window,
        sidebar_list,
        composer,
        mode_line,
        command_bar,
        toast_overlay,
    }
}

fn build_sidebar_list() -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("navigation-sidebar");

    for (name, preview, ts) in PLACEHOLDER_CHATS {
        list.append(&chat_row(name, preview, ts));
    }

    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }

    list
}

/// Two-line designed chat row: 36px tinted avatar disk with the contact's
/// initial, name (semibold) + timestamp on the top line, last-message
/// preview (dim) on the bottom. `chat-row`-class is on the outer row so
/// the palette CSS can paint hover, selection, and the accent stripe.
fn chat_row(name: &str, preview: &str, timestamp: &str) -> gtk::ListBoxRow {
    let initial = name
        .chars()
        .next()
        .unwrap_or('?')
        .to_uppercase()
        .to_string();
    let avatar = gtk::Label::builder()
        .label(&initial)
        .xalign(0.5)
        .yalign(0.5)
        .width_chars(2)
        .build();
    avatar.add_css_class("chat-avatar");
    avatar.set_size_request(36, 36);

    let name_label = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .hexpand(true)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    name_label.add_css_class("chat-name");

    let ts_label = gtk::Label::builder().label(timestamp).xalign(1.0).build();
    ts_label.add_css_class("chat-timestamp");

    let top_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    top_row.append(&name_label);
    top_row.append(&ts_label);

    let preview_label = gtk::Label::builder()
        .label(preview)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    preview_label.add_css_class("chat-preview");

    let text_col = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    text_col.append(&top_row);
    text_col.append(&preview_label);

    let row_inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .build();
    row_inner.append(&avatar);
    row_inner.append(&text_col);

    let row = gtk::ListBoxRow::new();
    row.add_css_class("kryptos-chat-row");
    row.set_child(Some(&row_inner));
    row
}

/// Sidebar search entry: hidden by default; the `/`-mode dispatcher (or
/// future search wiring) reveals it. The widget is created here so the
/// palette CSS can target it via the `sidebar-search` class.
fn build_sidebar_search() -> gtk::SearchEntry {
    let entry = gtk::SearchEntry::builder()
        .placeholder_text("Search chats")
        .visible(false)
        .build();
    entry.add_css_class("sidebar-search");
    entry
}

fn build_sidebar(list: &gtk::ListBox, search: &gtk::SearchEntry) -> gtk::Widget {
    let title = adw::WindowTitle::new("Chats", "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();
    header.add_css_class("flat");
    header.add_css_class("sidebar-header");

    let compose_btn = gtk::Button::from_icon_name("list-add-symbolic");
    compose_btn.set_tooltip_text(Some("New chat"));
    compose_btn.add_css_class("flat");
    header.pack_end(&compose_btn);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(list)
        .build();

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    body.append(search);
    body.append(&scroller);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    toolbar.upcast::<gtk::Widget>()
}

fn build_content() -> (gtk::Widget, Composer, gtk::Button, gtk::Button) {
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new(PLACEHOLDER_CHATS[0].0, ""))
        .build();
    header.add_css_class("flat");

    let prefs_button = gtk::Button::from_icon_name("open-menu-symbolic");
    prefs_button.set_tooltip_text(Some("Preferences"));
    prefs_button.add_css_class("flat");

    let link_button = gtk::Button::from_icon_name("phone-symbolic");
    link_button.set_tooltip_text(Some("Link new device"));
    link_button.add_css_class("flat");

    header.pack_end(&prefs_button);
    header.pack_end(&link_button);

    // 8px grid: 32 sides, 24 top, 16 bottom. The composer carries its
    // own breathing room so the message tail doesn't crowd against it.
    let messages_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    messages_box.add_css_class("kryptos-messages");
    messages_box.set_margin_start(32);
    messages_box.set_margin_end(32);
    messages_box.set_margin_top(24);
    messages_box.set_margin_bottom(16);

    for (mine, body, ts) in PLACEHOLDER_MESSAGES {
        messages_box.append(&message_row(*mine, body, ts));
    }

    let messages_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&messages_box)
        .build();

    // Swiss restraint: no card, no shadow around the composer. A single
    // hairline rule above is enough to anchor it. The mode tint comes
    // from a leading-edge stripe painted via `.composer-{normal,insert,…}`.
    let composer = Composer::new();
    let composer_host = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    composer_host.add_css_class("kryptos-composer-host");
    composer_host.append(composer.widget());

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(&messages_scroll);
    body.append(&composer_host);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    (
        toolbar.upcast::<gtk::Widget>(),
        composer,
        prefs_button,
        link_button,
    )
}

/// One message bubble in a row. Real chat apps don't shout "THEM" /
/// "YOU" before every message — alignment + bubble color carry the
/// authorship; the timestamp surfaces only on hover via CSS.
fn message_row(mine: bool, body: &str, timestamp: &str) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(body)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .build();
    label.add_css_class("bubble");
    label.add_css_class(if mine { "bubble-mine" } else { "bubble-theirs" });
    label.set_tooltip_text(Some(timestamp));

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    row.add_css_class("message-row");
    row.set_halign(if mine {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });
    row.append(&label);
    row.upcast::<gtk::Widget>()
}

/// JetBrains Mono Nerd Font as the system face. Setting `gtk-font-name`
/// on `gtk::Settings` makes every default-font widget — labels, entries,
/// buttons, tooltips — pick it up without per-widget CSS.
fn apply_default_font() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_property("gtk-font-name", "JetBrainsMono Nerd Font 11");
    }
}

/// Inject the structural stylesheet — sizes, paddings, hairlines, type
/// scale. Colour comes entirely from `@kryptos_*` palette tokens defined
/// in `src/theme/css/*.css`, so this layer is theme-agnostic.
///
/// We register at `PRIORITY_APPLICATION - 1` so the active palette (added
/// later at `PRIORITY_APPLICATION` by `ThemeManager`) wins on conflicts.
/// The relationship is a clean lattice: skeleton (this) < skin (palette)
/// < user override (custom-css at +1).
fn install_styles() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLES);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION - 1,
        );
    }
}

/// Structural stylesheet — Swiss/Bauhaus discipline. Type scale:
/// `xs=11`, `sm=12`, `md=13` (chat body), `lg=15` (chat title),
/// `xl=20` (linker hero). Weights: 400 / 500 / 600 / 700. Hairlines
/// use `alpha(currentColor, 0.06)` so they tint with the palette `fg`.
const STYLES: &str = r#"
/* ── Typography baseline ───────────────────────────────────────────── */
* {
    font-family: "JetBrainsMono Nerd Font", "JetBrains Mono", monospace;
}
window {
    font-size: 13px;
}
/* Tabular figures wherever digits appear so columns never shift. */
.chat-timestamp,
.kryptos-modeline,
.modeline-mode-block,
.modeline-section,
.command-bar,
.command-bar-entry,
.message-timestamp {
    font-feature-settings: "tnum" 1, "ss20" 1;
}

/* ── Header chrome: disappears ─────────────────────────────────────── */
headerbar.flat {
    background: transparent;
    box-shadow: none;
    border: none;
    min-height: 40px;
}
headerbar windowtitle {
    font-weight: 600;
    font-size: 15px;
    letter-spacing: -0.005em;
}
headerbar windowtitle .subtitle {
    /* No subtitle anywhere — the chat name carries weight on its own. */
    font-size: 0;
    margin: 0;
    padding: 0;
}
headerbar button.flat {
    border-radius: 6px;
    min-width: 32px;
    min-height: 32px;
    padding: 0;
    background: transparent;
    transition: background-color 120ms ease-out;
}
headerbar button.flat:hover {
    background-color: alpha(currentColor, 0.06);
}
headerbar button.flat:active {
    background-color: alpha(currentColor, 0.10);
}
windowcontrols button {
    min-width: 28px;
    min-height: 28px;
}

/* ── Sidebar header: small caps "Chats" ────────────────────────────── */
.sidebar-header {
    border-bottom: 1px solid alpha(currentColor, 0.06);
}
.sidebar-header windowtitle {
    font-size: 12px;
    font-weight: 600;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    opacity: 0.55;
}

/* ── Sidebar list & chat rows ──────────────────────────────────────── */
.sidebar-search {
    margin: 8px 16px 8px;
    padding: 6px 10px;
    border-radius: 4px;
    font-size: 13px;
}
.kryptos-chat-row {
    padding: 14px 16px;
    transition: background-color 100ms ease-out;
    border-left: 2px solid transparent;
}
.kryptos-chat-row:hover {
    background-color: alpha(currentColor, 0.04);
}
.kryptos-chat-row .chat-avatar {
    min-width: 36px;
    min-height: 36px;
    border-radius: 18px;
    font-size: 13px;
    font-weight: 600;
}
.kryptos-chat-row .chat-name {
    font-size: 13px;
    font-weight: 600;
    letter-spacing: -0.005em;
}
.kryptos-chat-row .chat-preview {
    font-size: 12px;
    font-weight: 400;
    opacity: 0.55;
}
.kryptos-chat-row .chat-timestamp {
    font-size: 11px;
    font-weight: 500;
    opacity: 0.45;
}

/* ── Messages area & bubbles ───────────────────────────────────────── */
.message-row {
    margin: 4px 0;
}
.bubble {
    padding: 10px 16px;
    border-radius: 16px;
    font-size: 13px;
    line-height: 1.5;
    box-shadow: none;
}

/* ── Composer host: hairline rule + mode-color leading stripe ──────── */
.kryptos-composer-host {
    border-top: 1px solid alpha(currentColor, 0.06);
    padding: 12px 32px 16px;
}
.kryptos-composer-wrapper {
    border-left: 3px solid transparent;
    padding-left: 12px;
    transition: border-color 120ms ease-out;
}
.kryptos-composer,
.kryptos-composer text {
    background-color: transparent;
    font-size: 13px;
    line-height: 1.55;
}

/* ── Mode line: three sections, monospace, tabular ─────────────────── */
.kryptos-modeline {
    border-top: 1px solid alpha(currentColor, 0.06);
    min-height: 28px;
    font-size: 12px;
}
.modeline-section {
    padding: 4px 12px;
}
.modeline-section.left {
    padding: 0;
}
.modeline-section.center {
    opacity: 0.55;
    font-weight: 500;
    letter-spacing: -0.005em;
}
.modeline-section.right {
    opacity: 0.55;
    padding-right: 14px;
}
.modeline-mode-block {
    font-weight: 700;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    padding: 4px 14px 4px 0;
}
.modeline-mode-glyph {
    font-weight: 700;
    padding: 0 8px 0 10px;
    letter-spacing: 0;
}
.modeline-separator {
    opacity: 0.25;
    padding: 0 8px;
}

/* ── Command bar: ghost line, mono ─────────────────────────────────── */
.command-bar {
    padding: 8px 16px;
    border-top: 1px solid alpha(currentColor, 0.06);
    font-size: 13px;
}
.command-bar-prefix {
    font-weight: 700;
    padding-right: 6px;
    letter-spacing: 0.06em;
}
.command-bar entry {
    background: transparent;
    border: none;
    box-shadow: none;
    padding: 0;
    min-height: 0;
}
"#;

/// Helper: select the row N positions from the current selection in
/// the sidebar list. `delta` is signed.
pub fn move_sidebar_selection(list: &gtk::ListBox, delta: i32) {
    let current = list.selected_row().map(|r| r.index()).unwrap_or(0);
    let mut count = 0;
    while list.row_at_index(count).is_some() {
        count += 1;
    }
    if count == 0 {
        return;
    }
    let mut next = current + delta;
    if next < 0 {
        next = 0;
    }
    if next >= count {
        next = count - 1;
    }
    if let Some(row) = list.row_at_index(next) {
        list.select_row(Some(&row));
        row.grab_focus();
    }
}

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
//! │  -- NORMAL --                              │
//! └────────────────────────────────────────────┘
//! ```

use adw::prelude::*;

use crate::config::Config;

use super::composer::Composer;
use super::settings::Settings;
use super::statusline::{CommandBar, ModeLine};

/// Pieces of the window the rest of the UI layer needs to talk to.
pub struct WindowParts {
    pub window: adw::ApplicationWindow,
    pub sidebar_list: gtk::ListBox,
    pub content_title: adw::WindowTitle,
    pub composer: Composer,
    pub mode_line: ModeLine,
    pub command_bar: CommandBar,
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
    let sidebar_list = build_sidebar_list();
    let sidebar_search = build_sidebar_search();
    let sidebar = build_sidebar(&sidebar_list, &sidebar_search);
    let (content, content_title, composer, prefs_button) = build_content();

    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar)
        .content(&content)
        .show_sidebar(true)
        .min_sidebar_width(220.0)
        .max_sidebar_width(360.0)
        .sidebar_width_fraction(0.27)
        .build();
    split.set_vexpand(true);

    let mode_line = ModeLine::new();
    let command_bar = CommandBar::new();

    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    root.append(&split);
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

    install_styles();

    WindowParts {
        window,
        sidebar_list,
        content_title,
        composer,
        mode_line,
        command_bar,
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
    let initial = name.chars().next().unwrap_or('?').to_uppercase().to_string();
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

    let ts_label = gtk::Label::builder()
        .label(timestamp)
        .xalign(1.0)
        .build();
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

fn build_content() -> (gtk::Widget, adw::WindowTitle, Composer, gtk::Button) {
    let title = adw::WindowTitle::new(PLACEHOLDER_CHATS[0].0, "online");
    let header = adw::HeaderBar::builder().title_widget(&title).build();

    let info_button = gtk::Button::from_icon_name("dialog-information-symbolic");
    info_button.set_tooltip_text(Some("Conversation info"));
    info_button.add_css_class("flat");

    let prefs_button = gtk::Button::from_icon_name("open-menu-symbolic");
    prefs_button.set_tooltip_text(Some("Preferences"));
    prefs_button.add_css_class("flat");

    header.pack_end(&prefs_button);
    header.pack_end(&info_button);

    let messages_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    messages_box.set_margin_start(16);
    messages_box.set_margin_end(16);
    messages_box.set_margin_top(16);
    messages_box.set_margin_bottom(16);

    let mut prev_mine: Option<bool> = None;
    for (mine, body, ts) in PLACEHOLDER_MESSAGES {
        let show_sender = prev_mine != Some(*mine);
        messages_box.append(&message_row(*mine, body, ts, show_sender));
        prev_mine = Some(*mine);
    }

    let messages_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(&messages_box)
        .build();

    let composer = Composer::new();

    let composer_frame = gtk::Frame::new(None);
    composer_frame.set_child(Some(composer.widget()));
    composer_frame.add_css_class("composer-frame");

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(&messages_scroll);
    body.append(&composer_frame);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    (toolbar.upcast::<gtk::Widget>(), title, composer, prefs_button)
}

/// One vertical message slot: optional sender label, the bubble itself,
/// and a timestamp that fades in on hover. The whole stack sits in a
/// `.message-row` container so the timestamp's hover transition can be
/// driven by the row, not the bubble.
fn message_row(mine: bool, body: &str, timestamp: &str, show_sender: bool) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(body)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .build();
    label.add_css_class("bubble");
    label.add_css_class(if mine { "bubble-mine" } else { "bubble-theirs" });

    let bubble_align = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    bubble_align.set_halign(if mine { gtk::Align::End } else { gtk::Align::Start });
    bubble_align.append(&label);

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    column.add_css_class("message-row");
    if mine {
        column.add_css_class("mine");
        column.set_halign(gtk::Align::End);
    } else {
        column.add_css_class("theirs");
        column.set_halign(gtk::Align::Start);
    }

    if show_sender {
        let sender = gtk::Label::builder()
            .label(if mine { "You" } else { "Them" })
            .xalign(if mine { 1.0 } else { 0.0 })
            .build();
        sender.add_css_class("message-sender");
        column.append(&sender);
    }
    column.append(&bubble_align);

    let ts = gtk::Label::builder()
        .label(timestamp)
        .xalign(if mine { 1.0 } else { 0.0 })
        .build();
    ts.add_css_class("message-timestamp");
    column.append(&ts);

    column.upcast::<gtk::Widget>()
}

/// Inject a small in-process stylesheet so the mode line + bubbles get
/// some shape without dragging in a `.css` resource yet.
fn install_styles() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(STYLES);
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Embedded fallback stylesheet. Used until/unless the theme system mounts
/// a palette `CssProvider` (which currently isn't wired into `activate()`),
/// so this string carries the same design polish the palette CSS does — it
/// just leans on libadwaita's `@accent_*` / `@card_*` tokens instead of
/// `@kryptos_*`. Runtime overrides from a palette will win.
const STYLES: &str = r#"
/* Mode line */
.mode-line-row,
.kryptos-modeline {
    background-color: @sidebar_bg_color;
    border-top: 1px solid @borders;
    min-height: 24px;
    padding: 0;
}
.modeline-section {
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 12px;
    padding: 4px 12px;
}
.modeline-section.left { padding: 0; }
.modeline-section.center { opacity: 0.85; }
.modeline-section.right { opacity: 0.7; font-feature-settings: "tnum" 1; }
.modeline-mode-block {
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 12px;
    font-weight: 700;
    letter-spacing: 0.10em;
    text-transform: uppercase;
    padding: 4px 14px;
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}
.modeline-mode-block.mode-normal { background-color: #89b4fa; color: #1e1e2e; }
.modeline-mode-block.mode-insert { background-color: #a6e3a1; color: #1e1e2e; }
.modeline-mode-block.mode-command { background-color: #f9e2af; color: #1e1e2e; }
.modeline-mode-block.mode-search  { background-color: #f38ba8; color: #1e1e2e; }
.modeline-separator { opacity: 0.35; padding: 0 6px; }

/* Command bar */
.command-bar {
    padding: 6px 12px;
    font-family: "JetBrains Mono", "Fira Code", monospace;
    border-top: 1px solid @borders;
    background-color: @sidebar_bg_color;
}
.command-bar-prefix {
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-weight: 700;
    padding-right: 4px;
    color: @accent_color;
}

/* Sidebar search */
.sidebar-search {
    border-radius: 8px;
    margin: 6px 10px 8px;
    transition: border-color 120ms ease-out;
}
.sidebar-search:focus-within {
    outline: none;
    border-color: @accent_color;
}

/* Chat row */
.kryptos-chat-row {
    padding: 12px 16px;
    transition: background-color 120ms ease-out;
}
.kryptos-chat-row .chat-avatar {
    min-width: 36px;
    min-height: 36px;
    border-radius: 18px;
    background-color: alpha(@accent_color, 0.22);
    color: @accent_color;
    font-size: 13px;
    font-weight: 700;
}
.kryptos-chat-row .chat-name { font-size: 14px; font-weight: 600; }
.kryptos-chat-row .chat-preview { font-size: 12px; opacity: 0.7; }
.kryptos-chat-row .chat-timestamp {
    font-size: 11px; font-weight: 500; opacity: 0.55;
    font-feature-settings: "tnum" 1;
}
.kryptos-chat-row:selected .chat-avatar {
    background-color: alpha(@accent_color, 0.34);
}

/* Header presence subtitle */
.chat-presence { font-size: 11px; font-weight: 500; opacity: 0.6; }

/* Messages */
.message-row { margin: 2px 0; }
.message-sender {
    font-size: 11px; font-weight: 600; letter-spacing: 0.04em;
    text-transform: uppercase; opacity: 0.55; margin: 4px 4px 2px;
}
.message-row.mine .message-sender { color: @accent_color; }
.message-timestamp {
    font-size: 11px; font-weight: 500; opacity: 0;
    margin: 2px 4px 0; font-feature-settings: "tnum" 1;
    transition: opacity 150ms ease-out;
}
.message-row:hover .message-timestamp { opacity: 0.6; }

.bubble {
    padding: 9px 14px;
    border-radius: 16px;
    box-shadow: 0 1px 2px rgba(0,0,0,0.10);
    transition: box-shadow 150ms ease-out;
}
.bubble-mine {
    background-color: alpha(@accent_bg_color, 0.92);
    color: @accent_fg_color;
}
.bubble-theirs {
    background-color: alpha(@card_shade_color, 1.0);
}

/* Composer */
.composer-frame {
    border-radius: 12px;
    margin: 8px 12px 12px;
    border: 1px solid alpha(@borders, 0.8);
    background-color: alpha(@card_bg_color, 0.6);
    transition: border-color 150ms ease-out, box-shadow 150ms ease-out;
}
.composer-frame:focus-within {
    border-color: @accent_color;
    box-shadow: 0 0 0 3px alpha(@accent_color, 0.18);
}
.kryptos-composer, .composer-frame textview { background-color: transparent; }

.composer-mode-badge {
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 10px;
    font-weight: 700;
    letter-spacing: 0.10em;
    text-transform: uppercase;
    padding: 2px 8px 2px 6px;
    border-radius: 999px;
    background-color: alpha(@accent_color, 0.18);
    color: @accent_color;
}
.composer-insert .composer-mode-badge {
    background-color: alpha(#a6e3a1, 0.20);
    color: #a6e3a1;
}
.composer-visual .composer-mode-badge {
    background-color: alpha(#f9e2af, 0.20);
    color: #f9e2af;
}
.composer-mode-dot { font-size: 12px; margin-right: 4px; opacity: 0.85; }

.kryptos-composer-wrapper.composer-normal { caret-color: alpha(@accent_color, 0.85); }
.kryptos-composer-wrapper.composer-insert { caret-color: @accent_color; }
.kryptos-composer-wrapper.composer-visual { caret-color: #f9e2af; }

/* Empty state */
.empty-state { padding: 32px 24px; }
.empty-state-glyph { font-size: 64px; opacity: 0.18; color: @accent_color; }
.empty-state-title { font-size: 15px; font-weight: 600; opacity: 0.7; }
.empty-state-subtitle { font-size: 12px; opacity: 0.7; }

headerbar button.flat {
    border-radius: 999px;
    transition: background-color 120ms ease-out;
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


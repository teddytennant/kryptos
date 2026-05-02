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
    let sidebar_list = build_sidebar_list();
    let sidebar_search = build_sidebar_search();
    let sidebar = build_sidebar(&sidebar_list, &sidebar_search);
    let (content, composer, prefs_button, link_button) = build_content();

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

    let messages_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    messages_box.set_margin_start(28);
    messages_box.set_margin_end(28);
    messages_box.set_margin_top(20);
    messages_box.set_margin_bottom(8);

    for (mine, body, ts) in PLACEHOLDER_MESSAGES {
        messages_box.append(&message_row(*mine, body, ts));
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
    composer_frame.set_margin_start(20);
    composer_frame.set_margin_end(20);
    composer_frame.set_margin_top(8);
    composer_frame.set_margin_bottom(16);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    body.append(&messages_scroll);
    body.append(&composer_frame);

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

/// Functional layer of styles. Visual tokens (colours, accents) come
/// from the active palette CSS in `src/theme/css/`; this layer just
/// sets sizes, paddings, weights, and transitions that don't depend
/// on the palette.
const STYLES: &str = r#"
/* Modeline */
.kryptos-modeline {
    border-top: 1px solid alpha(currentColor, 0.08);
    min-height: 26px;
    font-family: "JetBrains Mono", "Fira Code", monospace;
    font-size: 12px;
}
.modeline-section { padding: 4px 14px; }
.modeline-section.left { padding: 0; }
.modeline-section.center { opacity: 0.7; }
.modeline-section.right { opacity: 0.55; font-feature-settings: "tnum" 1; padding-right: 14px; }
.modeline-mode-block {
    font-weight: 700;
    letter-spacing: 0.12em;
    text-transform: uppercase;
    padding: 4px 14px;
    background-color: @accent_bg_color;
    color: @accent_fg_color;
}
.modeline-mode-block.mode-normal { background-color: #89b4fa; color: #1e1e2e; }
.modeline-mode-block.mode-insert { background-color: #a6e3a1; color: #1e1e2e; }
.modeline-mode-block.mode-command { background-color: #f9e2af; color: #1e1e2e; }
.modeline-mode-block.mode-search  { background-color: #f38ba8; color: #1e1e2e; }

/* Command bar */
.command-bar {
    padding: 8px 16px;
    font-family: "JetBrains Mono", "Fira Code", monospace;
    border-top: 1px solid alpha(currentColor, 0.08);
}
.command-bar-prefix { font-weight: 700; padding-right: 6px; opacity: 0.6; }

/* Sidebar search + chat row */
.sidebar-search { margin: 8px 12px 4px; border-radius: 10px; }
.kryptos-chat-row { padding: 12px 14px; transition: background-color 120ms ease-out; }
.kryptos-chat-row .chat-avatar {
    min-width: 36px; min-height: 36px; border-radius: 18px;
    background-color: alpha(@accent_color, 0.22);
    color: @accent_color;
    font-size: 13px; font-weight: 700;
}
.kryptos-chat-row .chat-name { font-size: 14px; font-weight: 600; }
.kryptos-chat-row .chat-preview { font-size: 12px; opacity: 0.62; }
.kryptos-chat-row .chat-timestamp {
    font-size: 11px; font-weight: 500; opacity: 0.5;
    font-feature-settings: "tnum" 1;
}

/* Header chrome — flat & quiet, the chat content is the story */
headerbar.flat { background: transparent; box-shadow: none; border: none; min-height: 44px; }
headerbar windowtitle { font-weight: 600; font-size: 14px; }
headerbar button.flat { border-radius: 999px; min-width: 30px; min-height: 30px; padding: 4px; }

/* Sidebar header gets a subtle separator from content */
.sidebar-header { border-bottom: 1px solid alpha(currentColor, 0.06); }

/* Messages */
.message-row { margin: 3px 0; }
.bubble {
    padding: 9px 14px;
    border-radius: 18px;
    font-size: 14px;
    line-height: 1.4;
}
.bubble-mine {
    background-color: @accent_bg_color;
    color: @accent_fg_color;
    border-bottom-right-radius: 6px;
}
.bubble-theirs {
    background-color: alpha(currentColor, 0.08);
    border-bottom-left-radius: 6px;
}

/* Composer */
.composer-frame {
    border-radius: 14px;
    border: 1px solid alpha(currentColor, 0.10);
    background-color: alpha(currentColor, 0.03);
    transition: border-color 150ms ease-out, box-shadow 150ms ease-out;
}
.composer-frame:focus-within {
    border-color: @accent_color;
    box-shadow: 0 0 0 3px alpha(@accent_color, 0.16);
}
.kryptos-composer, .composer-frame textview { background-color: transparent; font-size: 14px; }
.kryptos-composer-wrapper.composer-normal { caret-color: alpha(@accent_color, 0.85); }
.kryptos-composer-wrapper.composer-insert { caret-color: @accent_color; }
.kryptos-composer-wrapper.composer-visual { caret-color: #f9e2af; }
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

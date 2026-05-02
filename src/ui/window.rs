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

const PLACEHOLDER_CHATS: &[&str] = &["Family", "Work", "Linux Linux Linux"];
const PLACEHOLDER_MESSAGES: &[(bool, &str)] = &[
    (false, "Hey, did you see the new Kryptos release?"),
    (true, "Not yet — what's in it?"),
    (false, "Vim modes everywhere. It's wonderful."),
    (true, "Of course it is."),
    (false, "Try `:help` once you boot it."),
];

pub fn build(app: &adw::Application, cfg: &Config) -> WindowParts {
    let sidebar_list = build_sidebar_list();
    let sidebar = build_sidebar(&sidebar_list);
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

    for name in PLACEHOLDER_CHATS {
        list.append(&chat_row(name));
    }

    if let Some(first) = list.row_at_index(0) {
        list.select_row(Some(&first));
    }

    list
}

fn chat_row(name: &str) -> gtk::ListBoxRow {
    let title = gtk::Label::builder()
        .label(name)
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    title.add_css_class("heading");

    let preview = gtk::Label::builder()
        .label("…")
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    preview.add_css_class("dim-label");
    preview.add_css_class("caption");

    let vbox = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .build();
    vbox.append(&title);
    vbox.append(&preview);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);
    vbox.set_margin_top(8);
    vbox.set_margin_bottom(8);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&vbox));
    row
}

fn build_sidebar(list: &gtk::ListBox) -> gtk::Widget {
    let header = adw::HeaderBar::builder()
        .title_widget(&adw::WindowTitle::new("Chats", ""))
        .build();
    let search_btn = gtk::Button::from_icon_name("system-search-symbolic");
    search_btn.add_css_class("flat");
    header.pack_end(&search_btn);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .child(list)
        .build();

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroller));
    toolbar.upcast::<gtk::Widget>()
}

fn build_content() -> (gtk::Widget, adw::WindowTitle, Composer, gtk::Button) {
    let title = adw::WindowTitle::new(PLACEHOLDER_CHATS[0], "");
    let header = adw::HeaderBar::builder().title_widget(&title).build();

    let prefs_button = gtk::Button::from_icon_name("open-menu-symbolic");
    prefs_button.set_tooltip_text(Some("Preferences"));
    prefs_button.add_css_class("flat");
    header.pack_end(&prefs_button);

    let messages_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    messages_box.set_margin_start(16);
    messages_box.set_margin_end(16);
    messages_box.set_margin_top(16);
    messages_box.set_margin_bottom(16);
    for (mine, body) in PLACEHOLDER_MESSAGES {
        messages_box.append(&message_bubble(*mine, body));
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
    composer_frame.set_margin_start(12);
    composer_frame.set_margin_end(12);
    composer_frame.set_margin_bottom(12);
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

fn message_bubble(mine: bool, body: &str) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(body)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .build();
    label.add_css_class("bubble");
    label.add_css_class(if mine { "bubble-mine" } else { "bubble-theirs" });
    label.set_margin_start(12);
    label.set_margin_end(12);
    label.set_margin_top(6);
    label.set_margin_bottom(6);

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    if mine {
        row.set_halign(gtk::Align::End);
    } else {
        row.set_halign(gtk::Align::Start);
    }
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

const STYLES: &str = r#"
.mode-line {
    font-family: monospace;
    font-size: 0.85em;
    padding: 2px 8px;
    border-radius: 3px;
}
.mode-normal { color: #89b4fa; }
.mode-insert { color: #a6e3a1; }
.mode-command { color: #f9e2af; }
.mode-search { color: #fab387; }

.command-bar {
    padding: 2px 4px;
    font-family: monospace;
}
.command-bar-prefix {
    font-family: monospace;
    font-weight: bold;
    padding-right: 4px;
}

.bubble {
    padding: 8px 12px;
    border-radius: 12px;
}
.bubble-mine {
    background-color: alpha(@accent_bg_color, 0.85);
    color: @accent_fg_color;
}
.bubble-theirs {
    background-color: alpha(@card_shade_color, 1.0);
}

.composer-frame {
    border-radius: 8px;
}

.composer-mode-badge {
    font-family: monospace;
    font-size: 10px;
    letter-spacing: 0.08em;
    padding: 1px 6px;
    border-radius: 3px;
    color: alpha(@accent_color, 0.55);
}
.composer-insert .composer-mode-badge {
    color: @accent_color;
}
.composer-visual .composer-mode-badge {
    color: #f9e2af;
}

.kryptos-composer-wrapper.composer-normal {
    caret-color: alpha(@accent_color, 0.85);
}
.kryptos-composer-wrapper.composer-insert {
    caret-color: @accent_color;
}
.kryptos-composer-wrapper.composer-visual {
    caret-color: #f9e2af;
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


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

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use adw::prelude::*;

use crate::config::Config;
use crate::messenger::{ChatId, ConversationSummary, NormalizedMessage};

use super::composer::Composer;
use super::onboarding;
use super::settings::Settings;
use super::statusline::{CommandBar, ModeLine};

/// Pieces of the window the rest of the UI layer needs to talk to.
pub struct WindowParts {
    pub window: adw::ApplicationWindow,
    pub sidebar_list: gtk::ListBox,
    pub sidebar_scroller: gtk::ScrolledWindow,
    pub sidebar_empty: gtk::Widget,
    pub composer: Composer,
    pub mode_line: ModeLine,
    pub command_bar: CommandBar,
    pub toast_overlay: adw::ToastOverlay,
    /// Title widget for the content header — updated when the active
    /// conversation changes.
    pub content_title: adw::WindowTitle,
    /// Box that holds rendered message rows; rebuilt on chat switch and
    /// appended-to when a new event lands.
    pub messages_box: gtk::Box,
    /// Sidebar rows in the same order as the conversation list, paired
    /// with their `ChatId`s. The dispatcher reads this when the user
    /// clicks (or `j`/`k`s) to a row to find the active chat id.
    pub sidebar_index: Rc<RefCell<Vec<(ChatId, gtk::ListBoxRow)>>>,
}

impl WindowParts {
    /// Replace the sidebar rows with `convs`, in order. Empty input
    /// keeps the empty-state visible. The selection is preserved when
    /// the previously-selected chat id is still in the new list.
    pub fn set_conversations(&self, convs: &[ConversationSummary]) {
        let prev = self.sidebar_list.selected_row().and_then(|row| {
            self.sidebar_index
                .borrow()
                .iter()
                .find(|(_, r)| r == &row)
                .map(|(id, _)| id.clone())
        });

        while let Some(row) = self.sidebar_list.row_at_index(0) {
            self.sidebar_list.remove(&row);
        }
        self.sidebar_index.borrow_mut().clear();

        for c in convs {
            let ts_label = c
                .last_message_ts
                .map(format_clock_label)
                .unwrap_or_default();
            // Prefer the resolved display name; fall back to the
            // raw `title` (the backend's native id) when none.
            let row = chat_row(c.label(), "", &ts_label);
            self.sidebar_list.append(&row);
            self.sidebar_index
                .borrow_mut()
                .push((c.id.clone(), row.clone()));
        }

        let has_rows = self.sidebar_list.row_at_index(0).is_some();
        self.sidebar_scroller.set_visible(has_rows);
        self.sidebar_empty.set_visible(!has_rows);

        if has_rows {
            // Try to keep the same chat selected; otherwise default to
            // the top row so the right pane is never empty when there
            // is conversation data to show.
            let target = prev
                .and_then(|id| {
                    self.sidebar_index
                        .borrow()
                        .iter()
                        .find(|(cid, _)| cid == &id)
                        .map(|(_, r)| r.clone())
                })
                .or_else(|| self.sidebar_list.row_at_index(0));
            if let Some(row) = target {
                self.sidebar_list.select_row(Some(&row));
            }
        }
    }

    /// Replace the messages view with `msgs`, ordered oldest-first.
    /// The content header title is set from `active.native` so the
    /// user always knows which chat they're looking at.
    ///
    /// `own_id` is the active backend's `self_account()` — the local
    /// user's E.164 number for Signal, the user_id (decimal string)
    /// for Telegram. When `None`, no message is treated as "mine".
    #[allow(dead_code)] // exposed for the dispatcher; used via cloned widgets in mod.rs.
    pub fn set_messages(&self, msgs: &[NormalizedMessage], active: &ChatId, own_id: Option<&str>) {
        while let Some(child) = self.messages_box.first_child() {
            self.messages_box.remove(&child);
        }
        self.content_title.set_title(&active.native);

        if msgs.is_empty() {
            self.messages_box.append(&messages_empty_state());
            return;
        }

        let now = now_ms();
        let rows: Vec<(bool, String, i64)> = msgs
            .iter()
            .map(|m| {
                (
                    is_mine(&m.sender, own_id),
                    m.body.clone().unwrap_or_default(),
                    m.ts_ms,
                )
            })
            .collect();
        populate_messages(&self.messages_box, &rows, now);
    }
}

pub fn build(app: &adw::Application, cfg: &Config) -> WindowParts {
    // Force JetBrains Mono Nerd Font as the default app face *before* any
    // widget gets sized. Pango caches font extents per widget, so doing
    // this after `build_*` would leave existing widgets at the old metric.
    apply_default_font();

    let sidebar_index = Rc::new(RefCell::new(Vec::<(ChatId, gtk::ListBoxRow)>::new()));

    let sidebar_list = build_sidebar_list();
    let sidebar_search = build_sidebar_search();
    let sidebar_empty = build_sidebar_empty_state();
    let (sidebar, sidebar_scroller) = build_sidebar(&sidebar_list, &sidebar_search, &sidebar_empty);
    let (content, composer, prefs_button, link_button, content_title, messages_box) =
        build_content();

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
        .default_width(1180)
        .default_height(760)
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
        sidebar_scroller,
        sidebar_empty,
        composer,
        mode_line,
        command_bar,
        toast_overlay,
        content_title,
        messages_box,
        sidebar_index,
    }
}

fn build_sidebar_list() -> gtk::ListBox {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    list.add_css_class("navigation-sidebar");
    list
}

/// Two-line designed chat row: 36px tinted avatar disk with the contact's
/// initial, name (semibold) + timestamp on the top line, last-message
/// preview (dim) on the bottom. `chat-row`-class is on the outer row so
/// the palette CSS can paint hover, selection, and the accent stripe.
pub(super) fn chat_row(name: &str, preview: &str, timestamp: &str) -> gtk::ListBoxRow {
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
    avatar.add_css_class(avatar_class_for(name));
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

fn build_sidebar(
    list: &gtk::ListBox,
    search: &gtk::SearchEntry,
    empty_state: &gtk::Widget,
) -> (gtk::Widget, gtk::ScrolledWindow) {
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

    // The list is empty only at first run; we show the empty state below
    // it (also empty-vexpanded) so the layout stays the same once chats
    // arrive — the empty state simply hides itself.
    let has_rows = list.row_at_index(0).is_some();
    scroller.set_visible(has_rows);
    empty_state.set_visible(!has_rows);

    let body = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(0)
        .build();
    body.append(search);
    body.append(&scroller);
    body.append(empty_state);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&body));
    (toolbar.upcast::<gtk::Widget>(), scroller)
}

/// Sidebar empty state shown when no chats exist yet. The "Link to
/// Signal" button is wired in `build` once we have a window handle.
fn build_sidebar_empty_state() -> gtk::Widget {
    let title = gtk::Label::builder()
        .label("No chats yet")
        .halign(gtk::Align::Center)
        .build();
    title.add_css_class("empty-title");

    let subtitle = gtk::Label::builder()
        .label("Link Kryptos to your Signal account to get started.")
        .halign(gtk::Align::Center)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.5)
        .build();
    subtitle.add_css_class("empty-subtitle");

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    column.add_css_class("kryptos-empty-state");
    column.add_css_class("kryptos-sidebar-empty");
    column.set_margin_start(16);
    column.set_margin_end(16);
    column.append(&title);
    column.append(&subtitle);
    column.upcast::<gtk::Widget>()
}

fn build_content() -> (
    gtk::Widget,
    Composer,
    gtk::Button,
    gtk::Button,
    adw::WindowTitle,
    gtk::Box,
) {
    let content_title = adw::WindowTitle::new("Kryptos", "");
    let header = adw::HeaderBar::builder()
        .title_widget(&content_title)
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

    // Start with the empty-state — real messages get rendered the
    // moment a sidebar row is selected.
    messages_box.append(&messages_empty_state());

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
        content_title,
        messages_box,
    )
}

/// One message bubble in a row. Real chat apps don't shout "THEM" /
/// "YOU" before every message — alignment + bubble color carry the
/// authorship; the timestamp surfaces only on hover via CSS.
///
/// `cluster_top` / `cluster_bottom` describe the row's place in a run
/// of consecutive same-sender messages within 2 minutes:
///
/// - `cluster_top = false`  → there's a same-sender row above; tighten
///   the corner that points toward it (top, on the sender side).
/// - `cluster_bottom = false` → there's a same-sender row below;
///   tighten the bottom-side corner.
/// - The vertical gap between the bubble and its same-sender
///   neighbours is collapsed to 0 via CSS on `.cluster-{top,middle,bottom}`.
pub(super) fn message_row(
    mine: bool,
    body: &str,
    ts_label: &str,
    cluster_top: bool,
    cluster_bottom: bool,
) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(body)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .build();
    label.add_css_class("bubble");
    label.add_css_class(if mine { "bubble-mine" } else { "bubble-theirs" });

    // Cluster classes: `solo` (only one in cluster), `top`, `middle`, `bottom`.
    let cluster_class = match (cluster_top, cluster_bottom) {
        (true, true) => "cluster-solo",
        (true, false) => "cluster-top",
        (false, true) => "cluster-bottom",
        (false, false) => "cluster-middle",
    };
    label.add_css_class(cluster_class);
    label.set_tooltip_text(Some(ts_label));

    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    row.add_css_class("message-row");
    row.add_css_class(cluster_class);
    row.set_halign(if mine {
        gtk::Align::End
    } else {
        gtk::Align::Start
    });
    row.append(&label);
    row.upcast::<gtk::Widget>()
}

// --- Date / clustering helpers ------------------------------------------
//
// Day buckets are derived from a `now_ms` reference passed in by the
// caller (so tests stay deterministic and we don't surprise the user
// with a "Today" label after midnight). Older days get a fixed "Earlier"
// label; pulling in chrono just for `Mon, Apr 28` would balloon the
// build for one cosmetic line.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Day {
    Today,
    Yesterday,
    Older,
}

const ONE_DAY_MS: i64 = 86_400_000;
const TWO_MIN_MS: i64 = 120_000;

fn day_for(ts_ms: i64, now_ms: i64) -> Day {
    let delta = now_ms - ts_ms;
    if delta < ONE_DAY_MS {
        Day::Today
    } else if delta < 2 * ONE_DAY_MS {
        Day::Yesterday
    } else {
        Day::Older
    }
}

fn day_label(d: Day) -> &'static str {
    match d {
        Day::Today => "Today",
        Day::Yesterday => "Yesterday",
        Day::Older => "Earlier",
    }
}

/// Render an absolute epoch-ms timestamp as a 24h "HH:MM" wall-clock
/// label. We don't pull in `chrono`'s `clock` feature (which would drag
/// in `iana-time-zone`) just for a label, so this is a pure UTC clock —
/// the offset bake-in can come later as a config knob.
pub(super) fn format_clock_label(ts_ms: i64) -> String {
    let secs = ts_ms.div_euclid(1000);
    let day_secs = secs.rem_euclid(86_400);
    let h = (day_secs / 3600) % 24;
    let m = (day_secs / 60) % 60;
    format!("{h:02}:{m:02}")
}

fn date_divider(text: &str) -> gtk::Widget {
    let label = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Center)
        .build();
    label.add_css_class("date-divider");
    label.upcast::<gtk::Widget>()
}

/// Empty-state for an empty conversation: a thin glyph at low opacity
/// and a quietly framed instruction.
pub(super) fn messages_empty_state() -> gtk::Widget {
    let glyph = gtk::Label::builder()
        .label("◯")
        .halign(gtk::Align::Center)
        .build();
    glyph.add_css_class("empty-glyph");

    let title = gtk::Label::builder()
        .label("Send a message to start.")
        .halign(gtk::Align::Center)
        .build();
    title.add_css_class("empty-title");

    let subtitle = gtk::Label::builder()
        .label("Enter to send · Shift-Enter for newline")
        .halign(gtk::Align::Center)
        .build();
    subtitle.add_css_class("empty-subtitle");

    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .vexpand(true)
        .build();
    column.add_css_class("kryptos-empty-state");
    column.append(&glyph);
    column.append(&title);
    column.append(&subtitle);
    column.upcast::<gtk::Widget>()
}

/// Walk `rows` once, inserting a `.date-divider` when the day changes
/// and tagging each row with its cluster role (top/middle/bottom/solo).
/// A "cluster" is a run of same-sender messages within 2 minutes.
///
/// `rows` is a flat `(mine, body, ts_ms)` view so the same builder works
/// for placeholders, pending optimistic sends, and real history.
pub(super) fn populate_messages(
    messages_box: &gtk::Box,
    rows: &[(bool, String, i64)],
    now_ms: i64,
) {
    let mut prev_day: Option<Day> = None;
    for (i, (mine, body, ts)) in rows.iter().enumerate() {
        let day = day_for(*ts, now_ms);
        if prev_day != Some(day) {
            messages_box.append(&date_divider(day_label(day)));
            prev_day = Some(day);
        }

        // Cluster boundaries: top = no same-sender within 2min above
        // (or day changed); bottom = same condition below.
        let cluster_top = match i.checked_sub(1).and_then(|j| rows.get(j)) {
            Some((m, _, prev_ts)) => {
                let prev_day_for = day_for(*prev_ts, now_ms);
                *m != *mine || prev_day_for != day || (ts - prev_ts).abs() > TWO_MIN_MS
            }
            None => true,
        };
        let cluster_bottom = match rows.get(i + 1) {
            Some((m, _, next_ts)) => {
                let next_day = day_for(*next_ts, now_ms);
                *m != *mine || next_day != day || (next_ts - ts).abs() > TWO_MIN_MS
            }
            None => true,
        };

        let label = format_clock_label(*ts);
        messages_box.append(&message_row(
            *mine,
            body,
            &label,
            cluster_top,
            cluster_bottom,
        ));
    }
}

/// Coarse "now" used by the date-divider helper. Reads system time
/// monotonically; falls back to 0 only if the clock somehow predates
/// UNIX epoch.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// True if the message belongs to the local user. The caller passes
/// the active backend's own identifier (E.164 for Signal, user_id for
/// Telegram); we match it against the message's `sender`. `None` means
/// the backend hasn't reported its identity yet — be conservative and
/// treat nothing as "mine" rather than guessing.
pub(super) fn is_mine(sender: &str, own_id: Option<&str>) -> bool {
    match own_id {
        Some(me) => sender == me,
        None => false,
    }
}

/// JetBrains Mono Nerd Font as the system face. Setting `gtk-font-name`
/// on `gtk::Settings` makes every default-font widget — labels, entries,
/// buttons, tooltips — pick it up without per-widget CSS. Pango still
/// runs its own font-fallback chain on each Label, so our `*` rule in
/// the structural stylesheet covers anything that ignores `gtk-font-name`.
fn apply_default_font() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_property("gtk-font-name", "JetBrainsMono Nerd Font 11");
    }
    // Newer libadwaita versions keep their own font-name property that
    // can shadow `gtk-font-name`; set it when present so the style
    // manager doesn't override our face on dark/light scheme flips.
    let style_manager = adw::StyleManager::default();
    if style_manager.find_property("default-font-name").is_some() {
        style_manager.set_property("default-font-name", "JetBrainsMono Nerd Font 11");
    }
}

/// Map a chat name to one of 8 deterministic hues drawn from the active
/// theme tokens. Cheap & stable — same name always lands on the same
/// hue so the avatar disc colour reads as identity, not decoration.
///
/// The class names line up with `.avatar-hue-{0..7}` rules in every
/// palette CSS, where each hue is paired with a palette token (blue,
/// green, lavender, etc.).
pub fn avatar_class_for(name: &str) -> &'static str {
    const HUES: [&str; 8] = [
        "avatar-hue-0",
        "avatar-hue-1",
        "avatar-hue-2",
        "avatar-hue-3",
        "avatar-hue-4",
        "avatar-hue-5",
        "avatar-hue-6",
        "avatar-hue-7",
    ];
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    HUES[(hasher.finish() as usize) % HUES.len()]
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
    font-family: "JetBrainsMono Nerd Font", "JetBrains Mono", "Iosevka Nerd Font", monospace;
}
window {
    font-size: 13px;
}
/* Tabular figures wherever digits appear so columns never shift. */
.chat-timestamp,
.kryptos-modeline,
.modeline-mode-block,
.modeline-mode-glyph,
.modeline-section,
.modeline-section.right,
.modeline-pending,
.modeline-unread,
.command-bar,
.command-bar-entry,
.message-timestamp,
.bubble {
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
    border: none;
    transition: background-color 100ms ease-out;
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
    transition: background-color 120ms ease-out, border-left-color 120ms ease-out;
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
/* Default 4px-per-side margin gives a comfortable rhythm between
   different-sender messages. The cluster classes collapse the inner
   gap to zero so a run of same-sender bubbles reads as one block. */
.message-row {
    margin: 4px 0;
}
.message-row.cluster-top    { margin-bottom: 0; }
.message-row.cluster-middle { margin-top: 0; margin-bottom: 0; }
.message-row.cluster-bottom { margin-top: 0; }

.bubble {
    padding: 10px 16px;
    border-radius: 16px;
    font-size: 13px;
    line-height: 1.5;
    box-shadow: none;
}
/* Tighten the corner that points toward the cluster: for same-sender
   runs, the inside corners drop to 6px so the cluster reads as one. */
.bubble-mine.cluster-top    { border-bottom-right-radius: 6px; }
.bubble-mine.cluster-middle {
    border-top-right-radius: 6px;
    border-bottom-right-radius: 6px;
}
.bubble-mine.cluster-bottom { border-top-right-radius: 6px; }
.bubble-theirs.cluster-top    { border-bottom-left-radius: 6px; }
.bubble-theirs.cluster-middle {
    border-top-left-radius: 6px;
    border-bottom-left-radius: 6px;
}
.bubble-theirs.cluster-bottom { border-top-left-radius: 6px; }

/* ── Date dividers ─────────────────────────────────────────────────── */
.date-divider {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 0.14em;
    text-transform: uppercase;
    margin: 16px 0 10px;
    opacity: 0.55;
}

/* ── Empty states (sidebar + messages area) ────────────────────────── */
.kryptos-empty-state {
    padding: 32px 16px;
}
.kryptos-empty-state .empty-glyph {
    font-size: 56px;
    opacity: 0.18;
    margin-bottom: 12px;
}
.kryptos-empty-state .empty-title {
    font-size: 14px;
    font-weight: 500;
}
.kryptos-empty-state .empty-subtitle {
    font-size: 11px;
    letter-spacing: 0.02em;
    opacity: 0.55;
}

/* ── Composer host: hairline rule + mode-color leading stripe ──────── */
.kryptos-composer-host {
    border-top: 1px solid alpha(currentColor, 0.06);
    padding: 12px 32px 16px;
}
.kryptos-composer-wrapper {
    border-left: 3px solid transparent;
    padding-left: 12px;
    transition: border-color 150ms ease-out;
}
.kryptos-composer,
.kryptos-composer text {
    background-color: transparent;
    font-size: 13px;
    line-height: 1.55;
}
.kryptos-composer-placeholder {
    font-size: 13px;
    opacity: 0.45;
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
    /* Hairline column glyph (▎). Bold weight + zero letter-spacing
       keep it crisp at 12px; if the renderer ever goes fuzzy, swap to
       ▍ (U+258D) or ┃ (U+2503). */
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatar_class_is_one_of_eight_hues() {
        for name in ["Family", "Work", "Linux Linux Linux", "alice", ""] {
            let cls = avatar_class_for(name);
            assert!(
                cls.starts_with("avatar-hue-"),
                "{name:?} -> {cls:?} not in palette"
            );
            let idx: u8 = cls.trim_start_matches("avatar-hue-").parse().unwrap();
            assert!(idx < 8);
        }
    }

    #[test]
    fn avatar_class_is_deterministic() {
        let a = avatar_class_for("Family");
        let b = avatar_class_for("Family");
        assert_eq!(a, b);
    }

    #[test]
    fn day_for_buckets_relative_to_now() {
        let now = 86_400_000 * 10; // pretend "now" is day 10
        assert_eq!(day_for(now, now), Day::Today);
        assert_eq!(day_for(now - 3_600_000, now), Day::Today);
        assert_eq!(day_for(now - 86_400_001, now), Day::Yesterday);
        assert_eq!(day_for(now - 3 * 86_400_000, now), Day::Older);
    }

    #[test]
    fn format_clock_label_is_zero_padded() {
        // 00:00 UTC at the unix epoch.
        assert_eq!(format_clock_label(0), "00:00");
        // 12:34 UTC of an arbitrary day: 12 * 3600 + 34 * 60 = 45_240 seconds.
        assert_eq!(format_clock_label(45_240 * 1000), "12:34");
        // Negative timestamps round toward minus infinity, so
        // -1ms is "23:59" of the previous day.
        assert_eq!(format_clock_label(-1), "23:59");
    }
}

//! Native libadwaita preferences window for Kryptos.
//!
//! The window is purely a *view* over the on-disk config file. Each row's
//! "changed" signal mutates the corresponding field, debounces 250ms, and
//! atomically rewrites `~/.config/kryptos/config.toml`. The existing
//! `ConfigWatcher` then picks up the new file and live-reloads the rest
//! of the app — themes, keymaps, etc.
//!
//! We deliberately do *not* track external watcher updates back into the
//! window: the user opening the window is the source of truth for the
//! lifetime of that window. Closing and reopening reloads from disk.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;
use tracing::{error, info, warn};

use crate::config::loader;
use crate::config::Config;
use crate::core::{Error, Result};
use crate::dbus::SignalClient;
use crate::theme::builtin::PaletteSwatch;
use crate::theme::swatch;
use crate::vim::Key;

const DEBOUNCE: Duration = Duration::from_millis(250);
const REPO_URL: &str = "https://github.com/teddytennant/kryptos";

pub struct Settings;

impl Settings {
    /// Build and present the preferences window, anchored on `parent`.
    /// On error loading the on-disk config, the window opens with
    /// in-memory defaults; subsequent edits will write a fresh file.
    pub fn open(parent: &impl IsA<gtk::Window>) {
        let path = match loader::default_path() {
            Ok(p) => p,
            Err(e) => {
                error!(error = %e, "could not resolve config path; settings disabled");
                return;
            }
        };
        let cfg = loader::load_or_default(&path).unwrap_or_else(|e| {
            warn!(error = %e, "could not load config; opening with defaults");
            Config::default()
        });

        let writer = Rc::new(DebouncedWriter::new(path.clone()));

        let win = adw::PreferencesWindow::builder()
            .title("Preferences")
            .transient_for(parent)
            .modal(false)
            .search_enabled(true)
            .build();

        win.add(&appearance_page(&cfg, writer.clone()));
        win.add(&behavior_page(&cfg, writer.clone()));
        win.add(&notifications_page(&cfg, writer.clone()));
        win.add(&about_page());

        win.present();
    }
}

// ---------------------------------------------------------------------------
// Pages
// ---------------------------------------------------------------------------

fn appearance_page(cfg: &Config, writer: Rc<DebouncedWriter>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Appearance")
        .icon_name("applications-graphics-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Theme")
        .description("Built-in palettes hot-reload as you select them.")
        .build();
    group.add(&theme_picker(cfg, writer.clone()));
    page.add(&group);

    let typo = adw::PreferencesGroup::builder().title("Typography").build();

    let font = adw::EntryRow::builder()
        .title("Font")
        .text(&cfg.appearance.font)
        .build();
    let writer_font = writer.clone();
    font.connect_changed(move |row| {
        let value = row.text().to_string();
        writer_font.queue(move |cfg| cfg.appearance.font = value);
    });
    typo.add(&font);

    let bubbles = adw::SwitchRow::builder()
        .title("Message bubbles")
        .subtitle("Render messages as rounded chat bubbles")
        .active(cfg.appearance.message_bubbles)
        .build();
    let writer_bubbles = writer.clone();
    bubbles.connect_active_notify(move |row| {
        let v = row.is_active();
        writer_bubbles.queue(move |cfg| cfg.appearance.message_bubbles = v);
    });
    typo.add(&bubbles);

    page.add(&typo);
    page
}

// ---------------------------------------------------------------------------
// Theme picker — flow-grid of preview cards
// ---------------------------------------------------------------------------

const CARD_W: i32 = 96;
const CARD_H: i32 = 60;

/// Build a `FlowBox` of theme preview cards plus a "system" card. Selecting
/// a card writes `general.theme` through the debounced writer.
fn theme_picker(cfg: &Config, writer: Rc<DebouncedWriter>) -> gtk::FlowBox {
    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .row_spacing(12)
        .column_spacing(12)
        .min_children_per_line(3)
        .max_children_per_line(6)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(8)
        .margin_end(8)
        .build();

    let cards: Rc<RefCell<Vec<(String, gtk::Box)>>> = Rc::new(RefCell::new(Vec::new()));
    let current = cfg.general.theme.clone();

    // The "system" card shows two halves — light/dark — to read as
    // "follow the desktop" without needing a palette table entry.
    add_card(&flow, &cards, &writer, &current, "system", None);

    for sw in swatch::ALL {
        add_card(&flow, &cards, &writer, &current, sw.name, Some(sw));
    }

    flow
}

fn add_card(
    flow: &gtk::FlowBox,
    cards: &Rc<RefCell<Vec<(String, gtk::Box)>>>,
    writer: &Rc<DebouncedWriter>,
    current: &str,
    name: &str,
    swatch: Option<&'static PaletteSwatch>,
) {
    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["kryptos-theme-card"])
        .build();
    if name == current {
        outer.add_css_class("selected");
    }

    let area = gtk::DrawingArea::builder()
        .content_width(CARD_W)
        .content_height(CARD_H)
        .build();
    let owned_swatch = swatch.copied();
    area.set_draw_func(move |_, cr, w, h| {
        match owned_swatch {
            Some(s) => draw_swatch(cr, w as f64, h as f64, &s),
            None => draw_system(cr, w as f64, h as f64),
        }
    });

    let label = gtk::Label::builder()
        .label(name)
        .css_classes(["kryptos-theme-card-label"])
        .build();

    outer.append(&area);
    outer.append(&label);

    let child = gtk::FlowBoxChild::builder()
        .child(&outer)
        .focusable(true)
        .build();

    let click = gtk::GestureClick::new();
    let writer_click = writer.clone();
    let cards_click = cards.clone();
    let name_owned = name.to_string();
    click.connect_pressed(move |_, _, _, _| {
        let target = name_owned.clone();
        for (n, b) in cards_click.borrow().iter() {
            if n == &target {
                b.add_css_class("selected");
            } else {
                b.remove_css_class("selected");
            }
        }
        let to_write = target.clone();
        writer_click.queue(move |cfg| cfg.general.theme = to_write);
    });
    child.add_controller(click);

    flow.append(&child);
    cards.borrow_mut().push((name.to_string(), outer));
}

/// Tiny mock chat: split sidebar + content, one accent bubble, one neutral.
fn draw_swatch(cr: &gtk::cairo::Context, w: f64, h: f64, s: &PaletteSwatch) {
    set_rgb(cr, s.bg);
    let _ = cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();

    let sidebar_w = (w * 0.28).round();
    set_rgb(cr, s.mantle);
    let _ = cr.rectangle(0.0, 0.0, sidebar_w, h);
    let _ = cr.fill();

    // Three sidebar rows.
    set_rgb(cr, s.subtle);
    cr.set_line_width(1.0);
    for i in 0..3 {
        let y = 10.0 + (i as f64) * 12.0;
        let _ = cr.rectangle(6.0, y, sidebar_w - 12.0, 2.0);
        let _ = cr.fill();
    }

    // Bubble theirs (left, neutral surface).
    set_rgb(cr, s.surface);
    rounded_rect(cr, sidebar_w + 6.0, 10.0, 36.0, 10.0, 4.0);
    let _ = cr.fill();

    // Bubble mine (right, accent).
    set_rgb(cr, s.accent);
    rounded_rect(cr, w - 44.0, 26.0, 38.0, 10.0, 4.0);
    let _ = cr.fill();

    // Foreground hairline (mock composer).
    set_rgb(cr, s.fg);
    let _ = cr.rectangle(sidebar_w + 6.0, h - 10.0, w - sidebar_w - 12.0, 2.0);
    let _ = cr.fill();
}

/// "System" card: split light/dark halves with a diagonal seam.
fn draw_system(cr: &gtk::cairo::Context, w: f64, h: f64) {
    set_rgb(cr, [0xf2, 0xf2, 0xf2]);
    let _ = cr.rectangle(0.0, 0.0, w, h);
    let _ = cr.fill();

    set_rgb(cr, [0x1d, 0x1d, 0x1d]);
    let _ = cr.move_to(w, 0.0);
    let _ = cr.line_to(0.0, h);
    let _ = cr.line_to(w, h);
    let _ = cr.close_path();
    let _ = cr.fill();
}

fn set_rgb(cr: &gtk::cairo::Context, rgb: [u8; 3]) {
    cr.set_source_rgb(
        f64::from(rgb[0]) / 255.0,
        f64::from(rgb[1]) / 255.0,
        f64::from(rgb[2]) / 255.0,
    );
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    let r = r.min(w / 2.0).min(h / 2.0);
    let _ = cr.new_sub_path();
    let _ = cr.arc(
        x + w - r,
        y + r,
        r,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    let _ = cr.arc(x + w - r, y + h - r, r, 0.0, std::f64::consts::FRAC_PI_2);
    let _ = cr.arc(
        x + r,
        y + h - r,
        r,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    let _ = cr.arc(
        x + r,
        y + r,
        r,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    let _ = cr.close_path();
}

fn behavior_page(cfg: &Config, writer: Rc<DebouncedWriter>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Behavior")
        .icon_name("preferences-system-symbolic")
        .build();

    let keys = adw::PreferencesGroup::builder()
        .title("Keys")
        .description("Vim-style modal navigation.")
        .build();

    let leader = adw::EntryRow::builder()
        .title("Leader key")
        .text(&cfg.general.leader_key)
        .build();
    let writer_leader = writer.clone();
    let leader_for_handler = leader.clone();
    leader.connect_changed(move |row| {
        let raw = row.text().to_string();
        match Key::from_str(&raw) {
            Ok(_) => {
                leader_for_handler.remove_css_class("error");
                leader_for_handler.set_tooltip_text(None);
                writer_leader.queue(move |cfg| cfg.general.leader_key = raw);
            }
            Err(e) => {
                leader_for_handler.add_css_class("error");
                leader_for_handler.set_tooltip_text(Some(&format!("{e}")));
            }
        }
    });
    keys.add(&leader);
    page.add(&keys);

    let window_group = adw::PreferencesGroup::builder().title("Window").build();

    let maximized = adw::SwitchRow::builder()
        .title("Start maximized")
        .active(cfg.general.start_maximized)
        .build();
    let writer_max = writer.clone();
    maximized.connect_active_notify(move |row| {
        let v = row.is_active();
        writer_max.queue(move |cfg| cfg.general.start_maximized = v);
    });
    window_group.add(&maximized);

    let blur = adw::SwitchRow::builder()
        .title("Hyprland blur")
        .subtitle("Request transparency / blur on Hyprland")
        .active(cfg.general.hyprland_blur)
        .build();
    let writer_blur = writer.clone();
    blur.connect_active_notify(move |row| {
        let v = row.is_active();
        writer_blur.queue(move |cfg| cfg.general.hyprland_blur = v);
    });
    window_group.add(&blur);

    page.add(&window_group);
    page
}

fn notifications_page(cfg: &Config, writer: Rc<DebouncedWriter>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Notifications")
        .icon_name("notifications-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder().title("Desktop").build();

    let enabled = adw::SwitchRow::builder()
        .title("Enabled")
        .subtitle("Show desktop notifications for new messages")
        .active(cfg.notifications.enabled)
        .build();
    let writer_en = writer.clone();
    enabled.connect_active_notify(move |row| {
        let v = row.is_active();
        writer_en.queue(move |cfg| cfg.notifications.enabled = v);
    });
    group.add(&enabled);

    let sound = adw::EntryRow::builder()
        .title("Sound")
        .text(&cfg.notifications.sound)
        .build();
    let writer_sound = writer.clone();
    sound.connect_changed(move |row| {
        let value = row.text().to_string();
        writer_sound.queue(move |cfg| cfg.notifications.sound = value);
    });
    group.add(&sound);

    let dnd = adw::SwitchRow::builder()
        .title("Per-chat do not disturb")
        .subtitle("Honor per-conversation mute settings")
        .active(cfg.notifications.dnd_per_chat)
        .build();
    let writer_dnd = writer.clone();
    dnd.connect_active_notify(move |row| {
        let v = row.is_active();
        writer_dnd.queue(move |cfg| cfg.notifications.dnd_per_chat = v);
    });
    group.add(&dnd);

    page.add(&group);
    page
}

fn about_page() -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("About")
        .icon_name("dialog-information-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder().title("Kryptos").build();

    let version = adw::ActionRow::builder()
        .title("Version")
        .subtitle(env!("CARGO_PKG_VERSION"))
        .build();
    group.add(&version);

    let repo_label = gtk::Label::builder()
        .label(format!("<a href=\"{REPO_URL}\">{REPO_URL}</a>"))
        .use_markup(true)
        .selectable(true)
        .build();
    let repo = adw::ActionRow::builder().title("Repository").build();
    repo.add_suffix(&repo_label);
    group.add(&repo);

    let backend = adw::ActionRow::builder()
        .title("signal-cli backend")
        .subtitle("checking…")
        .build();
    group.add(&backend);
    spawn_version_probe(backend.clone());

    page.add(&group);
    page
}

// ---------------------------------------------------------------------------
// Async signal-cli version probe
// ---------------------------------------------------------------------------

/// Run an isolated tokio runtime on a worker thread to call
/// `SignalClient::version()`, then update `row` on the GTK main thread.
/// Errors degrade to "n/a" rather than blocking or panicking.
fn spawn_version_probe(row: adw::ActionRow) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    std::thread::Builder::new()
        .name("kryptos-prefs-version".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| Error::Config(format!("runtime: {e}")))
                    .and_then(|rt| {
                        rt.block_on(async { SignalClient::connect().await?.version().await })
                    })
            });
            let value = match result {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    warn!(error = %e, "signal-cli version probe failed");
                    "n/a".to_string()
                }
                Err(_) => {
                    warn!("signal-cli version probe panicked");
                    "n/a".to_string()
                }
            };
            let _ = tx.send(value);
        })
        .expect("spawn prefs version probe thread");

    glib::source::timeout_add_local(Duration::from_millis(150), move || match rx.try_recv() {
        Ok(value) => {
            row.set_subtitle(&value);
            glib::ControlFlow::Break
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            row.set_subtitle("n/a");
            glib::ControlFlow::Break
        }
    });
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Coalesces a burst of edits into a single disk write `DEBOUNCE` after
/// the last edit. Always re-reads from disk before mutating so concurrent
/// edits via $EDITOR aren't clobbered.
struct DebouncedWriter {
    path: PathBuf,
    pending: RefCell<Option<glib::SourceId>>,
    #[allow(clippy::type_complexity)]
    mutator: RefCell<Option<Box<dyn FnOnce(&mut Config)>>>,
}

impl DebouncedWriter {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            pending: RefCell::new(None),
            mutator: RefCell::new(None),
        }
    }

    /// Queue a config mutation. Multiple calls inside the debounce
    /// window collapse: the latest mutation wins per-field, and the
    /// final flush composes them in the order they arrived.
    fn queue<F>(self: &Rc<Self>, mutate: F)
    where
        F: FnOnce(&mut Config) + 'static,
    {
        let prev = self.mutator.borrow_mut().take();
        let combined: Box<dyn FnOnce(&mut Config)> = match prev {
            Some(p) => Box::new(move |cfg| {
                p(cfg);
                mutate(cfg);
            }),
            None => Box::new(mutate),
        };
        *self.mutator.borrow_mut() = Some(combined);

        if let Some(id) = self.pending.borrow_mut().take() {
            id.remove();
        }
        let me = Rc::clone(self);
        let id = glib::source::timeout_add_local_once(DEBOUNCE, move || me.flush());
        *self.pending.borrow_mut() = Some(id);
    }

    fn flush(self: Rc<Self>) {
        self.pending.borrow_mut().take();
        let Some(mutate) = self.mutator.borrow_mut().take() else {
            return;
        };
        let mut cfg = match loader::load_or_default(&self.path) {
            Ok(c) => c,
            Err(e) => {
                warn!(error = %e, "settings: failed to read config; using defaults");
                Config::default()
            }
        };
        mutate(&mut cfg);
        match write_config_atomic(&self.path, &cfg) {
            Ok(()) => info!(path = ?self.path, "settings persisted"),
            Err(e) => error!(error = %e, "settings: failed to write config"),
        }
    }
}

/// Serialize `cfg` as pretty TOML, write it to a sibling temp file, then
/// atomically rename over `path`. Creates parent dirs as needed. Pure
/// helper (no GTK) so it can be unit-tested.
pub fn write_config_atomic(path: &Path, cfg: &Config) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = toml::to_string_pretty(cfg)?;
    let tmp = tmp_path(path);
    std::fs::write(&tmp, body.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_else(|| std::ffi::OsString::from("config.toml"));
    name.push(".tmp");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use tempfile::tempdir;

    #[test]
    fn atomic_write_creates_parent_dirs() {
        let tmp = tempdir().unwrap();
        let nested = tmp.path().join("a/b/c/config.toml");
        let mut cfg = Config::default();
        cfg.general.theme = "gruvbox".into();
        write_config_atomic(&nested, &cfg).unwrap();
        let reloaded = loader::load(&nested).unwrap();
        assert_eq!(reloaded.general.theme, "gruvbox");
    }

    #[test]
    fn atomic_write_overwrites_existing_file() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("config.toml");
        std::fs::write(&p, "[general]\ntheme = \"catppuccin-mocha\"\n").unwrap();

        let mut cfg = Config::default();
        cfg.general.theme = "tokyo-night".into();
        write_config_atomic(&p, &cfg).unwrap();

        let reloaded = loader::load(&p).unwrap();
        assert_eq!(reloaded.general.theme, "tokyo-night");
    }

    #[test]
    fn atomic_write_leaves_no_stray_tmp() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("config.toml");
        write_config_atomic(&p, &Config::default()).unwrap();
        let stray = p.with_file_name("config.toml.tmp");
        assert!(!stray.exists(), "tmp file should have been renamed");
    }

    #[test]
    fn tmp_path_appends_tmp_suffix() {
        assert_eq!(
            tmp_path(Path::new("/etc/kryptos/config.toml")),
            PathBuf::from("/etc/kryptos/config.toml.tmp")
        );
    }
}

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
use crate::theme::builtin;
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

    let names = builtin::known_names();
    let model = gtk::StringList::new(&names);
    let combo = adw::ComboRow::builder()
        .title("Theme")
        .subtitle("Built-in palette or follow desktop")
        .model(&model)
        .build();
    if let Some(idx) = names.iter().position(|n| *n == cfg.general.theme.as_str()) {
        combo.set_selected(idx as u32);
    }
    let writer_combo = writer.clone();
    let names_owned: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    combo.connect_selected_notify(move |row| {
        let idx = row.selected() as usize;
        if let Some(name) = names_owned.get(idx).cloned() {
            writer_combo.queue(move |cfg| {
                cfg.general.theme = name;
            });
        }
    });
    group.add(&combo);
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
        .map(|n| n.to_os_string())
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

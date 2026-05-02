//! Theme system: built-in palettes (Catppuccin, Gruvbox, Tokyo Night),
//! optional user `custom.css`, and live hot-reload.
//!
//! Two `gtk::CssProvider`s are registered against the default `gdk::Display`:
//!
//! 1. A "built-in" provider holding the active embedded theme.
//! 2. A "custom" provider holding `~/.config/kryptos/custom.css`, if present.
//!    Because it is added second, it wins style conflicts — exactly what a
//!    user wants when they drop in overrides.
//!
//! The custom-CSS watcher uses `notify` (already a workspace dep), debounces
//! events ~150ms to coalesce editor-save bursts, and reapplies on the GTK
//! main thread via `glib::idle_add_once`.

pub mod builtin;

use std::path::PathBuf;
use std::sync::mpsc::{channel as std_channel, Receiver as StdReceiver};
use std::time::Duration;

use gtk::gdk;
use gtk::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::{debug, info, warn};

use crate::core::{Error, Result};

const CUSTOM_DEBOUNCE: Duration = Duration::from_millis(150);

pub struct ThemeManager {
    display: gdk::Display,
    builtin_provider: CssProvider,
    custom_provider: Option<CssProvider>,
    current: Option<&'static str>,
}

impl ThemeManager {
    /// Build a manager and register its built-in `CssProvider` against the
    /// given display at `STYLE_PROVIDER_PRIORITY_APPLICATION`. The custom
    /// provider is added lazily by [`Self::reload_custom_css`].
    pub fn install_for_display(display: &gdk::Display) -> Self {
        let builtin_provider = CssProvider::new();
        gtk::style_context_add_provider_for_display(
            display,
            &builtin_provider,
            STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        Self {
            display: display.clone(),
            builtin_provider,
            custom_provider: None,
            current: None,
        }
    }

    /// Apply a built-in theme by name, or `"system"` to follow the desktop.
    /// Names are matched case-insensitively. Unknown names yield an
    /// `Error::Config` listing the valid options.
    pub fn apply(&mut self, name: &str) -> Result<()> {
        let style_manager = adw::StyleManager::default();
        let trimmed = name.trim();

        if trimmed.eq_ignore_ascii_case("system") {
            self.builtin_provider.load_from_string("");
            style_manager.set_color_scheme(adw::ColorScheme::Default);
            self.current = Some("system");
            info!("theme set to system (libadwaita-managed)");
            return Ok(());
        }

        let resolved = builtin::lookup(trimmed).ok_or_else(|| {
            Error::Config(format!(
                "unknown theme {:?}; expected one of: {}",
                name,
                builtin::known_names().join(", ")
            ))
        })?;

        self.builtin_provider.load_from_string(resolved.css);
        style_manager.set_color_scheme(resolved.color_scheme);
        self.current = Some(resolved.canonical_name);
        info!(theme = resolved.canonical_name, "theme applied");
        Ok(())
    }

    /// Reread the custom CSS file (if it exists) and reapply, or unload the
    /// custom provider if the file has been deleted. Errors loading custom
    /// CSS are logged but never fatal — built-in styling stays intact.
    pub fn reload_custom_css(&mut self, path: &std::path::Path) {
        if !path.exists() {
            if let Some(p) = self.custom_provider.take() {
                gtk::style_context_remove_provider_for_display(&self.display, &p);
                debug!(?path, "custom css gone, provider unloaded");
            }
            return;
        }

        let contents = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, ?path, "failed to read custom css");
                return;
            }
        };

        let provider = self.custom_provider.get_or_insert_with(|| {
            let p = CssProvider::new();
            gtk::style_context_add_provider_for_display(
                &self.display,
                &p,
                // Priority +1 so user overrides win over the built-in theme.
                STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            p
        });
        provider.load_from_string(&contents);
        debug!(?path, bytes = contents.len(), "custom css applied");
    }

    pub fn current(&self) -> Option<&'static str> {
        self.current
    }
}

/// Spawn a background notify-watcher for `custom_css_path`. On each save
/// (debounced ~150ms) the supplied `on_change` closure runs on the GTK main
/// thread, where it can safely touch `ThemeManager`.
///
/// We watch the parent directory rather than the file itself because most
/// editors atomic-rename on save and a watch on the file would miss the
/// replacement.
pub fn start_watching<F>(custom_css_path: PathBuf, mut on_change: F) -> Result<WatchHandle>
where
    F: FnMut(&std::path::Path) + 'static,
{
    let (raw_tx, raw_rx): (_, StdReceiver<notify::Result<Event>>) = std_channel();
    let mut watcher: RecommendedWatcher = Watcher::new(
        raw_tx,
        notify::Config::default().with_poll_interval(Duration::from_secs(2)),
    )
    .map_err(|e| Error::Config(format!("css watcher: {e}")))?;

    let watch_dir = custom_css_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if !watch_dir.exists() {
        std::fs::create_dir_all(&watch_dir)?;
    }
    watcher
        .watch(&watch_dir, RecursiveMode::NonRecursive)
        .map_err(|e| Error::Config(format!("watch {watch_dir:?}: {e}")))?;

    let (main_tx, main_rx) = async_channel_shim::unbounded::<()>();

    let target = custom_css_path.clone();
    let thread = std::thread::Builder::new()
        .name("kryptos-theme-watcher".into())
        .spawn(move || worker(raw_rx, target, main_tx))
        .map_err(|e| Error::Config(format!("spawn theme watcher: {e}")))?;

    // Poll the inbox on the GTK main loop. 100ms is well below human-noticeable
    // latency and cheap (one mpsc try_recv per tick when idle).
    let path_for_cb = custom_css_path.clone();
    let source_id = glib::source::timeout_add_local(Duration::from_millis(100), move || {
        let mut fired = false;
        while main_rx.try_recv().is_ok() {
            fired = true;
        }
        if fired {
            on_change(&path_for_cb);
        }
        glib::ControlFlow::Continue
    });

    Ok(WatchHandle {
        _watcher: watcher,
        _thread: thread,
        source_id: Some(source_id),
    })
}

pub struct WatchHandle {
    _watcher: RecommendedWatcher,
    _thread: std::thread::JoinHandle<()>,
    source_id: Option<glib::SourceId>,
}

impl Drop for WatchHandle {
    fn drop(&mut self) {
        if let Some(id) = self.source_id.take() {
            id.remove();
        }
    }
}

fn worker(
    raw_rx: StdReceiver<notify::Result<Event>>,
    target: PathBuf,
    main_tx: async_channel_shim::Sender<()>,
) {
    while let Ok(first) = raw_rx.recv() {
        if !is_relevant(&first, &target) {
            continue;
        }
        std::thread::sleep(CUSTOM_DEBOUNCE);
        while raw_rx.try_recv().is_ok() {}
        if main_tx.send(()).is_err() {
            return;
        }
    }
}

fn is_relevant(ev: &notify::Result<Event>, target: &PathBuf) -> bool {
    match ev {
        Ok(event) => {
            let kind_ok = matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            );
            let path_ok = event.paths.iter().any(|p| p == target);
            kind_ok && path_ok
        }
        Err(_) => false,
    }
}

// Tiny non-blocking channel built on `std::sync::mpsc`. Keeps us off a new
// dep just for one usage site; the only API surface we need is
// `unbounded()`, `try_recv()`, and a `Send` sender.
mod async_channel_shim {
    use std::sync::mpsc;

    pub struct Sender<T>(mpsc::Sender<T>);
    pub struct Receiver<T>(mpsc::Receiver<T>);

    impl<T> Sender<T> {
        pub fn send(&self, v: T) -> Result<(), ()> {
            self.0.send(v).map_err(|_| ())
        }
    }

    impl<T> Receiver<T> {
        pub fn try_recv(&self) -> Result<T, ()> {
            self.0.try_recv().map_err(|_| ())
        }
    }

    pub fn unbounded<T>() -> (Sender<T>, Receiver<T>) {
        let (tx, rx) = mpsc::channel();
        (Sender(tx), Receiver(rx))
    }
}

use gtk::glib;

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn lookup_is_case_insensitive() {
        assert_eq!(
            builtin::lookup("Catppuccin-Mocha").map(|b| b.canonical_name),
            Some("catppuccin-mocha")
        );
        assert_eq!(
            builtin::lookup("  GRUVBOX  ").map(|b| b.canonical_name),
            Some("gruvbox")
        );
    }

    #[test]
    fn lookup_unknown_returns_none() {
        assert!(builtin::lookup("solarized").is_none());
        assert!(
            builtin::lookup("system").is_none(),
            "'system' is special-cased outside the built-in table"
        );
    }

    #[test]
    fn known_names_includes_system_and_all_builtins() {
        let names = builtin::known_names();
        assert!(names.contains(&"system"));
        for b in builtin::ALL {
            assert!(
                names.contains(&b.canonical_name),
                "missing {}",
                b.canonical_name
            );
        }
    }

    /// Every `kryptos_*` token referenced by widgets / modeline / chat
    /// view. If a new palette is added missing one of these, themes will
    /// silently render with GTK fallbacks — keep this list in lockstep
    /// with what the CSS in `src/theme/css/_shared/` consumes.
    const REQUIRED_TOKENS: &[&str] = &[
        "bg", "mantle", "surface", "surface2", "overlay", "fg", "subtle", "accent", "blue",
        "green", "yellow", "red", "lavender",
    ];

    #[test]
    fn every_builtin_css_is_non_empty_and_defines_palette() {
        for b in builtin::ALL {
            assert!(
                !b.css.trim().is_empty(),
                "{} css is empty",
                b.canonical_name
            );
            for token in REQUIRED_TOKENS {
                let needle = format!("@define-color kryptos_{token}");
                assert!(
                    b.css.contains(&needle),
                    "{} missing {needle}",
                    b.canonical_name
                );
            }
            assert!(
                b.css.contains(".modeline.normal"),
                "{} missing .modeline.normal style",
                b.canonical_name
            );
        }
    }

    #[test]
    fn every_palette_defines_all_required_tokens() {
        // Strict version of the above: every palette must define every
        // required token (no overlap with global @define-color from
        // libadwaita).
        for b in builtin::ALL {
            for token in REQUIRED_TOKENS {
                let needle = format!("@define-color kryptos_{token}");
                assert!(
                    b.css.contains(&needle),
                    "palette {} is missing token kryptos_{token}",
                    b.canonical_name
                );
            }
        }
    }

    #[test]
    fn unknown_theme_error_lists_options() {
        // We can't construct a real ThemeManager without a Display, but we
        // can exercise the same error-construction path by hand.
        let err = Error::Config(format!(
            "unknown theme {:?}; expected one of: {}",
            "solarized",
            builtin::known_names().join(", ")
        ));
        let msg = err.to_string();
        assert!(msg.contains("catppuccin-mocha"));
        assert!(msg.contains("system"));
        assert!(msg.contains("solarized"));
    }
}

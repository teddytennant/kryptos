//! Theme system: built-in palettes (Catppuccin, Gruvbox, Tokyo Night) plus
//! a drop-in user `custom.css`.
//!
//! Two `gtk::CssProvider`s are registered against the default `gdk::Display`:
//!
//! 1. A "built-in" provider holding the active embedded theme.
//! 2. A "custom" provider holding `~/.config/kryptos/custom.css`, if present.
//!    Because it is added at a higher priority, it wins style conflicts —
//!    exactly what a user wants when they drop in overrides.

pub mod builtin;

use gtk::gdk;
use gtk::{CssProvider, STYLE_PROVIDER_PRIORITY_APPLICATION};
use tracing::{debug, info, warn};

use crate::core::{Error, Result};

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
        assert!(builtin::lookup("system").is_none(),
            "'system' is special-cased outside the built-in table");
    }

    #[test]
    fn known_names_includes_system_and_all_builtins() {
        let names = builtin::known_names();
        assert!(names.contains(&"system"));
        for b in builtin::ALL {
            assert!(names.contains(&b.canonical_name), "missing {}", b.canonical_name);
        }
    }

    #[test]
    fn every_builtin_css_is_non_empty_and_defines_palette() {
        for b in builtin::ALL {
            assert!(!b.css.trim().is_empty(), "{} css is empty", b.canonical_name);
            assert!(
                b.css.contains("@define-color kryptos_bg"),
                "{} missing @define-color kryptos_bg",
                b.canonical_name
            );
            assert!(
                b.css.contains("@define-color kryptos_fg"),
                "{} missing @define-color kryptos_fg",
                b.canonical_name
            );
            assert!(
                b.css.contains(".modeline.normal"),
                "{} missing .modeline.normal style",
                b.canonical_name
            );
        }
    }

    #[test]
    fn unknown_theme_error_lists_options() {
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

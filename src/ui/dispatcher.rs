//! Glue between `vim::Action` and the GTK widget tree. The dispatcher
//! holds clones of widget handles and is invoked whenever the
//! [`Engine`](crate::vim::Engine) emits an action. It also runs
//! `:command` lines and `/search` queries.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use tracing::{debug, error, info};

use crate::config::loader;
use crate::theme::ThemeManager;
use crate::vim::{Action, Mode};

use super::commands::{
    apply_set, help_text, mutate_config_on_disk, parse_command, theme_names_csv, Command,
};
use super::composer::{Composer, ComposerMode};
use super::onboarding;
use super::settings::Settings;
use super::statusline::CommandBar;
use super::window::{move_sidebar_selection, WindowParts};

#[derive(Clone)]
pub struct Dispatcher {
    pub window: adw::ApplicationWindow,
    pub sidebar_list: gtk::ListBox,
    pub composer: Composer,
    pub command_bar: CommandBar,
    pub content_title: adw::WindowTitle,
    pub toast_overlay: adw::ToastOverlay,
    pub theme: Rc<RefCell<ThemeManager>>,
    pub config_path: PathBuf,
    /// Active substring filter for the sidebar (`/search`). Empty = no filter.
    search_filter: Rc<RefCell<String>>,
}

impl Dispatcher {
    pub fn new(
        parts: &WindowParts,
        theme: Rc<RefCell<ThemeManager>>,
        config_path: PathBuf,
    ) -> Self {
        let me = Self {
            window: parts.window.clone(),
            sidebar_list: parts.sidebar_list.clone(),
            composer: parts.composer.clone(),
            command_bar: parts.command_bar.clone(),
            content_title: parts.content_title.clone(),
            toast_overlay: parts.toast_overlay.clone(),
            theme,
            config_path,
            search_filter: Rc::new(RefCell::new(String::new())),
        };
        me.install_sidebar_filter();
        me
    }

    /// Returns the new [`Mode`] the engine should be in afterwards, or
    /// `None` if the action does not change mode.
    pub fn dispatch(&self, action: &Action, current: Mode) -> Option<Mode> {
        debug!(?action, ?current, "dispatch");
        match action {
            Action::NavigateDown => {
                move_sidebar_selection(&self.sidebar_list, 1);
                None
            }
            Action::NavigateUp => {
                move_sidebar_selection(&self.sidebar_list, -1);
                None
            }
            Action::CommandPalette => {
                self.command_bar.show(":");
                Some(Mode::Command)
            }
            Action::Search => {
                self.command_bar.show("/");
                Some(Mode::Search)
            }
            Action::ComposeNew | Action::Reply => {
                self.composer.focus();
                self.composer.set_mode(ComposerMode::Insert);
                Some(Mode::Insert)
            }
            Action::LeaveInsert => {
                self.composer.set_mode(ComposerMode::Normal);
                gtk::prelude::GtkWindowExt::set_focus(&self.window, gtk::Widget::NONE);
                Some(Mode::Normal)
            }
            Action::SendMessage => {
                let text = self.composer.drain();
                if text.trim().is_empty() {
                    debug!("SendMessage with empty composer; ignored");
                } else {
                    info!(message = %text, "send message (placeholder)");
                }
                self.composer.set_mode(ComposerMode::Normal);
                Some(Mode::Normal)
            }
            Action::Quit => {
                self.window.close();
                None
            }
            Action::SetTheme => {
                // The keymap-level :theme entry without args isn't useful
                // yet — surface a hint and the valid set of names.
                self.toast_info(&format!("themes: {}", theme_names_csv()));
                None
            }
            Action::ReloadConfig => {
                self.reload_config();
                None
            }
            other => {
                debug!(?other, "unhandled action");
                None
            }
        }
    }

    /// Run a `:command` line.
    pub fn run_command(&self, line: &str) {
        let cmd = parse_command(line);
        debug!(?cmd, "run_command");
        match cmd {
            Command::Empty => {}
            Command::Quit => self.window.close(),
            Command::Write => {
                // TODO: route through composer-send once we have a real Signal session.
                info!(":w — TODO send composer");
                self.toast_info(":w — send not yet wired");
            }
            Command::Theme(None) => {
                self.toast_info(&format!("themes: {}", theme_names_csv()));
            }
            Command::Theme(Some(name)) => match self.theme.borrow_mut().apply(&name) {
                Ok(()) => self.toast_info(&format!("theme: {name}")),
                Err(e) => self.toast_error(&format!("{e}")),
            },
            Command::Set { key, value } => self.apply_set_to_disk(&key, value.as_deref()),
            Command::Reload => self.reload_config(),
            Command::Settings => Settings::open(&self.window),
            Command::Link(_) => onboarding::open_linker(&self.window),
            Command::Help => self.toast_info(help_text()),
            Command::Unknown(head) => self.toast_error(&format!("unknown command: :{head}")),
        }
    }

    /// Run a `/search` query — substring-filter the sidebar list.
    /// Empty input clears the filter.
    pub fn run_search(&self, line: &str) {
        let q = line.trim().to_string();
        info!(query = %q, "search");
        *self.search_filter.borrow_mut() = q;
        self.sidebar_list.invalidate_filter();
    }

    /// Esc on the command bar in Search mode should clear the filter,
    /// not just close the bar. The window-level wiring calls this.
    pub fn clear_search(&self) {
        self.search_filter.borrow_mut().clear();
        self.sidebar_list.invalidate_filter();
    }

    // -----------------------------------------------------------------------
    // Internals
    // -----------------------------------------------------------------------

    fn install_sidebar_filter(&self) {
        let filter = self.search_filter.clone();
        self.sidebar_list
            .set_filter_func(move |row| row_matches(row, &filter.borrow()));
    }

    fn apply_set_to_disk(&self, key: &str, value: Option<&str>) {
        if key.is_empty() {
            self.toast_error(":set requires a key");
            return;
        }
        let key_owned = key.to_string();
        let value_owned = value.map(|s| s.to_string());
        let result = mutate_config_on_disk(&self.config_path, |cfg| {
            apply_set(cfg, &key_owned, value_owned.as_deref()).map(|_| ())
        });
        match result {
            Ok(()) => self.toast_info(&format!("{key} updated")),
            Err(e) => self.toast_error(&format!("{e}")),
        }
    }

    fn reload_config(&self) {
        let cfg = match loader::load_or_default(&self.config_path) {
            Ok(c) => c,
            Err(e) => {
                error!(error = %e, "reload: load failed");
                self.toast_error(&format!("reload: {e}"));
                return;
            }
        };
        match self.theme.borrow_mut().apply(&cfg.general.theme) {
            Ok(()) => self.toast_info(&format!("reloaded — theme {}", cfg.general.theme)),
            Err(e) => self.toast_error(&format!("theme: {e}")),
        }
    }

    fn toast_info(&self, msg: &str) {
        let toast = adw::Toast::builder().title(msg).timeout(3).build();
        self.toast_overlay.add_toast(toast);
    }

    fn toast_error(&self, msg: &str) {
        let toast = adw::Toast::builder()
            .title(msg)
            .timeout(5)
            .priority(adw::ToastPriority::High)
            .build();
        self.toast_overlay.add_toast(toast);
    }
}

/// Walk a `ListBoxRow`'s subtree for a `gtk::Label` and check if its
/// text contains `needle` (case-insensitive). v1: just match the first
/// label (the row's title). Empty `needle` ⇒ all rows pass.
fn row_matches(row: &gtk::ListBoxRow, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let needle_lc = needle.to_ascii_lowercase();
    if let Some(text) = first_label_text(row.upcast_ref::<gtk::Widget>()) {
        return text.to_ascii_lowercase().contains(&needle_lc);
    }
    true
}

/// Depth-first scan for the first `gtk::Label` descendant.
fn first_label_text(widget: &gtk::Widget) -> Option<String> {
    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
        return Some(label.text().to_string());
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        if let Some(found) = first_label_text(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

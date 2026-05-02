//! Glue between `vim::Action` and the GTK widget tree. The dispatcher
//! holds weak-ish (clone of) handles to widgets and is invoked whenever
//! the [`Engine`](crate::vim::Engine) emits an action.

use adw::prelude::*;
use tracing::{debug, info};

use crate::vim::{Action, Mode};

use super::statusline::CommandBar;
use super::window::{drain_composer, move_sidebar_selection, WindowParts};

#[derive(Clone)]
pub struct Dispatcher {
    pub window: adw::ApplicationWindow,
    pub sidebar_list: gtk::ListBox,
    pub composer: gtk::TextView,
    pub command_bar: CommandBar,
    pub content_title: adw::WindowTitle,
}

impl Dispatcher {
    pub fn from_parts(parts: &WindowParts) -> Self {
        Self {
            window: parts.window.clone(),
            sidebar_list: parts.sidebar_list.clone(),
            composer: parts.composer.clone(),
            command_bar: parts.command_bar.clone(),
            content_title: parts.content_title.clone(),
        }
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
                self.composer.grab_focus();
                Some(Mode::Insert)
            }
            Action::LeaveInsert => {
                gtk::prelude::GtkWindowExt::set_focus(&self.window, gtk::Widget::NONE);
                Some(Mode::Normal)
            }
            Action::SendMessage => {
                let text = drain_composer(&self.composer);
                if text.trim().is_empty() {
                    debug!("SendMessage with empty composer; ignored");
                } else {
                    info!(message = %text, "send message (placeholder)");
                }
                Some(Mode::Normal)
            }
            Action::Quit => {
                self.window.close();
                None
            }
            Action::SetTheme => {
                info!("set_theme action — TODO");
                None
            }
            Action::ReloadConfig => {
                info!("reload_config action — TODO");
                None
            }
            other => {
                debug!(?other, "unhandled action");
                None
            }
        }
    }

    /// Run a `:command` line. For v1 we recognise `:q`, `:quit`, and
    /// `:theme <name>`.
    pub fn run_command(&self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let mut parts = line.split_whitespace();
        let head = parts.next().unwrap_or("");
        match head {
            "q" | "quit" => self.window.close(),
            "theme" => {
                let name = parts.next().unwrap_or("");
                if name.is_empty() {
                    info!("`:theme` with no argument — TODO list themes");
                } else {
                    info!(theme = %name, "theme change (placeholder)");
                    self.content_title.set_subtitle(&format!("theme: {name}"));
                }
            }
            "reload" => info!("reload — TODO"),
            other => info!(cmd = %other, "unknown command"),
        }
    }

    pub fn run_search(&self, line: &str) {
        info!(query = %line.trim(), "search (placeholder)");
    }
}

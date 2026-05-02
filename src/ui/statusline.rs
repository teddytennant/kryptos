//! Bottom-of-window UI: a vim-style mode indicator and a single-line
//! command/search bar. Both widgets are stateless wrappers over plain
//! GTK widgets — the parent shell owns them and drives state changes.

use gtk::prelude::*;

use crate::vim::Mode;

const MODE_CSS_CLASSES: &[&str] = &[
    "mode-normal",
    "mode-insert",
    "mode-command",
    "mode-search",
];

#[derive(Debug, Clone)]
pub struct ModeLine {
    label: gtk::Label,
    container: gtk::Box,
}

impl ModeLine {
    pub fn new() -> Self {
        let label = gtk::Label::builder()
            .label("-- NORMAL --")
            .xalign(0.0)
            .build();
        label.add_css_class("mode-line");
        label.add_css_class("mode-normal");

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        container.add_css_class("mode-line-row");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_margin_top(2);
        container.set_margin_bottom(2);
        container.append(&label);

        Self { label, container }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.container.upcast_ref::<gtk::Widget>()
    }

    pub fn set_mode(&self, mode: Mode) {
        let (text, class) = match mode {
            Mode::Normal => ("-- NORMAL --", "mode-normal"),
            Mode::Insert => ("-- INSERT --", "mode-insert"),
            Mode::Command => (":", "mode-command"),
            Mode::Search => ("/", "mode-search"),
        };
        self.label.set_label(text);
        for c in MODE_CSS_CLASSES {
            self.label.remove_css_class(c);
        }
        self.label.add_css_class(class);
    }
}

impl Default for ModeLine {
    fn default() -> Self {
        Self::new()
    }
}

/// A single-line input bar used for both `:command` and `/search`. The
/// caller wires the activate / cancel callbacks; the bar itself doesn't
/// know what to do with the text.
#[derive(Debug, Clone)]
pub struct CommandBar {
    container: gtk::Box,
    prefix: gtk::Label,
    entry: gtk::Entry,
}

impl CommandBar {
    pub fn new() -> Self {
        let prefix = gtk::Label::builder().label(":").build();
        prefix.add_css_class("command-bar-prefix");

        let entry = gtk::Entry::builder()
            .hexpand(true)
            .has_frame(false)
            .build();
        entry.add_css_class("command-bar-entry");

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        container.add_css_class("command-bar");
        container.set_margin_start(8);
        container.set_margin_end(8);
        container.set_visible(false);
        container.append(&prefix);
        container.append(&entry);

        Self {
            container,
            prefix,
            entry,
        }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.container.upcast_ref::<gtk::Widget>()
    }

    pub fn entry(&self) -> &gtk::Entry {
        &self.entry
    }

    /// Show the bar with the given one-character prefix and focus the entry.
    pub fn show(&self, prefix: &str) {
        self.prefix.set_label(prefix);
        self.entry.set_text("");
        self.container.set_visible(true);
        self.entry.grab_focus();
    }

    pub fn hide(&self) {
        self.container.set_visible(false);
        self.entry.set_text("");
    }

    pub fn is_visible(&self) -> bool {
        self.container.is_visible()
    }
}

impl Default for CommandBar {
    fn default() -> Self {
        Self::new()
    }
}

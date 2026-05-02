//! Bottom-of-window UI: a vim-style mode indicator and a single-line
//! command/search bar. Both widgets are stateless wrappers over plain
//! GTK widgets — the parent shell owns them and drives state changes.
//!
//! The mode line is shaped like a Neovim statusline: a coloured mode
//! block on the far left, a centre region with the active chat / counts,
//! and a right region with pending vim keys + account number. The block
//! and section labels are public via [`ModeLine::set_mode`],
//! [`ModeLine::set_center`], [`ModeLine::set_pending`], and
//! [`ModeLine::set_account`] so the dispatcher can update them.

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
    /// Preserved for backward-compat with the original API: the legacy
    /// "-- NORMAL --" label still exists, just hidden inside the mode
    /// block, which means callers reading `.label` keep working.
    label: gtk::Label,
    mode_block: gtk::Label,
    center_label: gtk::Label,
    pending_label: gtk::Label,
    account_label: gtk::Label,
    container: gtk::Box,
}

impl ModeLine {
    pub fn new() -> Self {
        // Left: solid-colour mode block ("NORMAL" / "INSERT" / ...).
        let mode_block = gtk::Label::builder().label("NORMAL").build();
        mode_block.add_css_class("modeline-mode-block");
        mode_block.add_css_class("mode-normal");

        // The legacy `.mode-line` label is kept around as an invisible
        // shadow so external code that reads `.label` doesn't break.
        let label = gtk::Label::new(Some("-- NORMAL --"));
        label.add_css_class("mode-line");
        label.add_css_class("mode-normal");
        label.set_visible(false);

        let left = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        left.add_css_class("modeline-section");
        left.add_css_class("left");
        left.append(&mode_block);

        // Centre: chat name + counts (driven by `set_center`).
        let center_label = gtk::Label::builder().label("").xalign(0.5).build();
        let center = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .hexpand(true)
            .halign(gtk::Align::Center)
            .build();
        center.add_css_class("modeline-section");
        center.add_css_class("center");
        center.append(&center_label);

        // Right: pending vim keys (e.g. half-typed `d`, `gg`). Account
        // info is intentionally absent until we have something
        // meaningful to show (a real account is linked, or unread totals).
        let pending_label = gtk::Label::builder().label("").build();
        let account_label = gtk::Label::builder().label("").build();
        account_label.set_visible(false);
        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        right.add_css_class("modeline-section");
        right.add_css_class("right");
        right.append(&pending_label);
        right.append(&account_label);

        let container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .build();
        container.add_css_class("mode-line-row");
        container.add_css_class("kryptos-modeline");
        container.append(&left);
        container.append(&center);
        container.append(&right);
        container.append(&label);

        Self {
            label,
            mode_block,
            center_label,
            pending_label,
            account_label,
            container,
        }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.container.upcast_ref::<gtk::Widget>()
    }

    pub fn set_mode(&self, mode: Mode) {
        let (block_text, legacy_text, class) = match mode {
            Mode::Normal => ("NORMAL", "-- NORMAL --", "mode-normal"),
            Mode::Insert => ("INSERT", "-- INSERT --", "mode-insert"),
            Mode::Command => ("CMD", ":", "mode-command"),
            Mode::Search => ("SEARCH", "/", "mode-search"),
        };
        self.mode_block.set_label(block_text);
        self.label.set_label(legacy_text);
        for c in MODE_CSS_CLASSES {
            self.mode_block.remove_css_class(c);
            self.label.remove_css_class(c);
        }
        self.mode_block.add_css_class(class);
        self.label.add_css_class(class);
    }

    /// Update the centre section (chat name + counts).
    #[allow(dead_code)]
    pub fn set_center(&self, text: &str) {
        self.center_label.set_label(text);
    }

    /// Update the right-hand pending-keys readout (`d`, `g`, etc.).
    #[allow(dead_code)]
    pub fn set_pending(&self, text: &str) {
        self.pending_label.set_label(text);
    }

    /// Update the right-hand account indicator.
    #[allow(dead_code)]
    pub fn set_account(&self, text: &str) {
        self.account_label.set_label(text);
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

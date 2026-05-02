//! Bottom-of-window UI: a vim-style mode indicator and a single-line
//! command/search bar. Both widgets are stateless wrappers over plain
//! GTK widgets — the parent shell owns them and drives state changes.
//!
//! The mode line is shaped like a Neovim statusline: a coloured bar
//! glyph + uppercase letter-spaced label on the far left, the chat name
//! (dim, weight 500) in the centre, and pending vim keys / unread
//! counter on the right. We lay it out with `gtk::CenterBox` so the
//! centre section is geometrically centred regardless of the widths
//! of the side sections.
//!
//! The block and section labels are public via [`ModeLine::set_mode`],
//! [`ModeLine::set_center`], [`ModeLine::set_pending`], and
//! [`ModeLine::set_unread`] so the dispatcher can update them.

use gtk::prelude::*;

use crate::vim::Mode;

const MODE_CSS_CLASSES: &[&str] = &[
    "mode-normal",
    "mode-insert",
    "mode-command",
    "mode-search",
];

/// Glyph used as a coloured leading bar before the mode label. The
/// "left half block" reads as a hairline-thin column of accent colour.
const MODE_BAR_GLYPH: &str = "\u{258E}"; // ▎

/// Vertical hairline used between segments in the right-hand section.
const VERTICAL_SEPARATOR: &str = "\u{258F}"; // ▏

#[derive(Debug, Clone)]
pub struct ModeLine {
    /// Preserved for backward-compat with the original API: the legacy
    /// "▎ NORMAL" label still exists, just hidden inside the mode block,
    /// which means callers reading `.label` keep working.
    label: gtk::Label,
    /// `▎` accent-coloured glyph that sits before the uppercase label.
    mode_glyph: gtk::Label,
    mode_block: gtk::Label,
    center_label: gtk::Label,
    pending_label: gtk::Label,
    separator_label: gtk::Label,
    unread_label: gtk::Label,
    container: gtk::CenterBox,
}

impl ModeLine {
    pub fn new() -> Self {
        // Left: ▎ NORMAL — a colour bar carrying the mode meaning, then
        // an uppercase letter-spaced label.
        let mode_glyph = gtk::Label::builder().label(MODE_BAR_GLYPH).build();
        mode_glyph.add_css_class("modeline-mode-glyph");
        mode_glyph.add_css_class("mode-normal");

        let mode_block = gtk::Label::builder().label("NORMAL").build();
        mode_block.add_css_class("modeline-mode-block");
        mode_block.add_css_class("mode-normal");

        // The legacy `.mode-line` label is kept as an invisible shadow
        // so external code that reads `.label` doesn't break.
        let label = gtk::Label::new(Some("▎ NORMAL"));
        label.add_css_class("mode-line");
        label.add_css_class("mode-normal");
        label.set_visible(false);

        let left = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .build();
        left.add_css_class("modeline-section");
        left.add_css_class("left");
        left.append(&mode_glyph);
        left.append(&mode_block);

        // Centre: chat name (dim weight 500). Empty until something is
        // worth saying — keep the line quiet.
        let center_label = gtk::Label::builder()
            .label("")
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .single_line_mode(true)
            .build();
        center_label.add_css_class("modeline-center");
        let center = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        center.add_css_class("modeline-section");
        center.add_css_class("center");
        center.append(&center_label);

        // Right: pending vim keys (e.g. half-typed `g`), then a vertical
        // hairline `▏`, then `<n> unread` — both blank until non-empty.
        let pending_label = gtk::Label::builder().label("").build();
        pending_label.add_css_class("modeline-pending");

        let separator_label = gtk::Label::builder().label(VERTICAL_SEPARATOR).build();
        separator_label.add_css_class("modeline-separator");
        separator_label.set_visible(false);

        let unread_label = gtk::Label::builder().label("").build();
        unread_label.add_css_class("modeline-unread");
        unread_label.set_visible(false);

        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .build();
        right.add_css_class("modeline-section");
        right.add_css_class("right");
        right.append(&pending_label);
        right.append(&separator_label);
        right.append(&unread_label);
        right.append(&label);

        // CenterBox guarantees the center widget is geometrically
        // centred regardless of side widths — Müller-Brockmann would
        // approve.
        let container = gtk::CenterBox::new();
        container.add_css_class("mode-line-row");
        container.add_css_class("kryptos-modeline");
        container.set_start_widget(Some(&left));
        container.set_center_widget(Some(&center));
        container.set_end_widget(Some(&right));

        Self {
            label,
            mode_glyph,
            mode_block,
            center_label,
            pending_label,
            separator_label,
            unread_label,
            container,
        }
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.container.upcast_ref::<gtk::Widget>()
    }

    pub fn set_mode(&self, mode: Mode) {
        let (block_text, legacy_text, class) = match mode {
            Mode::Normal => ("NORMAL", "▎ NORMAL", "mode-normal"),
            Mode::Insert => ("INSERT", "▎ INSERT", "mode-insert"),
            Mode::Command => ("CMD", "▎ CMD", "mode-command"),
            Mode::Search => ("SEARCH", "▎ SEARCH", "mode-search"),
        };
        self.mode_block.set_label(block_text);
        self.label.set_label(legacy_text);
        for c in MODE_CSS_CLASSES {
            self.mode_glyph.remove_css_class(c);
            self.mode_block.remove_css_class(c);
            self.label.remove_css_class(c);
        }
        self.mode_glyph.add_css_class(class);
        self.mode_block.add_css_class(class);
        self.label.add_css_class(class);
    }

    /// Update the centre section (chat name).
    #[allow(dead_code)]
    pub fn set_center(&self, text: &str) {
        self.center_label.set_label(text);
    }

    /// Update the right-hand pending-keys readout (`d`, `g`, etc.).
    #[allow(dead_code)]
    pub fn set_pending(&self, text: &str) {
        self.pending_label.set_label(text);
        self.refresh_right_visibility();
    }

    /// Update the right-hand unread counter. Pass `0` to hide.
    #[allow(dead_code)]
    pub fn set_unread(&self, count: u32) {
        if count == 0 {
            self.unread_label.set_label("");
        } else {
            // Keep tnum-friendly, no comma grouping yet.
            self.unread_label.set_label(&format!("{count} unread"));
        }
        self.refresh_right_visibility();
    }

    /// Show/hide the vertical hairline based on whether both sides have
    /// content. The separator only earns its space when it's actually
    /// dividing two visible things.
    fn refresh_right_visibility(&self) {
        let has_pending = !self.pending_label.label().is_empty();
        let has_unread = !self.unread_label.label().is_empty();
        self.unread_label.set_visible(has_unread);
        self.separator_label.set_visible(has_pending && has_unread);
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

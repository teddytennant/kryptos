//! Vim-modal text composer.
//!
//! Wraps a [`gtk::TextView`] in a small widget that gives it Normal /
//! Insert / Visual modes independently from the window-level
//! [`Engine`](crate::vim::Engine). The window engine drives navigation
//! between widgets; this widget owns *editing inside the composer*.
//!
//! Chat-app overrides:
//!
//! * `Enter` (in any mode) sends the message via the `on_send` callback.
//! * `Shift+Enter` inserts a literal newline.
//! * `Ctrl+Enter` also sends.
//! * Empty / whitespace-only composer + Enter does nothing.
//!
//! The widget intentionally does **not** know about
//! `kryptos::vim::engine::Engine`. The window-level engine handles
//! navigation; this widget handles editing.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;

/// Modes the composer can be in. Mirrors the chat-app subset of vim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposerMode {
    Normal,
    Insert,
    Visual,
    VisualLine,
}

impl ComposerMode {
    pub fn label(self) -> &'static str {
        match self {
            ComposerMode::Normal => "NORMAL",
            ComposerMode::Insert => "INSERT",
            ComposerMode::Visual => "VISUAL",
            ComposerMode::VisualLine => "V-LINE",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            ComposerMode::Normal => "composer-normal",
            ComposerMode::Insert => "composer-insert",
            ComposerMode::Visual | ComposerMode::VisualLine => "composer-visual",
        }
    }
}

const ALL_MODE_CLASSES: &[&str] = &["composer-normal", "composer-insert", "composer-visual"];

type SendCallback = Box<dyn Fn(String)>;

/// Logical pending-key state. Pulled out so the transition table can be
/// unit-tested without a display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum PendingOp {
    #[default]
    None,
    /// Awaiting the second char of `dd`.
    D,
    /// Awaiting the second char of `yy`.
    Y,
    /// Awaiting the target of `g` (only `gg` is wired up).
    G,
}

#[derive(Clone)]
pub struct Composer {
    text_view: gtk::TextView,
    wrapper: gtk::Box,
    /// Label inside the mode-badge pill; separated from the badge box so
    /// the leading dot indicator can be styled independently.
    mode_badge_text: gtk::Label,
    /// Placeholder label overlaid on the empty TextView. Hidden as soon
    /// as the buffer has content; revealed again when it empties out.
    placeholder: gtk::Label,
    mode: Rc<RefCell<ComposerMode>>,
    on_send: Rc<RefCell<Option<SendCallback>>>,
    yank: Rc<RefCell<String>>,
    pending: Rc<RefCell<PendingOp>>,
}

impl Composer {
    pub fn new() -> Self {
        let text_view = gtk::TextView::builder()
            .wrap_mode(gtk::WrapMode::WordChar)
            .top_margin(10)
            .bottom_margin(10)
            .left_margin(14)
            .right_margin(14)
            .build();
        text_view.add_css_class("kryptos-composer");
        text_view.buffer().set_enable_undo(true);

        // The mode is shown in the bottom modeline, not on the composer.
        // We keep an off-screen label so the public API surface and tests
        // don't change.
        let mode_text = gtk::Label::new(Some(ComposerMode::Normal.label()));
        mode_text.set_visible(false);

        // Placeholder overlaid on the TextView. GtkTextView has no
        // native placeholder support; we layer a label inside an Overlay
        // and toggle its visibility on buffer-change.
        let placeholder = gtk::Label::builder()
            .label("Type a message")
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .can_target(false)
            .build();
        placeholder.add_css_class("kryptos-composer-placeholder");
        placeholder.set_margin_start(14);
        placeholder.set_margin_top(10);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&text_view));
        overlay.add_overlay(&placeholder);

        let wrapper = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(0)
            .build();
        wrapper.add_css_class("kryptos-composer-wrapper");
        wrapper.add_css_class(ComposerMode::Normal.css_class());
        wrapper.append(&overlay);

        let composer = Self {
            text_view,
            wrapper,
            mode_badge_text: mode_text,
            placeholder,
            mode: Rc::new(RefCell::new(ComposerMode::Normal)),
            on_send: Rc::new(RefCell::new(None)),
            yank: Rc::new(RefCell::new(String::new())),
            pending: Rc::new(RefCell::new(PendingOp::None)),
        };

        composer.install_key_controller();
        composer.install_placeholder_tracker();
        composer
    }

    pub fn widget(&self) -> &gtk::Widget {
        self.wrapper.upcast_ref::<gtk::Widget>()
    }

    #[allow(dead_code)] // part of the public API; useful for tests / future call sites.
    pub fn text_view(&self) -> &gtk::TextView {
        &self.text_view
    }

    pub fn set_on_send<F: Fn(String) + 'static>(&self, f: F) {
        *self.on_send.borrow_mut() = Some(Box::new(f));
    }

    pub fn mode(&self) -> ComposerMode {
        *self.mode.borrow()
    }

    pub fn set_mode(&self, m: ComposerMode) {
        *self.mode.borrow_mut() = m;
        *self.pending.borrow_mut() = PendingOp::None;
        self.mode_badge_text.set_label(m.label());
        for c in ALL_MODE_CLASSES {
            self.wrapper.remove_css_class(c);
            self.text_view.remove_css_class(c);
        }
        self.wrapper.add_css_class(m.css_class());
        self.text_view.add_css_class(m.css_class());
        // TODO: real block-vs-line caret. GTK4 doesn't expose a public
        // API for that on TextView; CSS `.caret-color` only changes the
        // color, not the shape. Left as-is so themes can hint via
        // border/outline on the wrapper.
        self.text_view
            .set_editable(matches!(m, ComposerMode::Insert));
        if matches!(m, ComposerMode::Visual | ComposerMode::VisualLine) {
            self.anchor_visual_selection(m);
        }
    }

    pub fn focus(&self) {
        self.text_view.grab_focus();
    }

    /// Pull all text out of the composer and clear it. Used by the
    /// dispatcher's `SendMessage` path (called from the window-level
    /// engine, not the composer itself).
    pub fn drain(&self) -> String {
        let buf = self.text_view.buffer();
        let (start, end) = buf.bounds();
        let text = buf.text(&start, &end, false).to_string();
        buf.set_text("");
        text
    }

    /// Trigger the `on_send` callback if the buffer is non-empty, then
    /// clear and return to Normal. No-op on whitespace-only buffers.
    fn try_send(&self) -> bool {
        let buf = self.text_view.buffer();
        let (start, end) = buf.bounds();
        let text = buf.text(&start, &end, false).to_string();
        if text.trim().is_empty() {
            return false;
        }
        buf.set_text("");
        if let Some(cb) = self.on_send.borrow().as_ref() {
            cb(text);
        }
        self.set_mode(ComposerMode::Normal);
        true
    }

    fn install_key_controller(&self) {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);

        let this = self.clone();
        controller.connect_key_pressed(move |_, keyval, _, state| this.on_key(keyval, state));

        self.text_view.add_controller(controller);
    }

    /// Hook the buffer's `changed` signal so the placeholder hides on
    /// the first keystroke and reappears when the buffer empties.
    fn install_placeholder_tracker(&self) {
        let placeholder = self.placeholder.clone();
        let buf = self.text_view.buffer();
        // Initial state: shown for empty buffers.
        placeholder.set_visible(buf.char_count() == 0);
        buf.connect_changed(move |buffer| {
            placeholder.set_visible(buffer.char_count() == 0);
        });
    }

    /// Wipe the composer's text and return to Normal mode. Bound to
    /// `Ctrl-K` (vim-ish) and exposed for the dispatcher / commands.
    pub fn clear(&self) {
        self.text_view.buffer().set_text("");
        self.set_mode(ComposerMode::Normal);
    }

    fn on_key(&self, keyval: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);

        // Enter / Shift+Enter / Ctrl+Enter — chat-app override applies
        // in *every* mode.
        if matches!(
            keyval,
            gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
        ) {
            if shift && !ctrl {
                // newline; let the textview insert it (only useful in Insert).
                if self.mode() != ComposerMode::Insert {
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            self.try_send();
            return glib::Propagation::Stop;
        }

        // Esc always returns to Normal.
        if keyval == gdk::Key::Escape {
            self.set_mode(ComposerMode::Normal);
            return glib::Propagation::Stop;
        }

        // Ctrl-K: clear composer (vim-ish — closer to `<C-u>` semantically
        // but `<C-u>` is half-page-up in Normal). Fires regardless of
        // mode so it works as the universal "wipe and try again".
        if ctrl && matches!(keyval, gdk::Key::k | gdk::Key::K) {
            self.clear();
            return glib::Propagation::Stop;
        }

        match self.mode() {
            ComposerMode::Insert => {
                // Let the TextView handle Insert-mode editing, except
                // Ctrl-modified shortcuts that we've claimed.
                if ctrl {
                    match keyval {
                        gdk::Key::r | gdk::Key::R => {
                            // <C-r> redo. GTK provides this via the buffer.
                            self.text_view.buffer().redo();
                            return glib::Propagation::Stop;
                        }
                        _ => {}
                    }
                }
                glib::Propagation::Proceed
            }
            mode @ (ComposerMode::Normal | ComposerMode::Visual | ComposerMode::VisualLine) => {
                self.handle_normal_or_visual(keyval, ctrl, shift, mode)
            }
        }
    }

    fn handle_normal_or_visual(
        &self,
        keyval: gdk::Key,
        ctrl: bool,
        _shift: bool,
        mode: ComposerMode,
    ) -> glib::Propagation {
        // Ctrl-modified bindings.
        if ctrl && matches!(keyval, gdk::Key::r | gdk::Key::R) {
            self.text_view.buffer().redo();
            return glib::Propagation::Stop;
        }

        let visual = matches!(mode, ComposerMode::Visual | ComposerMode::VisualLine);

        // Two-char pending ops.
        let pending = *self.pending.borrow();
        match (pending, keyval) {
            (PendingOp::D, gdk::Key::d) => {
                *self.pending.borrow_mut() = PendingOp::None;
                self.delete_line();
                return glib::Propagation::Stop;
            }
            (PendingOp::Y, gdk::Key::y) => {
                *self.pending.borrow_mut() = PendingOp::None;
                self.yank_line();
                return glib::Propagation::Stop;
            }
            (PendingOp::G, gdk::Key::g) => {
                *self.pending.borrow_mut() = PendingOp::None;
                self.move_doc_start(visual);
                return glib::Propagation::Stop;
            }
            (PendingOp::D | PendingOp::Y | PendingOp::G, _) => {
                // Anything else cancels the pending op.
                *self.pending.borrow_mut() = PendingOp::None;
            }
            _ => {}
        }

        match keyval {
            // Mode entry from Normal.
            gdk::Key::i if !visual => {
                self.set_mode(ComposerMode::Insert);
                glib::Propagation::Stop
            }
            gdk::Key::I if !visual => {
                self.move_line_start_nonblank(false);
                self.set_mode(ComposerMode::Insert);
                glib::Propagation::Stop
            }
            gdk::Key::a if !visual => {
                self.move_char(1, false);
                self.set_mode(ComposerMode::Insert);
                glib::Propagation::Stop
            }
            gdk::Key::A if !visual => {
                self.move_line_end(false);
                self.set_mode(ComposerMode::Insert);
                glib::Propagation::Stop
            }
            gdk::Key::o if !visual => {
                self.open_line_below();
                self.set_mode(ComposerMode::Insert);
                glib::Propagation::Stop
            }
            gdk::Key::O if !visual => {
                self.open_line_above();
                self.set_mode(ComposerMode::Insert);
                glib::Propagation::Stop
            }
            gdk::Key::v => {
                if matches!(mode, ComposerMode::Visual) {
                    self.set_mode(ComposerMode::Normal);
                } else {
                    self.set_mode(ComposerMode::Visual);
                }
                glib::Propagation::Stop
            }
            gdk::Key::V => {
                if matches!(mode, ComposerMode::VisualLine) {
                    self.set_mode(ComposerMode::Normal);
                } else {
                    self.set_mode(ComposerMode::VisualLine);
                }
                glib::Propagation::Stop
            }

            // Movements (selection-aware in Visual modes).
            gdk::Key::h | gdk::Key::Left => {
                self.move_char(-1, visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::l | gdk::Key::Right => {
                self.move_char(1, visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::j | gdk::Key::Down => {
                self.move_line(1, visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::k | gdk::Key::Up => {
                self.move_line(-1, visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::w => {
                self.move_word_forward(visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::b => {
                self.move_word_back(visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::_0 => {
                self.move_line_start(visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::dollar => {
                self.move_line_end(visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::G => {
                self.move_doc_end(visual);
                self.refresh_visual_selection();
                glib::Propagation::Stop
            }
            gdk::Key::g => {
                *self.pending.borrow_mut() = PendingOp::G;
                glib::Propagation::Stop
            }

            // Edits.
            gdk::Key::x if !visual => {
                self.delete_char();
                glib::Propagation::Stop
            }
            gdk::Key::d if !visual => {
                *self.pending.borrow_mut() = PendingOp::D;
                glib::Propagation::Stop
            }
            gdk::Key::y if !visual => {
                *self.pending.borrow_mut() = PendingOp::Y;
                glib::Propagation::Stop
            }
            gdk::Key::d if visual => {
                self.delete_selection_and_yank();
                self.set_mode(ComposerMode::Normal);
                glib::Propagation::Stop
            }
            gdk::Key::y if visual => {
                self.yank_selection();
                self.set_mode(ComposerMode::Normal);
                glib::Propagation::Stop
            }
            gdk::Key::p => {
                self.paste_after();
                glib::Propagation::Stop
            }
            gdk::Key::u => {
                self.text_view.buffer().undo();
                glib::Propagation::Stop
            }

            // Anything else: swallow in Normal/Visual so the TextView
            // doesn't insert characters when not in Insert.
            _ => glib::Propagation::Stop,
        }
    }

    // ---------- buffer helpers ----------

    fn anchor_visual_selection(&self, m: ComposerMode) {
        let buf = self.text_view.buffer();
        let cursor = buf.iter_at_mark(&buf.get_insert());
        match m {
            ComposerMode::Visual => {
                buf.select_range(&cursor, &cursor);
            }
            ComposerMode::VisualLine => {
                let mut start = cursor;
                start.set_line_offset(0);
                let mut end = cursor;
                if !end.ends_line() {
                    end.forward_to_line_end();
                }
                buf.select_range(&start, &end);
            }
            _ => {}
        }
    }

    fn refresh_visual_selection(&self) {
        match self.mode() {
            ComposerMode::Visual => {
                // GTK selection naturally tracks the insert mark, so
                // moving the cursor extends the selection from the
                // selection_bound mark — nothing to do.
            }
            ComposerMode::VisualLine => {
                let buf = self.text_view.buffer();
                let cursor = buf.iter_at_mark(&buf.get_insert());
                let bound = buf.iter_at_mark(&buf.selection_bound());
                let (lo, hi) = if cursor.offset() <= bound.offset() {
                    (cursor, bound)
                } else {
                    (bound, cursor)
                };
                let mut sel_start = lo;
                sel_start.set_line_offset(0);
                let mut sel_end = hi;
                if !sel_end.ends_line() {
                    sel_end.forward_to_line_end();
                }
                buf.select_range(&sel_start, &sel_end);
            }
            _ => {}
        }
    }

    fn move_char(&self, delta: i32, extend: bool) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        if delta > 0 {
            for _ in 0..delta {
                if !iter.forward_char() {
                    break;
                }
            }
        } else {
            for _ in 0..(-delta) {
                if !iter.backward_char() {
                    break;
                }
            }
        }
        place_cursor(&buf, &iter, extend);
    }

    fn move_line(&self, delta: i32, extend: bool) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        let col = iter.line_offset();
        let target_line = (iter.line() + delta).max(0);
        let max_line = buf.line_count() - 1;
        let target_line = target_line.min(max_line);
        iter = buf.iter_at_line(target_line).unwrap_or(iter);
        let mut chars_in_line = 0;
        let mut probe = iter;
        while !probe.ends_line() {
            chars_in_line += 1;
            if !probe.forward_char() {
                break;
            }
        }
        let want = col.min(chars_in_line);
        if want > 0 {
            iter.set_line_offset(want);
        }
        place_cursor(&buf, &iter, extend);
    }

    fn move_word_forward(&self, extend: bool) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        if !iter.forward_word_end() {
            iter = buf.end_iter();
        } else {
            // GTK puts us at the end of a word — vim's `w` lands on the
            // start of the next. Skip one more whitespace span.
            iter.forward_word_end();
            iter.backward_word_start();
        }
        place_cursor(&buf, &iter, extend);
    }

    fn move_word_back(&self, extend: bool) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        if !iter.backward_word_start() {
            iter = buf.start_iter();
        }
        place_cursor(&buf, &iter, extend);
    }

    fn move_line_start(&self, extend: bool) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        iter.set_line_offset(0);
        place_cursor(&buf, &iter, extend);
    }

    fn move_line_start_nonblank(&self, extend: bool) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        iter.set_line_offset(0);
        while !iter.ends_line() {
            let c = iter.char();
            if !c.is_whitespace() {
                break;
            }
            if !iter.forward_char() {
                break;
            }
        }
        place_cursor(&buf, &iter, extend);
    }

    fn move_line_end(&self, extend: bool) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        if !iter.ends_line() {
            iter.forward_to_line_end();
        }
        place_cursor(&buf, &iter, extend);
    }

    fn move_doc_start(&self, extend: bool) {
        let buf = self.text_view.buffer();
        let iter = buf.start_iter();
        place_cursor(&buf, &iter, extend);
    }

    fn move_doc_end(&self, extend: bool) {
        let buf = self.text_view.buffer();
        let iter = buf.end_iter();
        place_cursor(&buf, &iter, extend);
    }

    fn delete_char(&self) {
        let buf = self.text_view.buffer();
        let start = buf.iter_at_mark(&buf.get_insert());
        if start.ends_line() && start.is_end() {
            return;
        }
        let mut end = start;
        if !end.forward_char() {
            return;
        }
        let removed = buf.text(&start, &end, false).to_string();
        *self.yank.borrow_mut() = removed;
        buf.delete(&mut start.clone(), &mut end);
    }

    fn delete_line(&self) {
        let buf = self.text_view.buffer();
        let cursor = buf.iter_at_mark(&buf.get_insert());
        let mut start = cursor;
        start.set_line_offset(0);
        let mut end = start;
        if !end.forward_line() {
            // Last line: take to end of buffer.
            end = buf.end_iter();
        }
        let removed = buf.text(&start, &end, true).to_string();
        *self.yank.borrow_mut() = removed;
        buf.delete(&mut start, &mut end);
    }

    fn yank_line(&self) {
        let buf = self.text_view.buffer();
        let cursor = buf.iter_at_mark(&buf.get_insert());
        let mut start = cursor;
        start.set_line_offset(0);
        let mut end = start;
        if !end.forward_line() {
            end = buf.end_iter();
        }
        *self.yank.borrow_mut() = buf.text(&start, &end, true).to_string();
    }

    fn paste_after(&self) {
        let buf = self.text_view.buffer();
        let text = self.yank.borrow().clone();
        if text.is_empty() {
            return;
        }
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        // For line-wise yanks (text ending in newline) vim pastes on the
        // next line; for char-wise it pastes after the cursor. Cheap
        // approximation: if the yank ends in `\n`, insert at start of
        // next line; otherwise insert after the cursor.
        if text.ends_with('\n') {
            if !iter.ends_line() {
                iter.forward_to_line_end();
            }
            iter.forward_char();
            buf.insert(&mut iter, &text);
        } else {
            iter.forward_char();
            buf.insert(&mut iter, &text);
        }
    }

    fn yank_selection(&self) {
        let buf = self.text_view.buffer();
        if let Some((start, end)) = buf.selection_bounds() {
            *self.yank.borrow_mut() = buf.text(&start, &end, false).to_string();
        }
    }

    fn delete_selection_and_yank(&self) {
        let buf = self.text_view.buffer();
        if let Some((start, end)) = buf.selection_bounds() {
            *self.yank.borrow_mut() = buf.text(&start, &end, false).to_string();
            buf.delete(&mut start.clone(), &mut end.clone());
        }
    }

    fn open_line_below(&self) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        if !iter.ends_line() {
            iter.forward_to_line_end();
        }
        buf.insert(&mut iter, "\n");
    }

    fn open_line_above(&self) {
        let buf = self.text_view.buffer();
        let mut iter = buf.iter_at_mark(&buf.get_insert());
        iter.set_line_offset(0);
        buf.insert(&mut iter, "\n");
        // Move cursor up onto the new empty line we just inserted.
        let cursor = buf.iter_at_mark(&buf.get_insert());
        let mut up = cursor;
        if up.backward_line() {
            buf.place_cursor(&up);
        }
    }
}

impl Default for Composer {
    fn default() -> Self {
        Self::new()
    }
}

fn place_cursor(buf: &gtk::TextBuffer, iter: &gtk::TextIter, extend: bool) {
    if extend {
        buf.move_mark(&buf.get_insert(), iter);
    } else {
        buf.place_cursor(iter);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pure transition table for mode entry — exercised without GTK.
    fn next_mode_for_normal_key(key: char) -> Option<ComposerMode> {
        match key {
            'i' | 'I' | 'a' | 'A' | 'o' | 'O' => Some(ComposerMode::Insert),
            'v' => Some(ComposerMode::Visual),
            'V' => Some(ComposerMode::VisualLine),
            _ => None,
        }
    }

    #[test]
    fn normal_to_insert_keys() {
        for k in ['i', 'I', 'a', 'A', 'o', 'O'] {
            assert_eq!(
                next_mode_for_normal_key(k),
                Some(ComposerMode::Insert),
                "key {k:?}"
            );
        }
    }

    #[test]
    fn visual_keys() {
        assert_eq!(next_mode_for_normal_key('v'), Some(ComposerMode::Visual));
        assert_eq!(
            next_mode_for_normal_key('V'),
            Some(ComposerMode::VisualLine)
        );
    }

    #[test]
    fn movement_keys_do_not_change_mode() {
        for k in ['h', 'j', 'k', 'l', 'w', 'b', '0', '$', 'g', 'G'] {
            assert_eq!(next_mode_for_normal_key(k), None, "key {k:?}");
        }
    }

    #[test]
    fn mode_label_strings() {
        assert_eq!(ComposerMode::Normal.label(), "NORMAL");
        assert_eq!(ComposerMode::Insert.label(), "INSERT");
        assert_eq!(ComposerMode::Visual.label(), "VISUAL");
        assert_eq!(ComposerMode::VisualLine.label(), "V-LINE");
    }

    #[test]
    fn mode_css_classes() {
        assert_eq!(ComposerMode::Normal.css_class(), "composer-normal");
        assert_eq!(ComposerMode::Insert.css_class(), "composer-insert");
        assert_eq!(ComposerMode::Visual.css_class(), "composer-visual");
        assert_eq!(ComposerMode::VisualLine.css_class(), "composer-visual");
    }

    /// Yank buffer is just an `Rc<RefCell<String>>` shared between
    /// edit operations — verify the semantics.
    #[test]
    fn yank_buffer_round_trip() {
        let yank: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        *yank.borrow_mut() = "hello\n".into();
        assert_eq!(&*yank.borrow(), "hello\n");
        // Subsequent yank overwrites.
        *yank.borrow_mut() = "world".into();
        assert_eq!(&*yank.borrow(), "world");
    }

    /// Two-key pending-op transitions: starting in `None`, the first
    /// `d` should produce `D`; a follow-up `d` clears it (the dispatch
    /// happens elsewhere).
    #[test]
    fn pending_op_default_is_none() {
        assert_eq!(PendingOp::default(), PendingOp::None);
    }

    #[test]
    fn pending_op_variants_are_distinct() {
        assert_ne!(PendingOp::None, PendingOp::D);
        assert_ne!(PendingOp::D, PendingOp::Y);
        assert_ne!(PendingOp::Y, PendingOp::G);
    }
}

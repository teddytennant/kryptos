//! Translate raw GTK key events into [`crate::vim::Key`] and feed the
//! shared [`Engine`].
//!
//! The pure translation function [`translate_gdk`] is split out so the
//! tricky bits (named-key map, modifier handling, character normalisation)
//! can be unit-tested without a display.

use gtk::gdk;

use crate::vim::key::{Key, KeySym, Modifiers};

/// Map a `gdk::Key` + `gdk::ModifierType` pair to a vim [`Key`], or
/// `None` if the keysym is something we ignore (e.g. a bare modifier
/// press).
pub fn translate_gdk(keyval: gdk::Key, state: gdk::ModifierType) -> Option<Key> {
    let mods = translate_mods(state);

    if let Some(name) = named_for(keyval) {
        return Some(Key {
            sym: KeySym::named(name),
            mods,
        });
    }

    // Bare modifier key presses produce keysyms we don't want to feed
    // into the engine — they're not actionable on their own.
    if is_bare_modifier(keyval) {
        return None;
    }

    let c = keyval.to_unicode()?;
    if c.is_control() {
        return None;
    }

    // GTK reports an uppercase keyval for shifted letters. The engine's
    // single-char keys are post-shift, so we keep `c` as-is and drop
    // the shift modifier flag for printable characters to avoid double
    // counting — except when Ctrl/Alt/Meta are also held, where the
    // shift flag is meaningful (e.g. `<C-S-x>`).
    let mods = if mods.ctrl || mods.alt || mods.meta {
        mods
    } else {
        Modifiers {
            shift: false,
            ..mods
        }
    };

    Some(Key {
        sym: KeySym::Char(c),
        mods,
    })
}

fn translate_mods(state: gdk::ModifierType) -> Modifiers {
    Modifiers {
        ctrl: state.contains(gdk::ModifierType::CONTROL_MASK),
        alt: state.contains(gdk::ModifierType::ALT_MASK),
        shift: state.contains(gdk::ModifierType::SHIFT_MASK),
        meta: state.contains(gdk::ModifierType::SUPER_MASK)
            || state.contains(gdk::ModifierType::META_MASK),
    }
}

fn named_for(k: gdk::Key) -> Option<&'static str> {
    Some(match k {
        gdk::Key::Escape => "Esc",
        gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter => "Enter",
        gdk::Key::Tab | gdk::Key::ISO_Left_Tab => "Tab",
        gdk::Key::space | gdk::Key::KP_Space => "Space",
        gdk::Key::BackSpace => "BS",
        gdk::Key::Delete | gdk::Key::KP_Delete => "Del",
        gdk::Key::Insert | gdk::Key::KP_Insert => "Insert",
        gdk::Key::Up | gdk::Key::KP_Up => "Up",
        gdk::Key::Down | gdk::Key::KP_Down => "Down",
        gdk::Key::Left | gdk::Key::KP_Left => "Left",
        gdk::Key::Right | gdk::Key::KP_Right => "Right",
        gdk::Key::Home | gdk::Key::KP_Home => "Home",
        gdk::Key::End | gdk::Key::KP_End => "End",
        gdk::Key::Page_Up | gdk::Key::KP_Page_Up => "PageUp",
        gdk::Key::Page_Down | gdk::Key::KP_Page_Down => "PageDown",
        gdk::Key::F1 => "F1",
        gdk::Key::F2 => "F2",
        gdk::Key::F3 => "F3",
        gdk::Key::F4 => "F4",
        gdk::Key::F5 => "F5",
        gdk::Key::F6 => "F6",
        gdk::Key::F7 => "F7",
        gdk::Key::F8 => "F8",
        gdk::Key::F9 => "F9",
        gdk::Key::F10 => "F10",
        gdk::Key::F11 => "F11",
        gdk::Key::F12 => "F12",
        _ => return None,
    })
}

fn is_bare_modifier(k: gdk::Key) -> bool {
    matches!(
        k,
        gdk::Key::Shift_L
            | gdk::Key::Shift_R
            | gdk::Key::Control_L
            | gdk::Key::Control_R
            | gdk::Key::Alt_L
            | gdk::Key::Alt_R
            | gdk::Key::Super_L
            | gdk::Key::Super_R
            | gdk::Key::Meta_L
            | gdk::Key::Meta_R
            | gdk::Key::Caps_Lock
            | gdk::Key::Shift_Lock
            | gdk::Key::Num_Lock
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn t(k: gdk::Key, state: gdk::ModifierType) -> Option<Key> {
        translate_gdk(k, state)
    }

    #[test]
    fn translates_named_keys() {
        assert_eq!(
            t(gdk::Key::Escape, gdk::ModifierType::empty()),
            Some(Key::named("Esc"))
        );
        assert_eq!(
            t(gdk::Key::Return, gdk::ModifierType::empty()),
            Some(Key::named("Enter"))
        );
        assert_eq!(
            t(gdk::Key::space, gdk::ModifierType::empty()),
            Some(Key::named("Space"))
        );
        assert_eq!(
            t(gdk::Key::BackSpace, gdk::ModifierType::empty()),
            Some(Key::named("BS"))
        );
        assert_eq!(
            t(gdk::Key::Up, gdk::ModifierType::empty()),
            Some(Key::named("Up"))
        );
    }

    #[test]
    fn translates_printable_chars() {
        assert_eq!(
            t(gdk::Key::j, gdk::ModifierType::empty()),
            Some(Key::char('j'))
        );
        assert_eq!(
            t(gdk::Key::slash, gdk::ModifierType::empty()),
            Some(Key::char('/'))
        );
        assert_eq!(
            t(gdk::Key::colon, gdk::ModifierType::empty()),
            Some(Key::char(':'))
        );
    }

    #[test]
    fn applies_ctrl_modifier_to_named() {
        let key = t(gdk::Key::Return, gdk::ModifierType::CONTROL_MASK).unwrap();
        assert_eq!(key.sym, KeySym::named("Enter"));
        assert!(key.mods.ctrl);
        assert!(!key.mods.alt);
    }

    #[test]
    fn drops_shift_for_unmodified_printables() {
        // Shift+a comes in as keyval 'A' with SHIFT_MASK set; we want
        // the engine to see plain `Key::char('A')`, not `<S-A>`.
        let key = t(gdk::Key::A, gdk::ModifierType::SHIFT_MASK).unwrap();
        assert_eq!(key, Key::char('A'));
    }

    #[test]
    fn keeps_shift_when_combined_with_ctrl() {
        let key = t(
            gdk::Key::X,
            gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK,
        )
        .unwrap();
        assert!(key.mods.ctrl);
        assert!(key.mods.shift);
    }

    #[test]
    fn ignores_bare_modifier_keys() {
        assert!(t(gdk::Key::Shift_L, gdk::ModifierType::SHIFT_MASK).is_none());
        assert!(t(gdk::Key::Control_R, gdk::ModifierType::CONTROL_MASK).is_none());
    }
}

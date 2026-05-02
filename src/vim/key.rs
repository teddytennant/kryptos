//! Abstract keyboard key representation, decoupled from any toolkit.
//!
//! The UI layer translates GTK key events into [`Key`] values; the
//! [`engine`](super::engine) operates only on these. That keeps the
//! vim core toolkit-free and trivially unit-testable.

use std::fmt;
use std::str::FromStr;

use crate::core::{Error, Result};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        meta: false,
    };

    pub fn is_empty(&self) -> bool {
        *self == Self::NONE
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeySym {
    /// Printable single character (post-shift, e.g. 'A' if Shift+a).
    Char(char),
    /// Named non-printable key (e.g. "Esc", "Enter", "F1").
    /// Names are canonicalised on construction.
    Named(String),
}

impl KeySym {
    pub fn named<S: AsRef<str>>(s: S) -> Self {
        Self::Named(canonicalize_named(s.as_ref()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Key {
    pub sym: KeySym,
    pub mods: Modifiers,
}

impl Key {
    pub const fn raw(sym: KeySym, mods: Modifiers) -> Self {
        Self { sym, mods }
    }

    pub fn char(c: char) -> Self {
        Self {
            sym: KeySym::Char(c),
            mods: Modifiers::NONE,
        }
    }

    pub fn named<S: AsRef<str>>(name: S) -> Self {
        Self {
            sym: KeySym::named(name),
            mods: Modifiers::NONE,
        }
    }

    pub fn with_mods(mut self, mods: Modifiers) -> Self {
        self.mods = mods;
        self
    }

    /// Sentinel representing the user's configured leader key. The
    /// engine substitutes this for the real leader at bind time.
    pub fn leader() -> Self {
        Self::named("leader")
    }

    pub fn is_leader(&self) -> bool {
        self.mods.is_empty() && matches!(&self.sym, KeySym::Named(s) if s == "leader")
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.mods.is_empty() {
            if let KeySym::Char(c) = self.sym {
                return write!(f, "{c}");
            }
        }
        f.write_str("<")?;
        if self.mods.ctrl {
            f.write_str("C-")?;
        }
        if self.mods.alt {
            f.write_str("A-")?;
        }
        if self.mods.shift {
            f.write_str("S-")?;
        }
        if self.mods.meta {
            f.write_str("M-")?;
        }
        match &self.sym {
            KeySym::Char(c) => write!(f, "{c}")?,
            KeySym::Named(n) => f.write_str(n)?,
        }
        f.write_str(">")
    }
}

impl FromStr for Key {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::Config("empty key".into()));
        }

        if !s.starts_with('<') {
            let mut chars = s.chars();
            let c = chars.next().unwrap();
            if chars.next().is_some() {
                return Err(Error::Config(format!("multi-char key without <…>: {s:?}")));
            }
            return Ok(Key::char(c));
        }

        let inner = s
            .strip_prefix('<')
            .and_then(|t| t.strip_suffix('>'))
            .ok_or_else(|| Error::Config(format!("malformed key: {s:?}")))?;
        if inner.is_empty() {
            return Err(Error::Config(format!("empty <…>: {s:?}")));
        }

        // Last segment is the keysym; preceding ones are modifier flags.
        let parts: Vec<&str> = inner.split('-').collect();
        let (sym_str, mod_strs) = parts.split_last().unwrap();
        if sym_str.is_empty() {
            return Err(Error::Config(format!("missing keysym in {s:?}")));
        }

        let mut mods = Modifiers::NONE;
        for m in mod_strs {
            match *m {
                "C" => mods.ctrl = true,
                "A" => mods.alt = true,
                "S" => mods.shift = true,
                "M" => mods.meta = true,
                other => {
                    return Err(Error::Config(format!(
                        "unknown modifier {other:?} in {s:?}"
                    )));
                }
            }
        }

        let sym = if sym_str.chars().count() == 1 {
            KeySym::Char(sym_str.chars().next().unwrap())
        } else {
            KeySym::named(*sym_str)
        };
        Ok(Key { sym, mods })
    }
}

fn canonicalize_named(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    match lower.as_str() {
        "esc" | "escape" => "Esc".into(),
        "cr" | "enter" | "return" => "Enter".into(),
        "space" | "spc" => "Space".into(),
        "tab" => "Tab".into(),
        "bs" | "backspace" => "BS".into(),
        "del" | "delete" => "Del".into(),
        "ins" | "insert" => "Insert".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "home" => "Home".into(),
        "end" => "End".into(),
        "pageup" | "pgup" => "PageUp".into(),
        "pagedown" | "pgdn" | "pgdown" => "PageDown".into(),
        "leader" => "leader".into(),
        other if other.starts_with('f') && other[1..].parse::<u8>().is_ok() => {
            format!("F{}", &other[1..])
        }
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn p(s: &str) -> Key {
        Key::from_str(s).unwrap()
    }

    #[test]
    fn parses_single_char() {
        assert_eq!(p("j"), Key::char('j'));
        assert_eq!(p("/"), Key::char('/'));
        assert_eq!(p(":"), Key::char(':'));
    }

    #[test]
    fn parses_named() {
        assert_eq!(p("<Esc>"), Key::named("Esc"));
        assert_eq!(p("<Enter>"), Key::named("Enter"));
        assert_eq!(p("<Space>"), Key::named("Space"));
        assert_eq!(p("<F1>"), Key::named("F1"));
    }

    #[test]
    fn canonicalises_aliases() {
        assert_eq!(p("<CR>"), p("<Enter>"));
        assert_eq!(p("<escape>"), p("<Esc>"));
        assert_eq!(p("<spc>"), p("<Space>"));
    }

    #[test]
    fn parses_modifiers() {
        let mut mods = Modifiers::NONE;
        mods.ctrl = true;
        assert_eq!(p("<C-Enter>"), Key::named("Enter").with_mods(mods));

        let mut mods = Modifiers::NONE;
        mods.ctrl = true;
        mods.shift = true;
        assert_eq!(p("<C-S-x>"), Key::char('x').with_mods(mods));
    }

    #[test]
    fn rejects_garbage() {
        assert!(Key::from_str("").is_err());
        assert!(Key::from_str("<>").is_err());
        assert!(Key::from_str("<Esc").is_err());
        assert!(Key::from_str("ab").is_err());
        assert!(Key::from_str("<Z-x>").is_err()); // unknown modifier
        assert!(Key::from_str("<C->").is_err()); // missing sym
    }

    #[test]
    fn display_roundtrips() {
        for s in [
            "j",
            "<Esc>",
            "<Enter>",
            "<Space>",
            "<C-Enter>",
            "<C-S-x>",
            "<F1>",
        ] {
            assert_eq!(p(s).to_string(), s, "roundtrip for {s}");
        }
    }

    #[test]
    fn leader_sentinel() {
        let k = p("<leader>");
        assert!(k.is_leader());
        let other = p("<Space>");
        assert!(!other.is_leader());
    }
}

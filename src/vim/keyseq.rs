//! Sequences of [`Key`] values, parsed from concise vim-style strings
//! such as `"gg"`, `"<Space>c"`, or `"<C-Enter>"`.

use std::fmt;
use std::str::FromStr;

use crate::core::{Error, Result};

use super::key::Key;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeySeq(pub Vec<Key>);

impl KeySeq {
    pub fn new(keys: impl IntoIterator<Item = Key>) -> Self {
        Self(keys.into_iter().collect())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for KeySeq {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for k in &self.0 {
            write!(f, "{k}")?;
        }
        Ok(())
    }
}

impl FromStr for KeySeq {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        let mut keys = Vec::new();
        let mut chars = s.chars().peekable();
        while let Some(&c) = chars.peek() {
            if c == '<' {
                let mut buf = String::new();
                buf.push(chars.next().unwrap());
                let mut closed = false;
                for ch in chars.by_ref() {
                    buf.push(ch);
                    if ch == '>' {
                        closed = true;
                        break;
                    }
                }
                if !closed {
                    return Err(Error::Config(format!(
                        "unterminated <…> in key sequence {s:?}"
                    )));
                }
                keys.push(Key::from_str(&buf)?);
            } else {
                let c = chars.next().unwrap();
                keys.push(Key::char(c));
            }
        }
        if keys.is_empty() {
            return Err(Error::Config(format!("empty key sequence {s:?}")));
        }
        Ok(KeySeq(keys))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn p(s: &str) -> KeySeq {
        KeySeq::from_str(s).unwrap()
    }

    #[test]
    fn parses_single_chars() {
        assert_eq!(p("j"), KeySeq::new([Key::char('j')]));
        assert_eq!(p("gg"), KeySeq::new([Key::char('g'), Key::char('g')]));
        assert_eq!(p("dd"), KeySeq::new([Key::char('d'), Key::char('d')]));
    }

    #[test]
    fn parses_mixed() {
        assert_eq!(
            p("<Space>c"),
            KeySeq::new([Key::named("Space"), Key::char('c')])
        );
        assert_eq!(
            p("<leader>r"),
            KeySeq::new([Key::leader(), Key::char('r')])
        );
        assert_eq!(
            p("<C-x><C-c>"),
            KeySeq::new([
                Key::char('x').with_mods({
                    let mut m = super::super::key::Modifiers::NONE;
                    m.ctrl = true;
                    m
                }),
                Key::char('c').with_mods({
                    let mut m = super::super::key::Modifiers::NONE;
                    m.ctrl = true;
                    m
                }),
            ])
        );
    }

    #[test]
    fn display_roundtrips() {
        for s in ["j", "gg", "<Space>c", "<C-Enter>", "<leader>r"] {
            assert_eq!(p(s).to_string(), s, "roundtrip for {s}");
        }
    }

    #[test]
    fn rejects_empty_or_unterminated() {
        assert!(KeySeq::from_str("").is_err());
        assert!(KeySeq::from_str("<Esc").is_err());
    }
}

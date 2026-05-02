//! Trie-based keymap. Holds a tree of [`Key`] prefixes so we can
//! distinguish *pending* sequences (like `g` in `gg`) from
//! *unambiguous* matches (like `j`).

use std::collections::HashMap;

use crate::core::Result;

use super::action::Action;
use super::key::Key;
use super::keyseq::KeySeq;

#[derive(Debug, Default)]
pub struct Keymap {
    root: TrieNode,
}

#[derive(Debug, Default)]
struct TrieNode {
    action: Option<Action>,
    children: HashMap<Key, TrieNode>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Prefix is a binding and no longer binding extends it.
    /// Caller should fire the action immediately.
    Match(Action),
    /// Prefix is a binding *and* a longer binding could still extend
    /// it. Caller should wait for more keys (or commit on timeout).
    AmbiguousMatch(Action),
    /// Prefix is the start of one or more bindings — wait for more.
    Pending,
    /// No binding starts with this prefix — clear pending and abort.
    None,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a binding. Replacing an existing binding at the same
    /// sequence is allowed — silently overwrites.
    pub fn bind(&mut self, seq: KeySeq, action: Action) -> Result<()> {
        let mut node = &mut self.root;
        for key in seq.0 {
            node = node.children.entry(key).or_default();
        }
        node.action = Some(action);
        Ok(())
    }

    pub fn lookup(&self, prefix: &[Key]) -> Lookup {
        let mut node = &self.root;
        for key in prefix {
            match node.children.get(key) {
                Some(child) => node = child,
                None => return Lookup::None,
            }
        }
        match (&node.action, node.children.is_empty()) {
            (Some(a), true) => Lookup::Match(a.clone()),
            (Some(a), false) => Lookup::AmbiguousMatch(a.clone()),
            (None, true) => Lookup::None,
            (None, false) => Lookup::Pending,
        }
    }

    /// Number of bound sequences. O(N) — for tests / diagnostics.
    pub fn len(&self) -> usize {
        fn count(n: &TrieNode) -> usize {
            n.action.is_some() as usize + n.children.values().map(count).sum::<usize>()
        }
        count(&self.root)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use pretty_assertions::assert_eq;

    fn seq(s: &str) -> KeySeq {
        KeySeq::from_str(s).unwrap()
    }

    fn k(s: &str) -> Key {
        Key::from_str(s).unwrap()
    }

    #[test]
    fn bind_and_match_single_char() {
        let mut m = Keymap::new();
        m.bind(seq("j"), Action::NavigateDown).unwrap();
        assert_eq!(
            m.lookup(&[k("j")]),
            Lookup::Match(Action::NavigateDown)
        );
        assert_eq!(m.lookup(&[k("k")]), Lookup::None);
    }

    #[test]
    fn pending_then_match_for_multi_key() {
        let mut m = Keymap::new();
        m.bind(seq("dd"), Action::ArchiveChat).unwrap();
        assert_eq!(m.lookup(&[k("d")]), Lookup::Pending);
        assert_eq!(
            m.lookup(&[k("d"), k("d")]),
            Lookup::Match(Action::ArchiveChat)
        );
    }

    #[test]
    fn ambiguous_when_prefix_is_also_a_binding() {
        let mut m = Keymap::new();
        m.bind(seq("g"), Action::ScrollTop).unwrap();
        m.bind(seq("gg"), Action::ScrollTop).unwrap();
        // "g" alone matches but "gg" extends it.
        assert!(matches!(
            m.lookup(&[k("g")]),
            Lookup::AmbiguousMatch(Action::ScrollTop)
        ));
        assert_eq!(
            m.lookup(&[k("g"), k("g")]),
            Lookup::Match(Action::ScrollTop)
        );
    }

    #[test]
    fn rebinding_overwrites() {
        let mut m = Keymap::new();
        m.bind(seq("q"), Action::Quit).unwrap();
        m.bind(seq("q"), Action::ReloadConfig).unwrap();
        assert_eq!(
            m.lookup(&[k("q")]),
            Lookup::Match(Action::ReloadConfig)
        );
    }

    #[test]
    fn len_counts_bound_sequences_only() {
        let mut m = Keymap::new();
        assert_eq!(m.len(), 0);
        m.bind(seq("g"), Action::ScrollTop).unwrap();
        m.bind(seq("gg"), Action::ScrollTop).unwrap();
        m.bind(seq("dd"), Action::ArchiveChat).unwrap();
        assert_eq!(m.len(), 3);
    }
}

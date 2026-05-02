//! The modal state machine. Holds per-mode keymaps + the pending key
//! sequence, dispatches actions on unambiguous matches, and offers an
//! escape hatch (`commit_pending`) for callers that implement
//! timeout-based commit on ambiguous matches.

use std::str::FromStr;

use tracing::debug;

use crate::config::Config;
use crate::core::Result;

use super::action::Action;
use super::key::Key;
use super::keymap::{Keymap, Lookup};
use super::keyseq::KeySeq;
use super::mode::Mode;

#[derive(Debug, Default)]
pub struct KeymapSet {
    pub normal: Keymap,
    pub insert: Keymap,
    pub command: Keymap,
}

impl KeymapSet {
    /// Build a [`KeymapSet`] from a parsed [`Config`]. Leader-key
    /// substitution is performed here: any `<leader>` in a sequence is
    /// replaced with the configured `general.leader_key`.
    pub fn from_config(cfg: &Config) -> Result<Self> {
        let leader = Key::from_str(&cfg.general.leader_key)?;
        let mut s = Self::default();
        for (raw, action) in &cfg.keymap.normal {
            s.normal
                .bind(parse_seq(raw, &leader)?, Action::from_name(action))?;
        }
        for (raw, action) in &cfg.keymap.insert {
            s.insert
                .bind(parse_seq(raw, &leader)?, Action::from_name(action))?;
        }
        for (raw, action) in &cfg.keymap.command {
            s.command
                .bind(parse_seq(raw, &leader)?, Action::from_name(action))?;
        }
        Ok(s)
    }
}

fn parse_seq(raw: &str, leader: &Key) -> Result<KeySeq> {
    let parsed = KeySeq::from_str(raw)?;
    let expanded = parsed
        .0
        .into_iter()
        .map(|k| if k.is_leader() { leader.clone() } else { k })
        .collect();
    Ok(KeySeq(expanded))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Wait for more keys.
    Pending,
    /// A binding fired.
    Action(Action),
    /// A key arrived that doesn't continue any binding; pending cleared.
    Cancelled,
}

#[derive(Debug)]
pub struct Engine {
    mode: Mode,
    pending: Vec<Key>,
    maps: KeymapSet,
}

impl Engine {
    pub fn new(maps: KeymapSet) -> Self {
        Self {
            mode: Mode::default(),
            pending: Vec::new(),
            maps,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: Mode) {
        debug!(?mode, prev = ?self.mode, "mode change");
        self.mode = mode;
        self.pending.clear();
    }

    pub fn pending(&self) -> &[Key] {
        &self.pending
    }

    fn current_map(&self) -> &Keymap {
        match self.mode {
            Mode::Normal => &self.maps.normal,
            Mode::Insert => &self.maps.insert,
            // Search reuses the command map for the v1 cut.
            Mode::Command | Mode::Search => &self.maps.command,
        }
    }

    /// Process one key. Returns the dispatch outcome.
    pub fn feed(&mut self, key: Key) -> Outcome {
        self.pending.push(key);
        let map = self.current_map();
        match map.lookup(&self.pending) {
            Lookup::Match(a) => {
                self.pending.clear();
                Outcome::Action(a)
            }
            Lookup::AmbiguousMatch(_) | Lookup::Pending => Outcome::Pending,
            Lookup::None => {
                self.pending.clear();
                Outcome::Cancelled
            }
        }
    }

    /// Commit whatever's currently pending — used on the
    /// "ambiguous-match timeout" path (vim's `timeoutlen`). Returns
    /// `Some(action)` if the pending prefix is itself a binding,
    /// otherwise `None` (and pending is cleared either way).
    pub fn commit_pending(&mut self) -> Option<Action> {
        let map = self.current_map();
        let result = match map.lookup(&self.pending) {
            Lookup::Match(a) | Lookup::AmbiguousMatch(a) => Some(a),
            _ => None,
        };
        self.pending.clear();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn k(c: char) -> Key {
        Key::char(c)
    }

    fn map_with(bindings: &[(&str, Action)]) -> Keymap {
        let mut m = Keymap::new();
        for (s, a) in bindings {
            m.bind(KeySeq::from_str(s).unwrap(), a.clone()).unwrap();
        }
        m
    }

    #[test]
    fn dispatches_unambiguous_match() {
        let normal = map_with(&[("j", Action::NavigateDown)]);
        let mut e = Engine::new(KeymapSet {
            normal,
            ..Default::default()
        });
        assert_eq!(e.feed(k('j')), Outcome::Action(Action::NavigateDown));
        assert!(e.pending().is_empty());
    }

    #[test]
    fn waits_then_completes_multi_key() {
        let normal = map_with(&[("dd", Action::ArchiveChat)]);
        let mut e = Engine::new(KeymapSet {
            normal,
            ..Default::default()
        });
        assert_eq!(e.feed(k('d')), Outcome::Pending);
        assert_eq!(e.feed(k('d')), Outcome::Action(Action::ArchiveChat));
    }

    #[test]
    fn unrelated_key_cancels_pending() {
        let normal = map_with(&[("dd", Action::ArchiveChat)]);
        let mut e = Engine::new(KeymapSet {
            normal,
            ..Default::default()
        });
        assert_eq!(e.feed(k('d')), Outcome::Pending);
        assert_eq!(e.feed(k('x')), Outcome::Cancelled);
        assert!(e.pending().is_empty());
    }

    #[test]
    fn mode_switches_clear_pending() {
        let normal = map_with(&[("dd", Action::ArchiveChat)]);
        let mut e = Engine::new(KeymapSet {
            normal,
            ..Default::default()
        });
        assert_eq!(e.feed(k('d')), Outcome::Pending);
        e.set_mode(Mode::Insert);
        assert!(e.pending().is_empty());
        assert_eq!(e.mode(), Mode::Insert);
    }

    #[test]
    fn commit_pending_returns_ambiguous_action() {
        let normal = map_with(&[("g", Action::ScrollTop), ("gg", Action::ScrollTop)]);
        let mut e = Engine::new(KeymapSet {
            normal,
            ..Default::default()
        });
        assert_eq!(e.feed(k('g')), Outcome::Pending);
        assert_eq!(e.commit_pending(), Some(Action::ScrollTop));
        assert!(e.pending().is_empty());
    }

    #[test]
    fn keymap_set_built_from_example_config() {
        let cfg: Config = toml::from_str(include_str!("../../config.example.toml")).unwrap();
        let set = KeymapSet::from_config(&cfg).unwrap();
        // Every binding in the example config should be present.
        assert_eq!(set.normal.len(), cfg.keymap.normal.len());
        assert_eq!(set.insert.len(), cfg.keymap.insert.len());
        assert_eq!(set.command.len(), cfg.keymap.command.len());

        // Sample lookups.
        let mut e = Engine::new(set);
        assert_eq!(
            e.feed(Key::char('j')),
            Outcome::Action(Action::NavigateDown)
        );
        assert_eq!(e.feed(Key::char('d')), Outcome::Pending);
        assert_eq!(e.feed(Key::char('d')), Outcome::Action(Action::ArchiveChat));

        // Leader-prefixed binding from the config: "<Space>c" -> compose_new.
        assert_eq!(e.feed(Key::named("Space")), Outcome::Pending);
        assert_eq!(e.feed(Key::char('c')), Outcome::Action(Action::ComposeNew));
    }

    #[test]
    fn leader_sentinel_is_expanded() {
        // Build a config where one binding uses <leader>.
        let cfg = Config {
            general: crate::config::schema::General {
                leader_key: ",".into(),
                ..Default::default()
            },
            keymap: crate::config::schema::Keymap {
                normal: [("<leader>q".to_string(), "quit".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let set = KeymapSet::from_config(&cfg).unwrap();
        let mut e = Engine::new(set);
        // Press the actual configured leader key, then 'q'.
        assert_eq!(e.feed(Key::char(',')), Outcome::Pending);
        assert_eq!(e.feed(Key::char('q')), Outcome::Action(Action::Quit));
    }

    #[test]
    fn leader_can_be_a_named_key_like_space() {
        // The default config uses `<Space>` as the leader. Make sure the
        // round-trip through KeymapSet expansion preserves that — pressing
        // Space then a follower fires the bound action.
        let cfg = Config {
            general: crate::config::schema::General {
                leader_key: "<Space>".into(),
                ..Default::default()
            },
            keymap: crate::config::schema::Keymap {
                normal: [("<leader>q".to_string(), "quit".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let set = KeymapSet::from_config(&cfg).unwrap();
        let mut e = Engine::new(set);
        assert_eq!(e.feed(Key::named("Space")), Outcome::Pending);
        assert_eq!(e.feed(Key::char('q')), Outcome::Action(Action::Quit));
    }

    #[test]
    fn leader_works_in_multi_step_sequences() {
        // <leader>fr — three keys with leader at the head. Make sure the
        // expansion threads through every key in the sequence (not just
        // the first character).
        let cfg = Config {
            general: crate::config::schema::General {
                leader_key: ",".into(),
                ..Default::default()
            },
            keymap: crate::config::schema::Keymap {
                normal: [("<leader>fr".to_string(), "reload_config".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let set = KeymapSet::from_config(&cfg).unwrap();
        let mut e = Engine::new(set);
        assert_eq!(e.feed(Key::char(',')), Outcome::Pending);
        assert_eq!(e.feed(Key::char('f')), Outcome::Pending);
        assert_eq!(
            e.feed(Key::char('r')),
            Outcome::Action(Action::ReloadConfig)
        );
    }

    #[test]
    fn leader_inside_sequence_also_expands() {
        // `g<leader>` — leader appears mid-sequence, not at the head.
        // Expansion happens key-by-key, so this should still work.
        let cfg = Config {
            general: crate::config::schema::General {
                leader_key: ",".into(),
                ..Default::default()
            },
            keymap: crate::config::schema::Keymap {
                normal: [("g<leader>".to_string(), "scroll_top".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };
        let set = KeymapSet::from_config(&cfg).unwrap();
        let mut e = Engine::new(set);
        assert_eq!(e.feed(Key::char('g')), Outcome::Pending);
        assert_eq!(e.feed(Key::char(',')), Outcome::Action(Action::ScrollTop));
    }
}

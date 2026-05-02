//! Integration smoke test for the headless surface area of Kryptos.
//!
//! No GTK display is required. Confirms:
//! 1. The default `Config` round-trips through TOML serialization.
//! 2. Every advertised built-in theme name resolves via `theme::builtin::lookup`.
//! 3. The example config (which ships in `config.example.toml`) builds a
//!    `KeymapSet` with every binding wired and at least one multi-key
//!    sequence dispatching correctly.

use kryptos::config::Config;
use kryptos::theme::builtin;
use kryptos::vim::engine::{Engine, KeymapSet, Outcome};
use kryptos::vim::{Action, Key};

#[test]
fn default_config_round_trips_through_toml() {
    let cfg = Config::default();
    let body = toml::to_string_pretty(&cfg).expect("serialize default Config");
    let again: Config = toml::from_str(&body).expect("deserialize back");
    assert_eq!(again.general.theme, cfg.general.theme);
    assert_eq!(again.general.leader_key, cfg.general.leader_key);
    assert_eq!(again.keymap.normal.len(), cfg.keymap.normal.len());
    assert_eq!(again.notifications.enabled, cfg.notifications.enabled);
}

#[test]
fn every_advertised_theme_name_resolves_or_is_system() {
    for name in builtin::known_names() {
        if name == "system" {
            // "system" is special-cased; lookup intentionally returns None.
            assert!(
                builtin::lookup(name).is_none(),
                "'system' must not be a real builtin"
            );
            continue;
        }
        let resolved = builtin::lookup(name);
        assert!(resolved.is_some(), "advertised theme '{name}' must resolve");
        assert_eq!(resolved.unwrap().canonical_name, name);
    }
}

#[test]
fn example_config_builds_a_complete_keymap_set() {
    let raw = include_str!("../config.example.toml");
    let cfg: Config = toml::from_str(raw).expect("parse example config");
    let set = KeymapSet::from_config(&cfg).expect("build KeymapSet from example");
    assert_eq!(set.normal.len(), cfg.keymap.normal.len());
    assert_eq!(set.insert.len(), cfg.keymap.insert.len());
    assert_eq!(set.command.len(), cfg.keymap.command.len());
}

#[test]
fn example_config_dispatches_known_bindings() {
    let raw = include_str!("../config.example.toml");
    let cfg: Config = toml::from_str(raw).expect("parse example config");
    let set = KeymapSet::from_config(&cfg).expect("build KeymapSet");

    // Drive each example-config binding through the engine. We assert
    // that *every* configured normal-mode sequence either fires its
    // action immediately (single-key) or eventually (multi-key) — no
    // stuck-pending paths and no Cancelled.
    for (raw_seq, action_name) in &cfg.keymap.normal {
        let mut e = Engine::new(KeymapSet::from_config(&cfg).unwrap());
        let _ = &set; // hush unused if we ever simplify above
        let parsed = parse_seq(raw_seq, &cfg.general.leader_key);
        let mut fired = false;
        for (i, key) in parsed.iter().enumerate() {
            let last = i + 1 == parsed.len();
            let outcome = e.feed(key.clone());
            match outcome {
                Outcome::Action(a) => {
                    assert!(last, "{raw_seq} fired before consuming all keys");
                    assert_eq!(
                        a,
                        Action::from_name(action_name),
                        "{raw_seq} fired wrong action"
                    );
                    fired = true;
                }
                Outcome::Pending => {
                    assert!(!last, "{raw_seq} still pending after final key");
                }
                Outcome::Cancelled => {
                    panic!("{raw_seq} cancelled mid-feed at key {i}");
                }
            }
        }
        assert!(fired, "{raw_seq} never fired");
    }
}

/// Parse a key sequence the same way `KeymapSet::from_config` does, so
/// the smoke test can drive each binding key-by-key.
fn parse_seq(raw: &str, leader: &str) -> Vec<Key> {
    use std::str::FromStr;
    let leader_key = Key::from_str(leader).expect("leader parses");
    let parsed = kryptos::vim::keyseq::KeySeq::from_str(raw).expect("seq parses");
    parsed
        .0
        .into_iter()
        .map(|k| if k.is_leader() { leader_key.clone() } else { k })
        .collect()
}

//! `:command` palette — parsing and execution.
//!
//! The parser is pure ([`parse_command`]) and unit-tested. Execution
//! lives behind [`execute`] which the dispatcher invokes; it touches
//! widgets / disk / D-Bus and surfaces feedback through
//! `adw::ToastOverlay`.

use std::path::Path;

use crate::config::{loader, Config};
use crate::core::{Error, Result};
use crate::theme::builtin;

/// Parsed `:command` line. Whitespace-only / empty input becomes
/// [`Command::Empty`] (the dispatcher treats that as a no-op).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Empty,
    Quit,
    Write,
    Theme(Option<String>),
    /// `:set key` (toggle bool) or `:set key = value`.
    Set {
        key: String,
        value: Option<String>,
    },
    Reload,
    Settings,
    Link(Option<String>),
    Help,
    Unknown(String),
}

/// Parse a single `:command` line. The leading `:` is *not* expected
/// — the bar already strips it via the prefix label.
pub fn parse_command(line: &str) -> Command {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Command::Empty;
    }

    let mut it = trimmed.splitn(2, char::is_whitespace);
    let head = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").trim();

    match head {
        "q" | "quit" => Command::Quit,
        "w" | "write" => Command::Write,
        "theme" => {
            if rest.is_empty() {
                Command::Theme(None)
            } else {
                Command::Theme(Some(rest.to_string()))
            }
        }
        "set" => parse_set(rest),
        "reload" => Command::Reload,
        "settings" | "prefs" | "preferences" => Command::Settings,
        "link" => {
            if rest.is_empty() {
                Command::Link(None)
            } else {
                Command::Link(Some(rest.to_string()))
            }
        }
        "help" => Command::Help,
        other => Command::Unknown(other.to_string()),
    }
}

/// `:set key` or `:set key = value` (whitespace around `=` optional).
fn parse_set(rest: &str) -> Command {
    if rest.is_empty() {
        return Command::Set {
            key: String::new(),
            value: None,
        };
    }
    if let Some(eq) = rest.find('=') {
        let (k, v) = rest.split_at(eq);
        let key = k.trim().to_string();
        let value = v[1..].trim().to_string();
        Command::Set {
            key,
            value: Some(value),
        }
    } else {
        Command::Set {
            key: rest.trim().to_string(),
            value: None,
        }
    }
}

/// Apply a `:set key [= value]` to a `Config` in-place. Pure so tests
/// can verify field updates without disk / GTK.
pub fn apply_set(
    cfg: &mut Config,
    key: &str,
    value: Option<&str>,
) -> std::result::Result<String, String> {
    match key {
        "theme" => {
            let v = value.ok_or_else(|| "`:set theme = <name>` requires a value".to_string())?;
            cfg.general.theme = v.to_string();
            Ok(format!("theme = {v}"))
        }
        "start_maximized" => {
            let v = parse_bool_set(value, cfg.general.start_maximized)?;
            cfg.general.start_maximized = v;
            Ok(format!("start_maximized = {v}"))
        }
        "hyprland_blur" => {
            let v = parse_bool_set(value, cfg.general.hyprland_blur)?;
            cfg.general.hyprland_blur = v;
            Ok(format!("hyprland_blur = {v}"))
        }
        "notifications.enabled" => {
            let v = parse_bool_set(value, cfg.notifications.enabled)?;
            cfg.notifications.enabled = v;
            Ok(format!("notifications.enabled = {v}"))
        }
        "notifications.dnd_per_chat" => {
            let v = parse_bool_set(value, cfg.notifications.dnd_per_chat)?;
            cfg.notifications.dnd_per_chat = v;
            Ok(format!("notifications.dnd_per_chat = {v}"))
        }
        "appearance.font" => {
            let v = value
                .ok_or_else(|| "`:set appearance.font = <font>` requires a value".to_string())?;
            cfg.appearance.font = v.to_string();
            Ok(format!("appearance.font = {v}"))
        }
        "appearance.message_bubbles" => {
            let v = parse_bool_set(value, cfg.appearance.message_bubbles)?;
            cfg.appearance.message_bubbles = v;
            Ok(format!("appearance.message_bubbles = {v}"))
        }
        "general.leader_key" => {
            let v = value
                .ok_or_else(|| "`:set general.leader_key = <key>` requires a value".to_string())?;
            cfg.general.leader_key = v.to_string();
            Ok(format!("general.leader_key = {v}"))
        }
        other => Err(format!("unknown setting: {other}")),
    }
}

fn parse_bool_set(value: Option<&str>, current: bool) -> std::result::Result<bool, String> {
    match value {
        None => Ok(!current),
        Some(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Ok(true),
            "false" | "off" | "no" | "0" => Ok(false),
            other => Err(format!("expected bool, got {other:?}")),
        },
    }
}

/// Read config, mutate, and atomically rewrite. Used by `:set`.
pub fn mutate_config_on_disk<F>(path: &Path, mutate: F) -> Result<()>
where
    F: FnOnce(&mut Config) -> std::result::Result<(), String>,
{
    let mut cfg = loader::load_or_default(path)?;
    mutate(&mut cfg).map_err(Error::Config)?;
    super::settings::write_config_atomic(path, &cfg)?;
    Ok(())
}

/// Render the supported-commands hint shown by `:help`.
pub fn help_text() -> &'static str {
    ":q :w :theme <name> :set <key>[=<v>] :reload :settings :link <name> :help"
}

/// Comma-separated list of valid theme names, for error toasts.
pub fn theme_names_csv() -> String {
    builtin::known_names().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn parses_quit_aliases() {
        assert_eq!(parse_command("q"), Command::Quit);
        assert_eq!(parse_command("quit"), Command::Quit);
        assert_eq!(parse_command("  q  "), Command::Quit);
    }

    #[test]
    fn empty_line_is_empty() {
        assert_eq!(parse_command(""), Command::Empty);
        assert_eq!(parse_command("   "), Command::Empty);
    }

    #[test]
    fn unknown_command_carries_head() {
        assert_eq!(
            parse_command("doesnotexist"),
            Command::Unknown("doesnotexist".into())
        );
        assert_eq!(parse_command("foo bar baz"), Command::Unknown("foo".into()));
    }

    #[test]
    fn theme_with_and_without_arg() {
        assert_eq!(parse_command("theme"), Command::Theme(None));
        assert_eq!(
            parse_command("theme  gruvbox  "),
            Command::Theme(Some("gruvbox".into()))
        );
        assert_eq!(
            parse_command("theme catppuccin-mocha"),
            Command::Theme(Some("catppuccin-mocha".into()))
        );
    }

    #[test]
    fn set_parses_with_and_without_value() {
        assert_eq!(
            parse_command("set start_maximized"),
            Command::Set {
                key: "start_maximized".into(),
                value: None
            }
        );
        assert_eq!(
            parse_command("set theme = gruvbox"),
            Command::Set {
                key: "theme".into(),
                value: Some("gruvbox".into())
            }
        );
        assert_eq!(
            parse_command("set theme=gruvbox"),
            Command::Set {
                key: "theme".into(),
                value: Some("gruvbox".into())
            }
        );
        // = with no rhs is a valid (empty) set; apply_set handles it.
        assert_eq!(
            parse_command("set appearance.font ="),
            Command::Set {
                key: "appearance.font".into(),
                value: Some("".into())
            }
        );
    }

    #[test]
    fn settings_aliases() {
        assert_eq!(parse_command("settings"), Command::Settings);
        assert_eq!(parse_command("prefs"), Command::Settings);
        assert_eq!(parse_command("preferences"), Command::Settings);
    }

    #[test]
    fn link_with_and_without_name() {
        assert_eq!(parse_command("link"), Command::Link(None));
        assert_eq!(
            parse_command("link  nixos-laptop"),
            Command::Link(Some("nixos-laptop".into()))
        );
    }

    #[test]
    fn write_help_reload() {
        assert_eq!(parse_command("w"), Command::Write);
        assert_eq!(parse_command("write"), Command::Write);
        assert_eq!(parse_command("help"), Command::Help);
        assert_eq!(parse_command("reload"), Command::Reload);
    }

    #[test]
    fn apply_set_known_string() {
        let mut cfg = Config::default();
        apply_set(&mut cfg, "theme", Some("gruvbox")).unwrap();
        assert_eq!(cfg.general.theme, "gruvbox");
    }

    #[test]
    fn apply_set_bool_toggle() {
        let mut cfg = Config::default();
        let before = cfg.general.start_maximized;
        apply_set(&mut cfg, "start_maximized", None).unwrap();
        assert_eq!(cfg.general.start_maximized, !before);
    }

    #[test]
    fn apply_set_bool_explicit() {
        let mut cfg = Config::default();
        apply_set(&mut cfg, "notifications.enabled", Some("false")).unwrap();
        assert!(!cfg.notifications.enabled);
        apply_set(&mut cfg, "notifications.enabled", Some("on")).unwrap();
        assert!(cfg.notifications.enabled);
    }

    #[test]
    fn apply_set_unknown_key_errors() {
        let mut cfg = Config::default();
        let err = apply_set(&mut cfg, "frobnicate", Some("1")).unwrap_err();
        assert!(err.contains("unknown setting"));
    }

    #[test]
    fn apply_set_string_requires_value() {
        let mut cfg = Config::default();
        assert!(apply_set(&mut cfg, "theme", None).is_err());
        assert!(apply_set(&mut cfg, "appearance.font", None).is_err());
    }

    #[test]
    fn apply_set_bool_rejects_garbage() {
        let mut cfg = Config::default();
        assert!(apply_set(&mut cfg, "start_maximized", Some("maybe")).is_err());
    }

    #[test]
    fn parse_command_handles_tabs_and_mixed_whitespace() {
        // Tabs split head from args just like spaces.
        assert_eq!(
            parse_command("theme\tgruvbox"),
            Command::Theme(Some("gruvbox".into()))
        );
        // Leading whitespace + tab + trailing whitespace is fine.
        assert_eq!(
            parse_command("\t  set\t theme = gruvbox  "),
            Command::Set {
                key: "theme".into(),
                value: Some("gruvbox".into()),
            }
        );
    }

    #[test]
    fn parse_command_is_case_sensitive() {
        // We don't lower-case command heads, so capitalised aliases land
        // in Unknown — make sure that's deliberate, not accidental.
        assert_eq!(parse_command("Q"), Command::Unknown("Q".into()));
        assert_eq!(parse_command("Theme"), Command::Unknown("Theme".into()));
    }

    #[test]
    fn parse_command_set_keeps_first_equals_only() {
        // `:set` splits on the FIRST `=`, so values can themselves contain `=`.
        assert_eq!(
            parse_command("set keymap=foo=bar=baz"),
            Command::Set {
                key: "keymap".into(),
                value: Some("foo=bar=baz".into()),
            }
        );
    }

    #[test]
    fn parse_command_theme_keeps_inner_whitespace() {
        // Multi-word theme arg — splitn(2) means we keep the rest verbatim
        // (after a single trim), not collapse to a single token.
        assert_eq!(
            parse_command("theme catppuccin mocha"),
            Command::Theme(Some("catppuccin mocha".into())),
        );
    }
}

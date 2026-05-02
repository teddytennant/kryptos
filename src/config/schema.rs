use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub keymap: Keymap,
    pub notifications: Notifications,
    pub appearance: Appearance,
    pub backends: Backends,
    pub onboarding: Onboarding,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct General {
    pub theme: String,
    pub leader_key: String,
    pub start_maximized: bool,
    pub hyprland_blur: bool,
}

impl Default for General {
    fn default() -> Self {
        Self {
            theme: "catppuccin-mocha".into(),
            leader_key: "<Space>".into(),
            start_maximized: true,
            hyprland_blur: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Keymap {
    pub normal: BTreeMap<String, String>,
    pub insert: BTreeMap<String, String>,
    pub command: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Notifications {
    pub enabled: bool,
    pub sound: String,
    pub dnd_per_chat: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            enabled: true,
            sound: "default".into(),
            dnd_per_chat: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Appearance {
    pub font: String,
    pub message_bubbles: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            font: "Inter 11".into(),
            message_bubbles: true,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Backends {
    pub signal: SignalBackendConfig,
    pub telegram: TelegramBackendConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SignalBackendConfig {
    pub enabled: bool,
}

impl Default for SignalBackendConfig {
    fn default() -> Self {
        // Signal is the original backend; default to on so existing
        // configs without a [backends.signal] block keep working.
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TelegramBackendConfig {
    pub enabled: bool,
    pub api_id: i32,
    pub api_hash: String,
    /// Where `grammers-session` persists the auth blob. Empty string
    /// means "fall back to `$XDG_DATA_HOME/kryptos/telegram.session`";
    /// callers materialise the default at use-site so we don't need
    /// directories::ProjectDirs in the schema layer.
    pub session_path: String,
}

/// First-run onboarding state. Once the user completes (or explicitly
/// skips) the welcome flow we set `completed = true` so the welcome
/// window doesn't reappear on subsequent launches.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Onboarding {
    pub completed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn defaults_match_spec() {
        let cfg = Config::default();
        assert_eq!(cfg.general.theme, "catppuccin-mocha");
        assert_eq!(cfg.general.leader_key, "<Space>");
        assert!(cfg.general.start_maximized);
        assert!(cfg.general.hyprland_blur);
        assert!(cfg.notifications.enabled);
        assert_eq!(cfg.appearance.font, "Inter 11");
        assert!(cfg.backends.signal.enabled, "signal on by default");
        assert!(!cfg.backends.telegram.enabled, "telegram off by default");
    }

    #[test]
    fn parses_backends_section() {
        let cfg: Config = toml::from_str(
            r#"
[backends.signal]
enabled = false

[backends.telegram]
enabled = true
api_id = 12345
api_hash = "deadbeef"
session_path = "/tmp/tg.session"
"#,
        )
        .unwrap();
        assert!(!cfg.backends.signal.enabled);
        assert!(cfg.backends.telegram.enabled);
        assert_eq!(cfg.backends.telegram.api_id, 12345);
        assert_eq!(cfg.backends.telegram.api_hash, "deadbeef");
        assert_eq!(cfg.backends.telegram.session_path, "/tmp/tg.session");
    }

    #[test]
    fn parses_empty_input() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.general.theme, Config::default().general.theme);
    }

    #[test]
    fn parses_example_config() {
        let cfg: Config = toml::from_str(include_str!("../../config.example.toml")).unwrap();
        assert!(cfg.keymap.normal.contains_key("j"));
        assert!(cfg.keymap.command.contains_key("q"));
    }

    #[test]
    fn rejects_unknown_top_level() {
        let result: Result<Config, _> = toml::from_str("bogus = 1\n");
        assert!(result.is_err(), "expected unknown-field rejection");
    }

    #[test]
    fn onboarding_default_is_not_completed() {
        let cfg = Config::default();
        assert!(!cfg.onboarding.completed);
    }

    #[test]
    fn parses_onboarding_section() {
        let cfg: Config = toml::from_str("[onboarding]\ncompleted = true\n").unwrap();
        assert!(cfg.onboarding.completed);
    }
}

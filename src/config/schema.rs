use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub general: General,
    pub keymap: Keymap,
    pub notifications: Notifications,
    pub appearance: Appearance,
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
    }

    #[test]
    fn parses_empty_input() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.general.theme, Config::default().general.theme);
    }

    #[test]
    fn parses_example_config() {
        let cfg: Config =
            toml::from_str(include_str!("../../config.example.toml")).unwrap();
        assert!(cfg.keymap.normal.contains_key("j"));
        assert!(cfg.keymap.command.contains_key("q"));
    }

    #[test]
    fn rejects_unknown_top_level() {
        let result: Result<Config, _> = toml::from_str("bogus = 1\n");
        assert!(result.is_err(), "expected unknown-field rejection");
    }
}

use std::fs;
use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use tracing::{info, warn};

use super::schema::Config;
use crate::core::{Error, Result};

/// XDG-resolved path: `$XDG_CONFIG_HOME/kryptos/config.toml`
/// (typically `~/.config/kryptos/config.toml`).
pub fn default_path() -> Result<PathBuf> {
    let dirs = ProjectDirs::from("dev", "kryptos", "kryptos")
        .ok_or_else(|| Error::Config("could not resolve XDG config dir".into()))?;
    Ok(dirs.config_dir().join("config.toml"))
}

/// Load + parse a config from disk. Surfaces both I/O and parse errors.
pub fn load(path: &Path) -> Result<Config> {
    let raw = fs::read_to_string(path)?;
    let cfg: Config = toml::from_str(&raw)?;
    Ok(cfg)
}

/// Like [`load`], but a missing file yields the default config (with a
/// warning log). Other errors still propagate.
pub fn load_or_default(path: &Path) -> Result<Config> {
    match fs::read_to_string(path) {
        Ok(raw) => Ok(toml::from_str(&raw)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            warn!(?path, "config file not found, using defaults");
            Ok(Config::default())
        }
        Err(e) => Err(e.into()),
    }
}

/// Write the default config to `path`, creating parent dirs as needed.
/// Refuses to overwrite an existing file.
pub fn save_default(path: &Path) -> Result<()> {
    if path.exists() {
        return Err(Error::Config(format!(
            "refusing to overwrite existing config at {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(&Config::default())?;
    fs::write(path, toml_str)?;
    info!(?path, "wrote default config");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_default_config() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("config.toml");
        save_default(&p).unwrap();
        let cfg = load(&p).unwrap();
        assert_eq!(cfg.general.theme, Config::default().general.theme);
    }

    #[test]
    fn load_or_default_when_missing() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("does-not-exist.toml");
        let cfg = load_or_default(&p).unwrap();
        assert_eq!(cfg.general.theme, Config::default().general.theme);
    }

    #[test]
    fn save_default_refuses_overwrite() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("config.toml");
        save_default(&p).unwrap();
        assert!(save_default(&p).is_err());
    }

    #[test]
    fn load_rejects_unknown_field() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("config.toml");
        std::fs::write(&p, "garbage = true\n").unwrap();
        let err = load(&p).unwrap_err();
        assert!(matches!(err, Error::TomlParse(_)));
    }

    #[test]
    fn default_path_uses_kryptos_not_sigvim() {
        // Regression guard: the project was renamed sigvim → kryptos. If
        // ProjectDirs is ever fed the wrong slug the user's existing config
        // silently disappears at upgrade time. Lock the path segment in.
        let p = default_path().expect("default_path resolves");
        let s = p.to_string_lossy();
        assert!(
            s.contains("kryptos"),
            "default config path must contain 'kryptos', got {s}"
        );
        assert!(
            !s.contains("sigvim"),
            "default config path must not reference legacy 'sigvim', got {s}"
        );
        assert!(
            s.ends_with("config.toml"),
            "default config path must end with config.toml, got {s}"
        );
    }
}

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("config: {0}")]
    Config(String),

    #[error("dbus: {0}")]
    Dbus(#[from] zbus::Error),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("toml parse: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("toml serialize: {0}")]
    TomlSerialize(#[from] toml::ser::Error),

    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),

    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),

    #[error("notify: {0}")]
    Notify(#[from] notify_rust::error::Error),

    /// Telegram backend errors. `grammers` exposes several distinct
    /// error types (`InvocationError`, `AuthorizationError`,
    /// `SignInError`, plus plain `io::Error` for session i/o and a
    /// parse error for `PackedChat` hex). They land here as a
    /// flattened string so callers can pattern-match on
    /// `Error::Telegram(_)` without dragging the grammers dependency
    /// into the rest of the app.
    #[error("telegram: {0}")]
    Telegram(String),
}

pub type Result<T> = std::result::Result<T, Error>;

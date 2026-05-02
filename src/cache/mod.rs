//! Local SQLite cache for Signal data fetched over D-Bus.
//!
//! Backed by sqlx's tokio + sqlite driver. The pool is opened with WAL +
//! `synchronous=NORMAL` so the UI thread doesn't stall on fsyncs while a
//! background sync writes new messages.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use directories::ProjectDirs;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;

use crate::core::{Error, Result};

pub mod models;
pub mod repo;

/// Embedded migrations from `<crate>/migrations`.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Handle around the connection pool. Cheaply cloneable.
#[derive(Debug, Clone)]
pub struct Cache {
    pool: SqlitePool,
}

impl Cache {
    /// Open (creating if absent) the SQLite file at `path` and run migrations.
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new().connect_with(opts).await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// In-memory database for tests. Each instance is isolated.
    pub async fn open_in_memory() -> Result<Self> {
        // `:memory:` plus a single connection so all queries see the same db.
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(Error::from)?
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(opts)
            .await?;
        MIGRATOR.run(&pool).await?;
        Ok(Self { pool })
    }

    /// XDG default at `$XDG_DATA_HOME/kryptos/cache.db`.
    pub fn default_path() -> Result<PathBuf> {
        let dirs = ProjectDirs::from("dev", "kryptos", "kryptos")
            .ok_or_else(|| Error::Config("cannot resolve XDG data dir".into()))?;
        Ok(dirs.data_dir().join("cache.db"))
    }

    pub(crate) fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

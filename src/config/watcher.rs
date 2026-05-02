//! Filesystem watcher that emits a fresh `Config` whenever the
//! config file is saved (atomic-write or in-place).
//!
//! A background thread debounces raw notify events, reloads the file,
//! and broadcasts new snapshots through a [`tokio::sync::watch`]
//! channel — UI components subscribe and react.

use std::path::PathBuf;
use std::sync::mpsc::{channel as std_channel, Receiver as StdReceiver};
use std::time::Duration;

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::watch;
use tracing::{debug, error, warn};

use super::loader::load_or_default;
use super::schema::Config;
use crate::core::{Error, Result};

const DEBOUNCE: Duration = Duration::from_millis(150);

pub struct ConfigWatcher {
    pub rx: watch::Receiver<Config>,
    _watcher: RecommendedWatcher,
    _thread: std::thread::JoinHandle<()>,
}

impl ConfigWatcher {
    /// Begin watching `path`. The initial value of `rx` is the config
    /// loaded right now (or defaults, if the file is missing).
    pub fn new(path: PathBuf) -> Result<Self> {
        let initial = load_or_default(&path)?;
        let (tx, rx) = watch::channel(initial);

        let (raw_tx, raw_rx): (_, StdReceiver<notify::Result<Event>>) = std_channel();
        let mut watcher: RecommendedWatcher = Watcher::new(
            raw_tx,
            notify::Config::default().with_poll_interval(Duration::from_secs(2)),
        )
        .map_err(|e| Error::Config(format!("watcher: {e}")))?;

        // Watch the *parent* directory: editors commonly atomic-replace
        // the file via rename, which doesn't fire on the path itself.
        let watch_dir = path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .map_err(|e| Error::Config(format!("watch {watch_dir:?}: {e}")))?;

        let target = path;
        let thread = std::thread::Builder::new()
            .name("kryptos-config-watcher".into())
            .spawn(move || worker(raw_rx, target, tx))
            .map_err(|e| Error::Config(format!("spawn watcher thread: {e}")))?;

        Ok(Self {
            rx,
            _watcher: watcher,
            _thread: thread,
        })
    }
}

fn worker(raw_rx: StdReceiver<notify::Result<Event>>, target: PathBuf, tx: watch::Sender<Config>) {
    while let Ok(first) = raw_rx.recv() {
        if !is_relevant(&first, &target) {
            continue;
        }
        // Drain bursts within the debounce window — most editors emit
        // 2-5 events per save (write, chmod, rename, ...).
        std::thread::sleep(DEBOUNCE);
        while raw_rx.try_recv().is_ok() {}

        match load_or_default(&target) {
            Ok(cfg) => {
                debug!("config reloaded from disk");
                if tx.send(cfg).is_err() {
                    return; // all subscribers dropped
                }
            }
            Err(e) => {
                warn!(error = %e, "failed to reload config; keeping previous value");
            }
        }
    }
}

fn is_relevant(ev: &notify::Result<Event>, target: &PathBuf) -> bool {
    match ev {
        Ok(event) => {
            let kind_ok = matches!(
                event.kind,
                EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
            );
            let path_ok = event.paths.iter().any(|p| p == target);
            kind_ok && path_ok
        }
        Err(e) => {
            error!(error = %e, "watcher error");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn initial_value_is_loaded_immediately() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("config.toml");
        std::fs::write(&p, "[general]\ntheme = \"gruvbox\"\n").unwrap();

        let w = ConfigWatcher::new(p).unwrap();
        let cfg = w.rx.borrow().clone();
        assert_eq!(cfg.general.theme, "gruvbox");
    }

    #[test]
    fn missing_file_yields_defaults() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("does-not-exist.toml");
        let w = ConfigWatcher::new(p).unwrap();
        let cfg = w.rx.borrow().clone();
        assert_eq!(cfg.general.theme, Config::default().general.theme);
    }
}

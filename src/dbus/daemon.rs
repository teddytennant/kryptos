//! Make sure `signal-cli` is actually on the bus before we try to call it.
//!
//! signal-cli only registers `org.asamk.Signal` while running with
//! `daemon --dbus`. On a fresh system the user has done none of that,
//! so any link/list call returns `ServiceUnknown` and the UI looks
//! dead. We solve it by spawning the daemon ourselves the first time
//! we notice it's missing.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use tokio::time::sleep;
use tracing::{info, warn};
use zbus::fdo::DBusProxy;
use zbus::names::BusName;
use zbus::Connection;

use crate::core::{Error, Result};

const SIGNAL_BUS_NAME: &str = "org.asamk.Signal";
const SPAWN_WAIT: Duration = Duration::from_secs(8);
const POLL_EVERY: Duration = Duration::from_millis(200);

/// Returns once `org.asamk.Signal` is on the session bus. Spawns
/// `signal-cli daemon --dbus` if nothing currently owns the name.
pub async fn ensure_running(conn: &Connection) -> Result<()> {
    if name_has_owner(conn).await? {
        return Ok(());
    }

    info!("signal-cli not on the bus — spawning `signal-cli daemon --dbus`");
    spawn_daemon()?;

    let deadline = Instant::now() + SPAWN_WAIT;
    while Instant::now() < deadline {
        sleep(POLL_EVERY).await;
        if name_has_owner(conn).await? {
            info!("signal-cli is up");
            return Ok(());
        }
    }
    Err(Error::Config(
        "signal-cli daemon spawned but never registered on the bus".into(),
    ))
}

async fn name_has_owner(conn: &Connection) -> Result<bool> {
    let proxy = DBusProxy::new(conn)
        .await
        .map_err(|e| Error::Config(format!("DBus proxy: {e}")))?;
    let bus_name = BusName::try_from(SIGNAL_BUS_NAME)
        .map_err(|e| Error::Config(format!("invalid bus name: {e}")))?;
    proxy
        .name_has_owner(bus_name)
        .await
        .map_err(|e| Error::Config(format!("NameHasOwner: {e}")))
}

fn spawn_daemon() -> Result<()> {
    Command::new("signal-cli")
        .arg("daemon")
        .arg("--dbus")
        .arg("--no-receive-stdout")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            warn!(error = %e, "failed to spawn signal-cli — is it on $PATH?");
            Error::Config(format!(
                "signal-cli not available on PATH: {e}. Install signal-cli or run \
                 `signal-cli daemon --dbus` manually."
            ))
        })?;
    Ok(())
}

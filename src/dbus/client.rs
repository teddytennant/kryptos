//! High-level wrapper over the signal-cli D-Bus API.
//!
//! `SignalClient` owns a session-bus connection and exposes
//! semantically-named methods that internally build the right zbus
//! proxy. Input is validated at the boundary so D-Bus calls never see
//! malformed phone numbers / verification codes / device names.

use tracing::{debug, info};
use zbus::Connection;

use crate::core::{Error, Result};
use crate::dbus::proxy::{SignalControlProxy, SignalProxy};

pub struct SignalClient {
    conn: Connection,
}

impl SignalClient {
    /// Connect to the user session bus.
    pub async fn connect() -> Result<Self> {
        let conn = Connection::session().await?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Initiate the secondary-device linking flow.
    ///
    /// Returns a `tsdevice://...` URI; render it as a QR code and have
    /// the user scan it from Signal → Settings → Linked devices on the
    /// primary device.
    pub async fn link(&self, device_name: &str) -> Result<String> {
        validate_device_name(device_name)?;
        let proxy = SignalControlProxy::new(&self.conn).await?;
        info!(%device_name, "requesting device link URI");
        let uri = proxy.link(device_name).await?;
        debug!(%uri, "got link URI");
        Ok(uri)
    }

    /// Begin registration for a new account via SMS or voice call.
    pub async fn register(&self, number: &str, voice: bool) -> Result<()> {
        validate_phone_number(number)?;
        let proxy = SignalControlProxy::new(&self.conn).await?;
        proxy.register(number, voice).await?;
        Ok(())
    }

    /// Complete registration with the verification code.
    pub async fn verify(&self, number: &str, code: &str) -> Result<()> {
        validate_phone_number(number)?;
        validate_verify_code(code)?;
        let proxy = SignalControlProxy::new(&self.conn).await?;
        proxy.verify(number, code).await?;
        Ok(())
    }

    pub async fn list_accounts(&self) -> Result<Vec<String>> {
        let proxy = SignalControlProxy::new(&self.conn).await?;
        Ok(proxy.list_accounts().await?)
    }

    pub async fn version(&self) -> Result<String> {
        let proxy = SignalControlProxy::new(&self.conn).await?;
        Ok(proxy.version().await?)
    }

    /// Per-account messaging proxy.
    pub async fn account(&self, account: &str) -> Result<SignalProxy<'_>> {
        validate_phone_number(account)?;
        let path = account_object_path(account);
        let proxy = SignalProxy::builder(&self.conn)
            .path(path)
            .map_err(|e| Error::Config(format!("invalid object path: {e}")))?
            .build()
            .await?;
        Ok(proxy)
    }
}

/// signal-cli encodes E.164 numbers in the object path by stripping
/// the leading `+` (and any formatting) and prefixing with `_`.
fn account_object_path(account: &str) -> String {
    let digits: String = account.chars().filter(|c| c.is_ascii_digit()).collect();
    format!("/org/asamk/Signal/_{digits}")
}

fn validate_phone_number(s: &str) -> Result<()> {
    let trimmed = s.trim();
    if !trimmed.starts_with('+') || trimmed.len() < 8 || trimmed.len() > 20 {
        return Err(Error::Config(format!(
            "invalid phone number {trimmed:?}: must be E.164 like +14155552671"
        )));
    }
    if !trimmed[1..].chars().all(|c| c.is_ascii_digit()) {
        return Err(Error::Config(format!(
            "invalid phone number {trimmed:?}: only digits allowed after +"
        )));
    }
    Ok(())
}

fn validate_verify_code(s: &str) -> Result<()> {
    // Signal codes are formatted "123-456" but signal-cli accepts either form.
    let stripped: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if stripped.len() != 6 {
        return Err(Error::Config(format!(
            "invalid verification code {s:?}: expected 6 digits"
        )));
    }
    if stripped.len() != s.chars().filter(|c| !c.is_ascii_whitespace() && *c != '-').count() {
        return Err(Error::Config(format!(
            "invalid verification code {s:?}: only digits, spaces, and hyphens allowed"
        )));
    }
    Ok(())
}

fn validate_device_name(s: &str) -> Result<()> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(Error::Config("device name cannot be empty".into()));
    }
    if trimmed.len() > 50 {
        return Err(Error::Config(format!(
            "device name {trimmed:?}: max 50 characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_path_strips_plus_and_formatting() {
        assert_eq!(account_object_path("+14155552671"), "/org/asamk/Signal/_14155552671");
        assert_eq!(account_object_path("14155552671"), "/org/asamk/Signal/_14155552671");
        assert_eq!(account_object_path("+1 (415) 555-2671"), "/org/asamk/Signal/_14155552671");
    }

    #[test]
    fn phone_number_requires_e164() {
        assert!(validate_phone_number("+14155552671").is_ok());
        assert!(validate_phone_number("14155552671").is_err(), "missing +");
        assert!(validate_phone_number("+1abcd").is_err(), "non-digits");
        assert!(validate_phone_number("+1").is_err(), "too short");
        assert!(validate_phone_number(&format!("+{}", "9".repeat(20))).is_err(), "too long");
    }

    #[test]
    fn verify_code_must_be_six_digits() {
        assert!(validate_verify_code("123456").is_ok());
        assert!(validate_verify_code("123-456").is_ok(), "hyphens allowed");
        assert!(validate_verify_code("123 456").is_ok(), "spaces allowed");
        assert!(validate_verify_code("12345").is_err());
        assert!(validate_verify_code("1234567").is_err());
        assert!(validate_verify_code("12345a").is_err());
    }

    #[test]
    fn device_name_validation() {
        assert!(validate_device_name("nixos-laptop").is_ok());
        assert!(validate_device_name("").is_err());
        assert!(validate_device_name("   ").is_err());
        assert!(validate_device_name(&"x".repeat(51)).is_err());
    }
}

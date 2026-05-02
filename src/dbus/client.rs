//! High-level wrapper over the signal-cli D-Bus API.
//!
//! `SignalClient` owns a session-bus connection and exposes
//! semantically-named methods that internally build the right zbus
//! proxy. Input is validated at the boundary so D-Bus calls never see
//! malformed phone numbers / verification codes / device names.

use std::path::{Path, PathBuf};

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
        let paths = proxy.list_accounts().await?;
        Ok(paths
            .into_iter()
            .filter_map(|p| account_from_object_path(p.as_str()))
            .collect())
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

    /// Send a plain-text message to a single recipient (E.164 or UUID).
    /// Returns the timestamp signal-cli assigned to the message.
    pub async fn send_text(&self, account: &str, recipient: &str, message: &str) -> Result<i64> {
        validate_recipient(recipient)?;
        let proxy = self.account(account).await?;
        debug!(%account, %recipient, len = message.len(), "send_text");
        Ok(proxy.send_message(message, &[], recipient).await?)
    }

    /// Send a message with attachments. Each path must exist and be readable.
    pub async fn send_with_attachments(
        &self,
        account: &str,
        recipient: &str,
        message: &str,
        paths: &[PathBuf],
    ) -> Result<i64> {
        validate_recipient(recipient)?;
        // Owned strings keep the &str slice alive for the D-Bus call.
        let mut owned: Vec<String> = Vec::with_capacity(paths.len());
        for p in paths {
            validate_attachment_path(p)?;
            owned.push(p.to_string_lossy().into_owned());
        }
        let borrowed: Vec<&str> = owned.iter().map(String::as_str).collect();
        let proxy = self.account(account).await?;
        debug!(
            %account,
            %recipient,
            attachments = paths.len(),
            "send_with_attachments"
        );
        Ok(proxy.send_message(message, &borrowed, recipient).await?)
    }

    /// Send a plain-text message to a group.
    pub async fn send_group_text(
        &self,
        account: &str,
        group_id: &[u8],
        message: &str,
    ) -> Result<i64> {
        validate_group_id(group_id)?;
        let proxy = self.account(account).await?;
        debug!(%account, group_len = group_id.len(), "send_group_text");
        Ok(proxy.send_group_message(message, &[], group_id).await?)
    }
}

/// signal-cli encodes E.164 numbers in the object path by stripping
/// the leading `+` (and any formatting) and prefixing with `_`.
fn account_object_path(account: &str) -> String {
    let digits: String = account.chars().filter(char::is_ascii_digit).collect();
    format!("/org/asamk/Signal/_{digits}")
}

/// Reverse of [`account_object_path`]: pull the E.164 number back out of
/// a `/org/asamk/Signal/_<digits>` path. Tolerates a leading `+` after
/// `_` (newer signal-cli) and bare digits (older).
fn account_from_object_path(path: &str) -> Option<String> {
    let leaf = path.rsplit('/').next()?;
    let body = leaf.strip_prefix('_').unwrap_or(leaf);
    let digits: String = body.chars().filter(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    Some(format!("+{digits}"))
}

pub(crate) fn validate_phone_number(s: &str) -> Result<()> {
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
    let stripped: String = s.chars().filter(char::is_ascii_digit).collect();
    if stripped.len() != 6 {
        return Err(Error::Config(format!(
            "invalid verification code {s:?}: expected 6 digits"
        )));
    }
    if stripped.len()
        != s.chars()
            .filter(|c| !c.is_ascii_whitespace() && *c != '-')
            .count()
    {
        return Err(Error::Config(format!(
            "invalid verification code {s:?}: only digits, spaces, and hyphens allowed"
        )));
    }
    Ok(())
}

/// signal-cli accepts either an E.164 phone number or a Signal UUID
/// (32 hex digits, with or without the canonical 8-4-4-4-12 dashes).
pub(crate) fn validate_recipient(s: &str) -> Result<()> {
    let trimmed = s.trim();
    if trimmed.starts_with('+') {
        return validate_phone_number(trimmed);
    }
    let hex: String = trimmed.chars().filter(|c| *c != '-').collect();
    if hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(());
    }
    Err(Error::Config(format!(
        "invalid recipient {trimmed:?}: expected E.164 (+14155552671) or UUID"
    )))
}

pub(crate) fn validate_group_id(id: &[u8]) -> Result<()> {
    if id.is_empty() {
        return Err(Error::Config("group id cannot be empty".into()));
    }
    Ok(())
}

fn validate_attachment_path(p: &Path) -> Result<()> {
    let meta = std::fs::metadata(p)
        .map_err(|e| Error::Config(format!("attachment {}: {e}", p.display())))?;
    if !meta.is_file() {
        return Err(Error::Config(format!(
            "attachment {} is not a regular file",
            p.display()
        )));
    }
    // Probe readability — metadata alone doesn't guarantee read permission.
    std::fs::File::open(p)
        .map_err(|e| Error::Config(format!("attachment {} not readable: {e}", p.display())))?;
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
        assert_eq!(
            account_object_path("+14155552671"),
            "/org/asamk/Signal/_14155552671"
        );
        assert_eq!(
            account_object_path("14155552671"),
            "/org/asamk/Signal/_14155552671"
        );
        assert_eq!(
            account_object_path("+1 (415) 555-2671"),
            "/org/asamk/Signal/_14155552671"
        );
    }

    #[test]
    fn account_from_object_path_round_trips() {
        // The canonical path produced by `account_object_path` decodes back
        // to a `+`-prefixed E.164 number.
        let canonical = account_object_path("+14155552671");
        assert_eq!(
            account_from_object_path(&canonical),
            Some("+14155552671".into())
        );
        // Tolerates a `+` after `_` (newer signal-cli might emit either).
        assert_eq!(
            account_from_object_path("/org/asamk/Signal/_+14155552671"),
            Some("+14155552671".into())
        );
        // Non-account paths return None instead of producing a bogus number.
        assert_eq!(account_from_object_path("/"), None);
        assert_eq!(account_from_object_path(""), None);
        assert_eq!(account_from_object_path("/org/asamk/Signal/_"), None);
        assert_eq!(
            account_from_object_path("/org/asamk/Signal/notanumber"),
            None
        );
    }

    #[test]
    fn phone_number_requires_e164() {
        assert!(validate_phone_number("+14155552671").is_ok());
        assert!(validate_phone_number("14155552671").is_err(), "missing +");
        assert!(validate_phone_number("+1abcd").is_err(), "non-digits");
        assert!(validate_phone_number("+1").is_err(), "too short");
        assert!(
            validate_phone_number(&format!("+{}", "9".repeat(20))).is_err(),
            "too long"
        );
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

    #[test]
    fn recipient_accepts_e164() {
        assert!(validate_recipient("+14155552671").is_ok());
        assert!(
            validate_recipient("  +14155552671  ").is_ok(),
            "leading/trailing ws"
        );
    }

    #[test]
    fn recipient_accepts_uuid() {
        // Canonical dashed UUID.
        assert!(validate_recipient("550e8400-e29b-41d4-a716-446655440000").is_ok());
        // Bare 32 hex chars.
        assert!(validate_recipient("550e8400e29b41d4a716446655440000").is_ok());
        // Mixed case hex.
        assert!(validate_recipient("550E8400-E29B-41D4-A716-446655440000").is_ok());
    }

    #[test]
    fn recipient_rejects_garbage() {
        assert!(validate_recipient("").is_err(), "empty");
        assert!(validate_recipient("not-a-recipient").is_err(), "non-hex");
        assert!(
            validate_recipient("14155552671").is_err(),
            "phone without +"
        );
        assert!(
            validate_recipient("550e8400-e29b-41d4-a716-44665544000").is_err(),
            "uuid one digit short"
        );
        assert!(
            validate_recipient("550e8400-e29b-41d4-a716-4466554400gz").is_err(),
            "uuid non-hex"
        );
    }

    #[test]
    fn group_id_rejects_empty() {
        assert!(validate_group_id(&[]).is_err());
        assert!(validate_group_id(&[0u8]).is_ok());
        assert!(validate_group_id(&[1u8, 2, 3, 4]).is_ok());
    }

    #[test]
    fn attachment_path_rejects_nonexistent() {
        let bogus = Path::new("/this/path/should/not/exist/kryptos-test.bin");
        let err = validate_attachment_path(bogus).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn attachment_path_accepts_existing_readable_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("note.txt");
        std::fs::write(&p, b"hi").unwrap();
        validate_attachment_path(&p).expect("readable regular file is accepted");
    }

    #[test]
    fn attachment_path_rejects_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let err = validate_attachment_path(tmp.path()).unwrap_err();
        match err {
            Error::Config(msg) => assert!(msg.contains("not a regular file")),
            other => panic!("expected Error::Config for dir, got {other:?}"),
        }
    }
}

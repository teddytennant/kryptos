//! Type-safe zbus proxies for the signal-cli D-Bus API.
//!
//! signal-cli uses camelCase D-Bus method names (e.g. `listAccounts`,
//! not `ListAccounts`); we override every method's `name` so zbus
//! doesn't translate snake_case → PascalCase. Interface specs:
//! <https://github.com/AsamK/signal-cli/blob/master/man/signal-cli-dbus.5.adoc>.

use zbus::proxy;
use zbus::zvariant::OwnedObjectPath;

/// Multi-account control interface, exposed by signal-cli at
/// `/org/asamk/Signal`. Used for registration and device linking.
#[proxy(
    interface = "org.asamk.SignalControl",
    default_service = "org.asamk.Signal",
    default_path = "/org/asamk/Signal"
)]
pub trait SignalControl {
    #[zbus(name = "register")]
    fn register(&self, number: &str, voice_verification: bool) -> zbus::Result<()>;

    #[zbus(name = "registerWithCaptcha")]
    fn register_with_captcha(
        &self,
        number: &str,
        voice_verification: bool,
        captcha: &str,
    ) -> zbus::Result<()>;

    #[zbus(name = "verify")]
    fn verify(&self, number: &str, verify_code: &str) -> zbus::Result<()>;

    /// Link as a secondary device. Returns a `tsdevice://...` URI to
    /// render as a QR code; the primary device scans it to authorise.
    #[zbus(name = "link")]
    fn link(&self, new_device_name: &str) -> zbus::Result<String>;

    #[zbus(name = "version")]
    fn version(&self) -> zbus::Result<String>;

    /// All locally configured accounts. signal-cli returns object paths
    /// like `/org/asamk/Signal/_+12025551234`, *not* plain numbers.
    #[zbus(name = "listAccounts")]
    fn list_accounts(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
}

/// Per-account messaging interface. In single-account mode, lives at
/// `/org/asamk/Signal`; in multi-account daemon mode, at
/// `/org/asamk/Signal/_<digits>`.
#[proxy(
    interface = "org.asamk.Signal",
    default_service = "org.asamk.Signal",
    default_path = "/org/asamk/Signal"
)]
pub trait Signal {
    #[zbus(name = "sendMessage")]
    fn send_message(
        &self,
        message: &str,
        attachments: &[&str],
        recipient: &str,
    ) -> zbus::Result<i64>;

    #[zbus(name = "sendGroupMessage")]
    fn send_group_message(
        &self,
        message: &str,
        attachments: &[&str],
        group_id: &[u8],
    ) -> zbus::Result<i64>;

    #[zbus(name = "sendReadReceipt")]
    fn send_read_receipt(&self, recipient: &str, message_ids: &[i64]) -> zbus::Result<()>;

    /// Resolve a recipient (E.164 or UUID) to the locally-stored
    /// contact / profile name. signal-cli returns "" when the contact
    /// is unknown; the high-level wrapper in `client.rs` maps that to
    /// `None` so callers don't have to special-case empty strings.
    #[zbus(name = "getContactName")]
    fn get_contact_name(&self, number: &str) -> zbus::Result<String>;

    /// All E.164 numbers known to this account's contact store. Used
    /// to seed the sidebar; an empty contact list is normal on a
    /// freshly linked install until the phone syncs contacts over.
    #[zbus(name = "listNumbers")]
    fn list_numbers(&self) -> zbus::Result<Vec<String>>;

    /// All groups this account is a member of. The tuple is
    /// `(object_path, group_id_bytes, name)`; name may be empty for
    /// unnamed groups.
    #[zbus(name = "listGroups")]
    fn list_groups(&self) -> zbus::Result<Vec<(OwnedObjectPath, Vec<u8>, String)>>;

    /// Emitted on incoming 1:1 messages.
    #[zbus(signal, name = "MessageReceived")]
    fn message_received(
        &self,
        timestamp: i64,
        sender: String,
        group_id: Vec<u8>,
        message: String,
        attachments: Vec<String>,
    ) -> zbus::Result<()>;
}

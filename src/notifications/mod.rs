//! Desktop + in-app notifications.
//!
//! Wraps `notify-rust` for rich XDG desktop notifications (with reply and
//! click-to-focus actions) and exposes per-conversation Do Not Disturb plus
//! configurable sound hints.
//!
//! See [`badges`] for the in-app unread badge state used by the UI.

pub mod badges;

use std::collections::HashMap;

use notify_rust::{Hint, Notification};

use crate::config::schema::Notifications as NotificationConfig;
use crate::config::Config;
use crate::core::Result;

pub use badges::BadgeState;

/// Hard cap on the body preview the desktop notification displays.
/// Beyond this the daemon may truncate inconsistently across implementations,
/// so we truncate ourselves and append an ellipsis.
const PREVIEW_MAX_CHARS: usize = 280;

/// Reserved key for the "open chat" default action (libnotify convention).
const ACTION_DEFAULT: &str = "default";

/// Reserved key for the inline reply action.
const ACTION_REPLY: &str = "reply";

/// XDG icon id. May not exist on user's icon theme yet — that's intentional;
/// libnotify falls back gracefully when the icon is missing.
const ICON_ID: &str = "dev.kryptos.Kryptos";

const APP_NAME: &str = "Kryptos";

/// What the user did with a posted notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationAction {
    /// User clicked the body / default action — focus the conversation.
    Open,
    /// User invoked the reply action. The XDG spec's reply text is *not*
    /// surfaced through `notify-rust` 4.x's `wait_for_action` callback (which
    /// only receives the action key), so this carries an empty string and the
    /// caller is expected to prompt the user inline. Update once notify-rust
    /// exposes inline-reply payloads.
    Reply(String),
    /// Notification was dismissed without picking an action.
    Dismissed,
    /// An action key we didn't recognise — forwarded verbatim for diagnostics.
    Other(String),
}

/// A message worth a desktop notification.
#[derive(Debug, Clone)]
pub struct IncomingNotice {
    pub conversation_id: String,
    pub sender: String,
    pub preview: String,
}

pub struct Notifier {
    config: NotificationConfig,
    /// `conv_id` → unix-ms timestamp at which DND expires.
    dnd_until: HashMap<String, i64>,
}

impl Notifier {
    pub fn new(cfg: &Config) -> Self {
        Self {
            config: cfg.notifications.clone(),
            dnd_until: HashMap::new(),
        }
    }

    /// Replace the in-memory config snapshot. Called by the config-watcher on
    /// hot reload. DND state is intentionally preserved across reloads.
    pub fn reload(&mut self, cfg: &Config) {
        self.config = cfg.notifications.clone();
    }

    /// True iff a notification for `conversation_id` should be posted right now.
    pub fn should_notify(&self, conversation_id: &str, now_ms: i64) -> bool {
        if !self.config.enabled {
            return false;
        }
        if self.config.dnd_per_chat {
            if let Some(&until) = self.dnd_until.get(conversation_id) {
                if until > now_ms {
                    return false;
                }
            }
        }
        true
    }

    pub fn dnd_for(&mut self, conversation_id: &str, until_ms: i64) {
        self.dnd_until.insert(conversation_id.to_owned(), until_ms);
    }

    pub fn clear_dnd(&mut self, conversation_id: &str) {
        self.dnd_until.remove(conversation_id);
    }

    /// Post a desktop notification. Caller is responsible for gating on
    /// [`should_notify`](Self::should_notify) — this method always sends.
    pub async fn notify_incoming(&self, n: IncomingNotice) -> Result<NotificationHandle> {
        let body = truncate_preview(&n.preview, PREVIEW_MAX_CHARS);
        let mut builder = Notification::new();
        builder
            .appname(APP_NAME)
            .summary(&n.sender)
            .body(&body)
            .icon(ICON_ID)
            .action(ACTION_DEFAULT, "Open")
            .action(ACTION_REPLY, "Reply");

        // libnotify "sound-name" hint — daemons that ignore it (e.g. mako)
        // will simply skip it, so this is safe to set unconditionally when
        // the user hasn't disabled sound.
        if self.config.sound != "none" && !self.config.sound.is_empty() {
            builder.hint(Hint::SoundName(self.config.sound.clone()));
        }

        let handle = builder.show_async().await?;
        Ok(NotificationHandle { inner: handle })
    }
}

/// Lightweight wrapper so callers don't need to depend on `notify-rust` types.
pub struct NotificationHandle {
    inner: notify_rust::NotificationHandle,
}

impl NotificationHandle {
    /// Block until the user picks an action (or the notification closes), then
    /// invoke `on_action`. Consumes self because the underlying handle does.
    pub fn wait_for_action<F>(self, on_action: F)
    where
        F: FnOnce(NotificationAction),
    {
        self.inner.wait_for_action(|key| {
            on_action(NotificationAction::from_key(key));
        });
    }
}

impl NotificationAction {
    fn from_key(key: &str) -> Self {
        match key {
            ACTION_DEFAULT => Self::Open,
            // notify-rust 4.x can't surface the reply text through this
            // callback; see NotificationAction::Reply for context.
            ACTION_REPLY => Self::Reply(String::new()),
            "__closed" => Self::Dismissed,
            other => Self::Other(other.to_owned()),
        }
    }
}

fn truncate_preview(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn cfg_with_notifications(n: NotificationConfig) -> Config {
        Config {
            notifications: n,
            ..Default::default()
        }
    }

    #[test]
    fn should_notify_respects_global_disable() {
        let cfg = cfg_with_notifications(NotificationConfig {
            enabled: false,
            sound: "default".into(),
            dnd_per_chat: true,
        });
        let n = Notifier::new(&cfg);
        assert!(!n.should_notify("conv-1", 0));
    }

    #[test]
    fn should_notify_default_is_true() {
        let cfg = Config::default();
        let n = Notifier::new(&cfg);
        assert!(n.should_notify("conv-1", 1_700_000_000_000));
    }

    #[test]
    fn dnd_blocks_until_expiry() {
        let cfg = Config::default();
        let mut n = Notifier::new(&cfg);
        let now = 1_000_i64;
        n.dnd_for("conv-1", now + 5_000);

        assert!(!n.should_notify("conv-1", now));
        assert!(!n.should_notify("conv-1", now + 4_999));
        // Boundary: equality means DND has expired (until is exclusive).
        assert!(n.should_notify("conv-1", now + 5_000));
        assert!(n.should_notify("conv-1", now + 6_000));
        // Other conversations are unaffected.
        assert!(n.should_notify("conv-2", now));
    }

    #[test]
    fn clear_dnd_removes_silence() {
        let cfg = Config::default();
        let mut n = Notifier::new(&cfg);
        n.dnd_for("conv-1", i64::MAX);
        assert!(!n.should_notify("conv-1", 0));
        n.clear_dnd("conv-1");
        assert!(n.should_notify("conv-1", 0));
    }

    #[test]
    fn dnd_ignored_when_per_chat_disabled() {
        let cfg = cfg_with_notifications(NotificationConfig {
            enabled: true,
            sound: "default".into(),
            dnd_per_chat: false,
        });
        let mut n = Notifier::new(&cfg);
        n.dnd_for("conv-1", i64::MAX);
        assert!(n.should_notify("conv-1", 0));
    }

    #[test]
    fn reload_swaps_config_keeps_dnd() {
        let mut n = Notifier::new(&Config::default());
        n.dnd_for("conv-1", i64::MAX);

        let off = cfg_with_notifications(NotificationConfig {
            enabled: false,
            sound: "default".into(),
            dnd_per_chat: true,
        });
        n.reload(&off);
        assert!(!n.should_notify("conv-1", 0));

        let mut on = Config::default();
        on.notifications.enabled = true;
        n.reload(&on);
        // DND survived the reload.
        assert!(!n.should_notify("conv-1", 0));
    }

    #[test]
    fn truncate_short_string_is_unchanged() {
        assert_eq!(truncate_preview("hello", 280), "hello");
    }

    #[test]
    fn truncate_long_string_appends_ellipsis() {
        let body = "x".repeat(500);
        let out = truncate_preview(&body, PREVIEW_MAX_CHARS);
        // 280 chars + 1 ellipsis char.
        assert_eq!(out.chars().count(), PREVIEW_MAX_CHARS + 1);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_at_exact_boundary_no_ellipsis() {
        let body = "y".repeat(PREVIEW_MAX_CHARS);
        let out = truncate_preview(&body, PREVIEW_MAX_CHARS);
        assert_eq!(out.chars().count(), PREVIEW_MAX_CHARS);
        assert!(!out.ends_with('…'));
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        // Multi-byte chars must not be split mid-codepoint.
        let body = "🦀".repeat(400);
        let out = truncate_preview(&body, PREVIEW_MAX_CHARS);
        assert_eq!(out.chars().count(), PREVIEW_MAX_CHARS + 1);
    }

    #[test]
    fn action_from_key_maps_known_actions() {
        assert_eq!(
            NotificationAction::from_key("default"),
            NotificationAction::Open
        );
        assert_eq!(
            NotificationAction::from_key("reply"),
            NotificationAction::Reply(String::new())
        );
        assert_eq!(
            NotificationAction::from_key("__closed"),
            NotificationAction::Dismissed
        );
        assert_eq!(
            NotificationAction::from_key("custom"),
            NotificationAction::Other("custom".into())
        );
    }
}

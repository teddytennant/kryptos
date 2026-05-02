//! Vim-style actions a key binding can fire.
//!
//! Well-known actions are enumerated for type safety and exhaustive
//! match coverage in dispatchers; anything unrecognised falls through
//! to [`Action::Custom`] for plug-in / scripted handling.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    // Navigation
    NavigateDown,
    NavigateUp,
    SidebarFocus,
    MessagesFocus,
    ScrollTop,
    ScrollBottom,
    // Composition
    ComposeNew,
    Reply,
    SendMessage,
    LeaveInsert,
    // Chat / message ops
    ArchiveChat,
    CopyMessage,
    DeleteMessage,
    // Modes
    Search,
    CommandPalette,
    // Application
    Quit,
    SetTheme,
    ReloadConfig,
    // Anything else.
    Custom(String),
}

impl Action {
    /// Map a snake_case action name from config into an [`Action`].
    pub fn from_name(name: &str) -> Self {
        match name {
            "navigate_down" => Self::NavigateDown,
            "navigate_up" => Self::NavigateUp,
            "sidebar_focus" => Self::SidebarFocus,
            "messages_focus" => Self::MessagesFocus,
            "scroll_top" => Self::ScrollTop,
            "scroll_bottom" => Self::ScrollBottom,
            "compose_new" => Self::ComposeNew,
            "reply" => Self::Reply,
            "send_message" => Self::SendMessage,
            "leave_insert" => Self::LeaveInsert,
            "archive_chat" => Self::ArchiveChat,
            "copy_message" => Self::CopyMessage,
            "delete_message" => Self::DeleteMessage,
            "search" => Self::Search,
            "command_palette" => Self::CommandPalette,
            "quit" => Self::Quit,
            "set_theme" => Self::SetTheme,
            "reload_config" => Self::ReloadConfig,
            other => Self::Custom(other.to_string()),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Self::NavigateDown => "navigate_down",
            Self::NavigateUp => "navigate_up",
            Self::SidebarFocus => "sidebar_focus",
            Self::MessagesFocus => "messages_focus",
            Self::ScrollTop => "scroll_top",
            Self::ScrollBottom => "scroll_bottom",
            Self::ComposeNew => "compose_new",
            Self::Reply => "reply",
            Self::SendMessage => "send_message",
            Self::LeaveInsert => "leave_insert",
            Self::ArchiveChat => "archive_chat",
            Self::CopyMessage => "copy_message",
            Self::DeleteMessage => "delete_message",
            Self::Search => "search",
            Self::CommandPalette => "command_palette",
            Self::Quit => "quit",
            Self::SetTheme => "set_theme",
            Self::ReloadConfig => "reload_config",
            Self::Custom(s) => s,
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn well_known_round_trip() {
        for a in [
            Action::NavigateDown,
            Action::ComposeNew,
            Action::SendMessage,
            Action::Quit,
            Action::SetTheme,
        ] {
            assert_eq!(Action::from_name(a.name()), a);
        }
    }

    #[test]
    fn unknown_becomes_custom() {
        let a = Action::from_name("foo_bar");
        assert_eq!(a, Action::Custom("foo_bar".to_string()));
        assert_eq!(a.name(), "foo_bar");
    }

    #[test]
    fn display_uses_name() {
        assert_eq!(Action::Quit.to_string(), "quit");
        assert_eq!(Action::Custom("xyz".into()).to_string(), "xyz");
    }
}

use std::fmt;

/// Kryptos editor mode. Mirrors classic vim modes, adapted for a chat client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Default. Navigation and leader-prefixed actions.
    #[default]
    Normal,
    /// Composing a message.
    Insert,
    /// `:command` palette.
    Command,
    /// `/search` incremental search.
    Search,
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Mode::Normal => "NORMAL",
            Mode::Insert => "INSERT",
            Mode::Command => "COMMAND",
            Mode::Search => "SEARCH",
        };
        f.write_str(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_normal() {
        assert_eq!(Mode::default(), Mode::Normal);
    }

    #[test]
    fn display_is_uppercase() {
        assert_eq!(Mode::Normal.to_string(), "NORMAL");
        assert_eq!(Mode::Insert.to_string(), "INSERT");
        assert_eq!(Mode::Command.to_string(), "COMMAND");
        assert_eq!(Mode::Search.to_string(), "SEARCH");
    }
}

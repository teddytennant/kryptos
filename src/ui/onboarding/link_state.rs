//! Pure helpers for the onboarding poll loop.
//!
//! Kept separate so the I/O-free logic stays unit-testable without
//! signal-cli on the bus.

use std::collections::HashSet;

/// Result of comparing two `list_accounts()` snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkOutcome {
    /// No new account has appeared yet — keep polling.
    Pending,
    /// A previously-unknown account is now present. The `String` is
    /// the normalized account identifier (E.164 number) reported by
    /// signal-cli.
    Linked(String),
}

/// Compare a pre-link account snapshot to a post-poll snapshot and
/// decide whether the link finished. The first new entry wins —
/// signal-cli only links one account per `link()` call, but if a user
/// somehow added two between polls we still pick *something* sensible.
pub fn detect_new_account(before: &HashSet<String>, now: &[String]) -> LinkOutcome {
    for account in now {
        if !before.contains(account) {
            return LinkOutcome::Linked(account.clone());
        }
    }
    LinkOutcome::Pending
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    fn set(items: &[&str]) -> HashSet<String> {
        items.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn pending_when_no_new_accounts() {
        let before = set(&["+14155552671"]);
        let now = vec!["+14155552671".to_string()];
        assert_eq!(detect_new_account(&before, &now), LinkOutcome::Pending);
    }

    #[test]
    fn pending_when_both_empty() {
        let before = set(&[]);
        let now: Vec<String> = vec![];
        assert_eq!(detect_new_account(&before, &now), LinkOutcome::Pending);
    }

    #[test]
    fn detects_first_account_from_empty_baseline() {
        let before = set(&[]);
        let now = vec!["+14155552671".to_string()];
        assert_eq!(
            detect_new_account(&before, &now),
            LinkOutcome::Linked("+14155552671".to_string())
        );
    }

    #[test]
    fn detects_added_account_among_existing() {
        let before = set(&["+14155552671"]);
        let now = vec!["+14155552671".to_string(), "+15555550001".to_string()];
        assert_eq!(
            detect_new_account(&before, &now),
            LinkOutcome::Linked("+15555550001".to_string())
        );
    }

    #[test]
    fn ignores_removed_accounts() {
        // If signal-cli somehow drops an account between polls, we
        // don't surface that as a "linked" event.
        let before = set(&["+14155552671", "+15555550001"]);
        let now = vec!["+14155552671".to_string()];
        assert_eq!(detect_new_account(&before, &now), LinkOutcome::Pending);
    }
}

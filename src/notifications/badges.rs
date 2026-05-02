//! In-app unread badge tracking.
//!
//! Pure data, no UI bindings. The view layer reads this state to render
//! per-conversation counts and a global aggregate. All counters are
//! saturating: a wedged backend can pump increments forever without
//! crashing the UI.

use std::collections::HashMap;

/// Per-conversation unread counters with a saturating aggregate.
#[derive(Debug, Default, Clone)]
pub struct BadgeState {
    unread: HashMap<String, u32>,
}

impl BadgeState {
    /// Empty badge state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Increment the counter for `conv`, saturating at [`u32::MAX`].
    pub fn increment(&mut self, conv: &str) {
        let entry = self.unread.entry(conv.to_owned()).or_insert(0);
        *entry = entry.saturating_add(1);
    }

    /// Remove `conv`'s counter entirely (count becomes zero).
    pub fn clear(&mut self, conv: &str) {
        self.unread.remove(conv);
    }

    /// Current unread count for `conv`. Zero when not tracked.
    #[must_use]
    pub fn for_conv(&self, conv: &str) -> u32 {
        self.unread.get(conv).copied().unwrap_or(0)
    }

    /// Sum of all per-conversation counters, saturating at [`u32::MAX`].
    #[must_use]
    pub fn total(&self) -> u32 {
        self.unread
            .values()
            .copied()
            .fold(0u32, u32::saturating_add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn increment_starts_from_zero() {
        let mut b = BadgeState::new();
        assert_eq!(b.for_conv("a"), 0);
        b.increment("a");
        assert_eq!(b.for_conv("a"), 1);
    }

    #[test]
    fn increment_accumulates() {
        let mut b = BadgeState::new();
        for _ in 0..5 {
            b.increment("a");
        }
        assert_eq!(b.for_conv("a"), 5);
    }

    #[test]
    fn clear_resets_only_the_target_conv() {
        let mut b = BadgeState::new();
        b.increment("a");
        b.increment("a");
        b.increment("b");
        b.clear("a");
        assert_eq!(b.for_conv("a"), 0);
        assert_eq!(b.for_conv("b"), 1);
    }

    #[test]
    fn total_sums_across_conversations() {
        let mut b = BadgeState::new();
        b.increment("a");
        b.increment("b");
        b.increment("b");
        b.increment("c");
        assert_eq!(b.total(), 4);
    }

    #[test]
    fn total_is_zero_when_empty() {
        assert_eq!(BadgeState::new().total(), 0);
    }

    #[test]
    fn increment_saturates_at_u32_max() {
        // Pre-poison the counter at max — direct insert avoids 4B loop iters.
        let mut b = BadgeState::new();
        b.unread.insert("a".into(), u32::MAX);
        b.increment("a");
        assert_eq!(
            b.for_conv("a"),
            u32::MAX,
            "increment past u32::MAX must saturate, not panic or wrap"
        );
    }

    #[test]
    fn total_saturates_when_summing_overflows() {
        let mut b = BadgeState::new();
        b.unread.insert("a".into(), u32::MAX);
        b.unread.insert("b".into(), 1);
        // Naive .sum() would overflow-panic in debug; saturating fold returns MAX.
        assert_eq!(b.total(), u32::MAX);
    }
}

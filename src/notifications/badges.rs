//! In-app unread badge tracking.
//!
//! Pure data, no UI bindings. The view layer reads this state to render
//! per-conversation counts and a global aggregate.

use std::collections::HashMap;

#[derive(Debug, Default, Clone)]
pub struct BadgeState {
    unread: HashMap<String, u32>,
}

impl BadgeState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn increment(&mut self, conv: &str) {
        *self.unread.entry(conv.to_owned()).or_insert(0) += 1;
    }

    pub fn clear(&mut self, conv: &str) {
        self.unread.remove(conv);
    }

    pub fn for_conv(&self, conv: &str) -> u32 {
        self.unread.get(conv).copied().unwrap_or(0)
    }

    pub fn total(&self) -> u32 {
        self.unread.values().sum()
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
}

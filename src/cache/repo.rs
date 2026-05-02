//! Repository-style methods on [`Cache`].
//!
//! Each method is a single SQL operation (or short transaction) against the
//! pool. Returns are owned plain values from [`super::models`] — no sqlx types
//! leak.

use super::Cache;

impl Cache {
    // Repository methods land in follow-up commits.
    #[allow(dead_code)]
    pub(crate) fn _scaffold(&self) {}
}

//! The local cache of remote files: how big it may grow and what goes first.

use crate::units::{ByteSize, Timestamp};

/// Default cache size: 5 GB.
pub const DEFAULT_CACHE_LIMIT: ByteSize = ByteSize(5_000_000_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    pub limit: ByteSize,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self { limit: DEFAULT_CACHE_LIMIT }
    }
}

/// Whether the user asked to keep an item available offline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retention {
    /// Evicted when space is needed, least recently used first.
    Evictable,
    /// Kept until the user removes the offline copy; never evicted.
    Pinned,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedCopy<K> {
    pub key: K,
    pub size: ByteSize,
    pub last_used: Timestamp,
    pub retention: Retention,
}

impl CachePolicy {
    /// The copies to delete, least recently used first, so that the cache
    /// fits the limit. Pinned copies count towards the total but are never
    /// chosen, so a cache of mostly pinned items may stay over the limit.
    #[must_use]
    pub fn evictions<K: Clone>(&self, copies: &[CachedCopy<K>]) -> Vec<K> {
        let mut total: u64 = copies.iter().map(|copy| copy.size.0).sum();
        let mut candidates: Vec<&CachedCopy<K>> =
            copies.iter().filter(|copy| copy.retention == Retention::Evictable).collect();
        candidates.sort_by_key(|copy| copy.last_used);
        let mut evicted = Vec::new();
        for copy in candidates {
            if total <= self.limit.0 {
                break;
            }
            total = total.saturating_sub(copy.size.0);
            evicted.push(copy.key.clone());
        }
        evicted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(key: &'static str, size: u64, used: i64, retention: Retention) -> CachedCopy<&'static str> {
        CachedCopy { key, size: ByteSize(size), last_used: Timestamp::from_unix_millis(used), retention }
    }

    #[test]
    fn evicts_least_recently_used_until_under_the_limit() {
        let policy = CachePolicy { limit: ByteSize(100) };
        let copies = [
            copy("new", 50, 30, Retention::Evictable),
            copy("old", 40, 10, Retention::Evictable),
            copy("middle", 40, 20, Retention::Evictable),
        ];
        assert_eq!(policy.evictions(&copies), vec!["old"]);
    }

    #[test]
    fn pinned_copies_are_never_evicted() {
        let policy = CachePolicy { limit: ByteSize(10) };
        let copies = [copy("pinned", 100, 1, Retention::Pinned), copy("loose", 5, 2, Retention::Evictable)];
        assert_eq!(policy.evictions(&copies), vec!["loose"]);
    }

    #[test]
    fn nothing_is_evicted_within_the_limit() {
        let policy = CachePolicy::default();
        assert!(policy.evictions(&[copy("a", 1, 1, Retention::Evictable)]).is_empty());
    }
}

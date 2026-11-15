//! Transposition table (TT).
//!
//! A simple power-of-two hash table mapping position hashes to search results:
//! depth, score, bound flag, best move and age (search number). When the table
//! is full, entries from older searches are replaced first. The table is
//! shared between search threads (Gamma) behind a mutex; Alpha uses it
//! single-threaded with a smaller default size.

use crate::types::Move;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bound {
    Exact,
    Lower,
    Upper,
}

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub key: u64,
    pub depth: i32,
    pub flag: Bound,
    pub score: i32,
    pub best: Option<Move>,
    pub age: u8,
}

impl Entry {
    fn empty() -> Entry {
        Entry { key: 0, depth: -1, flag: Bound::Exact, score: 0, best: None, age: 0 }
    }
}

/// Inner table, guarded by a mutex so several search threads can share it.
struct Table {
    entries: Vec<Entry>,
    mask: usize,
    age: u8,
}

/// Thread-safe handle to the transposition table.
#[derive(Clone)]
pub struct TranspositionTable {
    inner: Arc<Mutex<Table>>,
}

#[cfg(feature = "large-tt")]
pub const DEFAULT_HASH_MB: usize = 64;
#[cfg(not(feature = "large-tt"))]
pub const DEFAULT_HASH_MB: usize = 8;

/// Maximum hash size advertised and allowed: Gamma 4096 MB, Alpha 8 MB
/// (matches the lightweight design target in README.md).
#[cfg(feature = "large-tt")]
pub const MAX_HASH_MB: usize = 4096;
#[cfg(not(feature = "large-tt"))]
pub const MAX_HASH_MB: usize = 8;

/// Effective default hash size: [`DEFAULT_HASH_MB`], overridable with the
/// `FERRUM_HASH_MB` environment variable. Containers and multi-tenant bots
/// can cap per-instance memory without changing the UCI defaults.
pub fn default_hash_mb() -> usize {
    std::env::var("FERRUM_HASH_MB")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(DEFAULT_HASH_MB)
        .clamp(1, MAX_HASH_MB)
}

impl TranspositionTable {
    pub fn new(size_mb: usize) -> TranspositionTable {
        let mut tt = TranspositionTable {
            inner: Arc::new(Mutex::new(Table { entries: Vec::new(), mask: 0, age: 0 })),
        };
        tt.resize(size_mb);
        tt
    }

    pub fn resize(&mut self, size_mb: usize) {
        let size_mb = size_mb.clamp(1, MAX_HASH_MB);
        // Each entry is ~32 bytes; entries = bytes / 32, rounded down to 2^k.
        let bytes = (size_mb * 1024 * 1024) as u64;
        let raw = (bytes / 32).max(1024);
        let count = raw.next_power_of_two();
        let count = if count > raw { count / 2 } else { count };
        let count = count.max(1024) as usize;
        let mut lock = self.inner.lock().unwrap();
        lock.entries = vec![Entry::empty(); count];
        lock.mask = count - 1;
        lock.age = 0;
    }

    /// Start a new search generation (increments the age).
    pub fn new_search(&self) {
        let mut lock = self.inner.lock().unwrap();
        lock.age = lock.age.wrapping_add(1);
    }

    pub fn probe(&self, hash: u64, depth: i32) -> Option<Entry> {
        let lock = self.inner.lock().unwrap();
        let e = &lock.entries[(hash & lock.mask as u64) as usize];
        if e.key == hash && e.depth >= depth {
            Some(*e)
        } else {
            None
        }
    }

    /// Single-lock probe returning both the ordering move (any depth)
    /// and the depth-sufficient entry (for cutoffs). Saves one mutex
    /// lock per non-PV node vs calling `probe_move` + `probe` separately.
    pub fn probe_both(&self, hash: u64, depth: i32) -> (Option<Move>, Option<Entry>) {
        let lock = self.inner.lock().unwrap();
        let e = &lock.entries[(hash & lock.mask as u64) as usize];
        if e.key != hash {
            return (None, None);
        }
        let entry = if e.depth >= depth { Some(*e) } else { None };
        (e.best, entry)
    }

    pub fn probe_move(&self, hash: u64) -> Option<Move> {
        let lock = self.inner.lock().unwrap();
        let e = &lock.entries[(hash & lock.mask as u64) as usize];
        if e.key == hash {
            e.best
        } else {
            None
        }
    }

    pub fn store(&self, hash: u64, depth: i32, flag: Bound, score: i32, best: Option<Move>) {
        let mut lock = self.inner.lock().unwrap();
        let idx = (hash & lock.mask as u64) as usize;
        let old = &lock.entries[idx];
        // Replace if empty, older, or (same age and deeper/equal).
        let replace = old.key == 0
            || old.age != lock.age
            || depth >= old.depth;
        if replace {
            lock.entries[idx] = Entry { key: hash, depth, flag, score, best, age: lock.age };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_and_probe() {
        let tt = TranspositionTable::new(1);
        let hash = 0x1234_5678_9ABC_DEF0u64;
        assert!(tt.probe(hash, 1).is_none());
        tt.store(hash, 3, Bound::Exact, 42, None);
        let e = tt.probe(hash, 2).unwrap();
        assert_eq!(e.score, 42);
        assert_eq!(e.depth, 3);
        // Too deep to use.
        assert!(tt.probe(hash, 4).is_none());
        // Different hash misses.
        assert!(tt.probe(hash ^ 1, 1).is_none());
    }
}

//! Transposition table (TT).
//!
//! A simple power-of-two hash table mapping position hashes to search results:
//! depth, score, bound flag, best move and age (search number). When the table
//! is full, entries from older searches are replaced first.
//!
//! The table is split into independently locked shards so Lazy SMP search
//! threads rarely contend on the same mutex. Shard selection uses the high
//! bits of the *slot index* and the low bits pick the entry inside the shard,
//! so the hash -> slot mapping is exactly the one a single flat table would
//! use. That keeps single-threaded search results node-identical.

use crate::types::Move;
use std::sync::atomic::{AtomicU8, Ordering};
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

/// Target number of independently locked shards. 64 is enough that four
/// search threads almost never touch the same lock, and the shards are
/// cache-line aligned so unrelated locks do not false-share.
const SHARD_COUNT: usize = 64;

/// One independently locked slice of the table. Aligned so each shard's
/// mutex and first entries sit on their own cache line.
#[repr(align(64))]
struct Shard {
    entries: Vec<Entry>,
}

/// Inner table: a fixed set of shards plus the shared search age.
struct Table {
    shards: Vec<Mutex<Shard>>,
    /// `entries per shard - 1`; the low bits of the slot index pick an entry.
    shard_mask: usize,
    /// log2(entries per shard): how far to shift a slot index to get the
    /// shard number.
    shard_shift: usize,
    /// Search generation, shared by all shards. Only incremented by
    /// `new_search`; read when stamping a store.
    age: AtomicU8,
}

/// Thread-safe handle to the transposition table.
#[derive(Clone)]
pub struct TranspositionTable {
    inner: Arc<Table>,
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
            inner: Arc::new(Table {
                shards: Vec::new(),
                shard_mask: 0,
                shard_shift: 0,
                age: AtomicU8::new(0),
            }),
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
        // Keep at least 64 entries per shard; the count is a power of two,
        // so the shard count and per-shard size stay powers of two too.
        let shard_count = SHARD_COUNT.min(count / 64).max(1);
        let shard_size = count / shard_count;
        let mut shards = Vec::with_capacity(shard_count);
        for _ in 0..shard_count {
            shards.push(Mutex::new(Shard { entries: vec![Entry::empty(); shard_size] }));
        }
        self.inner = Arc::new(Table {
            shards,
            shard_mask: shard_size - 1,
            shard_shift: shard_size.trailing_zeros() as usize,
            age: AtomicU8::new(0),
        });
    }

    /// The shard that owns `hash` and the entry index inside it. The full
    /// slot index is `hash & (total_entries - 1)`; its high bits choose the
    /// shard and its low bits choose the entry.
    #[inline]
    fn shard(&self, hash: u64) -> (&Mutex<Shard>, usize) {
        let id = ((hash >> self.inner.shard_shift) as usize) & (self.inner.shards.len() - 1);
        (&self.inner.shards[id], (hash as usize) & self.inner.shard_mask)
    }

    /// Start a new search generation (increments the age).
    pub fn new_search(&self) {
        self.inner.age.fetch_add(1, Ordering::Relaxed);
    }

    pub fn probe(&self, hash: u64, depth: i32) -> Option<Entry> {
        let (shard, idx) = self.shard(hash);
        let lock = shard.lock().unwrap();
        let e = &lock.entries[idx];
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
        let (shard, idx) = self.shard(hash);
        let lock = shard.lock().unwrap();
        let e = &lock.entries[idx];
        if e.key != hash {
            return (None, None);
        }
        let entry = if e.depth >= depth { Some(*e) } else { None };
        (e.best, entry)
    }

    pub fn probe_move(&self, hash: u64) -> Option<Move> {
        let (shard, idx) = self.shard(hash);
        let lock = shard.lock().unwrap();
        let e = &lock.entries[idx];
        if e.key == hash {
            e.best
        } else {
            None
        }
    }

    pub fn store(&self, hash: u64, depth: i32, flag: Bound, score: i32, best: Option<Move>) {
        let (shard, idx) = self.shard(hash);
        let mut lock = shard.lock().unwrap();
        let age = self.inner.age.load(Ordering::Relaxed);
        let old = &lock.entries[idx];
        // Replace if empty, older, or (same age and deeper/equal).
        let replace = old.key == 0
            || old.age != age
            || depth >= old.depth;
        if replace {
            lock.entries[idx] = Entry { key: hash, depth, flag, score, best, age };
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

    #[test]
    fn entries_stay_compact() {
        assert!(std::mem::size_of::<Entry>() <= 32);
    }

    #[test]
    fn concurrent_shards_keep_their_entries() {
        // 1 MB is 32768 slots: give every thread its own slice of low-bit
        // slots, so the test exercises parallel locking without relying on
        // collision behaviour. All hashes use low bits only.
        let tt = TranspositionTable::new(1);
        let mut handles = Vec::new();
        for t in 0..4u64 {
            let tt = tt.clone();
            handles.push(std::thread::spawn(move || {
                for i in 0..2048u64 {
                    let hash = 1 + t * 4096 + i;
                    tt.store(hash, 2, Bound::Exact, hash as i32, None);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        for t in 0..4u64 {
            for i in 0..2048u64 {
                let hash = 1 + t * 4096 + i;
                let e = tt.probe(hash, 1).expect("entry survived");
                assert_eq!(e.score, hash as i32);
            }
        }
    }
}

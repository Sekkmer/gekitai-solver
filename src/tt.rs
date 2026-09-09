use std::hash::BuildHasherDefault;
use std::io::{Read, Write};

use ahash::AHasher;
use anyhow::Result;
use hashbrown::HashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::position::PositionKey;
use crate::stats::Stats;

/// Bound flag for alpha-beta TT entries.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Bound {
    Exact,
    Lower,
    Upper,
}

/// Stored TT entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub depth: u16,
    pub bound: Bound,
    pub value: i16,
}

type HMap = HashMap<PositionKey, Entry, BuildHasherDefault<AHasher>>;

/// Sharded transposition table for multi-threaded search.
///
/// This is designed to scale on big-RAM CPUs:
/// - many shards => low lock contention
/// - each shard is an independent HashMap protected by a RwLock
///
/// TODO: consider “aging/generations” and eviction strategy if you want a hard cap.
pub struct TranspositionTable {
    shards: Vec<RwLock<HMap>>,
    stats: Stats,
}

impl TranspositionTable {
    /// Power-of-two number of shards.
    /// 4096 is a decent default for high thread counts and large TT.
    pub const SHARDS: usize = 4096;

    pub fn estimate_entries_from_mb(tt_mb: usize) -> usize {
        // Extremely rough heuristic.
        // HashMap entries have overhead; assume ~32 bytes per entry.
        // Adjust for your build/toolchain if you want tighter control.
        let bytes = tt_mb.saturating_mul(1024).saturating_mul(1024);
        (bytes / 32).max(1_000_000)
    }

    pub fn new(approx_entries: usize) -> Self {
        let mut shards = Vec::with_capacity(Self::SHARDS);
        let per_shard = (approx_entries / Self::SHARDS).max(1);

        for _ in 0..Self::SHARDS {
            let mut map: HMap = HashMap::default();
            map.reserve(per_shard);
            shards.push(RwLock::new(map));
        }

        Self {
            shards,
            stats: Stats::default(),
        }
    }

    #[inline]
    fn shard_idx(key: PositionKey) -> usize {
        // Fold high and low key bits before selecting a shard.
        ((key ^ (key >> 32)) as usize) & (Self::SHARDS - 1)
    }

    #[inline]
    pub fn get(&self, key: PositionKey) -> Option<Entry> {
        let idx = Self::shard_idx(key);
        let map = self.shards[idx].read();
        let e = map.get(&key).copied();
        if e.is_some() {
            self.stats.inc_tt_hits();
        }
        e
    }

    /// Store entry, replacing only if deeper (or empty).
    #[inline]
    pub fn store(&self, key: PositionKey, entry: Entry) {
        let idx = Self::shard_idx(key);
        let mut map = self.shards[idx].write();
        match map.get(&key) {
            Some(old) if old.depth > entry.depth => {
                // keep deeper
            }
            _ => {
                map.insert(key, entry);
                self.stats.inc_tt_stores();
            }
        }
    }

    #[inline]
    pub fn inc_nodes(&self) {
        self.stats.inc_nodes();
    }

    pub fn stats_snapshot(&self) -> (u64, u64, u64) {
        self.stats.snapshot()
    }

    pub fn total_len(&self) -> u64 {
        self.shards
            .iter()
            .map(|s| s.read().len() as u64)
            .sum::<u64>()
    }

    /// Stream the TT to a writer.
    ///
    /// Format:
    /// - for each shard:
    ///   - u64: number of entries
    ///   - repeated: (PositionKey, Entry) pairs
    pub fn write_to<W: Write>(&self, mut w: W) -> Result<()> {
        for shard in &self.shards {
            let map = shard.read();
            let len = map.len() as u64;
            bincode::serialize_into(&mut w, &len)?;
            for (k, v) in map.iter() {
                bincode::serialize_into(&mut w, k)?;
                bincode::serialize_into(&mut w, v)?;
            }
        }
        Ok(())
    }

    /// Load TT entries from a reader, replacing the current TT contents.
    ///
    /// NOTE: This will clear existing maps.
    pub fn read_from<R: Read>(&mut self, mut r: R) -> Result<()> {
        for shard in &self.shards {
            shard.write().clear();
        }

        for shard in &self.shards {
            let mut map = shard.write();
            let len: u64 = bincode::deserialize_from(&mut r)?;
            map.reserve(len as usize);
            for _ in 0..len {
                let k: PositionKey = bincode::deserialize_from(&mut r)?;
                let v: Entry = bincode::deserialize_from(&mut r)?;
                map.insert(k, v);
            }
        }
        Ok(())
    }
}

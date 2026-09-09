use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub struct Stats {
    pub nodes: AtomicU64,
    pub tt_hits: AtomicU64,
    pub tt_stores: AtomicU64,
}

impl Stats {
    #[inline]
    pub fn inc_nodes(&self) {
        self.nodes.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_tt_hits(&self) {
        self.tt_hits.fetch_add(1, Ordering::Relaxed);
    }

    #[inline]
    pub fn inc_tt_stores(&self) {
        self.tt_stores.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.nodes.load(Ordering::Relaxed),
            self.tt_hits.load(Ordering::Relaxed),
            self.tt_stores.load(Ordering::Relaxed),
        )
    }
}

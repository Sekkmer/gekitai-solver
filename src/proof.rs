use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::movegen::{apply_move, generate_moves, MoveOutcome};
use crate::position::{Position, PositionKey};
use crate::rules::{RepetitionRule, Rules};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Outcome {
    Win,
    Loss,
    Draw,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Win => "win",
            Outcome::Loss => "loss",
            Outcome::Draw => "draw",
        }
    }
}

#[derive(Debug)]
pub struct ProofResult {
    pub outcome: Outcome,
    pub states: usize,
    pub edges: usize,
}

#[derive(Clone, Debug)]
pub struct ProofConfig {
    pub max_states: Option<usize>,
    pub requested_max_ram_bytes: Option<u64>,
    pub automatic_ram_ceiling_bytes: u64,
    pub system_reserve_bytes: u64,
    pub disk_dir: Option<PathBuf>,
    pub resume: bool,
    pub fast_ram: bool,
    pub checkpoint_seconds: u64,
}

#[derive(Clone, Copy)]
enum LocalAction {
    ImmediateWin,
    ImmediateLoss,
    ImmediateDraw,
    Next { key: PositionKey },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ExpansionCheckpoint {
    layer: usize,
    next_idx: u64,
    edges: usize,
    frontier: Vec<u64>,
    frontier_pos: Vec<PositionKey>,
    processed_frontier: usize,
    next_frontier: Vec<u64>,
    next_frontier_pos: Vec<PositionKey>,
    pending_next: Vec<u32>,
    has_any_move: Vec<u8>,
    has_immediate_loss: Vec<u8>,
    has_immediate_win: Vec<u8>,
    edge_lengths: Vec<u64>,
}

#[derive(Clone, Copy)]
struct ExpansionStateRef<'a> {
    layer: usize,
    next_idx: u64,
    edges: usize,
    frontier: &'a [u64],
    frontier_pos: &'a [PositionKey],
    processed_frontier: usize,
    next_frontier: &'a [u64],
    next_frontier_pos: &'a [PositionKey],
    pending_next: &'a [u32],
    has_any_move: &'a [u8],
    has_immediate_loss: &'a [u8],
    has_immediate_win: &'a [u8],
}

#[derive(Serialize)]
struct ExpansionCheckpointRef<'a> {
    layer: usize,
    next_idx: u64,
    edges: usize,
    frontier: &'a [u64],
    frontier_pos: &'a [PositionKey],
    processed_frontier: usize,
    next_frontier: &'a [u64],
    next_frontier_pos: &'a [PositionKey],
    pending_next: &'a [u32],
    has_any_move: &'a [u8],
    has_immediate_loss: &'a [u8],
    has_immediate_win: &'a [u8],
    edge_lengths: Vec<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct RetrogradeCheckpoint {
    next_idx: u64,
    edges: usize,
    pending_next: Vec<u32>,
    outcome: Vec<u8>,
    changed: Vec<u64>,
    rounds: usize,
}

#[derive(Serialize)]
struct RetrogradeCheckpointRef<'a> {
    next_idx: u64,
    edges: usize,
    pending_next: &'a [u32],
    outcome: &'a [u8],
    changed: &'a [u64],
    rounds: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
enum ProofCheckpoint {
    Expansion(ExpansionCheckpoint),
    Retrograde(RetrogradeCheckpoint),
}

#[derive(Serialize)]
enum ProofCheckpointRef<'a> {
    Expansion(ExpansionCheckpointRef<'a>),
    Retrograde(RetrogradeCheckpointRef<'a>),
}

const UNKNOWN: u8 = 0;
const WIN: u8 = 1;
const LOSS: u8 = 2;
const DRAW: u8 = 3;

const EDGE_BUCKETS: usize = 64;
const EDGE_SUBBUCKETS: usize = 8;
const EDGE_SHARDS: usize = EDGE_BUCKETS * EDGE_SUBBUCKETS;
const FRONTIER_CHUNK: usize = 100_000;
const HOT_CACHE_LIMIT: usize = 2_000_000;
const CKPT_FILE: &str = "proof-expansion.ckpt.zst";
const CKPT_VERSION: u32 = 3;
const EDGE_REC_BYTES: usize = 16;
const EDGE_INDEX_STRIDE_RECS: u64 = 1024;
const TEMP_BATCH_MIN_KEYS: usize = 10_000;
const EDGE_FLUSH_BYTES: usize = 1 << 20;
const FAST_KEYS_FILE: &str = "states-fast.keys";
static MIN_SYSTEM_AVAILABLE_BYTES: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CheckpointEnvelope {
    version: u32,
    payload: ProofCheckpoint,
}

#[inline]
fn edge_shard_idx(child: u64) -> usize {
    let bucket = (child as usize) & (EDGE_BUCKETS - 1);
    let sub = ((child as usize) >> 6) & (EDGE_SUBBUCKETS - 1);
    bucket * EDGE_SUBBUCKETS + sub
}

fn hot_cache_lookup(
    hot_a: &mut hashbrown::HashMap<PositionKey, u64>,
    hot_b: &mut hashbrown::HashMap<PositionKey, u64>,
    key: PositionKey,
) -> Option<u64> {
    if let Some(&v) = hot_a.get(&key) {
        return Some(v);
    }
    if let Some(&v) = hot_b.get(&key) {
        hot_a.insert(key, v);
        return Some(v);
    }
    None
}

fn hot_cache_insert(
    hot_a: &mut hashbrown::HashMap<PositionKey, u64>,
    hot_b: &mut hashbrown::HashMap<PositionKey, u64>,
    key: PositionKey,
    value: u64,
) {
    hot_a.insert(key, value);
    if hot_a.len() > HOT_CACHE_LIMIT {
        std::mem::swap(hot_a, hot_b);
        hot_a.clear();
    }
}

fn current_rss_bytes() -> Option<u64> {
    let status = fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kb * 1024)
}

fn mem_available_bytes() -> Option<u64> {
    let meminfo = fs::read_to_string("/proc/meminfo").ok()?;
    let line = meminfo.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kb * 1024)
}

fn choose_ram_limit(
    requested: Option<u64>,
    automatic_ceiling: u64,
    system_reserve: u64,
) -> Result<u64> {
    let rss = current_rss_bytes().context("cannot read solver RSS from /proc/self/status")?;
    let available = mem_available_bytes().context("cannot read MemAvailable from /proc/meminfo")?;
    if available <= system_reserve {
        bail!(
            "not enough currently available RAM to preserve the requested system reserve (available={} GiB, reserve={} GiB)",
            available / (1024 * 1024 * 1024),
            system_reserve / (1024 * 1024 * 1024)
        );
    }
    let requested_or_ceiling = requested.unwrap_or(automatic_ceiling);
    let safe_from_current_load = rss.saturating_add(available - system_reserve);
    let effective = requested_or_ceiling.min(safe_from_current_load);
    if effective <= rss.saturating_add(1024 * 1024 * 1024) {
        bail!("safe proof memory budget is less than 1 GiB above current RSS");
    }
    info!(
        "proof memory budget: rss_limit={} GiB (requested/ceiling={} GiB, current_available={} GiB, system_reserve={} GiB)",
        effective / (1024 * 1024 * 1024),
        requested_or_ceiling / (1024 * 1024 * 1024),
        available / (1024 * 1024 * 1024),
        system_reserve / (1024 * 1024 * 1024)
    );
    Ok(effective)
}

fn enforce_ram_limit(max_ram_bytes: Option<u64>) -> Result<()> {
    let reserve = MIN_SYSTEM_AVAILABLE_BYTES.load(Ordering::Relaxed);
    if reserve != 0 && mem_available_bytes().is_some_and(|available| available < reserve) {
        bail!(
            "system MemAvailable fell below the reserved {} GiB",
            reserve / (1024 * 1024 * 1024)
        );
    }
    if let Some(limit) = max_ram_bytes {
        if let Some(rss) = current_rss_bytes() {
            if rss > limit {
                bail!(
                    "proof mode exceeded --proof-max-ram-gb limit (rss={} GiB > limit={} GiB)",
                    rss / (1024 * 1024 * 1024),
                    limit / (1024 * 1024 * 1024)
                );
            }
        }
    }
    Ok(())
}

fn ram_limit_exceeded(max_ram_bytes: Option<u64>) -> bool {
    let reserve = MIN_SYSTEM_AVAILABLE_BYTES.load(Ordering::Relaxed);
    if reserve != 0 && mem_available_bytes().is_some_and(|available| available < reserve) {
        return true;
    }
    if let Some(limit) = max_ram_bytes {
        if let Some(rss) = current_rss_bytes() {
            return rss > limit;
        }
    }
    false
}

fn should_stop(stop_flag: &Arc<AtomicBool>) -> bool {
    stop_flag.load(Ordering::Relaxed)
}

fn run_disk_dir(base: Option<&Path>, resume: bool) -> Result<PathBuf> {
    if resume {
        let dir = base.context("--proof-resume requires --proof-disk-dir")?;
        if !dir.exists() {
            bail!("proof resume dir does not exist: {}", dir.display());
        }
        return Ok(dir.to_path_buf());
    }

    let root = base.unwrap_or_else(|| Path::new("/tmp/gekitai-proof"));
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let dir = root.join(format!("run-{}-{ts}", std::process::id()));
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    Ok(dir)
}

fn key_sql(k: PositionKey) -> i64 {
    debug_assert!(k <= i64::MAX as u64);
    k as i64
}

fn open_state_index(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    conn.execute_batch(
        "
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA locking_mode = EXCLUSIVE;
        PRAGMA wal_autocheckpoint = 0;
        PRAGMA temp_store = FILE;
        PRAGMA mmap_size = 268435456;
        PRAGMA cache_size = -200000;
        CREATE TABLE IF NOT EXISTS states (
            key INTEGER PRIMARY KEY,
            idx INTEGER NOT NULL UNIQUE
        ) WITHOUT ROWID;
        CREATE UNIQUE INDEX IF NOT EXISTS states_by_idx ON states(idx);
        CREATE TEMP TABLE IF NOT EXISTS chunk_keys (key INTEGER PRIMARY KEY);
        ",
    )?;
    Ok(conn)
}

fn read_edge_record(rec: &[u8; EDGE_REC_BYTES]) -> (u64, u64) {
    let child = u64::from_le_bytes([
        rec[0], rec[1], rec[2], rec[3], rec[4], rec[5], rec[6], rec[7],
    ]);
    let parent = u64::from_le_bytes([
        rec[8], rec[9], rec[10], rec[11], rec[12], rec[13], rec[14], rec[15],
    ]);
    (child, parent)
}

fn write_edge_record(rec: &mut [u8; EDGE_REC_BYTES], child: u64, parent: u64) {
    rec[0..8].copy_from_slice(&child.to_le_bytes());
    rec[8..16].copy_from_slice(&parent.to_le_bytes());
}

fn sort_edge_shards(
    edge_paths: &[PathBuf],
    stop_flag: &Arc<AtomicBool>,
    max_ram_bytes: Option<u64>,
) -> Result<()> {
    info!("sorting edge shards for attractor pass...");
    for (i, path) in edge_paths.iter().enumerate() {
        enforce_ram_limit(max_ram_bytes)?;
        if should_stop(stop_flag) {
            bail!("proof mode interrupted by Ctrl+C");
        }
        let mut r =
            BufReader::new(File::open(path).with_context(|| format!("open {}", path.display()))?);
        let mut edges: Vec<(u64, u64)> = Vec::new();
        let mut rec = [0u8; EDGE_REC_BYTES];
        loop {
            match r.read_exact(&mut rec) {
                Ok(()) => edges.push(read_edge_record(&rec)),
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(e.into()),
            }
        }
        edges.sort_unstable_by_key(|&(child, _)| child);

        let tmp = path.with_extension("bin.tmp");
        let mut w = BufWriter::new(
            File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?,
        );
        for (child, parent) in edges {
            let mut out = [0u8; EDGE_REC_BYTES];
            write_edge_record(&mut out, child, parent);
            w.write_all(&out)?;
        }
        w.flush()?;
        fs::rename(&tmp, path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;

        if (i % 64) == 0 {
            info!("sorted edge shards: {}/{}", i + 1, edge_paths.len());
        }
    }
    info!("edge shard sorting complete");
    Ok(())
}

fn build_sparse_edge_index(path: &Path) -> Result<Vec<(u64, u64)>> {
    let mut r =
        BufReader::new(File::open(path).with_context(|| format!("open {}", path.display()))?);
    let mut out = Vec::new();
    let mut rec = [0u8; EDGE_REC_BYTES];
    let mut rec_no: u64 = 0;
    loop {
        match r.read_exact(&mut rec) {
            Ok(()) => {
                if rec_no.is_multiple_of(EDGE_INDEX_STRIDE_RECS) {
                    let child = u64::from_le_bytes([
                        rec[0], rec[1], rec[2], rec[3], rec[4], rec[5], rec[6], rec[7],
                    ]);
                    out.push((child, rec_no * EDGE_REC_BYTES as u64));
                }
                rec_no += 1;
            }
            Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(out)
}

fn save_ckpt<T: Serialize>(path: &Path, payload: &T) -> Result<()> {
    let tmp = path.with_extension("zst.tmp");
    let f = File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    let buf = BufWriter::new(f);
    let mut enc = zstd::stream::write::Encoder::new(buf, 3)?;
    #[derive(Serialize)]
    struct EnvelopeRef<'a, T> {
        version: u32,
        payload: &'a T,
    }
    let envelope = EnvelopeRef {
        version: CKPT_VERSION,
        payload,
    };
    bincode::serialize_into(&mut enc, &envelope)?;
    let mut buf = enc.finish()?;
    buf.flush()?;
    buf.get_ref().sync_all()?;
    fs::rename(&tmp, path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

fn load_ckpt(path: &Path) -> Result<ProofCheckpoint> {
    let f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let buf = BufReader::new(f);
    let mut dec = zstd::stream::read::Decoder::new(buf)?;
    let envelope: CheckpointEnvelope = bincode::deserialize_from(&mut dec)?;
    if envelope.version != CKPT_VERSION {
        bail!(
            "unsupported proof checkpoint version {} (expected {})",
            envelope.version,
            CKPT_VERSION
        );
    }
    Ok(envelope.payload)
}

fn save_expansion_checkpoint(
    ckpt_path: &Path,
    state: ExpansionStateRef<'_>,
    edge_writers: &mut [BufWriter<File>],
    conn: &Connection,
    mut fast_keys_writer: Option<&mut BufWriter<File>>,
) -> Result<()> {
    let mut edge_lengths = Vec::with_capacity(edge_writers.len());
    for w in edge_writers.iter_mut() {
        w.flush()?;
        w.get_ref().sync_data()?;
        edge_lengths.push(w.get_ref().metadata()?.len());
    }
    if let Some(w) = fast_keys_writer.as_mut() {
        w.flush()?;
        w.get_ref().sync_data()?;
    }
    conn.execute_batch("PRAGMA wal_checkpoint(FULL);")?;

    let ck = ExpansionCheckpointRef {
        layer: state.layer,
        next_idx: state.next_idx,
        edges: state.edges,
        frontier: state.frontier,
        frontier_pos: state.frontier_pos,
        processed_frontier: state.processed_frontier,
        next_frontier: state.next_frontier,
        next_frontier_pos: state.next_frontier_pos,
        pending_next: state.pending_next,
        has_any_move: state.has_any_move,
        has_immediate_loss: state.has_immediate_loss,
        has_immediate_win: state.has_immediate_win,
        edge_lengths,
    };
    save_ckpt(ckpt_path, &ProofCheckpointRef::Expansion(ck))?;
    Ok(())
}

pub fn solve_start_position(
    rules: &Rules,
    config: &ProofConfig,
    stop_flag: Arc<AtomicBool>,
) -> Result<ProofResult> {
    if rules.repetition != RepetitionRule::PathRepeatIsDraw {
        bail!("exact graph proof requires cycles allowed; forbidden repetition needs history in the state");
    }
    let t0 = Instant::now();
    MIN_SYSTEM_AVAILABLE_BYTES.store(config.system_reserve_bytes, Ordering::Relaxed);
    let max_ram_bytes = Some(choose_ram_limit(
        config.requested_max_ram_bytes,
        config.automatic_ram_ceiling_bytes,
        config.system_reserve_bytes,
    )?);
    let disk_dir = run_disk_dir(config.disk_dir.as_deref(), config.resume)?;
    info!("proof backing dir: {}", disk_dir.display());
    // Validate BEFORE opening/truncating backing stores. Older runs without
    // rule metadata cannot safely establish which game was being solved.
    let manifest_path = disk_dir.join("proof-rules.bin");
    if config.resume {
        let saved: (u32, Rules, bool) = bincode::deserialize_from(BufReader::new(
            File::open(&manifest_path).context("proof run lacks rule metadata; start a new run")?,
        ))?;
        if saved != (1, *rules, config.fast_ram) {
            bail!("proof checkpoint rules or storage mode differ from requested run");
        }
    } else {
        let mut f = File::create(&manifest_path)?;
        bincode::serialize_into(&mut f, &(1u32, *rules, config.fast_ram))?;
        f.sync_all()?;
        File::open(&disk_dir)?.sync_all()?;
    }
    let ckpt_path = disk_dir.join(CKPT_FILE);
    let fast_keys_path = disk_dir.join(FAST_KEYS_FILE);

    let state_db_path = disk_dir.join("states.sqlite");
    let mut conn = open_state_index(&state_db_path)?;
    let mut state_map = hashbrown::HashMap::<PositionKey, u64>::default();

    let mut edge_paths = Vec::with_capacity(EDGE_SHARDS);
    let mut edge_writers = Vec::with_capacity(EDGE_SHARDS);
    for b in 0..EDGE_BUCKETS {
        for s in 0..EDGE_SUBBUCKETS {
            let p = disk_dir.join(format!("edges-{b:03}-{s:02}.bin"));
            let f = if config.resume {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&p)
                    .with_context(|| format!("open {}", p.display()))?
            } else {
                File::create(&p).with_context(|| format!("create {}", p.display()))?
            };
            edge_paths.push(p);
            edge_writers.push(BufWriter::new(f));
        }
    }

    let loaded_ckpt = if config.resume {
        Some(load_ckpt(&ckpt_path).with_context(|| "failed to load proof checkpoint")?)
    } else {
        None
    };
    if let Some(ProofCheckpoint::Expansion(ck)) = loaded_ckpt.as_ref() {
        if ck.edge_lengths.len() != edge_writers.len() {
            bail!(
                "checkpoint records {} edge shards, but solver expects {}",
                ck.edge_lengths.len(),
                edge_writers.len()
            );
        }
        for (writer, &checkpoint_len) in edge_writers.iter_mut().zip(&ck.edge_lengths) {
            writer.get_mut().set_len(checkpoint_len)?;
            writer.get_mut().seek(SeekFrom::End(0))?;
        }
        if !config.fast_ram {
            conn.execute(
                "DELETE FROM states WHERE idx >= ?1",
                params![ck.next_idx as i64],
            )?;
            let retained: u64 = conn.query_row(
                "SELECT COUNT(*) FROM states WHERE idx < ?1",
                params![ck.next_idx as i64],
                |row| row.get(0),
            )?;
            if retained != ck.next_idx {
                bail!(
                    "state index is incomplete at checkpoint: retained={} expected={}",
                    retained,
                    ck.next_idx
                );
            }
            conn.execute_batch("PRAGMA wal_checkpoint(FULL);")?;
        }
    }
    let mut retro_resume: Option<RetrogradeCheckpoint> = None;
    let mut start_key_for_fresh: Option<PositionKey> = None;

    let (
        mut layer,
        mut next_idx,
        mut edges,
        mut frontier,
        mut frontier_pos,
        mut processed_frontier,
        mut next_frontier,
        mut next_frontier_pos,
        mut pending_next,
        mut has_any_move,
        mut has_immediate_loss,
        mut has_immediate_win,
    ) = if let Some(ProofCheckpoint::Expansion(ck)) = loaded_ckpt.as_ref() {
        info!(
            "resumed expansion checkpoint: layer={} states={} frontier={} processed={}",
            ck.layer,
            ck.next_idx,
            ck.frontier.len(),
            ck.processed_frontier
        );
        (
            ck.layer,
            ck.next_idx,
            ck.edges,
            ck.frontier.clone(),
            ck.frontier_pos.clone(),
            ck.processed_frontier,
            ck.next_frontier.clone(),
            ck.next_frontier_pos.clone(),
            ck.pending_next.clone(),
            ck.has_any_move.clone(),
            ck.has_immediate_loss.clone(),
            ck.has_immediate_win.clone(),
        )
    } else if let Some(ProofCheckpoint::Retrograde(ck)) = loaded_ckpt.as_ref() {
        info!(
            "resumed retrograde checkpoint: states={} changed={} rounds={}",
            ck.next_idx,
            ck.changed.len(),
            ck.rounds
        );
        retro_resume = Some(ck.clone());
        (
            0usize,
            ck.next_idx,
            ck.edges,
            Vec::new(),
            Vec::new(),
            0usize,
            Vec::new(),
            Vec::new(),
            ck.pending_next.clone(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    } else {
        let start = Position::start();
        let start_key = start.canonical_key();
        start_key_for_fresh = Some(start_key);
        if config.fast_ram {
            state_map.insert(start_key, 0);
        } else {
            conn.execute(
                "INSERT OR IGNORE INTO states(key, idx) VALUES (?1, ?2)",
                params![key_sql(start_key), 0_i64],
            )?;
        }
        (
            0usize,
            1u64,
            0usize,
            vec![0u64],
            vec![start_key],
            0usize,
            Vec::new(),
            Vec::new(),
            vec![0u32],
            vec![0u8],
            vec![0u8],
            vec![0u8],
        )
    };

    let mut fast_keys_writer: Option<BufWriter<File>> = None;
    if config.fast_ram {
        let mut f = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&fast_keys_path)
            .with_context(|| format!("open {}", fast_keys_path.display()))?;

        if config.resume && !frontier.is_empty() {
            let file_states = f.metadata()?.len() / 8;
            if file_states < next_idx {
                bail!(
                    "fast-ram key file has only {} states but checkpoint expects {}",
                    file_states,
                    next_idx
                );
            }
            let mut r = BufReader::new(
                File::open(&fast_keys_path)
                    .with_context(|| format!("open {}", fast_keys_path.display()))?,
            );
            for idx in 0..next_idx {
                let mut kb = [0u8; 8];
                r.read_exact(&mut kb)?;
                state_map.insert(PositionKey::from_le_bytes(kb), idx);
            }
        } else if !config.resume {
            f.set_len(0)?;
            if let Some(k) = start_key_for_fresh {
                f.write_all(&k.to_le_bytes())?;
            }
        }

        f.set_len(next_idx.saturating_mul(8))?;
        f.seek(SeekFrom::End(0))?;
        fast_keys_writer = Some(BufWriter::new(f));
    }

    let mut hot_cache_a = hashbrown::HashMap::<PositionKey, u64>::default();
    let mut hot_cache_b = hashbrown::HashMap::<PositionKey, u64>::default();
    let checkpoint_interval = std::time::Duration::from_secs(config.checkpoint_seconds.max(1));
    let mut last_checkpoint = Instant::now();
    while !frontier.is_empty() {
        // A resumed checkpoint may be part-way through the current layer.
        // Only advance the layer number when starting a fresh frontier.
        if processed_frontier == 0 {
            layer += 1;
        }
        if ram_limit_exceeded(max_ram_bytes) {
            let state = ExpansionStateRef {
                layer,
                next_idx,
                edges,
                frontier: &frontier,
                frontier_pos: &frontier_pos,
                processed_frontier,
                next_frontier: &next_frontier,
                next_frontier_pos: &next_frontier_pos,
                pending_next: &pending_next,
                has_any_move: &has_any_move,
                has_immediate_loss: &has_immediate_loss,
                has_immediate_win: &has_immediate_win,
            };
            save_expansion_checkpoint(
                &ckpt_path,
                state,
                &mut edge_writers,
                &conn,
                fast_keys_writer.as_mut(),
            )?;
            bail!("proof mode exceeded --proof-max-ram-gb (checkpoint saved)");
        }

        let frontier_len = frontier.len();
        let layer_t0 = Instant::now();
        let states_before_layer = next_idx;
        let edges_before_layer = edges;
        let layer_inlayer_hits: u64 = 0;
        let mut layer_hot_hits: u64 = 0;
        let mut layer_db_hits: u64 = 0;
        if processed_frontier == 0 {
            next_frontier.clear();
            next_frontier_pos.clear();
        }
        let mut processed_display = processed_frontier;

        for chunk_start in (processed_frontier..frontier_len).step_by(FRONTIER_CHUNK) {
            if should_stop(&stop_flag) {
                let state = ExpansionStateRef {
                    layer,
                    next_idx,
                    edges,
                    frontier: &frontier,
                    frontier_pos: &frontier_pos,
                    processed_frontier: chunk_start,
                    next_frontier: &next_frontier,
                    next_frontier_pos: &next_frontier_pos,
                    pending_next: &pending_next,
                    has_any_move: &has_any_move,
                    has_immediate_loss: &has_immediate_loss,
                    has_immediate_win: &has_immediate_win,
                };
                save_expansion_checkpoint(
                    &ckpt_path,
                    state,
                    &mut edge_writers,
                    &conn,
                    fast_keys_writer.as_mut(),
                )?;
                bail!("proof mode interrupted by Ctrl+C (checkpoint saved)");
            }

            let chunk_end = (chunk_start + FRONTIER_CHUNK).min(frontier_len);
            let idx_slice = &frontier[chunk_start..chunk_end];
            let pos_slice = &frontier_pos[chunk_start..chunk_end];
            if ram_limit_exceeded(max_ram_bytes) {
                let state = ExpansionStateRef {
                    layer,
                    next_idx,
                    edges,
                    frontier: &frontier,
                    frontier_pos: &frontier_pos,
                    processed_frontier: chunk_start,
                    next_frontier: &next_frontier,
                    next_frontier_pos: &next_frontier_pos,
                    pending_next: &pending_next,
                    has_any_move: &has_any_move,
                    has_immediate_loss: &has_immediate_loss,
                    has_immediate_win: &has_immediate_win,
                };
                save_expansion_checkpoint(
                    &ckpt_path,
                    state,
                    &mut edge_writers,
                    &conn,
                    fast_keys_writer.as_mut(),
                )?;
                bail!("proof mode exceeded --proof-max-ram-gb (checkpoint saved)");
            }

            let expansions: Vec<(u64, Vec<LocalAction>)> = pos_slice
                .par_iter()
                .zip(idx_slice.par_iter())
                .map(|(&packed_pos, &idx)| {
                    let pos = Position::from_key(packed_pos);
                    let moves = generate_moves(&pos);
                    let mut local = Vec::with_capacity(moves.len());

                    for mv in moves {
                        let applied = apply_move(&pos, mv, rules);
                        match applied.outcome {
                            MoveOutcome::MoverWin => {
                                local.clear();
                                local.push(LocalAction::ImmediateWin);
                                break;
                            }
                            MoveOutcome::MoverLoss => local.push(LocalAction::ImmediateLoss),
                            MoveOutcome::Draw => local.push(LocalAction::ImmediateDraw),
                            MoveOutcome::Ongoing => local.push(LocalAction::Next {
                                key: applied.next.canonical_key(),
                            }),
                        }
                    }

                    (idx, local)
                })
                .collect();

            let mut key_to_idx = hashbrown::HashMap::<PositionKey, u64>::default();
            let tx_t0 = Instant::now();

            // Phase 1: collect candidate keys in parallel, then dedupe/split by cache status.
            let mut unknown_keys: Vec<PositionKey> = expansions
                .par_iter()
                .flat_map_iter(|(_, local_actions)| {
                    local_actions.iter().filter_map(|la| match *la {
                        LocalAction::Next { key } => Some(key),
                        _ => None,
                    })
                })
                .collect();
            unknown_keys.sort_unstable();
            unknown_keys.dedup();
            let mut unresolved = Vec::<PositionKey>::with_capacity(unknown_keys.len());
            for key in unknown_keys {
                if let Some(j) = hot_cache_lookup(&mut hot_cache_a, &mut hot_cache_b, key) {
                    layer_hot_hits += 1;
                    key_to_idx.insert(key, j);
                } else {
                    unresolved.push(key);
                }
            }

            // Phase 2: resolve existing keys with one temp-table join, then insert the rest.
            let mut resolved =
                hashbrown::HashMap::<PositionKey, u64>::with_capacity(unresolved.len());
            let mut used_temp_batch = false;
            if config.fast_ram {
                for key in unresolved {
                    let j = if let Some(&v) = state_map.get(&key) {
                        layer_db_hits += 1;
                        v
                    } else {
                        let j = next_idx;
                        state_map.insert(key, j);
                        if let Some(w) = fast_keys_writer.as_mut() {
                            w.write_all(&key.to_le_bytes())?;
                        }
                        next_idx += 1;
                        pending_next.push(0);
                        has_any_move.push(0);
                        has_immediate_loss.push(0);
                        has_immediate_win.push(0);
                        next_frontier.push(j);
                        next_frontier_pos.push(key);
                        if (j % 1_000_000) == 0 {
                            info!("proof graph expansion: states={j}");
                            enforce_ram_limit(max_ram_bytes)?;
                        }
                        j
                    };
                    resolved.insert(key, j);
                    key_to_idx.insert(key, j);
                    hot_cache_insert(&mut hot_cache_a, &mut hot_cache_b, key, j);
                }
            } else {
                let tx = conn.transaction()?;
                let mut ins =
                    tx.prepare_cached("INSERT OR IGNORE INTO states(key, idx) VALUES (?1, ?2)")?;
                let mut sel_one = tx.prepare_cached("SELECT idx FROM states WHERE key = ?1")?;
                let mut existing = hashbrown::HashMap::<PositionKey, u64>::default();
                used_temp_batch = unresolved.len() >= TEMP_BATCH_MIN_KEYS;
                if used_temp_batch {
                    tx.execute_batch("DELETE FROM chunk_keys;")?;
                    let mut ins_chunk =
                        tx.prepare_cached("INSERT OR IGNORE INTO chunk_keys(key) VALUES (?1)")?;
                    for key in &unresolved {
                        ins_chunk.execute(params![key_sql(*key)])?;
                    }
                    drop(ins_chunk);

                    let mut q = tx.prepare_cached(
                        "SELECT s.key, s.idx FROM states s INNER JOIN chunk_keys c ON c.key=s.key",
                    )?;
                    let mut rows = q.query([])?;
                    while let Some(row) = rows.next()? {
                        let key = row.get::<_, i64>(0)? as PositionKey;
                        let idx: i64 = row.get(1)?;
                        existing.insert(key, idx as u64);
                    }
                }

                // Phase 3: resolve all unknown keys (existing or new).
                for key in unresolved {
                    let j = if let Some(&v) = existing.get(&key) {
                        layer_db_hits += 1;
                        v
                    } else {
                        let sql_key = key_sql(key);
                        let candidate = next_idx as i64;
                        let inserted = ins.execute(params![sql_key, candidate])?;
                        if inserted == 1 {
                            let j = next_idx;
                            next_idx += 1;
                            pending_next.push(0);
                            has_any_move.push(0);
                            has_immediate_loss.push(0);
                            has_immediate_win.push(0);
                            next_frontier.push(j);
                            next_frontier_pos.push(key);
                            if (j % 1_000_000) == 0 {
                                info!("proof graph expansion: states={j}");
                                enforce_ram_limit(max_ram_bytes)?;
                            }
                            j
                        } else {
                            // Race with duplicate in this chunk path, recover from preloaded map miss.
                            let q_idx: Option<i64> = sel_one
                                .query_row(params![sql_key], |row| row.get(0))
                                .optional()?;
                            let v = q_idx.context("state lookup failed after insert race")?;
                            layer_db_hits += 1;
                            v as u64
                        }
                    };
                    resolved.insert(key, j);
                    key_to_idx.insert(key, j);
                    hot_cache_insert(&mut hot_cache_a, &mut hot_cache_b, key, j);
                }
                drop(ins);
                drop(sel_one);
                tx.commit()?;
            }

            // Phase 4: emit edges using resolved mapping.
            let mut shard_buffers: Vec<Vec<u8>> = (0..EDGE_SHARDS).map(|_| Vec::new()).collect();
            for (parent_idx, local_actions) in expansions {
                let p = parent_idx as usize;
                let mut any = false;
                let mut imm_loss = false;
                let mut imm_win = false;

                for la in local_actions {
                    match la {
                        LocalAction::ImmediateWin => {
                            any = true;
                            imm_win = true;
                        }
                        LocalAction::ImmediateLoss => {
                            any = true;
                            imm_loss = true;
                        }
                        LocalAction::ImmediateDraw => {
                            any = true;
                            // A terminal draw is an option that can never be
                            // disproven as a loss, so keep one permanent count.
                            pending_next[p] = pending_next[p].saturating_add(1);
                        }
                        LocalAction::Next { key } => {
                            any = true;
                            pending_next[p] = pending_next[p].saturating_add(1);
                            let child_idx = *key_to_idx
                                .get(&key)
                                .or_else(|| resolved.get(&key))
                                .context("missing key mapping during edge emission")?;

                            let shard = edge_shard_idx(child_idx);
                            let buf = &mut shard_buffers[shard];
                            buf.extend_from_slice(&child_idx.to_le_bytes());
                            buf.extend_from_slice(&parent_idx.to_le_bytes());
                            if buf.len() >= EDGE_FLUSH_BYTES {
                                edge_writers[shard].write_all(buf)?;
                                buf.clear();
                            }
                            edges += 1;
                        }
                    }
                }

                has_any_move[p] = if any { 1 } else { 0 };
                if imm_loss {
                    has_immediate_loss[p] = 1;
                }
                if imm_win {
                    has_immediate_win[p] = 1;
                }
            }
            for (shard, buf) in shard_buffers.iter_mut().enumerate() {
                if !buf.is_empty() {
                    edge_writers[shard].write_all(buf)?;
                    buf.clear();
                }
            }

            let state_limit_hit = config
                .max_states
                .is_some_and(|limit| next_idx as usize >= limit);
            if last_checkpoint.elapsed() >= checkpoint_interval || state_limit_hit {
                let state = ExpansionStateRef {
                    layer,
                    next_idx,
                    edges,
                    frontier: &frontier,
                    frontier_pos: &frontier_pos,
                    processed_frontier: chunk_end,
                    next_frontier: &next_frontier,
                    next_frontier_pos: &next_frontier_pos,
                    pending_next: &pending_next,
                    has_any_move: &has_any_move,
                    has_immediate_loss: &has_immediate_loss,
                    has_immediate_win: &has_immediate_win,
                };
                save_expansion_checkpoint(
                    &ckpt_path,
                    state,
                    &mut edge_writers,
                    &conn,
                    fast_keys_writer.as_mut(),
                )?;
                last_checkpoint = Instant::now();
                info!(
                    "durable expansion checkpoint: layer={} processed={}/{} states={} edges={}",
                    layer, chunk_end, frontier_len, next_idx, edges
                );
                if let Some(limit) = config.max_states.filter(|_| state_limit_hit) {
                    bail!(
                        "proof mode reached --proof-max-states={limit} (checkpoint saved; actual states={next_idx})"
                    );
                }
            }

            processed_display += chunk_end - chunk_start;
            if (processed_display % 1_000_000) < FRONTIER_CHUNK {
                let dt = layer_t0.elapsed().as_secs_f64().max(0.001);
                let layer_states = next_idx.saturating_sub(states_before_layer);
                let layer_edges = edges.saturating_sub(edges_before_layer);
                info!(
                    "proof layer {}: processed {}/{} frontier nodes, states={}, next_frontier={}, layer_rate: states/s={:.0} edges/s={:.0}, dedupe hits in_layer/hot/db={}/{}/{}, tx_ms={:.1}",
                    layer,
                    processed_display,
                    frontier_len,
                    next_idx,
                    next_frontier.len(),
                    (layer_states as f64) / dt,
                    (layer_edges as f64) / dt,
                    layer_inlayer_hits,
                    layer_hot_hits,
                    layer_db_hits,
                    tx_t0.elapsed().as_secs_f64() * 1000.0
                );
                if used_temp_batch {
                    info!(
                        "proof layer {} chunk used temp-batch lookup (keys>={})",
                        layer, TEMP_BATCH_MIN_KEYS
                    );
                }
            }
        }

        processed_frontier = 0;
        if next_frontier.is_empty() {
            info!("proof expansion fixed point reached at states={next_idx}");
        }
        info!(
            "proof layer {} summary: frontier={} new_states={} new_edges={} elapsed={:.1}s",
            layer,
            frontier_len,
            next_idx.saturating_sub(states_before_layer),
            edges.saturating_sub(edges_before_layer),
            layer_t0.elapsed().as_secs_f64()
        );
        frontier = std::mem::take(&mut next_frontier);
        frontier_pos = std::mem::take(&mut next_frontier_pos);
    }

    for w in &mut edge_writers {
        w.flush()?;
    }
    if let Some(w) = fast_keys_writer.as_mut() {
        w.flush()?;
    }
    drop(edge_writers);
    drop(fast_keys_writer);
    state_map.clear();

    if retro_resume.is_none() {
        sort_edge_shards(&edge_paths, &stop_flag, max_ram_bytes)?;
    }
    let mut edge_sparse_idx: Vec<Vec<(u64, u64)>> = Vec::with_capacity(edge_paths.len());
    for p in &edge_paths {
        edge_sparse_idx.push(build_sparse_edge_index(p)?);
    }

    let n = next_idx as usize;
    let (mut outcome, mut changed, mut rounds) = if let Some(ck) = retro_resume {
        (ck.outcome, ck.changed, ck.rounds)
    } else {
        let mut outcome: Vec<u8> = vec![UNKNOWN; n];
        let mut changed = Vec::new();
        info!("proof expansion complete: states={n}, edges={edges}; initializing retrograde...");
        for i in 0..n {
            if (i % 200_000) == 0 {
                enforce_ram_limit(max_ram_bytes)?;
                if should_stop(&stop_flag) {
                    bail!("proof mode interrupted by Ctrl+C");
                }
            }
            if has_immediate_win[i] != 0 {
                outcome[i] = WIN;
                changed.push(i as u64);
            } else if has_any_move[i] == 0 {
                outcome[i] = DRAW;
            } else if pending_next[i] == 0 && has_immediate_loss[i] != 0 {
                outcome[i] = LOSS;
                changed.push(i as u64);
            }
        }
        has_any_move.clear();
        has_immediate_loss.clear();
        has_immediate_win.clear();
        (outcome, changed, 0usize)
    };

    info!("retrograde propagation start...");
    while !changed.is_empty() {
        rounds += 1;
        enforce_ram_limit(max_ram_bytes)?;
        if should_stop(&stop_flag) {
            let ck = RetrogradeCheckpointRef {
                next_idx,
                edges,
                pending_next: &pending_next,
                outcome: &outcome,
                changed: &changed,
                rounds,
            };
            save_ckpt(&ckpt_path, &ProofCheckpointRef::Retrograde(ck))?;
            bail!("proof mode interrupted by Ctrl+C (checkpoint saved)");
        }

        let mut changed_by_shard: Vec<Vec<u64>> = vec![Vec::new(); EDGE_SHARDS];
        for &ch in &changed {
            let shard = edge_shard_idx(ch);
            changed_by_shard[shard].push(ch);
        }
        for v in &mut changed_by_shard {
            v.sort_unstable();
        }

        let mut next_changed = Vec::new();
        for shard in 0..EDGE_SHARDS {
            let targets = &changed_by_shard[shard];
            if targets.is_empty() {
                continue;
            }
            let f = File::open(&edge_paths[shard])
                .with_context(|| format!("open {}", edge_paths[shard].display()))?;
            let mut r = BufReader::new(f);
            if let Some(&first_target) = targets.first() {
                let idx = &edge_sparse_idx[shard];
                if !idx.is_empty() {
                    let pos = match idx.binary_search_by_key(&first_target, |&(child, _)| child) {
                        Ok(i) => i,
                        Err(0) => 0,
                        Err(i) => i - 1,
                    };
                    r.seek(SeekFrom::Start(idx[pos].1))?;
                }
            }
            let mut rec = [0u8; EDGE_REC_BYTES];
            let mut target_i = 0usize;
            loop {
                match r.read_exact(&mut rec) {
                    Ok(()) => {
                        if target_i >= targets.len() {
                            break;
                        }
                        let (child, parent_u64) = read_edge_record(&rec);
                        while target_i < targets.len() && targets[target_i] < child {
                            target_i += 1;
                        }
                        if target_i >= targets.len() || targets[target_i] != child {
                            continue;
                        }
                        let parent = parent_u64 as usize;
                        if outcome[parent] != UNKNOWN {
                            continue;
                        }

                        let child_outcome = outcome[child as usize];
                        if child_outcome == LOSS {
                            outcome[parent] = WIN;
                            let p = parent as u64;
                            next_changed.push(p);
                        } else if child_outcome == WIN {
                            if pending_next[parent] > 0 {
                                pending_next[parent] -= 1;
                            }
                            if pending_next[parent] == 0 {
                                outcome[parent] = LOSS;
                                let p = parent as u64;
                                next_changed.push(p);
                            }
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(e.into()),
                }
            }
        }

        changed = next_changed;
        changed.sort_unstable();
        changed.dedup();

        if last_checkpoint.elapsed() >= checkpoint_interval {
            let ck = RetrogradeCheckpointRef {
                next_idx,
                edges,
                pending_next: &pending_next,
                outcome: &outcome,
                changed: &changed,
                rounds,
            };
            save_ckpt(&ckpt_path, &ProofCheckpointRef::Retrograde(ck))?;
            last_checkpoint = Instant::now();
            info!(
                "durable retrograde checkpoint: round={} frontier={}",
                rounds,
                changed.len()
            );
        }

        if (rounds % 5) == 0 {
            info!("retrograde round={rounds} frontier={}", changed.len());
        }
    }

    for o in &mut outcome {
        if *o == UNKNOWN {
            *o = DRAW;
        }
    }

    let solved = match outcome[0] {
        WIN => Outcome::Win,
        LOSS => Outcome::Loss,
        _ => Outcome::Draw,
    };

    let result_path = disk_dir.join("proof-result.txt");
    let result_tmp = disk_dir.join("proof-result.txt.tmp");
    {
        let mut result_file = BufWriter::new(
            File::create(&result_tmp)
                .with_context(|| format!("create {}", result_tmp.display()))?,
        );
        writeln!(result_file, "format_version=1")?;
        writeln!(result_file, "outcome={}", solved.as_str())?;
        writeln!(result_file, "states={n}")?;
        writeln!(result_file, "edges={edges}")?;
        writeln!(result_file, "push_directions={:?}", rules.push_directions)?;
        writeln!(result_file, "simultaneous_win={:?}", rules.simultaneous_win)?;
        result_file.flush()?;
        result_file.get_ref().sync_all()?;
    }
    fs::rename(&result_tmp, &result_path).with_context(|| {
        format!(
            "rename {} -> {}",
            result_tmp.display(),
            result_path.display()
        )
    })?;

    info!(
        "proof graph solved: states={} edges={} outcome={} time={:.3}s",
        n,
        edges,
        solved.as_str(),
        t0.elapsed().as_secs_f64()
    );
    info!("proof result written to {}", result_path.display());

    Ok(ProofResult {
        outcome: solved,
        states: n,
        edges,
    })
}

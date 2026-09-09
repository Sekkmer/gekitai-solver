//! Bounded reachability proofs and independently checked safety strategies.
//! A failed depth search alone is ONLY a depth bound.
//! Depth is part of the proposition, so cycles need no history-dependent cache.
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use clap::ValueEnum;
use parking_lot::{Mutex, RwLock};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tracing::info;

use crate::certificate::{Certificate, Header, Node, ALL_MOVES, MAX_NODES};
use crate::certificate_work::Work;
use crate::movegen::{apply_move, generate_moves, ApplyResult, MoveOutcome};
use crate::position::Position;
use crate::rules::{RepetitionRule, Rules, SimultaneousWin};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, ValueEnum)]
pub enum Objective {
    Clean,
    CleanOrDouble,
    DoubleOnly,
    Both,
    All,
}
impl Objective {
    fn label(self) -> &'static str {
        match self {
            Self::Clean => "clean",
            Self::CleanOrDouble => "clean-or-double",
            Self::DoubleOnly => "double-only",
            Self::Both => "both",
            Self::All => "all",
        }
    }
    pub(crate) fn terminal(self, applied: &ApplyResult, goal_turn: bool) -> Option<bool> {
        if applied.outcome == MoveOutcome::Ongoing {
            return None;
        }
        let clean = match applied.outcome {
            MoveOutcome::MoverWin => goal_turn,
            MoveOutcome::MoverLoss => !goal_turn,
            _ => false,
        };
        Some(match self {
            Self::Clean => clean,
            Self::CleanOrDouble => clean || applied.simultaneous_lines,
            Self::DoubleOnly => applied.simultaneous_lines,
            Self::All | Self::Both => unreachable!(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Side {
    First,
    Second,
    Both,
}

pub struct Config {
    pub objective: Objective,
    pub side: Side,
    pub table_mb: usize,
    pub threads: usize,
    pub max_depth: u16,
    pub dir: PathBuf,
    pub resume: bool,
    pub checkpoint_seconds: u64,
    pub checkpoint_mb: usize,
}

const SHARDS: usize = 256;
const TURN_BIT: u64 = 1 << 58;
const MAGIC: &[u8; 8] = b"BRFORC02";

// Exactly 16 bytes, fixed capacity. Eviction loses work, never correctness.
#[derive(Clone, Copy, Default)]
struct Entry {
    code: u64,
    good: u16,
    bad: u16,
    best: u8,
}
struct Table {
    shards: Vec<RwLock<Vec<Entry>>>,
    slots_per_shard: usize,
}
impl Table {
    fn new(bytes: usize) -> Self {
        let slots_per_shard = (bytes / SHARDS / std::mem::size_of::<Entry>()).max(1);
        Self {
            shards: (0..SHARDS)
                .map(|_| RwLock::new(vec![Entry::default(); slots_per_shard]))
                .collect(),
            slots_per_shard,
        }
    }
    fn slot(&self, code: u64) -> (usize, usize) {
        let mut h = code.wrapping_mul(0x9e3779b97f4a7c15);
        h ^= h >> 30;
        h = h.wrapping_mul(0xbf58476d1ce4e5b9);
        h ^= h >> 27;
        (
            (h as usize) & (SHARDS - 1),
            ((h >> 8) as usize) % self.slots_per_shard,
        )
    }
    fn get(&self, code: u64) -> Option<Entry> {
        let (s, i) = self.slot(code);
        let e = self.shards[s].read()[i];
        (e.code == code).then_some(e)
    }
    fn merge(&self, e: Entry) {
        let (s, i) = self.slot(e.code);
        let mut shard = self.shards[s].write();
        let old = &mut shard[i];
        if old.code == e.code {
            if (e.bad > 0 && e.bad >= old.bad)
                || (e.good != 0 && (old.good == 0 || e.good < old.good))
            {
                old.best = e.best;
            }
            old.bad = old.bad.max(e.bad);
            if e.good != 0 && (old.good == 0 || e.good < old.good) {
                old.good = e.good;
            }
            debug_assert!(old.good == 0 || old.bad < old.good);
        } else if old.code == 0 || e.good != 0 || e.bad >= old.bad {
            *old = e;
        }
    }
}

fn code(key: u64, goal_turn: bool) -> u64 {
    (key | if goal_turn { TURN_BIT } else { 0 }) + 1
}
fn decode(code: u64) -> (u64, bool) {
    ((code - 1) & (TURN_BIT - 1), (code - 1) & TURN_BIT != 0)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Progress {
    rules: Rules,
    objective: Objective,
    first: bool,
    completed_depth: u16,
    proved_depth: Option<u16>,
    avoided: bool,
}
struct Job {
    progress: Mutex<Progress>,
    table: Arc<Table>,
    // Only successful clean bounds and failed clean-or-double bounds imply
    // bounds for the other objective. Tables never refer back to jobs.
    implies: Option<(bool, Arc<Table>)>,
    stop: Arc<AtomicBool>,
    nodes: AtomicU64,
    name: String,
    deadline: Option<Instant>,
    blocked: Option<std::collections::HashSet<u64>>,
}
impl Job {
    fn merge(&self, e: Entry) {
        self.table.merge(e);
        if let Some((success, target)) = &self.implies {
            let transferred = Entry {
                good: if *success { e.good } else { 0 },
                bad: if *success { 0 } else { e.bad },
                ..e
            };
            if transferred.good != 0 || transferred.bad != 0 {
                target.merge(transferred);
            }
        }
    }
    fn store(&self, code: u64, depth: u16, value: bool, best: u8) {
        self.merge(Entry {
            code,
            good: if value { depth } else { 0 },
            bad: if value { 0 } else { depth },
            best,
        });
    }
    fn solve(
        &self,
        key: u64,
        goal_turn: bool,
        depth: u16,
        rules: &Rules,
        objective: Objective,
        parallel: bool,
    ) -> Result<bool> {
        if self.stop.load(Ordering::Relaxed)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            bail!("interrupted");
        }
        self.nodes.fetch_add(1, Ordering::Relaxed);
        if depth == 0 {
            return Ok(false);
        }
        let state = code(key, goal_turn);
        if self
            .blocked
            .as_ref()
            .is_some_and(|blocked| blocked.contains(&state))
        {
            return Ok(false); // Repeating an earlier game position is a draw.
        }
        let cached = self.table.get(state);
        if let Some(e) = cached {
            if e.good != 0 && e.good <= depth {
                return Ok(true);
            }
            if e.bad >= depth {
                return Ok(false);
            }
        }
        let pos = Position::from_key(key);
        let mut moves = generate_moves(&pos);
        if moves.is_empty() {
            self.store(state, depth, false, 0);
            return Ok(false);
        }
        if let Some(e) = cached {
            if let Some(i) = moves.iter().position(|m| m.0 == e.best) {
                moves.swap(0, i);
            }
        }
        // Scan terminals before spending time canonicalizing/searching any child.
        let mut ongoing = Vec::with_capacity(moves.len());
        for mv in moves {
            let applied = apply_move(&pos, mv, rules);
            match objective.terminal(&applied, goal_turn) {
                Some(value) if value == goal_turn => {
                    self.store(state, depth, value, mv.0);
                    return Ok(value);
                }
                Some(_) => {}
                None => ongoing.push((mv, applied.next)),
            }
        }
        // At the horizon every ongoing child is false. No child key is needed.
        if depth == 1 || ongoing.is_empty() {
            let value = !goal_turn && ongoing.is_empty();
            let best = ongoing.first().map_or(0, |(mv, _)| mv.0);
            self.store(state, depth, value, best);
            return Ok(value);
        }
        let value = if parallel {
            let mut children = Vec::with_capacity(ongoing.len());
            for (mv, next) in ongoing {
                let child = next.canonical_key();
                if !children.iter().any(|&(_, k)| k == child) {
                    children.push((mv, child));
                }
            }
            // Six distinct openings after D4 reduction; jobs for each objective
            // and side also run concurrently. try_reduce stops on cancellation.
            children
                .par_iter()
                .map(|&(mv, child)| {
                    self.solve(child, !goal_turn, depth - 1, rules, objective, false)
                        .map(|v| (v, mv.0))
                })
                .try_reduce(
                    || (!goal_turn, 0),
                    |a, b| Ok(if b.0 == goal_turn { b } else { a }),
                )?
        } else {
            let mut answer = (!goal_turn, 0);
            let mut seen = Vec::with_capacity(ongoing.len());
            for (mv, next) in ongoing {
                let child = next.canonical_key();
                if seen.contains(&child) {
                    continue;
                }
                seen.push(child);
                let v = self.solve(child, !goal_turn, depth - 1, rules, objective, false)?;
                if v == goal_turn {
                    answer = (v, mv.0);
                    break;
                }
            }
            answer
        };
        self.store(state, depth, value.0, value.1);
        Ok(value.0)
    }
    fn search(&self, max_depth: u16, dir: &Path) -> Result<()> {
        let p = self.progress.lock().clone();
        if p.proved_depth.is_some() || p.avoided {
            return Ok(());
        }
        let work_path = dir.join(format!("{}.certificate-work", self.name));
        let mut work = if work_path.exists() {
            let work = Work::load(&work_path)?;
            ensure!(
                work.certificate.header
                    == self.certificate_header(&p, work.certificate.header.forced_depth),
                "certificate work rules/objective/player mismatch"
            );
            ensure!(
                work.certificate.header.forced_depth.unwrap() > p.completed_depth,
                "certificate candidate contradicts a completed negative bound"
            );
            info!(
                "forcing {}: resumed UNVERIFIED certificate work ({} explicit states, {} pending)",
                self.name,
                work.certificate.nodes.len(),
                work.pending.len()
            );
            Some(work)
        } else {
            None
        };
        let start_depth = work
            .as_ref()
            .map_or(p.completed_depth.saturating_add(1), |w| {
                w.certificate.header.forced_depth.unwrap()
            });
        for depth in start_depth..=max_depth {
            let started = Instant::now();
            let proved =
                work.is_some() || self.solve(0, p.first, depth, &p.rules, p.objective, true)?;
            let mut avoided = false;
            if proved {
                info!("forcing {}: candidate success within {} plies; constructing certificate (limit {} nodes), not yet independently verified", self.name, depth, MAX_NODES);
                let certificate = self
                    .positive_certificate(&p, depth, dir, work.take(), MAX_NODES)
                    .with_context(|| {
                        format!(
                            "forcing {}: certificate construction at depth {depth}",
                            self.name
                        )
                    })?;
                let Some(certificate) = certificate else {
                    info!("forcing {}: certificate budget reached; work saved, verification pending; other jobs continue", self.name);
                    return Ok(());
                };
                info!(
                    "forcing {}: independently checking {} explicit certificate states",
                    self.name,
                    certificate.nodes.len()
                );
                certificate.save_with_stop(
                    &dir.join(format!("{}.certificate", self.name)),
                    Some(&self.stop),
                )?;
                fs::remove_file(&work_path)?;
                info!(
                    "forcing {}: forcing strategy independently verified ({} nodes)",
                    self.name,
                    certificate.nodes.len()
                );
            } else if depth >= 8 && depth % 2 == 0 {
                if let Some(certificate) = self.safety_certificate(&p, 100_000)? {
                    certificate.save(&dir.join(format!("{}.certificate", self.name)))?;
                    avoided = true;
                    info!("forcing {}: CANNOT FORCE, closed defensive strategy independently verified ({} nodes)", self.name, certificate.nodes.len());
                }
            }
            {
                let mut progress = self.progress.lock();
                progress.completed_depth = depth;
                progress.avoided = avoided;
                if proved {
                    progress.proved_depth = Some(depth);
                }
            }
            info!(
                "forcing {}: depth={} status={} nodes={} seconds={:.3}",
                self.name,
                depth,
                if proved {
                    "FORCED"
                } else {
                    "not-forceable-within-depth (unbounded unresolved)"
                },
                self.nodes.load(Ordering::Relaxed),
                started.elapsed().as_secs_f64()
            );
            if proved || avoided {
                break;
            }
        }
        Ok(())
    }
    fn certificate_header(&self, p: &Progress, depth: Option<u16>) -> Header {
        Header {
            rules: p.rules,
            objective: p.objective,
            first: p.first,
            start_key: 0,
            forced_depth: depth,
        }
    }

    fn safety_certificate(&self, p: &Progress, limit: usize) -> Result<Option<Certificate>> {
        let mut certificate = Certificate {
            header: self.certificate_header(p, None),
            nodes: std::collections::HashMap::new(),
        };
        let mut todo = vec![code(0, p.first)];
        while let Some(state) = todo.pop() {
            if self.stop.load(Ordering::Relaxed) {
                bail!("interrupted");
            }
            if certificate.nodes.contains_key(&state) {
                continue;
            }
            if certificate.nodes.len() >= limit {
                return Ok(None);
            }
            let (key, goal_turn) = decode(state);
            let cached = self.table.get(state);
            if cached.is_some_and(|e| e.good != 0) {
                return Ok(None);
            }
            let pos = Position::from_key(key);
            let moves = generate_moves(&pos);
            let chosen = if goal_turn || moves.is_empty() {
                ALL_MOVES
            } else {
                // A terminal that misses the target is always an avoidance win.
                let terminal = moves.iter().find(|&&mv| {
                    p.objective
                        .terminal(&apply_move(&pos, mv, &p.rules), goal_turn)
                        == Some(false)
                });
                if let Some(mv) = terminal {
                    mv.0
                } else if let Some(e) = cached.filter(|e| e.bad > 0) {
                    if !moves.iter().any(|m| m.0 == e.best) {
                        return Ok(None);
                    }
                    e.best
                } else {
                    return Ok(None);
                }
            };
            certificate.nodes.insert(state, Node { rank: 0, chosen });
            for mv in moves
                .into_iter()
                .filter(|m| chosen == ALL_MOVES || m.0 == chosen)
            {
                let applied = apply_move(&pos, mv, &p.rules);
                match p.objective.terminal(&applied, goal_turn) {
                    Some(true) => return Ok(None),
                    Some(false) => {}
                    None => todo.push(code(applied.next.canonical_key(), !goal_turn)),
                }
            }
        }
        certificate.verify()?;
        Ok(Some(certificate))
    }

    fn positive_certificate(
        &self,
        p: &Progress,
        depth: u16,
        dir: &Path,
        restored: Option<Work>,
        limit: usize,
    ) -> Result<Option<Certificate>> {
        let mut work =
            restored.unwrap_or_else(|| Work::new(self.certificate_header(p, Some(depth))));
        let root = code(work.certificate.header.start_key, p.first);
        let path = dir.join(format!("{}.certificate-work", self.name));
        // Preserve the candidate even if the process stops before its first
        // periodic snapshot. A complete snapshot is also saved before checking.
        work.save(&path)?;
        let mut checkpointed = Instant::now();
        while let Some(&(state, offered_rank)) = work.pending.last() {
            if self.stop.load(Ordering::Relaxed) {
                work.save(&path)?;
                bail!("interrupted; certificate work saved");
            }
            if work.visited & 4095 == 0 && checkpointed.elapsed() >= Duration::from_secs(60) {
                work.save(&path)?;
                info!(
                    "forcing {}: certificate work checkpoint saved ({} explicit states)",
                    self.name,
                    work.certificate.nodes.len()
                );
                checkpointed = Instant::now();
            }
            work.pending.pop();
            work.visited += 1;
            if work.visited.is_multiple_of(1_000_000) {
                info!("forcing {}: certificate construction depth={} visited={} explicit={} implicit={} pending={}",
                    self.name, depth, work.visited, work.certificate.nodes.len(), work.implicit, work.pending.len());
            }
            ensure!(offered_rank > 0, "forcing strategy exhausted its rank");
            let cached = self.table.get(state);
            let mut rank = cached
                .filter(|e| e.good != 0)
                .map_or(offered_rank, |e| offered_rank.min(e.good));
            if work
                .certificate
                .nodes
                .get(&state)
                .is_some_and(|n| n.rank <= rank)
            {
                continue;
            }
            // These leaves are checked independently, by generating their legal
            // moves, when each explicit parent is verified. No leaf claim or
            // cached best move is trusted by the checker.
            if state != root && cached.is_some_and(|e| e.good == 1) {
                work.implicit += 1;
                work.certificate.nodes.remove(&state);
                continue;
            }
            let (key, goal_turn) = decode(state);
            let pos = Position::from_key(key);
            let mut moves = generate_moves(&pos);
            if let Some(e) = cached {
                if let Some(i) = moves.iter().position(|m| m.0 == e.best) {
                    moves.swap(0, i);
                }
            }
            let applied: Vec<_> = moves
                .into_iter()
                .map(|mv| (mv, apply_move(&pos, mv, &p.rules)))
                .collect();
            ensure!(!applied.is_empty(), "no legal move in a forcing strategy");
            let terminal_choice = if goal_turn {
                applied
                    .iter()
                    .find(|(_, a)| p.objective.terminal(a, goal_turn) == Some(true))
                    .map(|(mv, _)| mv.0)
            } else {
                applied
                    .iter()
                    .all(|(_, a)| p.objective.terminal(a, goal_turn) == Some(true))
                    .then_some(ALL_MOVES)
            };
            let chosen = if let Some(chosen) = terminal_choice {
                rank = 1;
                self.store(state, 1, true, if chosen == ALL_MOVES { 0 } else { chosen });
                if state != root {
                    work.implicit += 1;
                    work.certificate.nodes.remove(&state);
                    continue;
                }
                chosen
            } else if goal_turn {
                let mut selected = None;
                for (mv, a) in &applied {
                    let success = match p.objective.terminal(a, goal_turn) {
                        Some(v) => v,
                        None => match self.solve(
                            a.next.canonical_key(),
                            !goal_turn,
                            rank - 1,
                            &p.rules,
                            p.objective,
                            false,
                        ) {
                            Ok(v) => v,
                            Err(e) => {
                                work.pending.push((state, offered_rank));
                                work.save(&path)?;
                                return Err(e);
                            }
                        },
                    };
                    if success {
                        selected = Some(mv.0);
                        break;
                    }
                }
                selected.context("forcing strategy lacks a successful move")?
            } else {
                ALL_MOVES
            };
            if work.certificate.nodes.len() >= limit && !work.certificate.nodes.contains_key(&state)
            {
                work.pending.push((state, offered_rank));
                work.save(&path)?;
                return Ok(None);
            }
            work.certificate.nodes.insert(state, Node { rank, chosen });
            let mut successors = Vec::new();
            for (_, a) in applied
                .into_iter()
                .filter(|(mv, _)| chosen == ALL_MOVES || mv.0 == chosen)
            {
                match p.objective.terminal(&a, goal_turn) {
                    Some(v) => ensure!(v, "forcing strategy reaches an unsuccessful terminal"),
                    None => {
                        ensure!(rank > 1, "forcing strategy has an ongoing move at rank one");
                        let child = code(a.next.canonical_key(), !goal_turn);
                        if !successors.contains(&child) {
                            successors.push(child);
                            work.pending.push((child, rank - 1));
                        }
                    }
                }
            }
        }
        work.save(&path)?;
        Ok(Some(work.certificate))
    }

    fn save(&self, dir: &Path, max_bytes: usize) -> Result<()> {
        let path = dir.join(format!("{}.ckpt", self.name));
        let tmp = path.with_extension("ckpt.tmp");
        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(MAGIC)?;
        bincode::serialize_into(&mut w, &*self.progress.lock())?;
        let mut count = 0usize;
        // Sparse, capped restart cache. Omitted entries are recomputed on resume.
        'shards: for shard in &self.table.shards {
            let shard = shard.read();
            for e in shard
                .iter()
                .filter(|e| e.code != 0 && (e.good != 0 || e.bad >= 2))
            {
                if (count + 1) * 13 + 1024 > max_bytes {
                    break 'shards;
                }
                w.write_all(&e.code.to_le_bytes())?;
                w.write_all(&e.good.to_le_bytes())?;
                w.write_all(&e.bad.to_le_bytes())?;
                w.write_all(&[e.best])?;
                count += 1;
            }
        }
        w.write_all(&0u64.to_le_bytes())?;
        w.flush()?;
        w.get_ref().sync_all()?;
        fs::rename(&tmp, &path)?;
        File::open(dir)?.sync_all()?;
        info!(
            "forcing {}: checkpoint saved ({} cache entries)",
            self.name, count
        );
        Ok(())
    }
    fn load(&self, dir: &Path) -> Result<()> {
        let path = dir.join(format!("{}.ckpt", self.name));
        let mut r =
            BufReader::new(File::open(&path).with_context(|| format!("open {}", path.display()))?);
        let mut magic = [0; 8];
        r.read_exact(&mut magic)?;
        ensure!(
            &magic == MAGIC || &magic == b"BRFORC01",
            "unsupported forcing checkpoint"
        );
        let p: Progress = if &magic == MAGIC {
            bincode::deserialize_from(&mut r)?
        } else {
            #[derive(Deserialize)]
            struct OldProgress {
                rules: Rules,
                objective: Objective,
                first: bool,
                completed_depth: u16,
                proved_depth: Option<u16>,
            }
            let old: OldProgress = bincode::deserialize_from(&mut r)?;
            Progress {
                rules: old.rules,
                objective: old.objective,
                first: old.first,
                completed_depth: old
                    .proved_depth
                    .map_or(old.completed_depth, |d| d.saturating_sub(1)),
                proved_depth: None,
                avoided: false,
            }
        };
        let expected = self.progress.lock().clone();
        ensure!(
            p.rules == expected.rules
                && p.objective == expected.objective
                && p.first == expected.first,
            "forcing checkpoint rules/objective/side mismatch"
        );
        loop {
            let mut kb = [0; 8];
            r.read_exact(&mut kb)?;
            let code = u64::from_le_bytes(kb);
            if code == 0 {
                break;
            }
            let mut gb = [0; 2];
            r.read_exact(&mut gb)?;
            let mut bb = [0; 2];
            r.read_exact(&mut bb)?;
            let mut mv = [0; 1];
            r.read_exact(&mut mv)?;
            let e = Entry {
                code,
                good: u16::from_le_bytes(gb),
                bad: u16::from_le_bytes(bb),
                best: mv[0],
            };
            ensure!(
                code <= (TURN_BIT | (3u64.pow(36) - 1)) + 1
                    && decode(code).0 < 3u64.pow(36)
                    && e.best < 36
                    && (e.good == 0 || e.good > e.bad),
                "invalid forcing cache entry"
            );
            self.merge(e);
        }
        let mut trailing = [0];
        ensure!(r.read(&mut trailing)? == 0, "trailing checkpoint data");
        info!(
            "forcing {}: resumed at completed depth {}",
            self.name, p.completed_depth
        );
        if p.avoided || p.proved_depth.is_some() {
            let certificate = Certificate::load(&dir.join(format!("{}.certificate", self.name)))?;
            ensure!(
                certificate.header.rules == p.rules
                    && certificate.header.objective == p.objective
                    && certificate.header.first == p.first
                    && certificate.header.start_key == 0
                    && certificate.header.forced_depth == p.proved_depth,
                "checkpoint/certificate mismatch"
            );
        }
        *self.progress.lock() = p;
        Ok(())
    }
}

pub fn run(rules: Rules, config: Config, stop: Arc<AtomicBool>) -> Result<()> {
    ensure!(
        rules.repetition == RepetitionRule::PathRepeatIsDraw,
        "forcing mode requires cycles allowed; forbidden repetition needs a history state"
    );
    ensure!(rules.simultaneous_win == SimultaneousWin::Draw, "forcing mode requires --simultaneous-win draw; each objective handles simultaneous lines separately");
    ensure!(
        config.max_depth > 0 && config.max_depth <= 512,
        "forcing depth must be 1..512"
    );
    ensure!(
        config.checkpoint_mb > 0,
        "checkpoint budget must be positive"
    );
    fs::create_dir_all(&config.dir)?;
    let run_lock = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(config.dir.join("forcing.lock"))?;
    run_lock
        .try_lock()
        .context("another forcing process is using this directory")?;
    let objectives = if config.objective == Objective::All {
        vec![
            Objective::Clean,
            Objective::CleanOrDouble,
            Objective::DoubleOnly,
        ]
    } else if config.objective == Objective::Both {
        vec![Objective::Clean, Objective::CleanOrDouble]
    } else {
        vec![config.objective]
    };
    let sides = match config.side {
        Side::First => vec![true],
        Side::Second => vec![false],
        Side::Both => vec![true, false],
    };
    let count = objectives.len() * sides.len();
    ensure!(
        config.table_mb >= count,
        "table budget must allow at least 1 MiB per job"
    );
    let per_objective = config
        .table_mb
        .checked_mul(1024 * 1024)
        .context("table budget overflow")?
        / objectives.len();
    let checkpoint_bytes = config
        .checkpoint_mb
        .checked_mul(1024 * 1024)
        .context("checkpoint budget overflow")?;
    let mut jobs = Vec::new();
    let tables: Vec<_> = objectives
        .iter()
        .map(|_| Arc::new(Table::new(per_objective)))
        .collect();
    for (index, &objective) in objectives.iter().enumerate() {
        let implication = match objective {
            Objective::Clean => Some((true, Objective::CleanOrDouble)),
            Objective::CleanOrDouble => Some((false, Objective::Clean)),
            _ => None,
        };
        let implies = implication.and_then(|(success, target)| {
            objectives
                .iter()
                .position(|&o| o == target)
                .map(|i| (success, tables[i].clone()))
        });
        for &first in &sides {
            let name = format!(
                "{}-{}",
                objective.label(),
                if first { "first" } else { "second" }
            );
            ensure!(
                config.resume
                    || (!config.dir.join(format!("{name}.ckpt")).exists()
                        && !config.dir.join(format!("{name}.certificate-work")).exists()),
                "checkpoint already exists for {name}; use --forcing-resume or a new directory"
            );
            let job = Job {
                progress: Mutex::new(Progress {
                    rules,
                    objective,
                    first,
                    completed_depth: 0,
                    proved_depth: None,
                    avoided: false,
                }),
                table: tables[index].clone(),
                implies: implies.clone(),
                stop: stop.clone(),
                nodes: AtomicU64::new(0),
                name,
                deadline: None,
                blocked: None,
            };
            if config.resume {
                job.load(&config.dir)?;
            }
            jobs.push(job);
        }
    }
    info!("forcing: {} objectives/sides sharing {} objective tables, total table={} MiB, checkpoint cap={} MiB per job; cycles allowed", count, tables.len(), config.table_mb, config.checkpoint_mb);
    let interval = Duration::from_secs(config.checkpoint_seconds.max(1));
    // A Rayon join can execute another injected root while waiting for its own
    // children. Across long iterative searches that couples unrelated jobs and
    // can leave completed roots waiting for a different objective's deeper
    // search. Separate pools keep the four questions making progress fairly.
    let threads_per_job = (config.threads / jobs.len()).max(1);
    let pools: Vec<_> = (0..jobs.len())
        .map(|_| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads_per_job)
                .build()
        })
        .collect::<std::result::Result<_, _>>()?;
    let (tx, rx) = std::sync::mpsc::channel();
    let mut error = None;
    std::thread::scope(|scope| {
        for (job, pool) in jobs.iter().zip(&pools) {
            let tx = tx.clone();
            let dir = &config.dir;
            scope.spawn(move || {
                let _ = tx.send(pool.install(|| job.search(config.max_depth, dir)));
            });
        }
        drop(tx);
        let mut last_checkpoint = Instant::now();
        let mut last_status = Instant::now();
        let mut finished = 0;
        while finished < jobs.len() {
            match rx.recv_timeout(Duration::from_secs(1)) {
                Ok(result) => {
                    finished += 1;
                    if let Err(e) = result {
                        if !stop.load(Ordering::Relaxed) {
                            error = Some(e);
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                    // A clean win settles every requested clean/union question
                    // on both sides. Stop duplicate work, then independently
                    // check the corresponding certificates after workers exit.
                    let clean_winner = jobs.iter().find_map(|job| {
                        let p = job.progress.lock();
                        (p.objective == Objective::Clean && p.proved_depth.is_some())
                            .then_some(p.first)
                    });
                    if let Some(first) = clean_winner {
                        let covers_all = jobs.iter().all(|job| {
                            let p = job.progress.lock();
                            p.first != first || p.objective != Objective::DoubleOnly
                        });
                        if covers_all {
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => break,
            }
            if last_status.elapsed() >= Duration::from_secs(30) {
                for job in &jobs {
                    let p = job.progress.lock();
                    info!(
                        "forcing status {}: completed_depth={} proved_depth={:?} nodes={}",
                        job.name,
                        p.completed_depth,
                        p.proved_depth,
                        job.nodes.load(Ordering::Relaxed)
                    );
                }
                last_status = Instant::now();
            }
            if last_checkpoint.elapsed() >= interval && !stop.load(Ordering::Relaxed) {
                for job in &jobs {
                    if let Err(e) = job.save(&config.dir, checkpoint_bytes) {
                        error = Some(e);
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                }
                last_checkpoint = Instant::now();
            }
        }
    });
    derive_clean_answers(&jobs, &config.dir)?;
    if jobs.iter().all(|job| {
        let p = job.progress.lock();
        p.proved_depth.is_some() || p.avoided
    }) {
        error = None;
    }
    for job in &jobs {
        job.save(&config.dir, checkpoint_bytes)?;
    }
    let mut report = String::from("Finite-horizon reachability proof results. Infinite play does not reach the target.\nA depth bound is NOT an unbounded impossibility proof.\n");
    report.push_str(&format!("rules={rules:?}\n"));
    for job in &jobs {
        let p = job.progress.lock();
        let mut status = match p.proved_depth {
            Some(d) => format!("FORCED within {d} plies"),
            None if p.avoided => "CANNOT FORCE; verified closed defensive strategy".into(),
            None => format!(
                "UNRESOLVED; cannot force within {} plies",
                p.completed_depth
            ),
        };
        if p.proved_depth.is_none()
            && !p.avoided
            && config
                .dir
                .join(format!("{}.certificate-work", job.name))
                .exists()
        {
            status.push_str("; unverified candidate certificate work saved");
        }
        report.push_str(&format!("{}: {}\n", job.name, status));
    }
    let tmp = config.dir.join("forcing-result.txt.tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(report.as_bytes())?;
    f.sync_all()?;
    fs::rename(tmp, config.dir.join("forcing-result.txt"))?;
    File::open(&config.dir)?.sync_all()?;
    info!("{report}");
    if let Some(e) = error {
        return Err(e);
    }
    Ok(())
}

fn derive_clean_answers(jobs: &[Job], dir: &Path) -> Result<()> {
    let Some(source) = jobs.iter().find(|job| {
        let p = job.progress.lock();
        p.objective == Objective::Clean && p.proved_depth.is_some()
    }) else {
        return Ok(());
    };
    let certificate = Certificate::load(&dir.join(format!("{}.certificate", source.name)))?;
    let source_progress = source.progress.lock().clone();
    ensure!(
        certificate.header
            == source.certificate_header(&source_progress, source_progress.proved_depth),
        "source certificate rules/objective/player/start mismatch"
    );
    for job in jobs {
        let p = job.progress.lock().clone();
        let same = p.first == certificate.header.first;
        if same && p.objective == Objective::DoubleOnly {
            continue;
        }
        ensure!(
            !(same && p.avoided) && (same || p.proved_depth.is_none()),
            "verified clean strategy contradicts another completed certificate"
        );
        if p.proved_depth.is_some() || p.avoided {
            continue;
        }
        let derived = certificate.derive_from_clean(p.objective, p.first)?;
        if let Some(depth) = derived.header.forced_depth {
            ensure!(
                depth > p.completed_depth,
                "verified strategy contradicts a completed negative bound"
            );
        }
        derived.save(&dir.join(format!("{}.certificate", job.name)))?;
        {
            let mut progress = job.progress.lock();
            progress.proved_depth = derived.header.forced_depth;
            progress.avoided = derived.header.forced_depth.is_none();
            if let Some(depth) = progress.proved_depth {
                progress.completed_depth = depth;
            }
        }
        let work_path = dir.join(format!("{}.certificate-work", job.name));
        if work_path.exists() {
            fs::remove_file(work_path)?;
        }
        info!(
            "forcing {}: {} independently verified from {} ({} explicit strategy states)",
            job.name,
            if same { "FORCED" } else { "CANNOT FORCE" },
            source.name,
            derived.nodes.len()
        );
    }
    Ok(())
}

/// Find and independently check a small clean-win strategy from an arbitrary
/// side-to-move position. Failure/time exhaustion means unknown, never a loss.
/// `blocked` uses goal-turn codes, and excludes the current root itself.
pub(crate) fn tactical_certificate(
    pos: Position,
    rules: Rules,
    deadline: Instant,
    stop: Arc<AtomicBool>,
    blocked: std::collections::HashSet<u64>,
) -> Option<Certificate> {
    if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
        return None;
    }
    let progress = Progress {
        rules,
        objective: Objective::Clean,
        first: true,
        completed_depth: 0,
        proved_depth: None,
        avoided: false,
    };
    let job = Job {
        progress: Mutex::new(progress),
        table: Arc::new(Table::new(16 * 1024 * 1024)),
        implies: None,
        stop,
        nodes: AtomicU64::new(0),
        name: "browser".into(),
        deadline: Some(deadline),
        blocked: Some(blocked),
    };
    let key = pos.canonical_key();
    for depth in 1..=31 {
        if job
            .solve(key, true, depth, &rules, Objective::Clean, false)
            .ok()?
        {
            return tactical_witness(&job, key, depth, rules).ok();
        }
    }
    None
}

fn tactical_witness(job: &Job, start_key: u64, depth: u16, rules: Rules) -> Result<Certificate> {
    let mut certificate = Certificate {
        header: Header {
            rules,
            objective: Objective::Clean,
            first: true,
            start_key,
            forced_depth: Some(depth),
        },
        nodes: std::collections::HashMap::new(),
    };
    let root = code(start_key, true);
    let mut pending = vec![(root, depth)];
    while let Some((state, offered)) = pending.pop() {
        ensure!(
            !job.stop.load(Ordering::Relaxed)
                && job
                    .deadline
                    .is_none_or(|deadline| Instant::now() < deadline),
            "analysis budget exhausted"
        );
        ensure!(
            !job.blocked
                .as_ref()
                .is_some_and(|blocked| blocked.contains(&state)),
            "strategy repeats an earlier game position"
        );
        let cached = job.table.get(state);
        let rank = cached
            .filter(|e| e.good > 0)
            .map_or(offered, |e| offered.min(e.good));
        ensure!(rank > 0, "strategy exhausted its rank");
        if certificate
            .nodes
            .get(&state)
            .is_some_and(|n| n.rank <= rank)
        {
            continue;
        }
        let (key, goal_turn) = decode(state);
        let pos = Position::from_key(key);
        if state != root
            && crate::certificate::immediate_proof(&pos, goal_turn, &certificate.header)
        {
            continue;
        }
        let mut moves = generate_moves(&pos);
        if let Some(cached) = cached {
            if let Some(index) = moves.iter().position(|mv| mv.0 == cached.best) {
                moves.swap(0, index);
            }
        }
        let chosen = if goal_turn {
            let mut chosen = None;
            for mv in &moves {
                let applied = apply_move(&pos, *mv, &rules);
                let wins = match Objective::Clean.terminal(&applied, goal_turn) {
                    Some(wins) => wins,
                    None => job.solve(
                        applied.next.canonical_key(),
                        false,
                        rank - 1,
                        &rules,
                        Objective::Clean,
                        false,
                    )?,
                };
                if wins {
                    chosen = Some(mv.0);
                    break;
                }
            }
            chosen.context("strategy has no winning move")?
        } else {
            ALL_MOVES
        };
        // Keep interactive certificate checking and memory use bounded.
        ensure!(
            certificate.nodes.len() < 10_000,
            "interactive strategy is too large"
        );
        certificate.nodes.insert(state, Node { rank, chosen });
        for mv in moves
            .into_iter()
            .filter(|mv| chosen == ALL_MOVES || mv.0 == chosen)
        {
            let applied = apply_move(&pos, mv, &rules);
            match Objective::Clean.terminal(&applied, goal_turn) {
                Some(wins) => ensure!(wins, "strategy reaches the wrong result"),
                None => {
                    ensure!(rank > 1, "strategy exceeds its bound");
                    pending.push((code(applied.next.canonical_key(), !goal_turn), rank - 1));
                }
            }
        }
    }
    certificate.verify()?;
    // Independent of cache/search: check that every ongoing strategy edge also
    // avoids the game history which precedes this new certificate's root.
    for (&state, node) in &certificate.nodes {
        let (key, turn) = decode(state);
        let pos = Position::from_key(key);
        for mv in generate_moves(&pos)
            .into_iter()
            .filter(|mv| node.chosen == ALL_MOVES || mv.0 == node.chosen)
        {
            let applied = apply_move(&pos, mv, &rules);
            if applied.outcome == MoveOutcome::Ongoing {
                ensure!(
                    !job.blocked.as_ref().is_some_and(
                        |blocked| blocked.contains(&code(applied.next.canonical_key(), !turn))
                    ),
                    "strategy repeats game history"
                );
            }
        }
    }
    Ok(certificate)
}

#[cfg(test)]
mod tests;

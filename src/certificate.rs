//! Strategy checker independent of the search/cache. It regenerates legal moves.
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

use crate::board::has_three_in_row;
use crate::forcing::Objective;
use crate::movegen::{apply_move, generate_moves};
use crate::position::Position;
use crate::rules::{RepetitionRule, Rules, SimultaneousWin};

pub const ALL_MOVES: u8 = 255;
// A large forcing strategy can exceed a million states even when the search
// fits comfortably. At this cap the encoded records occupy 1.1 GB; the map
// takes several GiB (including temporary growth), within the server's headroom.
pub const MAX_NODES: usize = 100_000_000;
const MAGIC: &[u8; 8] = b"BRCERT02";
const LEGACY_MAGIC: &[u8; 8] = b"BRCERT01";
const TURN_BIT: u64 = 1 << 58;

pub fn code(key: u64, goal_turn: bool) -> u64 {
    (key | if goal_turn { TURN_BIT } else { 0 }) + 1
}
pub fn decode(code: u64) -> (u64, bool) {
    ((code - 1) & (TURN_BIT - 1), (code - 1) & TURN_BIT != 0)
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Header {
    pub rules: Rules,
    pub objective: Objective,
    pub first: bool,
    pub start_key: u64,
    /// Some(d): ranked forcing strategy; None: closed avoidance strategy.
    pub forced_depth: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Node {
    pub rank: u16,
    pub chosen: u8,
}

pub struct Certificate {
    pub header: Header,
    /// One strategy choice and decreasing rank per full state code.
    pub nodes: HashMap<u64, Node>,
}

/// A missing successor can be discharged by regenerating every required move
/// and proving the objective (or avoidance) terminates on the very next ply.
/// This does not consult the search, its cache, or any claimed strategy move.
pub fn immediate_proof(pos: &Position, goal_turn: bool, h: &Header) -> bool {
    let positive = h.forced_depth.is_some();
    let moves = generate_moves(pos);
    if moves.is_empty() {
        return !positive;
    }
    let mut outcomes = moves.into_iter().map(|mv| {
        h.objective
            .terminal(&apply_move(pos, mv, &h.rules), goal_turn)
            == Some(positive)
    });
    if goal_turn == positive {
        outcomes.any(|v| v)
    } else {
        outcomes.all(|v| v)
    }
}

impl Certificate {
    pub fn verify(&self) -> Result<()> {
        self.verify_with_stop(None)
    }
    fn verify_with_stop(&self, stop: Option<&AtomicBool>) -> Result<()> {
        let h = &self.header;
        ensure!(
            h.rules.board_n == 6 && h.rules.line_len == 3 && h.rules.pieces_per_player == 8,
            "unsupported certificate board rules"
        );
        ensure!(
            h.rules.repetition == RepetitionRule::PathRepeatIsDraw
                && h.rules.simultaneous_win == SimultaneousWin::Draw
                && !matches!(h.objective, Objective::All | Objective::Both),
            "unsupported certificate semantics"
        );
        ensure!(h.start_key < 3u64.pow(36), "invalid certificate start key");
        let positive = h.forced_depth.is_some();
        if let Some(d) = h.forced_depth {
            ensure!(d > 0 && d <= 512, "invalid forcing rank");
        }
        let root = code(h.start_key, h.first);
        ensure!(self.nodes.contains_key(&root), "certificate omits the root");
        ensure!(self.nodes.len() <= MAX_NODES, "certificate too large");
        for (&state, &Node { rank, chosen }) in &self.nodes {
            ensure!(
                !stop.is_some_and(|s| s.load(Ordering::Relaxed)),
                "interrupted certificate verification"
            );
            ensure!(
                state > 0 && state <= (TURN_BIT | (3u64.pow(36) - 1)) + 1,
                "invalid state code"
            );
            let (key, goal_turn) = decode(state);
            ensure!(key < 3u64.pow(36), "invalid board key");
            let pos = Position::from_key(key);
            ensure!(pos.is_legal(&h.rules), "illegal certificate board");
            ensure!(
                pos.us_count() < 8
                    && pos.them_count() < 8
                    && !has_three_in_row(pos.us)
                    && !has_three_in_row(pos.them),
                "certificate contains an already terminal board"
            );
            ensure!(
                pos.canonical_key() == key,
                "certificate key is not canonical"
            );
            ensure!(
                if positive {
                    rank > 0 && rank <= h.forced_depth.unwrap()
                } else {
                    rank == 0
                },
                "invalid node rank"
            );
            let moves = generate_moves(&pos);
            if moves.is_empty() {
                ensure!(
                    !positive && chosen == ALL_MOVES,
                    "no move cannot force a target"
                );
                continue;
            }
            // To reach: choose on goal turns, cover all defender replies.
            // To avoid: cover all goal moves, choose on defender turns.
            let choose_one = goal_turn == positive;
            ensure!(
                if choose_one {
                    moves.iter().any(|m| m.0 == chosen)
                } else {
                    chosen == ALL_MOVES
                },
                "strategy omits a required reply or selects an illegal move"
            );
            for mv in moves.into_iter().filter(|m| !choose_one || m.0 == chosen) {
                let a = apply_move(&pos, mv, &h.rules);
                if let Some(reached) = h.objective.terminal(&a, goal_turn) {
                    ensure!(reached == positive, "strategy reaches the wrong terminal");
                } else {
                    let child = code(a.next.canonical_key(), !goal_turn);
                    if let Some(next) = self.nodes.get(&child) {
                        ensure!(
                            !positive || next.rank < rank,
                            "strategy rank does not decrease"
                        );
                    } else {
                        ensure!(
                            (!positive || rank >= 2) && immediate_proof(&a.next, !goal_turn, h),
                            "strategy has an unverified successor"
                        );
                    }
                }
            }
        }
        // In a positive certificate rank strictly decreases to successful
        // terminals. In a negative one closure excludes targets even on cycles.
        Ok(())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        self.save_with_stop(path, None)
    }
    pub fn save_with_stop(&self, path: &Path, stop: Option<&AtomicBool>) -> Result<()> {
        self.verify_with_stop(stop)?;
        let tmp = path.with_extension("certificate.tmp");
        let mut w = BufWriter::new(File::create(&tmp)?);
        w.write_all(MAGIC)?;
        bincode::serialize_into(&mut w, &self.header)?;
        bincode::serialize_into(&mut w, &(self.nodes.len() as u64))?;
        for (&state, &Node { rank, chosen }) in &self.nodes {
            ensure!(
                !stop.is_some_and(|s| s.load(Ordering::Relaxed)),
                "interrupted certificate writing"
            );
            bincode::serialize_into(&mut w, &(state, rank, chosen))?;
        }
        w.flush()?;
        w.get_ref().sync_all()?;
        fs::rename(tmp, path)?;
        File::open(
            path.parent()
                .context("certificate needs parent directory")?,
        )?
        .sync_all()?;
        Ok(())
    }
    pub fn load(path: &Path) -> Result<Self> {
        let mut r = BufReader::new(File::open(path)?);
        let mut magic = [0; 8];
        r.read_exact(&mut magic)?;
        ensure!(
            &magic == MAGIC || &magic == LEGACY_MAGIC,
            "unsupported certificate format"
        );
        let header: Header = bincode::deserialize_from(&mut r)?;
        let len: u64 = bincode::deserialize_from(&mut r)?;
        ensure!(len <= MAX_NODES as u64, "certificate too large");
        let mut nodes: HashMap<u64, Node> = HashMap::with_capacity(len as usize);
        for _ in 0..len {
            let (state, rank, mv): (u64, u16, u8) = bincode::deserialize_from(&mut r)?;
            if let Some(old) = nodes.get_mut(&state) {
                ensure!(&magic == LEGACY_MAGIC, "duplicate certificate node");
                // Old certificates repeated a state at different horizons.
                // Its smallest rank retains a complete, stronger strategy.
                if rank < old.rank {
                    *old = Node { rank, chosen: mv };
                }
            } else {
                nodes.insert(state, Node { rank, chosen: mv });
            }
        }
        let mut trailing = [0];
        ensure!(r.read(&mut trailing)? == 0, "trailing certificate bytes");
        let certificate = Self { header, nodes };
        certificate.verify()?;
        Ok(certificate)
    }

    /// A verified clean win also reaches clean-or-double for its owner and
    /// supplies a closed avoidance strategy for every opposing objective.
    /// The returned certificate must still pass the independent checker.
    pub fn derive_from_clean(&self, objective: Objective, first: bool) -> Result<Self> {
        ensure!(
            self.header.objective == Objective::Clean && self.header.forced_depth.is_some(),
            "derivation requires a positive clean certificate"
        );
        ensure!(
            !matches!(objective, Objective::All | Objective::Both),
            "invalid derived objective"
        );
        let same = first == self.header.first;
        ensure!(
            !same || objective != Objective::DoubleOnly,
            "a clean win does not imply forcing simultaneous lines"
        );
        let header = Header {
            objective,
            first,
            forced_depth: if same { self.header.forced_depth } else { None },
            ..self.header.clone()
        };
        let nodes = self
            .nodes
            .iter()
            .map(|(&state, &node)| {
                if same {
                    (state, node)
                } else {
                    let (key, turn) = decode(state);
                    (code(key, !turn), Node { rank: 0, ..node })
                }
            })
            .collect();
        Ok(Self { header, nodes })
    }
}

pub fn verify_file(path: &Path) -> Result<()> {
    let cert = Certificate::load(path)?;
    println!(
        "Verified {:?} for {} player from key {}: {} ({} strategy states)",
        cert.header.objective,
        if cert.header.first { "first" } else { "second" },
        cert.header.start_key,
        match cert.header.forced_depth {
            Some(d) => format!("FORCED within {d} plies"),
            None => "CANNOT FORCE; opponent has a closed avoidance strategy".into(),
        },
        cert.nodes.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests;

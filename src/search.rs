use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use rayon::prelude::*;

use crate::board::{has_three_in_row, BOARD_MASK};
use crate::movegen::{apply_move, generate_moves, Move, MoveOutcome};
use crate::position::{Position, PositionKey};
use crate::rules::{RepetitionRule, Rules};
use crate::tt::{Bound, Entry, TranspositionTable};

pub const WIN_SCORE: i16 = 30_000;

#[derive(Clone)]
pub struct SearchConfig {
    pub rules: Rules,
    pub max_depth: u16,
    pub save_every_depth: bool,
    pub checkpoint_path: PathBuf,
    pub compress_checkpoint: bool,
    pub stop_flag: Arc<AtomicBool>,
    pub once: bool,
}

/// Persisted across checkpoints so you can resume iterative deepening.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct SearchMeta {
    pub depth_completed: u16,
    pub best_move: Option<Move>,
    pub best_score: i16,
    pub nodes: u64,
    pub tt_hits: u64,
    pub tt_stores: u64,
    pub elapsed_ms: u64,
}

#[derive(Debug)]
pub struct Abort;

/// Run iterative deepening from (meta.depth_completed+1) up to cfg.max_depth.
///
/// This is a scouting engine, not a proof procedure. Its transposition keys
/// omit path history; use forcing certificates or the retrograde graph solver
/// for game-theoretic claims about the loopy game.
pub fn search_iterative_deepening(
    start: &Position,
    tt: &mut TranspositionTable,
    meta: &mut SearchMeta,
    cfg: &SearchConfig,
) -> Result<()> {
    let start_depth = meta.depth_completed.saturating_add(1);
    let end_depth = cfg.max_depth;

    info!("Iterative deepening: {start_depth}..={end_depth}");

    let mut elapsed_total = Duration::from_millis(meta.elapsed_ms);
    let mut nodes_total = meta.nodes;

    for depth in start_depth..=end_depth {
        if cfg.stop_flag.load(Ordering::Relaxed) {
            warn!("Stop requested before starting depth {depth}.");
            break;
        }

        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner} depth={msg} elapsed={elapsed_precise}")
                .unwrap(),
        );
        pb.set_message(format!("{depth}"));

        let start_t = Instant::now();
        let (best_move, best_score, nodes, (tt_hits, tt_stores)) =
            match search_root_parallel(start, depth, tt, cfg, &pb) {
                Ok(v) => v,
                Err(Abort) => {
                    warn!("Aborted during depth {depth} (Ctrl+C).");
                    break;
                }
            };
        let dt = start_t.elapsed();
        elapsed_total += dt;
        nodes_total += nodes;

        pb.finish_and_clear();

        info!(
            "depth {depth}: best={:?} score={} nodes={} dt={:.3}s (TT hits={}, stores={})",
            best_move,
            best_score,
            nodes,
            dt.as_secs_f64(),
            tt_hits,
            tt_stores
        );

        meta.depth_completed = depth;
        meta.best_move = Some(best_move);
        meta.best_score = best_score;
        meta.nodes = nodes_total;
        meta.tt_hits += tt_hits;
        meta.tt_stores += tt_stores;
        meta.elapsed_ms = elapsed_total.as_millis() as u64;

        // The caller (main) does a final save too, but saving every depth is convenient for resuming.
        if cfg.save_every_depth {
            crate::checkpoint::save_checkpoint(
                &cfg.checkpoint_path,
                cfg.compress_checkpoint,
                &cfg.rules,
                *meta,
                tt,
            )?;
        }

        if cfg.once {
            break;
        }
    }

    Ok(())
}

fn win_at_ply(ply: u16) -> i16 {
    WIN_SCORE - (ply as i16)
}

fn loss_at_ply(ply: u16) -> i16 {
    -WIN_SCORE + (ply as i16)
}

/// Root search: evaluate all legal moves in parallel and pick the best.
///
/// This sacrifices some root alpha-beta pruning, but scales well with cores,
/// and still benefits from the shared TT.
///
/// TODO: PV-splitting / YBWC to recover more pruning while staying parallel.
fn search_root_parallel(
    start: &Position,
    depth: u16,
    tt: &TranspositionTable,
    cfg: &SearchConfig,
    pb: &ProgressBar,
) -> std::result::Result<(Move, i16, u64, (u64, u64)), Abort> {
    let moves = generate_moves(start);
    if moves.is_empty() {
        // No legal move: treat as draw for now.
        return Ok((Move(0), 0, 0, (0, 0)));
    }

    // Shared counters are inside TT.stats, but we snapshot them at the end.
    let (nodes_before, hits_before, stores_before) = tt.stats_snapshot();

    let best = moves
        .par_iter()
        .map(|&mv| {
            let applied = apply_move(start, mv, &cfg.rules);

            match applied.outcome {
                MoveOutcome::MoverWin => return Ok((mv, win_at_ply(0))),
                MoveOutcome::MoverLoss => return Ok((mv, loss_at_ply(0))),
                MoveOutcome::Draw => return Ok((mv, 0)),
                MoveOutcome::Ongoing => {}
            }

            let mut path = HashSet::with_capacity(128);
            // Include current position in the path for repetition detection.
            path.insert(start.canonical_key());

            let score = match negamax(
                &applied.next,
                depth.saturating_sub(1),
                1,
                -WIN_SCORE,
                WIN_SCORE,
                tt,
                cfg,
                &mut path,
                pb,
            ) {
                Ok(s) => -s, // negamax: opponent perspective
                Err(Abort) => return Err(Abort),
            };
            Ok((mv, score))
        })
        .try_reduce_with(|a, b| Ok(if a.1 >= b.1 { a } else { b }))
        .ok_or(Abort)??;

    let (nodes_after, hits_after, stores_after) = tt.stats_snapshot();
    let nodes = nodes_after.saturating_sub(nodes_before);
    let tt_hits = hits_after.saturating_sub(hits_before);
    let tt_stores = stores_after.saturating_sub(stores_before);

    Ok((best.0, best.1, nodes, (tt_hits, tt_stores)))
}

/// Alpha-beta negamax with TT.
///
/// `ply` counts plies from the root (for mate-distance scoring).
///
/// `path` is used for repetition detection (loopy games).
#[allow(clippy::too_many_arguments)]
fn negamax(
    pos: &Position,
    depth: u16,
    ply: u16,
    mut alpha: i16,
    beta: i16,
    tt: &TranspositionTable,
    cfg: &SearchConfig,
    path: &mut HashSet<PositionKey>,
    pb: &ProgressBar,
) -> std::result::Result<i16, Abort> {
    if cfg.stop_flag.load(Ordering::Relaxed) {
        return Err(Abort);
    }

    // Count a visited node (for stats).
    tt.inc_nodes();

    // Progress/UI: keep very light.
    if (ply & 0x3F) == 0 {
        pb.set_position(ply as u64);
        pb.set_message(format!("{}", cfg.max_depth));
    }

    // Terminal detection:
    // If the opponent (them) already has a line / all pieces on board, the previous move ended the game.
    let them = pos.them & BOARD_MASK;
    if has_three_in_row(them) || them.count_ones() == cfg.rules.pieces_per_player as u32 {
        return Ok(loss_at_ply(ply));
    }

    // Repetition handling.
    let key = pos.canonical_key();
    match cfg.rules.repetition {
        RepetitionRule::PathRepeatIsDraw => {
            if path.contains(&key) {
                return Ok(0); // draw
            }
        }
        RepetitionRule::Forbidden => {
            if path.contains(&key) {
                return Ok(loss_at_ply(ply));
            }
        }
    }

    if depth == 0 {
        // Material-only evaluation for depth-limited scouting.
        // For proof search you will want something stronger than depth-limited anyway.
        return Ok(0);
    }

    // TT lookup
    if let Some(e) = tt.get(key) {
        if e.depth >= depth {
            match e.bound {
                Bound::Exact => return Ok(e.value),
                Bound::Lower => alpha = alpha.max(e.value),
                Bound::Upper => {
                    // reduce beta
                    // (we keep beta as immutable param, but emulate by early cutoff check)
                    if e.value <= alpha {
                        return Ok(e.value);
                    }
                }
            }
            if alpha >= beta {
                return Ok(e.value);
            }
        }
    }

    let alpha_orig = alpha;

    // Generate moves
    let moves = generate_moves(pos);
    if moves.is_empty() {
        // No legal moves -> draw under this model.
        return Ok(0);
    }

    // Mark in path for repetition detection
    if matches!(
        cfg.rules.repetition,
        RepetitionRule::PathRepeatIsDraw | RepetitionRule::Forbidden
    ) {
        path.insert(key);
    }

    let mut best = loss_at_ply(ply); // pessimistic
    let mut saw_legal = false;
    for mv in moves {
        let applied = apply_move(pos, mv, &cfg.rules);
        if cfg.rules.repetition == RepetitionRule::Forbidden
            && path.contains(&applied.next.canonical_key())
        {
            continue;
        }
        saw_legal = true;

        let score = match applied.outcome {
            MoveOutcome::MoverWin => win_at_ply(ply),
            MoveOutcome::MoverLoss => loss_at_ply(ply),
            MoveOutcome::Draw => 0,
            MoveOutcome::Ongoing => {
                let s = negamax(
                    &applied.next,
                    depth - 1,
                    ply + 1,
                    -beta,
                    -alpha,
                    tt,
                    cfg,
                    path,
                    pb,
                )?;
                -s
            }
        };

        if score > best {
            best = score;
        }
        if best > alpha {
            alpha = best;
        }
        if alpha >= beta {
            break;
        }
    }
    if !saw_legal {
        best = 0;
    }

    // Unmark in path
    if matches!(
        cfg.rules.repetition,
        RepetitionRule::PathRepeatIsDraw | RepetitionRule::Forbidden
    ) {
        path.remove(&key);
    }

    // Store to TT
    let bound = if best <= alpha_orig {
        Bound::Upper
    } else if best >= beta {
        Bound::Lower
    } else {
        Bound::Exact
    };
    tt.store(
        key,
        Entry {
            depth,
            bound,
            value: best,
        },
    );

    Ok(best)
}

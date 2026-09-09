//! Time-limited second-player practice. Only independently checked certificates
//! are described as forced wins; a heuristic score is never used as a proof.
use crate::board::LINE_MASKS;
use crate::certificate::{code, immediate_proof, Certificate};
use crate::movegen::{apply_move, generate_moves, ApplyResult, Move, MoveOutcome};
use crate::player::certified_move;
use crate::position::Position;
use crate::rules::Rules;
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct Trainer {
    pos: Position,
    first_turn: bool,
    rules: Rules,
    moves: Vec<u8>,
    seen: HashSet<u64>,
    result: Option<String>,
    proof: Option<Arc<Certificate>>,
    remaining: Option<u16>,
    proof_at: Option<usize>,
    searched_depth: u16,
}
impl Trainer {
    pub fn new(rules: Rules) -> Self {
        Self {
            pos: Position::start(),
            first_turn: true,
            rules,
            moves: vec![],
            seen: HashSet::from([code(0, true)]),
            result: None,
            proof: None,
            remaining: None,
            proof_at: None,
            searched_depth: 0,
        }
    }
    pub fn needs_reply(&self) -> bool {
        !self.first_turn && self.result.is_none()
    }
    pub fn human_move(&mut self, square: u8) -> Result<()> {
        ensure!(self.first_turn, "The computer is thinking");
        let mut next = self.clone();
        next.play(Move(square))?;
        *self = next;
        Ok(())
    }
    fn check_proof(&mut self) -> Result<()> {
        let Some(proof) = &self.proof else {
            return Ok(());
        };
        let goal_turn = !self.first_turn;
        let rank = match proof.nodes.get(&code(self.pos.canonical_key(), goal_turn)) {
            Some(node) => node.rank,
            None => {
                ensure!(
                    immediate_proof(&self.pos, goal_turn, &proof.header),
                    "Position is outside the winning strategy"
                );
                1
            }
        };
        ensure!(
            self.remaining
                .is_some_and(|remaining| rank > 0 && rank <= remaining),
            "Winning strategy exceeded its bound"
        );
        self.remaining = Some(rank);
        Ok(())
    }
    fn play(&mut self, mv: Move) -> Result<()> {
        ensure!(self.result.is_none(), "Game has finished");
        ensure!(
            mv.0 < 36 && generate_moves(&self.pos).contains(&mv),
            "Choose an empty square"
        );
        let applied = apply_move(&self.pos, mv, &self.rules);
        if let Some(proof) = &self.proof {
            if let Some(wins) = proof.header.objective.terminal(&applied, !self.first_turn) {
                ensure!(wins, "Move contradicts the winning strategy");
            }
        }
        self.result = outcome(applied.outcome, self.first_turn);
        self.pos = applied.next;
        self.first_turn = !self.first_turn;
        self.moves.push(mv.0);
        if let Some(remaining) = self.remaining {
            self.remaining = Some(
                remaining
                    .checked_sub(1)
                    .context("Winning strategy exhausted its bound")?,
            );
        }
        if self.result.is_none() {
            if !self
                .seen
                .insert(code(self.pos.canonical_key(), self.first_turn))
            {
                ensure!(
                    self.proof.is_none(),
                    "Winning strategy repeats game history"
                );
                self.result = Some("Draw by repetition".into());
            } else {
                self.check_proof()?;
            }
        }
        Ok(())
    }
    pub fn reply(&mut self, budget: Duration, stop: Arc<AtomicBool>) -> Result<()> {
        ensure!(self.needs_reply(), "No computer move is needed");
        if let Some(proof) = &self.proof {
            return self.play(certified_move(proof, &self.pos, true)?);
        }
        let start = Instant::now();
        let (fallback, depth) =
            best_effort(self.pos, &self.rules, &self.seen, start + budget / 3, &stop);
        self.searched_depth = depth;
        ensure!(!stop.load(Ordering::Relaxed), "Analysis cancelled");
        // Convert absolute first-player turn bits into computer-goal turn bits.
        let mut blocked: HashSet<_> = self
            .seen
            .iter()
            .map(|&state| {
                let (key, first_turn) = crate::certificate::decode(state);
                code(key, !first_turn)
            })
            .collect();
        blocked.remove(&code(self.pos.canonical_key(), true));
        if let Some(proof) = crate::forcing::tactical_certificate(
            self.pos,
            self.rules,
            start + budget,
            stop.clone(),
            blocked,
        ) {
            self.remaining = proof.header.forced_depth;
            self.proof_at = Some(self.moves.len());
            self.proof = Some(Arc::new(proof));
            self.check_proof()?;
        }
        ensure!(!stop.load(Ordering::Relaxed), "Analysis cancelled");
        let mv = if let Some(proof) = &self.proof {
            certified_move(proof, &self.pos, true)?
        } else {
            fallback
        };
        self.play(mv)
    }
    pub fn state(&self) -> Value {
        let (first, second) = if self.first_turn {
            (self.pos.us, self.pos.them)
        } else {
            (self.pos.them, self.pos.us)
        };
        let board: Vec<_> = (0..36)
            .map(|i| {
                if first & (1 << i) != 0 {
                    1
                } else if second & (1 << i) != 0 {
                    2
                } else {
                    0
                }
            })
            .collect();
        json!({"phase":"ready","mode":"play-first","board":board,"computerFirst":false,
            "firstTurn":self.first_turn,"humanTurn":self.first_turn && self.result.is_none(),
            "result":self.result,"remaining":self.remaining,"bound":self.proof.as_ref().and_then(|p|p.header.forced_depth),
            "proofAt":self.proof_at,"searchedDepth":self.searched_depth,"objective":"Clean","goalFirst":false,
            "moves":self.moves,"firstSupply":8-first.count_ones(),"secondSupply":8-second.count_ones(),
            "nonemptyStart":false,"diagonalPush":self.rules.push_directions == crate::rules::PushDirections::AllEight})
    }
}
fn outcome(result: MoveOutcome, first_turn: bool) -> Option<String> {
    match result {
        MoveOutcome::Ongoing => None,
        MoveOutcome::Draw => Some("Draw: both players completed a win condition".into()),
        MoveOutcome::MoverWin | MoveOutcome::MoverLoss => Some(format!(
            "{} wins",
            if first_turn == (result == MoveOutcome::MoverWin) {
                "First player (X)"
            } else {
                "Second player (O)"
            }
        )),
    }
}

const WIN: i32 = 30_000;
fn evaluate(pos: Position) -> i32 {
    let mut score = 12 * (pos.us_count() as i32 - pos.them_count() as i32);
    for &line in LINE_MASKS.iter() {
        let us = (pos.us & line).count_ones();
        let them = (pos.them & line).count_ones();
        if them == 0 {
            score += [0, 1, 9, 50][us as usize];
        }
        if us == 0 {
            score -= [0, 1, 9, 50][them as usize];
        }
    }
    score
}
fn terminal_score(outcome: MoveOutcome, ply: u16) -> Option<i32> {
    match outcome {
        MoveOutcome::MoverWin => Some(WIN - i32::from(ply)),
        MoveOutcome::MoverLoss => Some(-WIN + i32::from(ply)),
        MoveOutcome::Draw => Some(0),
        MoveOutcome::Ongoing => None,
    }
}
fn ordered(pos: Position, rules: &Rules) -> Vec<(Move, ApplyResult)> {
    let mut moves: Vec<_> = generate_moves(&pos)
        .into_iter()
        .map(|mv| (mv, apply_move(&pos, mv, rules)))
        .collect();
    moves.sort_by_cached_key(|(_, applied)| {
        -terminal_score(applied.outcome, 0).unwrap_or_else(|| -evaluate(applied.next))
    });
    moves
}
struct Evaluation<'a> {
    rules: &'a Rules,
    deadline: Instant,
    stop: &'a AtomicBool,
    path: HashSet<u64>,
}
impl Evaluation<'_> {
    fn search(
        &mut self,
        pos: Position,
        first_turn: bool,
        depth: u16,
        ply: u16,
        mut alpha: i32,
        beta: i32,
    ) -> Option<i32> {
        if Instant::now() >= self.deadline || self.stop.load(Ordering::Relaxed) {
            return None;
        }
        let key = code(pos.canonical_key(), first_turn);
        if self.path.contains(&key) {
            return Some(0);
        }
        if depth == 0 {
            return Some(evaluate(pos));
        }
        self.path.insert(key);
        let mut best = -WIN;
        for (_, applied) in ordered(pos, self.rules) {
            let score = match terminal_score(applied.outcome, ply) {
                Some(score) => score,
                None => {
                    -self.search(applied.next, !first_turn, depth - 1, ply + 1, -beta, -alpha)?
                }
            };
            best = best.max(score);
            alpha = alpha.max(score);
            if alpha >= beta {
                break;
            }
        }
        self.path.remove(&key);
        Some(best)
    }
}
fn best_effort(
    pos: Position,
    rules: &Rules,
    seen: &HashSet<u64>,
    deadline: Instant,
    stop: &AtomicBool,
) -> (Move, u16) {
    let mut moves = ordered(pos, rules);
    let mut best = moves[0].0;
    let mut completed = 0;
    'depth: for depth in 1..=16 {
        let mut search = Evaluation {
            rules,
            deadline,
            stop,
            path: seen.clone(),
        };
        let mut alpha = -WIN;
        let mut candidate = best;
        for &(mv, applied) in &moves {
            if Instant::now() >= deadline || stop.load(Ordering::Relaxed) {
                break 'depth;
            }
            let score = match terminal_score(applied.outcome, 0) {
                Some(score) => score,
                None => match search.search(applied.next, true, depth - 1, 1, -WIN, -alpha) {
                    Some(score) => -score,
                    None => break 'depth,
                },
            };
            if score > alpha {
                alpha = score;
                candidate = mv;
            }
        }
        best = candidate;
        completed = depth;
        if alpha.abs() > WIN - 100 {
            break;
        }
        let index = moves.iter().position(|(mv, _)| *mv == best).unwrap();
        moves.swap(0, index);
    }
    (best, completed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn human_can_start_and_cancelled_analysis_is_not_a_proof() {
        let mut game = Trainer::new(Rules::default());
        game.human_move(14).unwrap();
        assert!(game.needs_reply());
        let before = game.state();
        assert!(game.human_move(15).is_err());
        assert_eq!(game.state(), before);
        assert!(game
            .reply(Duration::ZERO, Arc::new(AtomicBool::new(true)))
            .is_err());
        assert!(game.proof.is_none());
    }
    #[test]
    fn tactical_win_is_verified_and_followed() {
        let rules = Rules::default();
        let pos = Position {
            us: (1 << 0) | (1 << 6),
            them: 1 << 22,
        };
        let proof = crate::forcing::tactical_certificate(
            pos,
            rules,
            Instant::now() + Duration::from_secs(1),
            Arc::new(AtomicBool::new(false)),
            HashSet::new(),
        )
        .unwrap();
        proof.verify().unwrap();
        assert_eq!(proof.header.forced_depth, Some(1));
        let mut game = Trainer::new(rules);
        game.pos = pos;
        game.first_turn = false;
        game.reply(Duration::from_millis(100), Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert_eq!(game.result.as_deref(), Some("Second player (O) wins"));
        assert!(game.proof.is_some());
    }
    #[test]
    fn a_retained_proof_wins_against_every_human_reply() {
        use crate::certificate::{Header, Node, ALL_MOVES};
        let pos = Position {
            us: 0,
            them: (1 << 0) | (1 << 1) | (1 << 28) | (1 << 29),
        };
        let key = pos.canonical_key();
        let pos = Position::from_key(key);
        let proof = Certificate {
            header: Header {
                rules: Rules::default(),
                objective: crate::forcing::Objective::Clean,
                first: false,
                start_key: key,
                forced_depth: Some(2),
            },
            nodes: std::collections::HashMap::from([(
                code(key, false),
                Node {
                    rank: 2,
                    chosen: ALL_MOVES,
                },
            )]),
        };
        proof.verify().unwrap();
        let proof = Arc::new(proof);
        for mv in generate_moves(&pos) {
            let mut game = Trainer::new(Rules::default());
            game.pos = pos;
            game.seen = HashSet::from([code(key, true)]);
            game.proof = Some(proof.clone());
            game.remaining = Some(2);
            game.human_move(mv.0).unwrap();
            if game.needs_reply() {
                game.reply(Duration::ZERO, Arc::new(AtomicBool::new(false)))
                    .unwrap();
            }
            assert_eq!(game.result.as_deref(), Some("Second player (O) wins"));
        }
    }
    #[test]
    fn earlier_position_blocks_a_tactical_proof() {
        let pos = Position {
            us: (1 << 0) | (1 << 6),
            them: 0,
        };
        let proof = crate::forcing::tactical_certificate(
            pos,
            Rules::default(),
            Instant::now() + Duration::from_millis(10),
            Arc::new(AtomicBool::new(false)),
            HashSet::from([code(pos.canonical_key(), true)]),
        );
        assert!(proof.is_none());
    }
    #[test]
    fn fallback_does_not_hand_over_an_immediate_win() {
        let pos = Position {
            us: 0,
            them: (1 << 0) | (1 << 6),
        };
        let rules = Rules::default();
        let (mv, depth) = best_effort(
            pos,
            &rules,
            &HashSet::new(),
            Instant::now() + Duration::from_millis(100),
            &AtomicBool::new(false),
        );
        assert!(depth >= 2);
        let applied = apply_move(&pos, mv, &rules);
        assert_ne!(applied.outcome, MoveOutcome::MoverLoss);
        assert!(!generate_moves(&applied.next)
            .into_iter()
            .any(|m| apply_move(&applied.next, m, &rules).outcome == MoveOutcome::MoverWin));
    }
}

//! Executes only independently verified certificate strategies.
use crate::certificate::{code, immediate_proof, Certificate};
use crate::movegen::{apply_move, generate_moves, Move, MoveOutcome};
use crate::position::Position;
use crate::symmetry::TRANSFORMS;
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::sync::Arc;

#[derive(Clone)]
pub struct Game {
    certificate: Arc<Certificate>,
    pos: Position,
    first_turn: bool,
    remaining: Option<u16>,
    moves: Vec<u8>,
    result: Option<String>,
    seen: HashSet<u64>,
}

impl Game {
    /// Caller supplies a certificate returned by Certificate::load (which verifies it).
    pub fn new(certificate: Arc<Certificate>) -> Result<Self> {
        let mut game = Self::start(certificate)?;
        game.computer_move()?;
        Ok(game)
    }
    fn start(certificate: Arc<Certificate>) -> Result<Self> {
        let mut game = Self {
            pos: Position::from_key(certificate.header.start_key),
            first_turn: true,
            remaining: certificate.header.forced_depth,
            moves: Vec::new(),
            result: None,
            seen: HashSet::new(),
            certificate,
        };
        game.check_position()?;
        game.seen.insert(code(game.pos.canonical_key(), true));
        Ok(game)
    }
    pub fn reset(&self) -> Result<Self> {
        Self::new(self.certificate.clone())
    }
    fn positive(&self) -> bool {
        self.certificate.header.forced_depth.is_some()
    }
    fn goal_turn(&self) -> bool {
        self.first_turn == self.certificate.header.first
    }
    fn computer_turn(&self) -> bool {
        self.goal_turn() == self.positive()
    }
    fn check_position(&mut self) -> Result<()> {
        let state = code(self.pos.canonical_key(), self.goal_turn());
        let rank = if let Some(node) = self.certificate.nodes.get(&state) {
            node.rank
        } else {
            ensure!(
                immediate_proof(&self.pos, self.goal_turn(), &self.certificate.header),
                "Position is outside the verified strategy"
            );
            1
        };
        if let Some(left) = self.remaining {
            ensure!(
                rank > 0 && rank <= left,
                "Strategy exceeded its verified move bound"
            );
            self.remaining = Some(rank);
        }
        Ok(())
    }
    fn strategy_move(&self) -> Result<Move> {
        certified_move(&self.certificate, &self.pos, self.goal_turn())
    }
    fn play(&mut self, mv: Move) -> Result<()> {
        ensure!(self.result.is_none(), "Game has finished");
        ensure!(
            mv.0 < 36 && generate_moves(&self.pos).contains(&mv),
            "Choose an empty square"
        );
        let applied = apply_move(&self.pos, mv, &self.certificate.header.rules);
        if let Some(reached) = self
            .certificate
            .header
            .objective
            .terminal(&applied, self.goal_turn())
        {
            ensure!(
                reached == self.positive(),
                "Move contradicts the verified strategy"
            );
            self.result = Some(match applied.outcome {
                MoveOutcome::Draw => "Draw: both players completed a win condition".into(),
                MoveOutcome::MoverWin | MoveOutcome::MoverLoss => {
                    let winner_first =
                        self.first_turn == (applied.outcome == MoveOutcome::MoverWin);
                    format!(
                        "{} wins",
                        if winner_first {
                            "First player (X)"
                        } else {
                            "Second player (O)"
                        }
                    )
                }
                MoveOutcome::Ongoing => unreachable!(),
            });
        }
        self.pos = applied.next;
        self.first_turn = !self.first_turn;
        self.moves.push(mv.0);
        if let Some(left) = self.remaining {
            self.remaining = Some(left.checked_sub(1).context("Move bound exhausted")?);
        }
        if self.result.is_none() {
            self.check_position()?;
            if !self
                .seen
                .insert(code(self.pos.canonical_key(), self.first_turn))
            {
                // A positive strategy strictly decreases rank and cannot repeat.
                ensure!(!self.positive(), "Forcing strategy repeated a position");
                self.result = Some("Draw by repetition; the target was avoided".into());
            }
        }
        Ok(())
    }
    fn computer_move(&mut self) -> Result<()> {
        if self.result.is_none() && self.computer_turn() {
            self.play(self.strategy_move()?)?;
        }
        Ok(())
    }
    pub fn human_move(&mut self, square: u8) -> Result<()> {
        ensure!(!self.computer_turn(), "It is the computer's turn");
        // Keep the board unchanged if any strategy invariant fails.
        let mut next = self.clone();
        next.play(Move(square))?;
        next.computer_move()?;
        *self = next;
        Ok(())
    }
    pub fn state(&self) -> Value {
        let first = if self.first_turn {
            self.pos.us
        } else {
            self.pos.them
        };
        let second = if self.first_turn {
            self.pos.them
        } else {
            self.pos.us
        };
        let computer_first = self.certificate.header.first == self.positive();
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
        json!({"phase":"ready", "board":board, "computerFirst":computer_first,
            "firstTurn":self.first_turn, "humanTurn":self.result.is_none() && !self.computer_turn(),
            "result":self.result, "remaining":self.remaining, "bound":self.certificate.header.forced_depth,
            "objective":format!("{:?}",self.certificate.header.objective), "goalFirst":self.certificate.header.first,
            "moves":self.moves, "firstSupply":8-first.count_ones(), "secondSupply":8-second.count_ones(),
            "nonemptyStart":self.certificate.header.start_key != 0,
            "diagonalPush":self.certificate.header.rules.push_directions == crate::rules::PushDirections::AllEight})
    }
}

pub(crate) fn certified_move(
    certificate: &Certificate,
    pos: &Position,
    goal_turn: bool,
) -> Result<Move> {
    let key = pos.canonical_key();
    if let Some(node) = certificate.nodes.get(&code(key, goal_turn)) {
        let canonical = Position::from_key(key);
        for map in TRANSFORMS.iter() {
            let transform = |bb: u64| {
                (0..36)
                    .filter(|&i| bb & (1 << i) != 0)
                    .fold(0, |acc, i| acc | (1 << map[i]))
            };
            if transform(pos.us) == canonical.us && transform(pos.them) == canonical.them {
                let square = map
                    .iter()
                    .position(|&s| s == node.chosen)
                    .context("Invalid strategy choice")?;
                return Ok(Move(square as u8));
            }
        }
        bail!("Cannot map strategy to this board");
    }
    generate_moves(pos)
        .into_iter()
        .find(|&mv| {
            certificate
                .header
                .objective
                .terminal(&apply_move(pos, mv, &certificate.header.rules), goal_turn)
                == Some(certificate.header.forced_depth.is_some())
        })
        .context("No verified immediate move")
}

/// A checked line through the winning strategy. Defender replies are selected
/// to reach the full bound when such a line can be found within the budget.
#[derive(Clone)]
pub struct Demo {
    game: Game,
    line: Vec<u8>,
}
impl Demo {
    pub fn new(certificate: Arc<Certificate>) -> Result<Self> {
        ensure!(
            certificate.header.forced_depth.is_some(),
            "A winning certificate is required for the demo"
        );
        Ok(Self {
            game: Game::start(certificate)?,
            line: vec![],
        })
    }
    pub fn prepare(&mut self, stop: &std::sync::atomic::AtomicBool) -> Result<()> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let mut failed = HashSet::new();
        let target = self.game.remaining.unwrap();
        if let Some(line) = demo_line(
            &self.game.certificate,
            self.game.pos,
            true,
            target,
            deadline,
            stop,
            &mut failed,
        ) {
            self.line = line;
        } else {
            ensure!(
                !stop.load(std::sync::atomic::Ordering::Relaxed),
                "Demo cancelled"
            );
            // Even when finding a full-bound line costs too much, every move in
            // this fallback line is still inside the independently checked proof.
            let mut game = self.game.clone();
            while game.result.is_none() {
                let mv = if game.goal_turn() {
                    game.strategy_move()?
                } else {
                    demo_moves(&game.certificate, game.pos, game.goal_turn())
                        .first()
                        .context("Demo has no move")?
                        .0
                };
                self.line.push(mv.0);
                game.play(mv)?;
            }
        }
        // Replay the complete selected line before presenting its exact length.
        let mut checked = self.game.clone();
        for &square in &self.line {
            checked.play(Move(square))?;
        }
        ensure!(
            checked.result.is_some(),
            "Demo line does not finish the game"
        );
        Ok(())
    }
    pub fn step(&mut self) -> Result<()> {
        let square = *self
            .line
            .get(self.game.moves.len())
            .context("Demo has finished or is still preparing")?;
        self.game.play(Move(square))
    }
    pub fn state(&self) -> Value {
        let mut value = self.game.state();
        value["mode"] = json!("demo");
        value["humanTurn"] = json!(false);
        value["demoLength"] = json!(self.line.len());
        value["demoComplete"] = json!(self.game.result.is_some());
        value
    }
}
fn demo_moves(cert: &Certificate, pos: Position, goal_turn: bool) -> Vec<(Move, u16)> {
    let mut moves: Vec<_> = generate_moves(&pos)
        .into_iter()
        .map(|mv| {
            let applied = apply_move(&pos, mv, &cert.header.rules);
            let rank = if applied.outcome == MoveOutcome::Ongoing {
                cert.nodes
                    .get(&code(applied.next.canonical_key(), !goal_turn))
                    .map_or(1, |n| n.rank)
            } else {
                0
            };
            (mv, rank)
        })
        .collect();
    moves.sort_by_key(|&(_, rank)| std::cmp::Reverse(rank));
    moves
}
#[allow(clippy::too_many_arguments)]
fn demo_line(
    cert: &Certificate,
    pos: Position,
    first_turn: bool,
    target: u16,
    deadline: std::time::Instant,
    stop: &std::sync::atomic::AtomicBool,
    failed: &mut HashSet<(u64, u16)>,
) -> Option<Vec<u8>> {
    if target == 0
        || std::time::Instant::now() >= deadline
        || stop.load(std::sync::atomic::Ordering::Relaxed)
        || failed.len() >= 100_000
    {
        return None;
    }
    let goal_turn = first_turn == cert.header.first;
    let state = code(pos.canonical_key(), goal_turn);
    if failed.contains(&(state, target)) {
        return None;
    }
    let rank = cert.nodes.get(&state).map_or(1, |node| node.rank);
    if rank < target {
        return None;
    }
    let moves = if goal_turn {
        vec![(certified_move(cert, &pos, goal_turn).ok()?, 0)]
    } else {
        demo_moves(cert, pos, goal_turn)
    };
    for (mv, _) in moves {
        let applied = apply_move(&pos, mv, &cert.header.rules);
        match cert.header.objective.terminal(&applied, goal_turn) {
            Some(true) if target == 1 => return Some(vec![mv.0]),
            None if target > 1 => {
                if let Some(mut rest) = demo_line(
                    cert,
                    applied.next,
                    !first_turn,
                    target - 1,
                    deadline,
                    stop,
                    failed,
                ) {
                    rest.insert(0, mv.0);
                    return Some(rest);
                }
            }
            _ => (),
        }
    }
    failed.insert((state, target));
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificate::{Header, Node, ALL_MOVES};
    use crate::forcing::Objective;
    use crate::rules::Rules;
    use std::collections::HashMap;
    fn fixture() -> Certificate {
        let key = Position {
            us: 0,
            them: (1 << 0) | (1 << 1) | (1 << 28) | (1 << 29),
        }
        .canonical_key();
        Certificate {
            header: Header {
                rules: Rules::default(),
                objective: Objective::Clean,
                first: false,
                start_key: key,
                forced_depth: Some(2),
            },
            nodes: HashMap::from([(
                code(key, false),
                Node {
                    rank: 2,
                    chosen: ALL_MOVES,
                },
            )]),
        }
    }
    #[test]
    fn demo_replays_the_full_bound_and_locks_after_the_result() {
        let cert = fixture();
        cert.verify().unwrap();
        let mut demo = Demo::new(Arc::new(cert)).unwrap();
        demo.prepare(&std::sync::atomic::AtomicBool::new(false))
            .unwrap();
        assert_eq!(demo.line.len(), 2);
        assert_eq!(demo.game.moves.len(), 0);
        demo.step().unwrap();
        demo.step().unwrap();
        assert_eq!(demo.game.result.as_deref(), Some("Second player (O) wins"));
        assert!(demo.step().is_err());
    }
    #[test]
    fn every_human_reply_gets_verified_winning_response() {
        let cert = fixture();
        cert.verify().unwrap();
        let cert = Arc::new(cert);
        for mv in generate_moves(&Position::from_key(cert.header.start_key)) {
            let mut game = Game::new(cert.clone()).unwrap();
            game.human_move(mv.0).unwrap();
            assert_eq!(game.result.as_deref(), Some("Second player (O) wins"));
            assert!(game.moves.len() <= 2);
        }
    }
    #[test]
    fn inverse_symmetry_and_avoidance_control_the_correct_side() {
        let pos = Position {
            us: (1 << 0) | (1 << 6),
            them: 1 << 22,
        };
        let key = pos.canonical_key();
        let canonical = Position::from_key(key);
        let rules = Rules::default();
        let chosen = generate_moves(&canonical)
            .into_iter()
            .find(|&m| {
                Objective::Clean.terminal(&apply_move(&canonical, m, &rules), true) == Some(true)
            })
            .unwrap()
            .0;
        let cert = Certificate {
            header: Header {
                rules,
                objective: Objective::Clean,
                first: true,
                start_key: key,
                forced_depth: Some(1),
            },
            nodes: HashMap::from([(code(key, true), Node { rank: 1, chosen })]),
        };
        cert.verify().unwrap();
        let avoidance = cert.derive_from_clean(Objective::Clean, false).unwrap();
        avoidance.verify().unwrap();
        for certificate in [cert, avoidance] {
            let certificate = Arc::new(certificate);
            for map in TRANSFORMS.iter() {
                let transform = |bb| crate::symmetry::transform_bb(bb, map);
                let mut game = Game {
                    certificate: certificate.clone(),
                    pos: Position {
                        us: transform(pos.us),
                        them: transform(pos.them),
                    },
                    first_turn: true,
                    remaining: certificate.header.forced_depth,
                    moves: vec![],
                    result: None,
                    seen: HashSet::new(),
                };
                game.check_position().unwrap();
                game.computer_move().unwrap();
                assert_eq!(game.result.as_deref(), Some("First player (X) wins"));
            }
        }
    }
    #[test]
    fn illegal_moves_are_atomic_and_reset_restores_start() {
        let mut game = Game::new(Arc::new(fixture())).unwrap();
        let before = game.state();
        assert!(game.human_move(255).is_err());
        let occupied = game.pos.occupied().trailing_zeros() as u8;
        assert!(game.human_move(occupied).is_err());
        assert_eq!(game.state(), before);
        game.human_move(generate_moves(&game.pos)[0].0).unwrap();
        assert!(game.human_move(0).is_err());
        assert_eq!(game.reset().unwrap().state(), before);
    }
    #[test]
    #[ignore = "Generates a prepared-position certificate for manual browser testing"]
    fn write_gui_fixture_when_requested() {
        std::fs::create_dir_all("test/evidence").unwrap();
        fixture()
            .save(std::path::Path::new(
                "test/evidence/prepared-position.certificate",
            ))
            .unwrap();
    }
}

use serde::{Deserialize, Serialize};

use crate::board::{
    bit, has_three_in_row, in_bounds, rc, ALL_DIRS, BOARD_MASK, CENTER_ORDER, N, ORTHOGONAL_DIRS,
};
use crate::position::Position;
use crate::rules::{PushDirections, Rules, SimultaneousWin};

/// A move is just a placement square (0..35).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct Move(pub u8);

impl Move {
    #[inline]
    pub fn square(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ApplyResult {
    pub next: Position,
    pub outcome: MoveOutcome,
    /// Both owners have a line of three after pushing (distinct from all-eight).
    pub simultaneous_lines: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoveOutcome {
    Ongoing,
    MoverWin,
    MoverLoss,
    Draw,
}

/// Generate all legal placement moves (empty squares).
///
/// Ordered using `CENTER_ORDER` for better pruning.
pub fn generate_moves(pos: &Position) -> Vec<Move> {
    // Having all eight pieces on the board is terminal, so there is no piece
    // left in the derived supply to place.
    if pos.us_count() == 8 {
        return Vec::new();
    }

    let empty = pos.empty();
    let mut out = Vec::with_capacity(empty.count_ones() as usize);
    for &sq in CENTER_ORDER.iter() {
        if (empty & bit(sq)) != 0 {
            out.push(Move(sq));
        }
    }
    out
}

/// Apply a move: place a piece, then push adjacent pieces outward.
///
/// Returns next position normalized to the next player (swap sides).
///
/// NOTE: This implements the documented Gekitai rule model:
/// - pushes happen in the directions selected by the rules
/// - you push an adjacent piece 1 square away if that destination is empty or off-board
/// - if blocked, it does not move
/// - pieces pushed off-board return to the owner's supply
///
/// If both owners satisfy a win condition after the same push, the move is a
/// terminal draw.
pub fn apply_move(pos: &Position, mv: Move, rules: &Rules) -> ApplyResult {
    debug_assert!(pos.is_legal(rules));
    let sq = mv.square();

    let place_bit = bit(sq);
    debug_assert!(
        (pos.occupied() & place_bit) == 0,
        "move must be on empty square"
    );
    debug_assert!(pos.us_count() < 8, "must have a piece in supply to place");

    let mut us = pos.us;
    let mut them = pos.them;
    // Place piece.
    us |= place_bit;

    // Push surrounding neighbors in the selected directions.
    let (r, c) = rc(sq);
    let directions: &[(i8, i8)] = match rules.push_directions {
        PushDirections::AllEight => &ALL_DIRS,
        PushDirections::Orthogonal => &ORTHOGONAL_DIRS,
    };
    for &(dr, dc) in directions {
        let nr = r + dr;
        let nc = c + dc;
        if !in_bounds(nr, nc) {
            continue;
        }
        let nsq = (nr as u8) * (N as u8) + (nc as u8);
        let nbit = bit(nsq);
        let neighbor_is_us = (us & nbit) != 0;
        let neighbor_is_them = (them & nbit) != 0;
        if !neighbor_is_us && !neighbor_is_them {
            continue;
        }

        // Destination one step further away.
        let dr2 = dr * 2;
        let dc2 = dc * 2;
        let drs = r + dr2;
        let dcs = c + dc2;

        // Off-board pieces leave the board and are automatically available to
        // place again because supply is derived from the on-board count.
        if !in_bounds(drs, dcs) {
            if neighbor_is_us {
                us &= !nbit;
            } else {
                them &= !nbit;
            }
            continue;
        }

        let dsq = (drs as u8) * (N as u8) + (dcs as u8);
        let dbit = bit(dsq);

        // Blocked? No push.
        if ((us | them) & dbit) != 0 {
            continue;
        }

        // Move neighbor into destination.
        if neighbor_is_us {
            us &= !nbit;
            us |= dbit;
        } else {
            them &= !nbit;
            them |= dbit;
        }
    }

    // Win checks after the move (end of mover's turn).
    let mover_line = has_three_in_row(us & BOARD_MASK);
    let mover_all = (us & BOARD_MASK).count_ones() == rules.pieces_per_player as u32;

    let opp_line = has_three_in_row(them & BOARD_MASK);
    let opp_all = (them & BOARD_MASK).count_ones() == rules.pieces_per_player as u32;

    let outcome = match (mover_line || mover_all, opp_line || opp_all) {
        (true, true) => match rules.simultaneous_win {
            SimultaneousWin::Draw => MoveOutcome::Draw,
            SimultaneousWin::MoverWins => MoveOutcome::MoverWin,
        },
        (true, false) => MoveOutcome::MoverWin,
        (false, true) => MoveOutcome::MoverLoss,
        (false, false) => MoveOutcome::Ongoing,
    };

    // Normalize to next player to move.
    let next = Position { us: them, them: us };

    ApplyResult {
        next,
        outcome,
        simultaneous_lines: mover_line && opp_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::idx;

    #[test]
    fn official_mode_pushes_diagonal_neighbour() {
        let mut rules = Rules::default();
        let p = Position {
            us: bit(idx(1, 1)),
            them: 0,
        };
        let result = apply_move(&p, Move(idx(2, 2)), &rules);
        assert_ne!(result.next.them & bit(idx(0, 0)), 0);
        assert_eq!(result.next.them & bit(idx(1, 1)), 0);

        rules.push_directions = PushDirections::Orthogonal;
        let result = apply_move(&p, Move(idx(2, 2)), &rules);
        assert_ne!(result.next.them & bit(idx(1, 1)), 0);
        assert_eq!(result.next.them & bit(idx(0, 0)), 0);
    }

    #[test]
    fn pushed_off_piece_returns_to_derived_supply() {
        let rules = Rules::default();
        let p = Position {
            us: bit(idx(0, 0)),
            them: 0,
        };
        let result = apply_move(&p, Move(idx(1, 1)), &rules);
        assert_eq!(result.next.them.count_ones(), 1);
    }

    #[test]
    fn terminal_outcomes_cover_both_lines_and_simultaneous_draw() {
        let rules = Rules::default();

        let mover_only = Position {
            us: bit(idx(0, 0)) | bit(idx(1, 0)),
            them: 0,
        };
        assert_eq!(
            apply_move(&mover_only, Move(idx(2, 0)), &rules).outcome,
            MoveOutcome::MoverWin
        );

        let opponent_only = Position {
            us: 0,
            them: bit(idx(2, 1)) | bit(idx(2, 3)) | bit(idx(2, 4)),
        };
        assert_eq!(
            apply_move(&opponent_only, Move(idx(2, 0)), &rules).outcome,
            MoveOutcome::MoverLoss
        );

        let simultaneous = Position {
            us: bit(idx(0, 0)) | bit(idx(1, 0)),
            them: bit(idx(2, 1)) | bit(idx(2, 3)) | bit(idx(2, 4)),
        };
        assert_eq!(
            apply_move(&simultaneous, Move(idx(2, 0)), &rules).outcome,
            MoveOutcome::Draw
        );

        let mut mover_wins_ties = rules;
        mover_wins_ties.simultaneous_win = SimultaneousWin::MoverWins;
        assert_eq!(
            apply_move(&simultaneous, Move(idx(2, 0)), &mover_wins_ties).outcome,
            MoveOutcome::MoverWin
        );
    }
}

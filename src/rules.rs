use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum PushDirections {
    /// Push every surrounding piece, including diagonal neighbours.
    AllEight,
    /// Compatibility mode for the recovered solver's original assumption.
    Orthogonal,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SimultaneousWin {
    Draw,
    MoverWins,
}

/// Repetition convention for depth-limited play.
///
/// Reachability proofs allow cycles: infinite play never reaches the target.
/// The interactive player ends a repeated canonical position as a draw.
/// Forbidding repetition requires path-dependent state and is rejected by the
/// forcing and retrograde proof engines.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RepetitionRule {
    /// If the same canonical position appears on the current search path, treat as draw.
    PathRepeatIsDraw,

    /// Disallow repetition (treat repeated positions as illegal).
    Forbidden,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Rules {
    pub push_directions: PushDirections,
    pub simultaneous_win: SimultaneousWin,
    pub repetition: RepetitionRule,

    /// Total pieces per player (8)
    pub pieces_per_player: u8,

    /// Board size (6). Fixed in this codebase, but kept for clarity.
    pub board_n: u8,

    /// Line length (3). Fixed in this codebase, but kept for clarity.
    pub line_len: u8,
}

impl Default for Rules {
    /// The rule model used by the published certificates:
    /// - 6x6
    /// - 8 pieces each
    /// - pushed-off pieces return to the player
    /// - win by 3-in-row, or by having all 8 on board at end of your move
    fn default() -> Self {
        Self {
            push_directions: PushDirections::AllEight,
            simultaneous_win: SimultaneousWin::Draw,
            repetition: RepetitionRule::PathRepeatIsDraw,
            pieces_per_player: 8,
            board_n: 6,
            line_len: 3,
        }
    }
}

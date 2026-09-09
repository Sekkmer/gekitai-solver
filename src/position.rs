use serde::{Deserialize, Serialize};

use once_cell::sync::Lazy;

use crate::board::{BOARD_MASK, CELLS};
use crate::rules::Rules;
use crate::symmetry::TRANSFORMS;

/// Collision-free base-3 board encoding. Each cell is empty/us/them.
///
/// `3^36` is less than `2^58`, so the complete side-to-move-normalized board
/// fits in one `u64`. Supply counts are not stored: Gekitai returns
/// pushed-off pieces, so each player's supply is `8 - pieces_on_board`.
pub type PositionKey = u64;

static POW3: Lazy<[u64; CELLS]> = Lazy::new(|| {
    let mut out = [0u64; CELLS];
    let mut value = 1u64;
    let mut i = 0;
    while i < CELLS {
        out[i] = value;
        value *= 3;
        i += 1;
    }
    out
});

static TRANSFORMED_WEIGHTS: Lazy<[[u64; 8]; CELLS]> = Lazy::new(|| {
    let mut weights = [[0; 8]; CELLS];
    for (i, row) in weights.iter_mut().enumerate() {
        for (t, weight) in row.iter_mut().enumerate() {
            *weight = POW3[TRANSFORMS[t][i] as usize];
        }
    }
    weights
});

/// Side-to-move normalized position.
///
/// Invariant: this struct is always “from the perspective of the player to move”
/// (i.e., `us` is the side to act now).
///
/// After applying a move, we swap `us` and `them` so the returned position is
/// again normalized.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub us: u64,
    pub them: u64,
}

impl Position {
    pub fn start() -> Self {
        Self { us: 0, them: 0 }
    }

    #[inline]
    pub fn occupied(&self) -> u64 {
        (self.us | self.them) & BOARD_MASK
    }

    #[inline]
    pub fn empty(&self) -> u64 {
        (!self.occupied()) & BOARD_MASK
    }

    #[inline]
    pub fn us_count(&self) -> u32 {
        (self.us & BOARD_MASK).count_ones()
    }

    #[inline]
    pub fn them_count(&self) -> u32 {
        (self.them & BOARD_MASK).count_ones()
    }

    #[inline]
    pub fn is_legal(&self, rules: &Rules) -> bool {
        // Basic consistency checks; keep cheap.
        if (self.us & self.them) != 0 {
            return false;
        }
        if (self.us | self.them) & !BOARD_MASK != 0 {
            return false;
        }
        // Don't exceed total pieces.
        if self.us_count() > rules.pieces_per_player as u32 {
            return false;
        }
        if self.them_count() > rules.pieces_per_player as u32 {
            return false;
        }
        true
    }

    #[inline]
    pub fn from_key(mut key: PositionKey) -> Self {
        let mut us = 0u64;
        let mut them = 0u64;
        for i in 0..CELLS {
            match key % 3 {
                1 => us |= 1u64 << i,
                2 => them |= 1u64 << i,
                _ => {}
            }
            key /= 3;
        }
        debug_assert_eq!(key, 0);
        Self { us, them }
    }

    /// Canonicalize under D4 symmetries (8 transforms).
    ///
    /// This *greatly* reduces state space and TT size.
    #[inline]
    pub fn canonical_key(&self) -> PositionKey {
        // Add all eight transformed ternary weights in parallel. This avoids
        // building temporary transformed bitboards, and the eight sums vectorize.
        let weights = &*TRANSFORMED_WEIGHTS;
        let mut keys = [0u64; 8];
        let mut bits = self.us;
        while bits != 0 {
            let i = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            for (key, weight) in keys.iter_mut().zip(weights[i]) {
                *key += weight;
            }
        }
        bits = self.them;
        while bits != 0 {
            let i = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            for (key, weight) in keys.iter_mut().zip(weights[i]) {
                *key += 2 * weight;
            }
        }
        *keys.iter().min().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::bit;
    use crate::symmetry::transform_bb;

    #[test]
    fn ternary_key_fits_signed_sqlite_integer() {
        assert!(3u64.pow(CELLS as u32) < i64::MAX as u64);
    }

    #[test]
    fn vectorized_key_preserves_persisted_encoding() {
        let mut rng = 31u64;
        for _ in 0..2000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let us = rng & BOARD_MASK;
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let them = rng & BOARD_MASK & !us;
            let slow = TRANSFORMS
                .iter()
                .map(|map| {
                    let a = transform_bb(us, map);
                    let b = transform_bb(them, map);
                    (0..CELLS)
                        .map(|i| {
                            POW3[i]
                                * if a & bit(i as u8) != 0 {
                                    1
                                } else if b & bit(i as u8) != 0 {
                                    2
                                } else {
                                    0
                                }
                        })
                        .sum::<u64>()
                })
                .min()
                .unwrap();
            assert_eq!(Position { us, them }.canonical_key(), slow);
        }
    }

    #[test]
    fn key_round_trip() {
        let p = Position {
            us: bit(0) | bit(17) | bit(35),
            them: bit(1) | bit(7) | bit(30),
        };
        let key = {
            let mut value = 0;
            for i in 0..CELLS {
                if p.us & (1u64 << i) != 0 {
                    value += POW3[i];
                } else if p.them & (1u64 << i) != 0 {
                    value += 2 * POW3[i];
                }
            }
            value
        };
        assert_eq!(Position::from_key(key), p);
    }

    #[test]
    fn canonical_key_is_symmetry_invariant() {
        let p = Position {
            us: bit(0) | bit(8) | bit(19),
            them: bit(5) | bit(14),
        };
        for map in TRANSFORMS.iter() {
            let transformed = Position {
                us: transform_bb(p.us, map),
                them: transform_bb(p.them, map),
            };
            assert_eq!(p.canonical_key(), transformed.canonical_key());
        }
    }
}

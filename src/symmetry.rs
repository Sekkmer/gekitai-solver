use once_cell::sync::Lazy;

use crate::board::{idx, in_bounds, rc, N};

/// 8 dihedral symmetries of the square board.
///
/// We represent each symmetry as a mapping old_index -> new_index.
pub static TRANSFORMS: Lazy<[[u8; 36]; 8]> = Lazy::new(|| {
    fn map_index(r: i8, c: i8, t: usize) -> (i8, i8) {
        match t {
            0 => (r, c),                 // identity
            1 => (c, N - 1 - r),         // rot 90
            2 => (N - 1 - r, N - 1 - c), // rot 180
            3 => (N - 1 - c, r),         // rot 270
            4 => (r, N - 1 - c),         // reflect vertical (mirror left-right)
            5 => (N - 1 - r, c),         // reflect horizontal (mirror top-bottom)
            6 => (c, r),                 // reflect main diagonal
            7 => (N - 1 - c, N - 1 - r), // reflect anti-diagonal
            _ => unreachable!(),
        }
    }

    let mut out = [[0u8; 36]; 8];
    for (t, transform) in out.iter_mut().enumerate() {
        for i in 0u8..36u8 {
            let (r, c) = rc(i);
            let (nr, nc) = map_index(r, c, t);
            debug_assert!(in_bounds(nr, nc));
            transform[i as usize] = idx(nr, nc);
        }
    }
    out
});

#[cfg(test)]
#[inline]
pub fn transform_bb(bb: u64, map: &[u8; 36]) -> u64 {
    // Board is only 36 cells; a straight 36-iteration loop is fast and predictable.
    let mut out = 0u64;
    let mut mask = bb;
    // Slight optimization: iterate set bits.
    while mask != 0 {
        let lsb = mask.trailing_zeros() as u8;
        mask &= mask - 1;
        out |= 1u64 << (map[lsb as usize] as u64);
    }
    out
}

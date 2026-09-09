use once_cell::sync::Lazy;

/// Board is fixed 6×6 in Gekitai.
pub const N: i8 = 6;
pub const CELLS: usize = (N as usize) * (N as usize);

/// Bit mask covering only the lowest 36 bits.
pub const BOARD_MASK: u64 = (1u64 << CELLS) - 1;

/// The four edge-adjacent directions.
pub const ORTHOGONAL_DIRS: [(i8, i8); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];

/// Every adjacent square, including diagonals.
pub const ALL_DIRS: [(i8, i8); 8] = [
    (-1, -1),
    (-1, 0),
    (-1, 1),
    (0, -1),
    (0, 1),
    (1, -1),
    (1, 0),
    (1, 1),
];

#[inline]
pub fn idx(r: i8, c: i8) -> u8 {
    debug_assert!((0..N).contains(&r) && (0..N).contains(&c));
    (r as u8) * (N as u8) + (c as u8)
}

#[inline]
pub fn rc(i: u8) -> (i8, i8) {
    let r = (i as i8) / N;
    let c = (i as i8) % N;
    (r, c)
}

#[inline]
pub fn in_bounds(r: i8, c: i8) -> bool {
    (0..N).contains(&r) && (0..N).contains(&c)
}

#[inline]
pub fn bit(i: u8) -> u64 {
    1u64 << (i as u64)
}

/// Precomputed list of all 3-in-a-row masks (horizontal/vertical/diagonal).
///
/// Total should be:
/// - horizontal: 6 rows × 4 windows = 24
/// - vertical: 6 cols × 4 windows = 24
/// - diag down-right: 4×4 = 16
/// - diag down-left: 4×4 = 16
///   Total = 80
pub static LINE_MASKS: Lazy<Vec<u64>> = Lazy::new(|| {
    let mut masks = Vec::with_capacity(80);

    // Horizontal
    for r in 0..N {
        for c in 0..=(N - 3) {
            let m = bit(idx(r, c)) | bit(idx(r, c + 1)) | bit(idx(r, c + 2));
            masks.push(m);
        }
    }

    // Vertical
    for c in 0..N {
        for r in 0..=(N - 3) {
            let m = bit(idx(r, c)) | bit(idx(r + 1, c)) | bit(idx(r + 2, c));
            masks.push(m);
        }
    }

    // Diag down-right
    for r in 0..=(N - 3) {
        for c in 0..=(N - 3) {
            let m = bit(idx(r, c)) | bit(idx(r + 1, c + 1)) | bit(idx(r + 2, c + 2));
            masks.push(m);
        }
    }

    // Diag down-left
    for r in 0..=(N - 3) {
        for c in 2..N {
            let m = bit(idx(r, c)) | bit(idx(r + 1, c - 1)) | bit(idx(r + 2, c - 2));
            masks.push(m);
        }
    }

    debug_assert_eq!(masks.len(), 80);
    masks
});

/// Move ordering: squares ordered from center outward.
///
/// Alpha-beta pruning depends heavily on move ordering.
/// A simple and surprisingly effective ordering is “center first”.
pub static CENTER_ORDER: Lazy<Vec<u8>> = Lazy::new(|| {
    let mut v: Vec<u8> = (0..CELLS as u8).collect();
    let center = (N as f32 - 1.0) / 2.0; // 2.5 for N=6

    v.sort_by(|&a, &b| {
        let (ra, ca) = rc(a);
        let (rb, cb) = rc(b);
        let da = (ra as f32 - center).powi(2) + (ca as f32 - center).powi(2);
        let db = (rb as f32 - center).powi(2) + (cb as f32 - center).powi(2);
        da.partial_cmp(&db).unwrap()
    });

    v
});

#[inline]
pub fn has_three_in_row(bb: u64) -> bool {
    // Starting columns prevent horizontal/diagonal shifts wrapping rows.
    const ROW_STARTS: u64 = BOARD_MASK / 63;
    const LEFT_FOUR: u64 = ROW_STARTS * 0b001111;
    const RIGHT_FOUR: u64 = ROW_STARTS * 0b111100;
    let bb = bb & BOARD_MASK;
    ((bb & (bb >> 1) & (bb >> 2) & LEFT_FOUR)
        | (bb & (bb >> 6) & (bb >> 12))
        | (bb & (bb >> 7) & (bb >> 14) & LEFT_FOUR)
        | (bb & (bb >> 5) & (bb >> 10) & RIGHT_FOUR))
        != 0
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shifted_line_detector_matches_all_triples_and_dense_boards() {
        let slow = |bb: u64| LINE_MASKS.iter().any(|&m| (bb & m).count_ones() == 3);
        for a in 0..36 {
            for b in a + 1..36 {
                for c in b + 1..36 {
                    let bb = bit(a) | bit(b) | bit(c);
                    assert_eq!(has_three_in_row(bb), slow(bb), "{a} {b} {c}");
                }
            }
        }
        let mut rng = 9u64;
        for _ in 0..10000 {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            let bb = rng & BOARD_MASK;
            assert_eq!(has_three_in_row(bb), slow(bb));
        }
    }
}

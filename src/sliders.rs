//! Slider attacks via precomputed occupancy-indexed tables.
//!
//! For every square, the *relevant rays* (interior ray squares; the outer
//! board-edge square at the ray's end is excluded because its occupancy
//! cannot change the attack set — the edge square is always attacked) are
//! at most 12 occupied bits, i.e. ≤ 4096 blocker arrangements. Tables are
//! built once at startup by explicit loops (ray walking); there are no
//! magic numbers anywhere, and the tables can be validated exhaustively
//! against the ray-walk reference (see the test below).
//!
//! Lookup: `table[sq][pext(occupied, relevant_mask)]` in O(1). PEXT is used
//! when BMI2 is detected at runtime (`x86_64`); a portable bit-compaction
//! loop is used otherwise. Node output is identical to ray stepping; only
//! speed changes.

use crate::bitboard::*;
use std::sync::OnceLock;

const ROOK_DIRS: [usize; 4] = [0, 1, 2, 3];
const BISHOP_DIRS: [usize; 4] = [4, 6, 5, 7];

const DIR_DFDR: [(i32, i32); 8] = [
    (0, 1),   // 0 N
    (0, -1),  // 1 S
    (1, 0),   // 2 E
    (-1, 0),  // 3 W
    (1, 1),   // 4 NE
    (-1, 1),  // 5 NW
    (1, -1),  // 6 SE
    (-1, -1), // 7 SW
];

struct SliderTables {
    /// Occupancy bits (inner ray squares only) that index the attack table.
    bishop_mask: [Bitboard; 64],
    rook_mask: [Bitboard; 64],
    // Heap-allocated (Vec) so the 4 MiB of tables never touches the stack.
    bishop_attacks: Vec<Bitboard>,
    rook_attacks: Vec<Bitboard>,
}

static TABLES: OnceLock<SliderTables> = OnceLock::new();
static HAS_PEXT: OnceLock<bool> = OnceLock::new();

/// Inner-ray occupancy mask for `sq` in `dirs` (outer board-edge end of
/// each ray excluded).
fn relevant_rays(sq: u8, dirs: &[usize]) -> Bitboard {
    let mut mask = 0u64;
    for &d in dirs {
        let (df, dr) = DIR_DFDR[d];
        let mut f = (sq % 8) as i32 + df;
        let mut r = (sq / 8) as i32 + dr;
        let mut squares = Vec::new();
        while (0..8).contains(&f) && (0..8).contains(&r) {
            squares.push((r * 8 + f) as u8);
            f += df;
            r += dr;
        }
        for (i, s) in squares.iter().enumerate() {
            if i + 1 == squares.len() {
                // Outer board-edge square: always attacked, its occupancy
                // cannot change the attack set, so it is not in the mask.
                continue;
            }
            mask |= bit(*s);
        }
    }
    mask
}

/// Reference attack generator (ray walking) used at table build time and in tests.
fn raywalk_attacks(sq: u8, occupied: Bitboard, dirs: &[usize]) -> Bitboard {
    let mut out = 0u64;
    for &d in dirs {
        let (df, dr) = DIR_DFDR[d];
        let mut f = (sq % 8) as i32 + df;
        let mut r = (sq / 8) as i32 + dr;
        while (0..8).contains(&f) && (0..8).contains(&r) {
            let s = (r * 8 + f) as u8;
            out |= bit(s);
            if (occupied >> s) & 1 != 0 {
                break;
            }
            f += df;
            r += dr;
        }
    }
    out
}

/// Compact the occupied `mask` bits to a low contiguous index.
/// Uses BMI2 PEXT on x86-64 when available; a portable loop otherwise.
#[inline]
fn unpack_index(occupied: Bitboard, mask: Bitboard) -> usize {
    #[cfg(target_arch = "x86_64")]
    {
        if *HAS_PEXT.get_or_init(|| std::is_x86_feature_detected!("bmi2")) {
            // SAFETY: PEXT is a documented BMI2 instruction; used only when
            // runtime-detected as supported.
            return unsafe { std::arch::x86_64::_pext_u64(occupied, mask) } as usize;
        }
    }
    let mut out = 0u64;
    let mut m = mask;
    let mut i = 0u32;
    while m != 0 {
        let lsb = m & m.wrapping_neg();
        if (occupied & lsb) != 0 {
            out |= 1 << i;
        }
        m &= m - 1;
        i += 1;
    }
    out as usize
}

fn build_tables() -> SliderTables {
    let mut t = SliderTables {
        bishop_mask: [0; 64],
        rook_mask: [0; 64],
        bishop_attacks: vec![0; 64 * 4096],
        rook_attacks: vec![0; 64 * 4096],
    };
    for sq in 0..64u8 {
        let bmask = relevant_rays(sq, &BISHOP_DIRS);
        let rmask = relevant_rays(sq, &ROOK_DIRS);
        t.bishop_mask[sq as usize] = bmask;
        t.rook_mask[sq as usize] = rmask;
        // Bishop table.
        fill_table(sq, bmask, &BISHOP_DIRS, &mut t.bishop_attacks);
        // Rook table.
        fill_table(sq, rmask, &ROOK_DIRS, &mut t.rook_attacks);
    }
    t
}

fn fill_table(sq: u8, mask: Bitboard, dirs: &[usize], table: &mut Vec<Bitboard>) {
    let bits: Vec<u8> = {
        let mut v = Vec::new();
        let mut m = mask;
        while m != 0 {
            let (s, rest) = pop_lsb(m);
            m = rest;
            v.push(s);
        }
        v
    };
    debug_assert!(bits.len() <= 12, "relevant mask exceeds 12 bits");
    for idx in 0..(1usize << bits.len()) {
        let mut occ = 0u64;
        for (i, &s) in bits.iter().enumerate() {
            if (idx >> i) & 1 != 0 {
                occ |= bit(s);
            }
        }
        table[(sq as usize) * 4096 + idx] = raywalk_attacks(sq, occ, dirs);
    }
}

fn tables() -> &'static SliderTables {
    TABLES.get_or_init(build_tables)
}

/// Bishop attacks from `sq` given the blockers in `occupied`.
pub fn bishop_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    let t = tables();
    let idx = unpack_index(occupied, t.bishop_mask[sq as usize]);
    t.bishop_attacks[(sq as usize) * 4096 + idx]
}

/// Rook attacks from `sq` given the blockers in `occupied`.
pub fn rook_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    let t = tables();
    let idx = unpack_index(occupied, t.rook_mask[sq as usize]);
    t.rook_attacks[(sq as usize) * 4096 + idx]
}

/// Queen attacks = rook + bishop.
pub fn queen_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    rook_attacks(sq, occupied) | bishop_attacks(sq, occupied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_tables_match_raywalk_all_occupancies() {
        // Exhaustive: every square, every blocker subset of its relevant
        // mask must equal the ray-walk reference exactly.
        for sq in 0..64u8 {
            for (dirs, table_fn) in [(&BISHOP_DIRS, bishop_attacks as fn(u8, u64) -> u64), (&ROOK_DIRS, rook_attacks as fn(u8, u64) -> u64)] {
                let mask = relevant_rays(sq, dirs);
                let mut sub = mask;
                loop {
                    let expected = raywalk_attacks(sq, sub, dirs);
                    let got = table_fn(sq, sub);
                    assert_eq!(got, expected, "sq={sq} occ={sub:#x}");
                    if sub == 0 {
                        break;
                    }
                    sub = (sub - 1) & mask;
                }
            }
        }
    }
}

//! Bitboards and attack generation.
//!
//! Bit 0 = a1, bit 63 = h8. Leaper (knight, king, pawn) attacks are
//! precomputed tables built at startup by looping over the board — no magic
//! numbers, so the tables can be verified by inspection. Slider attacks
//! (rook, bishop, queen) use the occupancy-indexed tables in
//! [`crate::sliders`], built at startup by explicit loops and validated
//! exhaustively against ray walking.

use crate::types::Color;
use std::sync::OnceLock;

pub type Bitboard = u64;

pub fn bit(sq: u8) -> Bitboard {
    1u64 << sq
}

pub fn popcount(bb: Bitboard) -> u32 {
    bb.count_ones()
}

/// Lowest set bit as a square, and the board without it.
/// Panics on an empty board (callers check first).
pub fn pop_lsb(bb: Bitboard) -> (u8, Bitboard) {
    let sq = bb.trailing_zeros() as u8;
    (sq, bb & (bb - 1))
}

// ------------------------------------------------------------------ tables

struct Tables {
    knight: [Bitboard; 64],
    king: [Bitboard; 64],
    pawn_white: [Bitboard; 64],
    pawn_black: [Bitboard; 64],
    // Reverse: squares whose pawns of that colour attack `sq`.
    pawn_attackers_white: [Bitboard; 64],
    pawn_attackers_black: [Bitboard; 64],
    // Passed-pawn masks: for a pawn on `sq`, the squares where an enemy
    // pawn would block its passed status (same/adjacent files, in front).
    passed_white: [Bitboard; 64],
    passed_black: [Bitboard; 64],
}

static TABLES: OnceLock<Tables> = OnceLock::new();

fn build_tables() -> Tables {
    let mut knight = [0u64; 64];
    let mut king = [0u64; 64];
    let mut pawn_white = [0u64; 64];
    let mut pawn_black = [0u64; 64];
    let mut pawn_attackers_white = [0u64; 64];
    let mut pawn_attackers_black = [0u64; 64];
    let mut passed_white = [0u64; 64];
    let mut passed_black = [0u64; 64];
    for sq in 0..64u8 {
        let (f, r) = (file_of(sq) as i32, rank_of(sq) as i32);
        // Knight: 8 L-shapes.
        for &(df, dr) in &[(1, 2), (2, 1), (2, -1), (1, -2), (-1, -2), (-2, -1), (-2, 1), (-1, 2)] {
            let nf = f + df;
            let nr = r + dr;
            if (0..8).contains(&nf) && (0..8).contains(&nr) {
                knight[sq as usize] |= bit((nr * 8 + nf) as u8);
            }
        }
        // King: 8 neighbours.
        for df in -1..=1 {
            for dr in -1..=1 {
                if df == 0 && dr == 0 {
                    continue;
                }
                let nf = f + df;
                let nr = r + dr;
                if (0..8).contains(&nf) && (0..8).contains(&nr) {
                    king[sq as usize] |= bit((nr * 8 + nf) as u8);
                }
            }
        }
        // White pawn captures one rank up; Black one rank down.
        for &df in &[-1, 1] {
            let nf = f + df;
            let wr = r + 1;
            if (0..8).contains(&nf) && (0..8).contains(&wr) {
                pawn_white[sq as usize] |= bit((wr * 8 + nf) as u8);
            }
            let br = r - 1;
            if (0..8).contains(&nf) && (0..8).contains(&br) {
                pawn_black[sq as usize] |= bit((br * 8 + nf) as u8);
            }
        }
        // Reverse: White attackers stand one rank below `sq`,
        // Black attackers one rank above it.
        for &df in &[-1, 1] {
            let nf = f + df;
            if !(0..8).contains(&nf) {
                continue;
            }
            let wr = r - 1;
            if (0..8).contains(&wr) {
                pawn_attackers_white[sq as usize] |= bit((wr * 8 + nf) as u8);
            }
            let br = r + 1;
            if (0..8).contains(&br) {
                pawn_attackers_black[sq as usize] |= bit((br * 8 + nf) as u8);
            }
        }
        // Passed-pawn masks: squares in front on same/adjacent files.
        // White pawn advances toward higher ranks; Black toward lower.
        for &df in &[-1, 0, 1] {
            let nf = f + df;
            if !(0..8).contains(&nf) {
                continue;
            }
            for nr in (r + 1)..8 {
                passed_white[sq as usize] |= bit((nr * 8 + nf) as u8);
            }
            for nr in 0..r {
                passed_black[sq as usize] |= bit((nr * 8 + nf) as u8);
            }
        }
    }
    Tables { knight, king, pawn_white, pawn_black, pawn_attackers_white, pawn_attackers_black, passed_white, passed_black }
}

pub fn knight_attacks(sq: u8) -> Bitboard {
    TABLES.get_or_init(build_tables).knight[sq as usize]
}

pub fn king_attacks(sq: u8) -> Bitboard {
    TABLES.get_or_init(build_tables).king[sq as usize]
}

/// Pawn attacks *from* `sq` for `color` (precomputed per colour).
pub fn pawn_attacks(sq: u8, color: Color) -> Bitboard {
    let t = TABLES.get_or_init(build_tables);
    match color {
        Color::White => t.pawn_white[sq as usize],
        Color::Black => t.pawn_black[sq as usize],
    }
}

/// Squares attacked *by* pawns of `color` that target `sq` (reverse table).
pub fn pawn_attackers_to(sq: u8, color: Color) -> Bitboard {
    let t = TABLES.get_or_init(build_tables);
    match color {
        Color::White => t.pawn_attackers_white[sq as usize],
        Color::Black => t.pawn_attackers_black[sq as usize],
    }
}

/// Passed-pawn mask for a pawn on `sq` of `color`: squares where an enemy
/// pawn would block its passed status (same/adjacent files, in front).
pub fn passed_pawn_mask(sq: u8, color: Color) -> Bitboard {
    let t = TABLES.get_or_init(build_tables);
    match color {
        Color::White => t.passed_white[sq as usize],
        Color::Black => t.passed_black[sq as usize],
    }
}

/// Piece attacks (ray stepping first): the module only wires the fast
/// path in via [`crate::sliders`], which is validated exhaustively against
/// ray walking at startup-table build time.
pub fn rook_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    crate::sliders::rook_attacks(sq, occupied)
}

pub fn bishop_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    crate::sliders::bishop_attacks(sq, occupied)
}

pub fn queen_attacks(sq: u8, occupied: Bitboard) -> Bitboard {
    crate::sliders::queen_attacks(sq, occupied)
}

/// Is `target` attacked by any piece of colour `attacker`?
///
/// `pieces[i]` holds the bitboard for colour `i/6` and piece `i%6`
/// (see [`crate::types::PieceType::idx`]); `occupied` is the union of all
/// pieces. Used for check detection and king-safety tests.
pub fn is_attacked(target: u8, attacker: Color, pieces: &[Bitboard; 12], occupied: Bitboard) -> bool {
    let base = attacker.idx() * 6;
    let pawns = pieces[base];
    let knights = pieces[base + 1];
    let bishops = pieces[base + 2];
    let rooks = pieces[base + 3];
    let queens = pieces[base + 4];
    let king = pieces[base + 5];
    if pawn_attackers_to(target, attacker) & pawns != 0 {
        return true;
    }
    if knight_attacks(target) & knights != 0 {
        return true;
    }
    if king_attacks(target) & king != 0 {
        return true;
    }
    if bishop_attacks(target, occupied) & (bishops | queens) != 0 {
        return true;
    }
    if rook_attacks(target, occupied) & (rooks | queens) != 0 {
        return true;
    }
    false
}

pub fn file_of(sq: u8) -> u8 {
    sq % 8
}
pub fn rank_of(sq: u8) -> u8 {
    sq / 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knight_from_e4() {
        // Ne4 attacks d6, f6, g5, g3, f2, d2, c3, c5 → 8 squares.
        let bb = knight_attacks(28); // e4
        assert_eq!(popcount(bb), 8);
        // Knight on a1 has 2 attacks: b3, c2.
        assert_eq!(popcount(knight_attacks(0)), 2);
    }

    #[test]
    fn king_and_pawn_tables() {
        assert_eq!(popcount(king_attacks(0)), 3); // a1: a2, b1, b2
        assert_eq!(popcount(king_attacks(28)), 8);
        // White pawn on e2 attacks d3 and f3.
        let p = pawn_attacks(12, Color::White);
        assert!(p & bit(19) != 0 && p & bit(21) != 0); // d3=19, f3=21
        // Black pawn on e7 attacks d6 and f6.
        let p = pawn_attacks(52, Color::Black);
        assert!(p & bit(43) != 0 && p & bit(45) != 0); // d6=43, f6=45
    }

    #[test]
    fn sliding_blockers() {
        // Empty board rook on e4 attacks 14 squares.
        let occ = bit(28);
        assert_eq!(popcount(rook_attacks(28, occ)), 14);
        // A blocker on e6 stops the north ray: squares e5, e6 only.
        let occ2 = bit(28) | bit(44);
        let r = rook_attacks(28, occ2);
        assert!(r & bit(36) != 0 && r & bit(44) != 0); // e5, e6
        assert!(r & bit(52) == 0); // e7 unreachable
        // Bishop on c1 empty: 7 squares (b2..h6 diagonal + d2..a5 diagonal).
        assert_eq!(popcount(bishop_attacks(2, bit(2))), 7);
    }
}

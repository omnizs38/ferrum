//! Static evaluation.
//!
//! The Alpha build (`--no-default-features`) uses a *simple* evaluation:
//! material plus a single piece-square table, a defensive king-shield term
//! and (when trained non-zero) defensive king-danger / own-loose terms.
//! The Gamma build adds *advanced* evaluation (feature `advanced-eval`):
//! tapered middlegame / endgame scores, mobility, pawn structure, bishop
//! pair, rook files and king safety on top of the same piece-square tables.
//! Material+PST is maintained incrementally by `Board` (like the Zobrist
//! hash) for speed: see `Board::mg_scores`/`eg_scores` and `material_pst`.
//!
//! # Trainable parameters
//!
//! Every positional weight lives in [`EvalParams`]. The defaults reproduce
//! the hand-written, measured baseline exactly; `examples/train.rs` (and
//! `tools/train.py`) can sweep them without a rebuild by passing a
//! `key=value,...` spec. This is the hook the per-variant training runs use:
//! Alpha is trained towards a *defensive* bias, Gamma towards a *trapping*
//! bias, both with game results as the strength objective.
//!
//! All scores are in centipawns. [`evaluate`] returns a score **from the
//! side-to-move's perspective** (positive = side to move is better), which
//! is what the negamax search expects.

use crate::bitboard::*;
use crate::board::Board;
use crate::params::param_accessors;
use crate::types::Color;

// Material values (MG / EG for the tapered version).
#[cfg(feature = "advanced-eval")]
const MG_VALUE: [i32; 6] = [82, 337, 365, 477, 1025, 0];
#[cfg(feature = "advanced-eval")]
const EG_VALUE: [i32; 6] = [94, 281, 297, 512, 936, 0];

// Simple (Alpha) material values are trainable: see `EvalParams::alpha_*`.

// ------------------------------------------------------------------- PSTs
// Tables are written from White's perspective, rank 8 first (row 0) down to
// rank 1 (row 7). For a square `sq` (a1 = 0): the White lookup index is
// `(7 - rank) * 8 + file` and the Black index is `rank * 8 + file` (mirror).

// Shared base tables. Alpha uses these directly; Gamma reuses them as the
// middlegame tables (MG) and adds separate endgame tables (EG).
const PST: [[i32; 64]; 6] = [
    // Pawn
    [
        0, 0, 0, 0, 0, 0, 0, 0,
        50, 50, 50, 50, 50, 50, 50, 50,
        10, 10, 20, 30, 30, 20, 10, 10,
        5, 5, 10, 25, 25, 10, 5, 5,
        0, 0, 0, 20, 20, 0, 0, 0,
        5, -5, -10, 0, 0, -10, -5, 5,
        5, 10, 10, -20, -20, 10, 10, 5,
        0, 0, 0, 0, 0, 0, 0, 0,
    ],
    // Knight
    [
        -50, -40, -30, -30, -30, -30, -40, -50,
        -40, -20, 0, 0, 0, 0, -20, -40,
        -30, 0, 10, 15, 15, 10, 0, -30,
        -30, 5, 15, 20, 20, 15, 5, -30,
        -30, 0, 15, 20, 20, 15, 0, -30,
        -30, 5, 10, 15, 15, 10, 5, -30,
        -40, -20, 0, 5, 5, 0, -20, -40,
        -50, -40, -30, -30, -30, -30, -40, -50,
    ],
    // Bishop
    [
        -20, -10, -10, -10, -10, -10, -10, -20,
        -10, 0, 0, 0, 0, 0, 0, -10,
        -10, 0, 5, 10, 10, 5, 0, -10,
        -10, 5, 5, 10, 10, 5, 5, -10,
        -10, 0, 10, 10, 10, 10, 0, -10,
        -10, 10, 10, 10, 10, 10, 10, -10,
        -10, 5, 0, 0, 0, 0, 5, -10,
        -20, -10, -10, -10, -10, -10, -10, -20,
    ],
    // Rook
    [
        0, 0, 0, 0, 0, 0, 0, 0,
        5, 10, 10, 10, 10, 10, 10, 5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        0, 0, 0, 5, 5, 0, 0, 0,
    ],
    // Queen
    [
        -20, -10, -10, -5, -5, -10, -10, -20,
        -10, 0, 0, 0, 0, 0, 0, -10,
        -10, 0, 5, 5, 5, 5, 0, -10,
        -5, 0, 5, 5, 5, 5, 0, -5,
        0, 0, 5, 5, 5, 5, 0, -5,
        -10, 5, 5, 5, 5, 5, 0, -10,
        -10, 0, 5, 0, 0, 0, 0, -10,
        -20, -10, -10, -5, -5, -10, -10, -20,
    ],
    // King (middlegame hiding in the corner)
    [
        -30, -40, -40, -50, -50, -40, -40, -30,
        -30, -40, -40, -50, -50, -40, -40, -30,
        -30, -40, -40, -50, -50, -40, -40, -30,
        -30, -40, -40, -50, -50, -40, -40, -30,
        -20, -30, -30, -40, -40, -30, -30, -20,
        -10, -20, -20, -20, -20, -20, -20, -10,
        20, 20, 0, 0, 0, 0, 20, 20,
        20, 30, 10, 0, 0, 10, 30, 20,
    ],
];

#[cfg(feature = "advanced-eval")]
const PST_MG: [[i32; 64]; 6] = PST;

#[cfg(feature = "advanced-eval")]
const PST_EG: [[i32; 64]; 6] = [
    // Pawn EG
    [
        0, 0, 0, 0, 0, 0, 0, 0,
        50, 50, 50, 50, 50, 50, 50, 50,
        30, 30, 30, 35, 35, 30, 30, 30,
        20, 20, 20, 25, 25, 20, 20, 20,
        10, 10, 10, 15, 15, 10, 10, 10,
        5, 5, 5, 10, 10, 5, 5, 5,
        0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0, 0, 0, 0,
    ],
    // Knight EG
    [
        -30, -20, -10, -10, -10, -10, -20, -30,
        -20, -10, 0, 0, 0, 0, -10, -20,
        -10, 0, 10, 15, 15, 10, 0, -10,
        -10, 5, 15, 20, 20, 15, 5, -10,
        -10, 0, 15, 20, 20, 15, 0, -10,
        -10, 5, 10, 15, 15, 10, 5, -10,
        -20, -10, 0, 5, 5, 0, -10, -20,
        -30, -20, -10, -10, -10, -10, -20, -30,
    ],
    // Bishop EG
    [
        -10, -10, -10, -10, -10, -10, -10, -10,
        -10, 0, 0, 0, 0, 0, 0, -10,
        -10, 0, 5, 10, 10, 5, 0, -10,
        -10, 5, 5, 10, 10, 5, 5, -10,
        -10, 0, 10, 10, 10, 10, 0, -10,
        -10, 10, 10, 10, 10, 10, 10, -10,
        -10, 5, 0, 0, 0, 0, 5, -10,
        -10, 0, 0, 0, 0, 0, 0, -10,
    ],
    // Rook EG
    [
        0, 0, 0, 0, 0, 0, 0, 0,
        5, 10, 10, 10, 10, 10, 10, 5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 0, 0, 0, 0, 0, -5,
        0, 0, 0, 0, 0, 0, 0, 0,
    ],
    // Queen EG
    [
        -10, -5, -5, -5, -5, -5, -5, -10,
        -5, 0, 0, 0, 0, 0, 0, -5,
        -5, 0, 5, 5, 5, 5, 0, -5,
        0, 0, 5, 5, 5, 5, 0, -5,
        0, 0, 5, 5, 5, 5, 0, -5,
        -5, 0, 5, 5, 5, 5, 0, -5,
        -5, 0, 5, 0, 0, 0, 0, -5,
        -10, -5, -5, -5, -5, -5, -5, -10,
    ],
    // King EG
    [
        -50, -40, -30, -20, -20, -30, -40, -50,
        -30, -20, -10, 0, 0, -10, -20, -30,
        -30, -10, 20, 30, 30, 20, -10, -30,
        -30, -10, 30, 40, 40, 30, -10, -30,
        -30, -10, 30, 40, 40, 30, -10, -30,
        -30, -10, 20, 30, 30, 20, -10, -30,
        -30, -30, 0, 0, 0, 0, -30, -30,
        -50, -30, -30, -30, -30, -30, -30, -50,
    ],
];

/// Passed-pawn bonus by rank, from the owning side's point of view.
/// Alpha scales it by `EvalParams::alpha_passed`; Gamma by `passed_scale`.
const PASSED_BONUS: [i32; 8] = [0, 0, 5, 10, 20, 35, 60, 0];

/// PST value for a piece on `sq` (white-oriented tables, mirrored for Black).
#[inline]
fn pst_index(sq: u8, color: Color) -> usize {
    let rank = rank_of(sq) as usize;
    let file = file_of(sq) as usize;
    match color {
        Color::White => (7 - rank) * 8 + file,
        Color::Black => rank * 8 + file,
    }
}

// ------------------------------------------------------------- EvalParams

/// Every tunable positional weight. `DEFAULT` is the hand-written baseline
/// (measured in `ELO.md`); the training tools search around it.
///
/// Units: centipawns per unit, except `*_scale` fields which are percent of
/// the underlying base value. `DEFAULT` is the trained baseline; individual
/// style terms can be zeroed to reproduce the pre-training engine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EvalParams {
    // --- Gamma advanced evaluation (also read by Alpha where marked) ---
    /// Mobility: cp per attacked non-own square, per piece kind.
    pub mob_knight: i32,
    pub mob_bishop: i32,
    pub mob_rook: i32,
    pub mob_queen: i32,
    /// Bishop pair bonus.
    pub bishop_pair: i32,
    /// Percent scaling of the passed-pawn rank table.
    pub passed_scale: i32,
    /// Pawn-structure penalties/bonuses (cp).
    pub doubled: i32,
    pub isolated: i32,
    pub connected: i32,
    pub blocked: i32,
    /// Rook file bonuses (cp).
    pub rook_open: i32,
    pub rook_semi: i32,
    /// Rook on the seventh rank (relative to its own side).
    pub rook7: i32,
    /// King pawn shield: cp per shield pawn, cap, and per missing pawn.
    pub shield_pawn: i32,
    pub shield_cap: i32,
    pub shield_missing: i32,
    /// Percent of enemy loose material awarded (Gamma "trapping" core).
    pub pressure_scale: i32,
    /// Good-trapping term: bonus per enemy knight/bishop/rook with at most
    /// one safe square (a piece running out of squares).
    pub trap_bonus: i32,
    /// Good-trapping term: bonus per attack by our pieces on the enemy
    /// king's ring (king square + its neighbours).
    pub king_pressure: i32,
    /// Defensive king safety: penalty per enemy attack on our own king ring.
    pub king_danger: i32,
    // --- Alpha simple evaluation style terms ---
    /// Defensive: cp per own pawn in the three squares in front of the king.
    pub alpha_shield: i32,
    /// Defensive: bonus for having castled (king on g1/c1 or g8/c8).
    pub alpha_castle: i32,
    /// Defensive: penalty per enemy N/B/R/Q attack on our king ring.
    pub alpha_king_danger: i32,
    /// Defensive: penalty as a percent of our own loose material.
    pub alpha_own_loose: i32,
    /// Alpha's cheap mobility (cp per attacked non-own square).
    pub alpha_mob_knight: i32,
    pub alpha_mob_bishop: i32,
    /// Alpha's bishop-pair bonus (0 = off).
    pub alpha_bishop_pair: i32,
    /// Alpha's passed-pawn bonus as a percent of the shared rank table
    /// (0 = off, 100 = full table).
    pub alpha_passed: i32,
    // --- Alpha simple-evaluation material ---
    pub alpha_pawn: i32,
    pub alpha_knight: i32,
    pub alpha_bishop: i32,
    pub alpha_rook: i32,
    pub alpha_queen: i32,
    // --- shared ---
    /// Tempo bonus for the side to move (0 = off in the baseline).
    pub tempo: i32,
}

impl EvalParams {
    /// Hand-written baseline, updated with the 2026-11-15 training run:
    /// * `alpha_own_loose = 24` — Alpha's defensive term measured +47 Elo
    ///   at 80k nodes/move (95% CI +13..+82) and cut its own hanging material
    ///   from 57.2 to 40.5 cp on average.
    /// * `tempo = 15` — side-to-move bonus, measured neutral-to-positive for
    ///   both variants; kept as a standard tempo term.
    /// * Gamma trapping trio (`pressure_scale=16`, `trap_bonus=8`,
    ///   `king_pressure=3`) — +41 Elo at 20k nodes/move (95% CI +14..+70)
    ///   and +15 Elo at 80k (CI -19..+49, i.e. strength-neutral at longer
    ///   TC) while moving every trapping metric in the intended direction
    ///   (enemy loose material 45.5 vs 37.2 cp, enemy trapped pieces 0.31
    ///   vs 0.24, enemy-king pressure 5.17 vs 4.52 at 20k nodes).
    /// * Second pass: `bishop_pair = 40` measured +30 Elo (CI +2..+58) at
    ///   20k nodes over 600 games; 50 lost to 40 by −20. `alpha_castle = 20`
    ///   is strength-neutral (−5, CI −33..+23) and moves the defensive
    ///   king-ring metric the right way (3.21 vs 3.39), so it is kept for
    ///   Alpha's identity.
    /// * 2026-11-16 Alpha pass: knight/bishop mobility (3/2), passed pawns
    ///   (full shared table) and bishop pair (30) measured **+31 Elo**
    ///   (CI +16..+47) over 2000 paired games at 20k nodes/move; the
    ///   individual terms were +16/+17/+9. This closes most of Alpha's
    ///   remaining evaluation gap without giving it Gamma's tapered eval.
    ///
    /// Gamma's search prunings (the larger gain) are in
    /// `SearchParams::DEFAULT`, not here.
    pub const DEFAULT: EvalParams = EvalParams {
        mob_knight: 4,
        mob_bishop: 3,
        mob_rook: 2,
        mob_queen: 1,
        bishop_pair: 40,
        passed_scale: 100,
        doubled: 12,
        isolated: 10,
        connected: 12,
        blocked: 8,
        rook_open: 15,
        rook_semi: 8,
        rook7: 0,
        shield_pawn: 15,
        shield_cap: 45,
        shield_missing: 10,
        pressure_scale: 16,
        trap_bonus: 8,
        king_pressure: 3,
        king_danger: 0,
        alpha_shield: 8,
        alpha_castle: 20,
        alpha_king_danger: 0,
        alpha_own_loose: 24,
        alpha_mob_knight: 3,
        alpha_mob_bishop: 2,
        alpha_bishop_pair: 30,
        alpha_passed: 100,
        alpha_pawn: 100,
        alpha_knight: 320,
        alpha_bishop: 330,
        alpha_rook: 500,
        alpha_queen: 900,
        tempo: 15,
    };

    // Keys, `set` and `get` are generated from this one field list.
    param_accessors!("eval";
        mob_knight, mob_bishop, mob_rook, mob_queen,
        bishop_pair, passed_scale,
        doubled, isolated, connected, blocked,
        rook_open, rook_semi, rook7,
        shield_pawn, shield_cap, shield_missing,
        pressure_scale, trap_bonus, king_pressure, king_danger,
        alpha_shield, alpha_castle, alpha_king_danger, alpha_own_loose,
        alpha_mob_knight, alpha_mob_bishop, alpha_bishop_pair, alpha_passed,
        alpha_pawn, alpha_knight, alpha_bishop, alpha_rook, alpha_queen,
        tempo
    );

    /// Parse `key=value,key=value` (commas or semicolons). An empty spec is
    /// the baseline. Values are plain integers.
    pub fn from_spec(spec: &str) -> Result<EvalParams, String> {
        let mut p = EvalParams::DEFAULT;
        crate::params::parse_spec(spec, |k, v| p.set(k, v))?;
        Ok(p)
    }

    /// Serialise every parameter (stable key order, suitable for logs).
    pub fn to_spec(&self) -> String {
        let mut out = String::new();
        for &key in Self::KEYS {
            if !out.is_empty() {
                out.push(',');
            }
            out.push_str(key);
            out.push('=');
            out.push_str(&self.get(key).to_string());
        }
        out
    }
}

impl Default for EvalParams {
    fn default() -> Self {
        EvalParams::DEFAULT
    }
}

/// Evaluate the position with the baseline weights.
pub fn evaluate(board: &Board) -> i32 {
    evaluate_with(board, &EvalParams::DEFAULT)
}

/// Evaluate the position with explicit trained weights.
/// Positive = side to move is better.
pub fn evaluate_with(board: &Board, p: &EvalParams) -> i32 {
    #[cfg(feature = "advanced-eval")]
    {
        // One pressure scan serves both colours: attack tables for a square
        // are built once instead of once per evaluation side.
        let pressure = Pressure::scan(board, p);
        let white = evaluate_color(board, Color::White, &pressure, p);
        let black = evaluate_color(board, Color::Black, &pressure, p);
        let score = white - black;
        let stm = match board.side {
            Color::White => score,
            Color::Black => -score,
        };
        stm + p.tempo
    }
    #[cfg(not(feature = "advanced-eval"))]
    {
        let value = [
            p.alpha_pawn,
            p.alpha_knight,
            p.alpha_bishop,
            p.alpha_rook,
            p.alpha_queen,
            0,
        ];
        let mut score = 0i32;
        for sq in 0..64u8 {
            if let Some(piece) = board.mailbox[sq as usize] {
                let v = value[piece.kind.idx()] + PST[piece.kind.idx()][pst_index(sq, piece.color)];
                match piece.color {
                    Color::White => score += v,
                    Color::Black => score -= v,
                }
            }
        }
        // Alpha "defensive" playstyle terms. Each is skipped when its weight
        // is zero so the baseline pays nothing for the machinery.
        score += alpha_style(board, Color::White, p);
        score -= alpha_style(board, Color::Black, p);
        let stm = match board.side {
            Color::White => score,
            Color::Black => -score,
        };
        stm + p.tempo
    }
}

#[cfg(not(feature = "advanced-eval"))]
fn alpha_style(board: &Board, color: Color, p: &EvalParams) -> i32 {
    let mut s = shield_bonus(board, color, p.alpha_shield);
    s += alpha_mobility(board, color, p);
    if p.alpha_bishop_pair != 0
        && popcount(board.pieces[color.idx() * 6 + 2]) >= 2
    {
        s += p.alpha_bishop_pair;
    }
    s += alpha_passed(board, color, p.alpha_passed);
    if p.alpha_castle != 0 && king_camped(board, color) {
        s += p.alpha_castle;
    }
    if p.alpha_king_danger != 0 {
        s -= king_ring_attacks(board, color) * p.alpha_king_danger;
    }
    if p.alpha_own_loose != 0 {
        let loose = loose_material_by_owner(board)
            .get(color.idx())
            .copied()
            .unwrap_or(0);
        s -= loose * p.alpha_own_loose / 100;
    }
    s
}

/// Alpha's cheap mobility: knight and bishop moves that do not land on an
/// own piece. Off by default, and far cheaper than Gamma's full mobility.
#[cfg(not(feature = "advanced-eval"))]
fn alpha_mobility(board: &Board, color: Color, p: &EvalParams) -> i32 {
    if p.alpha_mob_knight == 0 && p.alpha_mob_bishop == 0 {
        return 0;
    }
    let base = color.idx() * 6;
    let own = color_bitboard(board, color);
    let occ = board.occupied;
    let mut s = 0;
    let mut knights = board.pieces[base + 1];
    while knights != 0 {
        let (sq, rest) = pop_lsb(knights);
        knights = rest;
        s += popcount(knight_attacks(sq) & !own) as i32 * p.alpha_mob_knight;
    }
    let mut bishops = board.pieces[base + 2];
    while bishops != 0 {
        let (sq, rest) = pop_lsb(bishops);
        bishops = rest;
        s += popcount(bishop_attacks(sq, occ) & !own) as i32 * p.alpha_mob_bishop;
    }
    s
}

/// Alpha's passed-pawn bonus (a percent of the shared rank table): cheap
/// because it is one precomputed mask AND per pawn.
#[cfg(not(feature = "advanced-eval"))]
fn alpha_passed(board: &Board, color: Color, scale: i32) -> i32 {
    if scale == 0 {
        return 0;
    }
    let enemy_pawns = board.pieces[color.other().idx() * 6];
    let mut pawns = board.pieces[color.idx() * 6];
    let mut s = 0;
    while pawns != 0 {
        let (sq, rest) = pop_lsb(pawns);
        pawns = rest;
        if enemy_pawns & passed_pawn_mask(sq, color) == 0 {
            let rank = match color {
                Color::White => rank_of(sq),
                Color::Black => 7 - rank_of(sq),
            };
            s += PASSED_BONUS[rank as usize] * scale / 100;
        }
    }
    s
}

/// True if `color`'s king stands on a castled square (g1/c1, g8/c8).
#[cfg(not(feature = "advanced-eval"))]
fn king_camped(board: &Board, color: Color) -> bool {
    match color {
        Color::White => matches!(board.king_sq[0], 6 | 2),
        Color::Black => matches!(board.king_sq[1], 62 | 58),
    }
}

/// Number of attacks by the enemy's pawns/knights/bishops/rooks/queens on
/// the squares around `color`'s king (king square included).
#[cfg(not(feature = "advanced-eval"))]
fn king_ring_attacks(board: &Board, color: Color) -> i32 {
    let ksq = board.king_sq[color.idx()];
    let ring = king_attacks(ksq) | bit(ksq);
    let them = color.other();
    let base = them.idx() * 6;
    let occ = board.occupied;
    let mut count = 0;
    let mut pawns = board.pieces[base];
    while pawns != 0 {
        let (sq, rest) = pop_lsb(pawns);
        pawns = rest;
        count += popcount(pawn_attacks(sq, them) & ring) as i32;
    }
    let mut knights = board.pieces[base + 1];
    while knights != 0 {
        let (sq, rest) = pop_lsb(knights);
        knights = rest;
        count += popcount(knight_attacks(sq) & ring) as i32;
    }
    let mut diag = board.pieces[base + 2] | board.pieces[base + 4];
    while diag != 0 {
        let (sq, rest) = pop_lsb(diag);
        diag = rest;
        count += popcount(bishop_attacks(sq, occ) & ring) as i32;
    }
    let mut straight = board.pieces[base + 3] | board.pieces[base + 4];
    while straight != 0 {
        let (sq, rest) = pop_lsb(straight);
        straight = rest;
        // A queen is counted once on the diagonal side and once here; that
        // over-counts a queen's ring attacks, which is intentional: the
        // queen is the piece that matters for king danger.
        count += popcount(rook_attacks(sq, occ) & ring) as i32;
    }
    count
}

/// Material + PST for one piece: `(MG_VALUE+PST_MG, EG_VALUE+PST_EG)`.
/// Used by `Board` to maintain its incremental scores.
#[cfg(feature = "advanced-eval")]
pub(crate) fn material_pst(color: Color, kind_idx: usize, sq: u8) -> (i32, i32) {
    (
        MG_VALUE[kind_idx] + PST_MG[kind_idx][pst_index(sq, color)],
        EG_VALUE[kind_idx] + PST_EG[kind_idx][pst_index(sq, color)],
    )
}

/// Material + tapered PSTs for `color`, before positional bonuses.
/// Returns `(mg, eg, phase_units)`; phase_units counts N/B=1, R=2, Q=4.
/// Kept for verification: incremental scores must match this recomputation.
#[cfg(all(feature = "advanced-eval", test))]
fn tapered_material(board: &Board, color: Color) -> (i32, i32, i32) {
    let mut mg = 0i32;
    let mut eg = 0i32;
    let mut phase_units = 0i32;

    // Walk only occupied squares via the piece bitboards (32 squares max)
    // instead of scanning all 64 mailbox entries twice per evaluate.
    for k in 0..6 {
        let mut bb = board.pieces[color.idx() * 6 + k];
        while bb != 0 {
            let (sq, rest) = pop_lsb(bb);
            bb = rest;
            mg += MG_VALUE[k] + PST_MG[k][pst_index(sq, color)];
            eg += EG_VALUE[k] + PST_EG[k][pst_index(sq, color)];
            phase_units += match k {
                1 | 2 => 1, // knight, bishop
                3 => 2,     // rook
                4 => 4,     // queen
                _ => 0,
            };
        }
    }
    (mg, eg, phase_units)
}

/// Interpolate MG/EG scores by phase: opening weight = phase (0..=256),
/// endgame weight = 256 - phase.
#[cfg(feature = "advanced-eval")]
fn taper(mg: i32, eg: i32, phase_units: i32) -> i32 {
    let phase = ((phase_units * 256) / 24).clamp(0, 256);
    (mg * phase + eg * (256 - phase)) / 256
}

#[cfg(feature = "advanced-eval")]
fn evaluate_color(board: &Board, color: Color, pressure: &Pressure, p: &EvalParams) -> i32 {
    // Base material+PST is maintained incrementally by `Board` (see
    // `put_piece`/`remove_from`), so this is O(1) instead of a 32-piece
    // loop. Phase comes from popcounts (no square walk).
    let mut mg = board.mg_scores[color.idx()];
    let mut eg = board.eg_scores[color.idx()];
    let base = color.idx() * 6;
    let phase_units = (popcount(board.pieces[base + 1]) + popcount(board.pieces[base + 2])) as i32
        + (popcount(board.pieces[base + 3]) as i32) * 2
        + (popcount(board.pieces[base + 4]) as i32) * 4;
    let bishop_count = popcount(board.pieces[color.idx() * 6 + 2]) as i32;

    // --- positional features (applied evenly to the MG score; they taper
    // naturally because the endgame score dominates late).
    let mut bonus = 0i32;
    bonus += pressure.mobility[color.idx()];
    bonus += pawn_structure(board, color, p);
    bonus += rook_files(board, color, p);
    bonus += king_safety(board, color, p);
    // Gamma "cunning/trapping" playstyle: loose enemy material is prey.
    let enemy_loose = pressure.loose[color.other().idx()];
    bonus += enemy_loose * p.pressure_scale / 100;
    // Trap terms: pieces and kings with no escape squares.
    if p.trap_bonus != 0 {
        bonus += pressure.trapped[color.other().idx()] * p.trap_bonus;
    }
    if p.king_pressure != 0 {
        bonus += pressure.king_ring[color.idx()] * p.king_pressure;
    }
    if p.king_danger != 0 {
        bonus -= pressure.king_ring[color.other().idx()] * p.king_danger;
    }
    if bishop_count >= 2 {
        bonus += p.bishop_pair;
    }
    mg += bonus;
    eg += bonus / 2;

    taper(mg, eg, phase_units)
}

fn color_bitboard(board: &Board, color: Color) -> Bitboard {
    let base = color.idx() * 6;
    board.pieces[base]
        | board.pieces[base + 1]
        | board.pieces[base + 2]
        | board.pieces[base + 3]
        | board.pieces[base + 4]
        | board.pieces[base + 5]
}

#[cfg(feature = "advanced-eval")]
fn pawn_structure(board: &Board, color: Color, p: &EvalParams) -> i32 {
    let pawns = board.pieces[color.idx() * 6];
    let enemy_pawns = board.pieces[color.other().idx() * 6];
    let mut penalty = 0i32;
    // Passed-pawn bonus by rank (from this colour's point of view).
    // File masks let doubled/isolated be answered with two popcounts
    // per pawn instead of nested loops over all pawns.
    let mut file_count = [0u32; 8];
    let mut file_mask = [0u64; 8];
    {
        let mut b = pawns;
        while b != 0 {
            let (sq, rest) = pop_lsb(b);
            b = rest;
            let f = file_of(sq) as usize;
            file_count[f] += 1;
            file_mask[f] |= bit(sq);
        }
    }
    // Pawn chain (one bitboard op per feature, no per-pawn loop):
    // - connected: a pawn defended by another own pawn (supports the chain;
    //   the defending-pawn square is found by shifting the pawn bitboard
    //   one diagonal step toward the target colour, masking the a/h files
    //   so shifts do not wrap around the board edge).
    // - blocked: a pawn with an own pawn directly in front (it cannot move,
    //   so it is a small liability). Shifting 8 keeps the file, so no mask
    //   is needed there.
    const FILE_A: Bitboard = 0x0101_0101_0101_0101;
    const FILE_H: Bitboard = 0x8080_8080_8080_8080;
    let defended = match color {
        Color::White => ((pawns & !FILE_A) << 7) | ((pawns & !FILE_H) << 9),
        Color::Black => ((pawns & !FILE_H) >> 7) | ((pawns & !FILE_A) >> 9),
    };
    penalty -= (popcount(pawns & defended) as i32) * p.connected;
    let blocked = match color {
        Color::White => pawns & (pawns << 8),
        Color::Black => pawns & (pawns >> 8),
    };
    penalty += (popcount(blocked) as i32) * p.blocked;
    let mut b = pawns;
    while b != 0 {
        let (sq, rest) = pop_lsb(b);
        b = rest;
        let f = file_of(sq) as usize;
        let rank = rank_of(sq) as i32;
        // Doubled: count via the file total (includes this pawn).
        let same_file = file_count[f].saturating_sub(1) as i32;
        if same_file > 0 {
            penalty += p.doubled * same_file;
        }
        // Isolated: check neighbouring file masks instead of looping.
        let has_neighbour = (f > 0 && file_mask[f - 1] != 0)
            || (f < 7 && file_mask[f + 1] != 0);
        if !has_neighbour {
            penalty += p.isolated;
        }
        // Passed: no enemy pawn in front on same/adjacent files.
        // Bitboard test against the precomputed passed mask (same score
        // as scanning enemy pawns, but one AND instead of a loop).
        let rank_from_own = match color {
            Color::White => rank,
            Color::Black => 7 - rank,
        };
        if enemy_pawns & passed_pawn_mask(sq, color) == 0 {
            penalty -= PASSED_BONUS[rank_from_own as usize] * p.passed_scale / 100;
        }
    }
    // Desired bonus = passed - doubled - isolated = -(doubled + isolated - passed).
    -penalty
}

#[cfg(feature = "advanced-eval")]
fn rook_files(board: &Board, color: Color, p: &EvalParams) -> i32 {
    let pawns_white = board.pieces[0];
    let pawns_black = board.pieces[6];
    let mut bonus = 0i32;
    let mut rooks = board.pieces[color.idx() * 6 + 3];
    while rooks != 0 {
        let (sq, rest) = pop_lsb(rooks);
        rooks = rest;
        let f = file_of(sq) as usize;
        let file_mask: Bitboard = 0x0101_0101_0101_0101u64 << f;
        let any = pawns_white | pawns_black;
        let own_pawns = match color {
            Color::White => pawns_white,
            Color::Black => pawns_black,
        };
        if any & file_mask == 0 {
            bonus += p.rook_open; // open file
        } else if own_pawns & file_mask == 0 {
            bonus += p.rook_semi; // semi-open file
        }
        if p.rook7 != 0 {
            let seventh = match color {
                Color::White => 6,
                Color::Black => 1,
            };
            if rank_of(sq) as i32 == seventh {
                bonus += p.rook7;
            }
        }
    }
    bonus
}

#[cfg(feature = "advanced-eval")]
fn king_safety(board: &Board, color: Color, p: &EvalParams) -> i32 {
    let shield = pawn_shield(board, color);
    // Up to a full-shield cap; missing pawns scale the penalty.
    // (Enemy-attack penalties were tried and measured to widen nodes
    // +83% at d8 and lose Elo; the shield alone is kept. Castling
    // bonus +20 was neutral at 200ms (2–17–1, same as baseline), reverted.)
    (shield * p.shield_pawn).min(p.shield_cap) - (3 - shield.min(3)) * p.shield_missing
}

/// Number of own pawns on the three squares directly in front of `color`'s
/// king (the pawn shield). Shared by the Alpha and Gamma king-safety terms.
fn pawn_shield(board: &Board, color: Color) -> i32 {
    let ksq = board.king_sq[color.idx()];
    let kfile = file_of(ksq) as i32;
    let krank = rank_of(ksq) as i32;
    let pawns = board.pieces[color.idx() * 6];
    let dir: i32 = match color {
        Color::White => 1,
        Color::Black => -1,
    };
    let mut shield = 0;
    for df in [-1, 0, 1] {
        let f = kfile + df;
        let r = krank + dir;
        if (0..8).contains(&f) && (0..8).contains(&r) {
            let sq = (r * 8 + f) as u8;
            if pawns & bit(sq) != 0 {
                shield += 1;
            }
        }
    }
    shield
}

/// Alpha "defensive" playstyle: bonus for pawns shielding your own king.
#[cfg(not(feature = "advanced-eval"))]
fn shield_bonus(board: &Board, color: Color, per_pawn: i32) -> i32 {
    pawn_shield(board, color) * per_pawn
}

/// Value of a loose piece, by kind (index matches `PieceType`), in
/// centipawns. Pawns and kings are not part of the term.
const LOOSE_VALUE: [i32; 6] = [0, 320, 330, 500, 900, 0];

/// Result of the single pressure pass shared by both colours.
#[derive(Clone, Copy, Default)]
struct Pressure {
    /// `loose[owner]` = cp value of `owner`'s hanging N/B/R/Q.
    loose: [i32; 2],
    /// `trapped[owner]` = number of `owner`'s N/B/R with ≤1 safe square.
    trapped: [i32; 2],
    /// `king_ring[attacker]` = attacker's pieces hitting the enemy king ring.
    king_ring: [i32; 2],
    /// Weighted mobility per colour, accumulated from the same attack sets
    /// the scan already computes (avoids a second pass in `mobility`).
    mobility: [i32; 2],
}

impl Pressure {
    /// Scan every N/B/R/Q once, plus pawns/king only when the corresponding
    /// style weight is active. Zero-weight style terms cost nothing.
    fn scan(board: &Board, p: &EvalParams) -> Pressure {
        let mut out = Pressure::default();
        let need_loose = p.pressure_scale != 0 || p.alpha_own_loose != 0;
        let need_trapped = p.trap_bonus != 0;
        let need_ring = p.king_pressure != 0 || p.king_danger != 0;
        let need_mobility = p.mob_knight != 0
            || p.mob_bishop != 0
            || p.mob_rook != 0
            || p.mob_queen != 0;
        if !(need_loose || need_trapped || need_ring || need_mobility) {
            return out;
        }
        let occ = board.occupied;
        for owner in [Color::White, Color::Black] {
            let attacker = owner.other();
            let ab = attacker.idx() * 6;
            let ob = owner.idx() * 6;
            let own_bb = color_bitboard(board, owner);
            let ring = if need_ring {
                let ksq = board.king_sq[owner.idx()];
                king_attacks(ksq) | bit(ksq)
            } else {
                0
            };
            let a_pawns_bb = board.pieces[ab];
            let a_knights_bb = board.pieces[ab + 1];
            let a_bishops_bb = board.pieces[ab + 2];
            let a_rooks_bb = board.pieces[ab + 3];
            let a_queens_bb = board.pieces[ab + 4];
            let o_pawns_bb = board.pieces[ob];
            let o_knights_bb = board.pieces[ob + 1];
            let o_bishops_bb = board.pieces[ob + 2];
            let o_rooks_bb = board.pieces[ob + 3];
            let o_queens_bb = board.pieces[ob + 4];

            if need_ring {
                // Pawn and king pressure are cheap and complete the ring.
                let mut pawns = a_pawns_bb;
                while pawns != 0 {
                    let (sq, rest) = pop_lsb(pawns);
                    pawns = rest;
                    out.king_ring[attacker.idx()] +=
                        popcount(pawn_attacks(sq, attacker) & ring) as i32;
                }
                let mut kings = board.pieces[ab + 5];
                while kings != 0 {
                    let (sq, rest) = pop_lsb(kings);
                    kings = rest;
                    out.king_ring[attacker.idx()] +=
                        popcount(king_attacks(sq) & ring) as i32;
                }
            }

            for (k, &value) in LOOSE_VALUE.iter().enumerate().skip(1).take(4) {
                let mut bb = board.pieces[ob + k];
                while bb != 0 {
                    let (sq, rest) = pop_lsb(bb);
                    bb = rest;
                    // Piece attacks, computed once and reused by all terms.
                    let attacks = match k {
                        1 => knight_attacks(sq),
                        2 => bishop_attacks(sq, occ),
                        3 => rook_attacks(sq, occ),
                        _ => queen_attacks(sq, occ),
                    };
                    let safe_squares = popcount(attacks & !own_bb) as i32;
                    let mob_w = match k {
                        1 => p.mob_knight,
                        2 => p.mob_bishop,
                        3 => p.mob_rook,
                        _ => p.mob_queen,
                    };
                    if mob_w != 0 {
                        out.mobility[owner.idx()] += safe_squares * mob_w;
                    }
                    if need_trapped && k <= 3 && safe_squares <= 1 {
                        out.trapped[owner.idx()] += 1;
                    }
                    if need_ring {
                        out.king_ring[attacker.idx()] +=
                            popcount(attacks & ring) as i32;
                    }
                    if !need_loose {
                        continue;
                    }
                    // Attacker's attacks on the target square, computed once
                    // and reused for the count and the cheap-attacker test.
                    let a_pawns = pawn_attackers_to(sq, attacker) & a_pawns_bb;
                    let a_knights = knight_attacks(sq) & a_knights_bb;
                    let a_diag = bishop_attacks(sq, occ) & (a_bishops_bb | a_queens_bb);
                    let a_straight = rook_attacks(sq, occ) & (a_rooks_bb | a_queens_bb);
                    let a_king = king_attacks(sq) & board.pieces[ab + 5];
                    let attackers = popcount(a_pawns | a_knights | a_diag | a_straight | a_king) as i32;
                    if attackers == 0 {
                        continue;
                    }
                    // Count defenders, stopping as soon as they match the
                    // attackers (only "attacked more than defended" matters).
                    let mut defenders = popcount(pawn_attackers_to(sq, owner) & o_pawns_bb) as i32;
                    if defenders < attackers {
                        defenders += popcount(knight_attacks(sq) & o_knights_bb) as i32;
                    }
                    if defenders < attackers {
                        defenders += popcount(king_attacks(sq) & board.pieces[ob + 5]) as i32;
                    }
                    if defenders < attackers {
                        defenders += popcount(bishop_attacks(sq, occ) & (o_bishops_bb | o_queens_bb)) as i32;
                    }
                    if defenders < attackers {
                        defenders += popcount(rook_attacks(sq, occ) & (o_rooks_bb | o_queens_bb)) as i32;
                    }
                    if attackers <= defenders {
                        continue;
                    }
                    // Only a piece the attacker can profitably take is loose:
                    // undefended, or attacked by something no more valuable.
                    let cheap = if defenders == 0 {
                        1
                    } else {
                        let mut c = popcount(a_pawns) as i32;
                        if value >= 320 {
                            c += popcount(a_knights) as i32;
                        }
                        if value >= 330 {
                            c += popcount(a_diag) as i32;
                        }
                        if value >= 500 {
                            c += popcount(a_straight) as i32;
                        }
                        c
                    };
                    if cheap > 0 {
                        out.loose[owner.idx()] += value;
                    }
                }
            }
        }
        out
    }
}

/// Loose material per *owner*: `loose[owner]` is the value of `owner`'s
/// N/B/R/Q that the opponent attacks more often than the owner defends
/// them, and that at least one cheap-enough attacker can profit from.
/// Pins are not modelled — this is a static heuristic, not a full exchange
/// evaluation. Used by the Alpha style term and the Gamma unit tests.
#[cfg(any(not(feature = "advanced-eval"), test))]
fn loose_material_by_owner(board: &Board) -> [i32; 2] {
    Pressure::scan(
        board,
        &EvalParams {
            pressure_scale: 8,
            alpha_own_loose: 1,
            ..EvalParams::DEFAULT
        },
    )
    .loose
}

/// Playstyle metrics for one side, used by the training harnesses to
/// measure whether a variant is actually playing the way it is supposed to.
#[derive(Clone, Copy, Debug, Default)]
pub struct StyleStats {
    /// cp value of this side's own hanging N/B/R/Q (lower = tighter play).
    pub own_loose: i32,
    /// Number of this side's N/B/R with at most one safe square.
    pub own_trapped: i32,
    /// Enemy attacks on this side's king ring (lower = safer king).
    pub king_ring_against: i32,
    /// Our attacks on the enemy king ring (higher = more pressure).
    pub king_ring_for: i32,
    /// Number of enemy N/B/R with at most one safe square.
    pub enemy_trapped: i32,
    /// cp value of the enemy's hanging N/B/R/Q (higher = more pressure).
    pub enemy_loose: i32,
}

/// Compute the playstyle metrics for `color` from the current position.
pub fn style_stats(board: &Board, color: Color) -> StyleStats {
    let p = Pressure::scan(
        board,
        &EvalParams {
            pressure_scale: 8,
            trap_bonus: 1,
            king_pressure: 1,
            alpha_own_loose: 1,
            ..EvalParams::DEFAULT
        },
    );
    let us = color.idx();
    let them = color.other().idx();
    StyleStats {
        own_loose: p.loose[us],
        own_trapped: p.trapped[us],
        king_ring_against: p.king_ring[them],
        king_ring_for: p.king_ring[us],
        enemy_trapped: p.trapped[them],
        enemy_loose: p.loose[them],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::Board;

    #[test]
    fn startpos_is_balanced() {
        let b = Board::startpos();
        let s = evaluate(&b);
        assert!(s.abs() < 50, "startpos eval {s} should be near 0");
    }

    #[test]
    fn extra_queen_wins() {
        // White is up a queen (Black queen missing from d8); eval from
        // White to move must be large positive.
        let b = Board::from_fen("r1b1kbnr/pppp1ppp/8/4p3/4P3/8/PPPP1PPP/RNBQKBNR w KQkq - 0 1").unwrap();
        let s = evaluate(&b);
        assert!(s > 200, "got {s}");
        // Same position with Black to move must be (roughly) negated.
        let mut b2 = b.clone();
        b2.side = Color::Black;
        let s2 = evaluate(&b2);
        assert!((s + s2).abs() < 200, "s={s} s2={s2}");
    }

    #[test]
    fn symmetry_consistency() {
        // Kiwipete: evaluation must be deterministic.
        let b = Board::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1").unwrap();
        let a = evaluate(&b);
        let c = evaluate(&b);
        assert_eq!(a, c);
    }

    #[test]
    fn default_params_are_the_baseline() {
        let b = Board::from_fen("r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1").unwrap();
        assert_eq!(evaluate(&b), evaluate_with(&b, &EvalParams::DEFAULT));
        // Round-trip through a spec string preserves every weight.
        let spec = EvalParams::DEFAULT.to_spec();
        assert_eq!(EvalParams::from_spec(&spec).unwrap(), EvalParams::DEFAULT);
    }

    #[test]
    fn param_spec_parsing() {
        let p = EvalParams::from_spec("pressure_scale=12, trap_bonus=7;alpha_shield=11").unwrap();
        assert_eq!(p.pressure_scale, 12);
        assert_eq!(p.trap_bonus, 7);
        assert_eq!(p.alpha_shield, 11);
        // Untouched fields keep the baseline.
        assert_eq!(p.mob_knight, EvalParams::DEFAULT.mob_knight);
        assert!(EvalParams::from_spec("nonsense=3").is_err());
        assert!(EvalParams::from_spec("pressure_scale=x").is_err());
        assert!(EvalParams::from_spec("").is_ok());
    }

    #[cfg(feature = "advanced-eval")]
    #[test]
    fn weight_changes_move_the_eval() {
        // Castled White king with a full shield vs a bare Black king.
        let b = Board::from_fen("r3k2r/8/8/8/8/8/3PPP2/R3K2R w KQkq - 0 1").unwrap();
        let base = evaluate_with(&b, &EvalParams::DEFAULT);
        let p = EvalParams {
            shield_pawn: 40,
            shield_missing: 40,
            ..EvalParams::DEFAULT
        };
        assert_ne!(base, evaluate_with(&b, &p));
    }

    #[cfg(not(feature = "advanced-eval"))]
    #[test]
    fn alpha_defensive_weight_changes_the_eval() {
        // Black rook down the d-file attacks the undefended White knight on
        // d1: the own-loose penalty must change White's score.
        let b = Board::from_fen("3rk3/8/8/8/8/8/8/K2N4 b - - 0 1").unwrap();
        let base = evaluate_with(&b, &EvalParams::DEFAULT);
        let p = EvalParams {
            alpha_own_loose: 50,
            ..EvalParams::DEFAULT
        };
        assert_ne!(base, evaluate_with(&b, &p));
    }

    // The loose-material scan is Gamma-only (advanced eval), so this
    // test only exists in the Gamma build.
    #[cfg(feature = "advanced-eval")]
    #[test]
    fn loose_material_scan_works() {
        let loose = |b: &Board, c: Color| loose_material_by_owner(b)[c.idx()];
        // A black rook on e8 (down the file) and one on a4 (along the rank).
        // Both attack the white knight on e4; nothing defends it, so the
        // knight is loose.
        let b = Board::from_fen("4r3/8/8/8/r3N3/8/8/K7 b - - 0 1").unwrap();
        assert_eq!(loose(&b, Color::White), 320); // hanging even to rooks
        assert_eq!(loose(&b, Color::Black), 0);
        // A white pawn on d3 defends the knight: two attackers against one
        // defender is NOT a free win, and the cheap attackers outside the
        // trade are rooks (value 500 > 320) — so not loose.
        let b2 = Board::from_fen("4r3/8/8/8/r3N3/3P4/8/K7 b - - 0 1").unwrap();
        assert_eq!(loose(&b2, Color::White), 0);
        // Two black knights (c5 and f6) attacking the knight, one white pawn
        // defending: cheap attackers (320 ≤ 320) outnumber the defender — loose.
        let b3 = Board::from_fen("8/8/5n2/2n5/4N3/3P4/8/K7 b - - 0 1").unwrap();
        assert_eq!(loose(&b3, Color::White), 320);
        // The mirrored position tells the same story for Black: rooks on
        // a5/e1 attack the black knight on e5, the d6 pawn defends it, and
        // the rooks are too valuable to be cheap attackers — not loose.
        let b4 = Board::from_fen("k7/8/3p4/R7/8/8/8/4R2K w - - 0 1").unwrap();
        assert_eq!(loose(&b4, Color::Black), 0);
        assert_eq!(loose(&b4, Color::White), 0);
    }

    #[cfg(feature = "advanced-eval")]
    #[test]
    fn trapped_piece_term_works() {
        // Black knight on a8 boxed in by its own pawns b7/c7: only b6 is a
        // safe square, so the trap term must reward White for it.
        let b = Board::from_fen("n1k5/1pp5/8/8/8/8/8/K7 w - - 0 1").unwrap();
        let p = EvalParams {
            trap_bonus: 25,
            ..EvalParams::DEFAULT
        };
        let base = evaluate_with(&b, &EvalParams::DEFAULT);
        let trapped = evaluate_with(&b, &p);
        // White should score the trapped black knight higher than baseline.
        assert!(trapped > base, "base={base} trapped={trapped}");
    }

    #[cfg(feature = "advanced-eval")]
    #[test]
    fn incremental_scores_match_recomputed() {
        // Incremental Board scores must equal the loop recomputation
        // after every make/unmake (promotion, capture, castling included).
        use crate::types::Color;
        let mut b = Board::startpos();
        for color in [Color::White, Color::Black] {
            let (mg, eg, _) = tapered_material(&b, color);
            assert_eq!(mg, b.mg_scores[color.idx()]);
            assert_eq!(eg, b.eg_scores[color.idx()]);
        }
        let moves = b.generate_legal_moves();
        for m in moves.iter() {
            b.make_move(m);
            for color in [Color::White, Color::Black] {
                let (mg, eg, _) = tapered_material(&b, color);
                assert_eq!(mg, b.mg_scores[color.idx()]);
                assert_eq!(eg, b.eg_scores[color.idx()]);
            }
            b.unmake_move();
        }
    }
}

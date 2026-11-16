//! Board state, move generation, and make/unmake.
//!
//! The board stores both bitboards (for move generation and attack tests)
//! and a mailbox array (for fast piece lookups and evaluation). Every move
//! is applied with [`Board::make_move`] and undone with [`Board::unmake_move`]
//! (LIFO undo stack). The Zobrist hash and (Gamma, `advanced-eval`) the
//! tapered material+PST scores are maintained incrementally on every
//! make/unmake, so evaluation is O(1) for material instead of a
//! 32-piece loop. Incremental scores are verified against recomputation
//! in `eval::tests::incremental_scores_match_recomputed`.

use crate::bitboard::*;
use crate::types::{Color, Move, MoveList, Piece, PieceType};
use std::sync::OnceLock;

// ----------------------------------------------------------------- zobrist

struct Zobrist {
    piece: [[u64; 64]; 12],
    side: u64,
    castling: [u64; 16],
    ep_file: [u64; 8],
}

static ZOBRIST: OnceLock<Zobrist> = OnceLock::new();

fn zobrist() -> &'static Zobrist {
    ZOBRIST.get_or_init(|| {
        // Fixed seed so hashes are deterministic across runs.
        let mut state: u64 = 0x9E3779B97F4A7C15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut piece = [[0u64; 64]; 12];
        for p in piece.iter_mut() {
            for sq in p.iter_mut() {
                *sq = next();
            }
        }
        let side = next();
        let mut castling = [0u64; 16];
        for c in castling.iter_mut() {
            *c = next();
        }
        let mut ep_file = [0u64; 8];
        for e in ep_file.iter_mut() {
            *e = next();
        }
        Zobrist { piece, side, castling, ep_file }
    })
}

// ------------------------------------------------------------------ board

/// Castling-rights bit flags.
pub const CASTLE_WK: u8 = 1;
pub const CASTLE_WQ: u8 = 2;
pub const CASTLE_BK: u8 = 4;
pub const CASTLE_BQ: u8 = 8;

pub const START_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Clone, Debug)]
pub struct Board {
    /// 12 bitboards: index = colour.idx()*6 + kind.idx().
    pub pieces: [Bitboard; 12],
    /// Mailbox: `mailbox[sq]` is the piece on `sq`, or `None`.
    pub mailbox: [Option<Piece>; 64],
    pub occupied: Bitboard,
    pub side: Color,
    pub castling: u8,
    /// En passant target square (the square a double push jumped over).
    pub ep_square: Option<u8>,
    pub halfmove: u32,
    pub fullmove: u32,
    pub hash: u64,
    /// Undo stack (one entry per make call, cleared by FEN load).
    pub undo_stack: Vec<Undo>,
    /// King squares (cached, updated on king moves).
    pub king_sq: [u8; 2],
    /// Incremental tapered material+PST scores (Gamma speed feature,
    /// maintained by `put_piece`/`remove_from` like the Zobrist hash).
    /// `mg_scores[color.idx()]` / `eg_scores[color.idx()]` are the sums
    /// of `MG_VALUE+PST_MG` / `EG_VALUE+PST_EG` for that colour's pieces.
    #[cfg(feature = "advanced-eval")]
    pub mg_scores: [i32; 2],
    #[cfg(feature = "advanced-eval")]
    pub eg_scores: [i32; 2],
}

#[derive(Clone, Copy, Debug)]
pub struct Undo {
    pub captured: Option<Piece>,
    pub castling: u8,
    pub ep_square: Option<u8>,
    pub halfmove: u32,
    pub hash: u64,
    pub moved_piece: Piece,
    pub flags: Move,
    /// Incremental material+PST scores before the move (Gamma only), so
    /// unmake can restore them instead of recomputing every PST lookup.
    #[cfg(feature = "advanced-eval")]
    pub mg: [i32; 2],
    #[cfg(feature = "advanced-eval")]
    pub eg: [i32; 2],
}

impl Board {
    /// Empty board (no pieces). Prefer [`Board::startpos`] or `from_fen`.
    pub fn empty() -> Board {
        Board {
            pieces: [0; 12],
            mailbox: [None; 64],
            occupied: 0,
            side: Color::White,
            castling: 0,
            ep_square: None,
            halfmove: 0,
            fullmove: 1,
            hash: 0,
            undo_stack: Vec::with_capacity(128),
            king_sq: [0, 0],
            #[cfg(feature = "advanced-eval")]
            mg_scores: [0, 0],
            #[cfg(feature = "advanced-eval")]
            eg_scores: [0, 0],
        }
    }

    pub fn startpos() -> Board {
        Board::from_fen(START_FEN).expect("startpos FEN must parse")
    }

    fn slot(color: Color, kind: PieceType) -> usize {
        color.idx() * 6 + kind.idx()
    }

    /// `(from, to)` squares for the castling rook: kingside and queenside
    /// for both colours (a1=0, h1=7, a8=56, h8=63).
    fn castle_rook_squares(color: Color, kingside: bool) -> (u8, u8) {
        match (color, kingside) {
            (Color::White, true) => (7u8, 5u8),   // h1 -> f1
            (Color::White, false) => (0u8, 3u8),   // a1 -> d1
            (Color::Black, true) => (63u8, 61u8), // h8 -> f8
            (Color::Black, false) => (56u8, 59u8), // a8 -> d8
        }
    }

    fn put_piece(&mut self, sq: u8, piece: Piece) {
        self.put_piece_impl(sq, piece, true);
    }

    /// `put_piece` without touching the incremental scores (used by unmake,
    /// which restores them from the undo entry).
    fn put_piece_restore(&mut self, sq: u8, piece: Piece) {
        self.put_piece_impl(sq, piece, false);
    }

    fn put_piece_impl(&mut self, sq: u8, piece: Piece, scores: bool) {
        #[cfg(not(feature = "advanced-eval"))]
        let _ = scores;
        let i = Self::slot(piece.color, piece.kind);
        self.pieces[i] |= bit(sq);
        self.occupied |= bit(sq);
        self.mailbox[sq as usize] = Some(piece);
        if piece.kind == PieceType::King {
            self.king_sq[piece.color.idx()] = sq;
        }
        #[cfg(feature = "advanced-eval")]
        if scores {
            let (mg, eg) = crate::eval::material_pst(piece.color, piece.kind.idx(), sq);
            self.mg_scores[piece.color.idx()] += mg;
            self.eg_scores[piece.color.idx()] += eg;
        }
    }

    pub fn piece_on(&self, sq: u8) -> Option<Piece> {
        self.mailbox[sq as usize]
    }

    /// Current side's attacking check test: is `color`'s king under attack?
    pub fn is_in_check(&self, color: Color) -> bool {
        let ksq = self.king_sq[color.idx()];
        is_attacked(ksq, color.other(), &self.pieces, self.occupied)
    }

    /// Squares attacked by `attacker` — a helper for castling legality.
    pub fn is_square_attacked(&self, sq: u8, attacker: Color) -> bool {
        is_attacked(sq, attacker, &self.pieces, self.occupied)
    }

    pub fn compute_hash(&self) -> u64 {
        let z = zobrist();
        let mut h = 0u64;
        for sq in 0..64u8 {
            if let Some(p) = self.mailbox[sq as usize] {
                h ^= z.piece[Self::slot(p.color, p.kind)][sq as usize];
            }
        }
        if self.side == Color::Black {
            h ^= z.side;
        }
        h ^= z.castling[self.castling as usize];
        if let Some(ep) = self.ep_square {
            h ^= z.ep_file[file_of(ep) as usize];
        }
        h
    }

    /// Hash of the position after a null move (side flipped, en passant
    /// cleared), computed incrementally. Search uses this instead of the
    /// O(64) [`Board::compute_hash`] in every null-move node.
    pub fn null_move_hash(&self) -> u64 {
        let z = zobrist();
        let mut h = self.hash;
        if let Some(ep) = self.ep_square {
            h ^= z.ep_file[file_of(ep) as usize];
        }
        h ^ z.side
    }

    // ------------------------------------------------------------ FEN I/O

    pub fn from_fen(fen: &str) -> Result<Board, String> {
        Self::parse_fen(fen, false)
    }

    /// Parse and *validate* a FEN for untrusted input (UCI clients, bots).
    /// Rejects missing/duplicate kings, pawns on back ranks and adjacent
    /// kings in addition to the syntax checks in [`Board::from_fen`].
    pub fn from_fen_strict(fen: &str) -> Result<Board, String> {
        Self::parse_fen(fen, true)
    }

    fn parse_fen(fen: &str, strict: bool) -> Result<Board, String> {
        let mut b = Board::empty();
        let parts: Vec<&str> = fen.split_whitespace().collect();
        if parts.len() < 4 {
            return Err("FEN needs at least 4 fields".into());
        }
        // Piece placement.
        let mut sq: u8;
        let rows: Vec<&str> = parts[0].split('/').collect();
        if rows.len() != 8 {
            return Err("FEN board must have 8 ranks".into());
        }
        for (pi, row) in rows.iter().enumerate() {
            sq = (7 - pi as u8) * 8;
            let mut files = 0u8;
            for ch in row.chars() {
                if ch.is_ascii_digit() {
                    let n = ch as u8 - b'0';
                    sq += n;
                    files += n;
                } else {
                    files += 1;
                    let color = if ch.is_uppercase() { Color::White } else { Color::Black };
                    let kind = match ch.to_ascii_lowercase() {
                        'p' => PieceType::Pawn,
                        'n' => PieceType::Knight,
                        'b' => PieceType::Bishop,
                        'r' => PieceType::Rook,
                        'q' => PieceType::Queen,
                        'k' => PieceType::King,
                        _ => return Err(format!("bad FEN piece '{ch}'")),
                    };
                    if sq > 63 {
                        return Err("FEN overflow".into());
                    }
                    b.put_piece(sq, Piece { kind, color });
                    sq += 1;
                }
            }
            if files != 8 {
                return Err(format!("FEN rank {} has {files} files, expected 8", 8 - pi));
            }
        }
        // Side to move.
        b.side = match parts[1] {
            "w" => Color::White,
            "b" => Color::Black,
            _ => return Err("bad FEN side field".into()),
        };
        // Castling.
        b.castling = 0;
        if parts[2] != "-" {
            for ch in parts[2].chars() {
                match ch {
                    'K' => b.castling |= CASTLE_WK,
                    'Q' => b.castling |= CASTLE_WQ,
                    'k' => b.castling |= CASTLE_BK,
                    'q' => b.castling |= CASTLE_BQ,
                    _ => return Err("bad FEN castling field".into()),
                }
            }
        }
        // En passant.
        b.ep_square = if parts[3] == "-" {
            None
        } else {
            Some(crate::types::parse_square(parts[3]).ok_or("bad FEN ep square")?)
        };
        b.halfmove = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
        b.fullmove = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(1);
        if strict {
            // Servers receive FENs from untrusted clients: reject impossible
            // positions instead of searching with a default `king_sq`.
            let white_kings = b.pieces[5].count_ones();
            let black_kings = b.pieces[11].count_ones();
            if white_kings != 1 || black_kings != 1 {
                return Err(format!(
                    "FEN must have exactly one king per side (white {white_kings}, black {black_kings})"
                ));
            }
            const RANK_1: u64 = 0x0000_0000_0000_00FF;
            const RANK_8: u64 = 0xFF00_0000_0000_0000;
            if (b.pieces[0] | b.pieces[6]) & (RANK_1 | RANK_8) != 0 {
                return Err("FEN has a pawn on the first or last rank".into());
            }
            if king_attacks(b.king_sq[0]) & bit(b.king_sq[1]) != 0 {
                return Err("FEN has adjacent kings".into());
            }
        }
        b.undo_stack.clear();
        b.hash = b.compute_hash();
        Ok(b)
    }

    pub fn to_fen(&self) -> String {
        let mut s = String::new();
        for rank in (0..8u8).rev() {
            let mut empty = 0;
            for file in 0..8u8 {
                let sq = rank * 8 + file;
                match self.mailbox[sq as usize] {
                    None => empty += 1,
                    Some(p) => {
                        if empty > 0 {
                            s.push_str(&empty.to_string());
                            empty = 0;
                        }
                        let c = match p.kind {
                            PieceType::Pawn => 'p',
                            PieceType::Knight => 'n',
                            PieceType::Bishop => 'b',
                            PieceType::Rook => 'r',
                            PieceType::Queen => 'q',
                            PieceType::King => 'k',
                        };
                        s.push(if p.color == Color::White { c.to_ascii_uppercase() } else { c });
                    }
                }
            }
            if empty > 0 {
                s.push_str(&empty.to_string());
            }
            if rank > 0 {
                s.push('/');
            }
        }
        s.push(' ');
        s.push(if self.side == Color::White { 'w' } else { 'b' });
        s.push(' ');
        if self.castling == 0 {
            s.push('-');
        } else {
            if self.castling & CASTLE_WK != 0 { s.push('K'); }
            if self.castling & CASTLE_WQ != 0 { s.push('Q'); }
            if self.castling & CASTLE_BK != 0 { s.push('k'); }
            if self.castling & CASTLE_BQ != 0 { s.push('q'); }
        }
        s.push(' ');
        match self.ep_square {
            None => s.push('-'),
            Some(ep) => s.push_str(&crate::types::square_to_str(ep)),
        }
        s.push_str(&format!(" {} {}", self.halfmove, self.fullmove));
        s
    }

    // ------------------------------------------------------------- make/unmake

    /// Apply `mv` (assumed pseudo-legal from `generate_moves`).
    pub fn make_move(&mut self, mv: &Move) {
        let from = mv.from;
        let to = mv.to;
        let piece = self.mailbox[from as usize].expect("make_move: no piece on 'from'");
        let prev_castling = self.castling;
        let prev_ep = self.ep_square;
        let prev_half = self.halfmove;
        let prev_hash = self.hash;
        let z = zobrist();

        let mut rollback = Undo {
            captured: None,
            castling: prev_castling,
            ep_square: prev_ep,
            halfmove: prev_half,
            hash: prev_hash,
            moved_piece: piece,
            flags: *mv,
            #[cfg(feature = "advanced-eval")]
            mg: self.mg_scores,
            #[cfg(feature = "advanced-eval")]
            eg: self.eg_scores,
        };

        // Old EP file leaves the hash.
        if let Some(old_ep) = prev_ep {
            self.hash ^= z.ep_file[file_of(old_ep) as usize];
        }

        // Captures (including en passant).
        if mv.is_en_passant {
            let cap_sq = if piece.color == Color::White { to - 8 } else { to + 8 };
            let cap_piece = self.mailbox[cap_sq as usize].unwrap();
            self.remove_from(cap_sq, cap_piece);
            rollback.captured = Some(cap_piece);
            self.hash ^= z.piece[Self::slot(cap_piece.color, cap_piece.kind)][cap_sq as usize];
        } else if let Some(cap) = self.mailbox[to as usize] {
            self.remove_from(to, cap);
            rollback.captured = Some(cap);
            self.hash ^= z.piece[Self::slot(cap.color, cap.kind)][to as usize];
        }

        // Move the piece (or promote).
        self.remove_from(from, piece);
        let placed = match mv.promo {
            Some(p) => Piece { kind: p, color: piece.color },
            None => piece,
        };
        self.put_piece(to, placed);
        self.hash ^= z.piece[Self::slot(piece.color, piece.kind)][from as usize];
        self.hash ^= z.piece[Self::slot(placed.color, placed.kind)][to as usize];

        // Castle: also move the rook.
        if mv.is_castle_k || mv.is_castle_q {
            let (rook_from, rook_to) = Self::castle_rook_squares(piece.color, mv.is_castle_k);
            let rook = self.mailbox[rook_from as usize].unwrap();
            self.remove_from(rook_from, rook);
            self.put_piece(rook_to, rook);
            self.hash ^= z.piece[Self::slot(rook.color, rook.kind)][rook_from as usize];
            self.hash ^= z.piece[Self::slot(rook.color, rook.kind)][rook_to as usize];
        }

        // Castling rights.
        let mut new_castling = prev_castling;
        if piece.kind == PieceType::King {
            new_castling &= match piece.color {
                Color::White => !(CASTLE_WK | CASTLE_WQ),
                Color::Black => !(CASTLE_BK | CASTLE_BQ),
            };
        }
        // Rook leaves its home square, or rook is captured on its home square.
        for &(sq, flag) in &[(0u8, CASTLE_WQ), (7u8, CASTLE_WK), (56u8, CASTLE_BQ), (63u8, CASTLE_BK)] {
            if from == sq || to == sq {
                new_castling &= !flag;
            }
        }
        self.castling = new_castling;
        self.hash ^= z.castling[prev_castling as usize] ^ z.castling[new_castling as usize];

        // En passant target square.
        self.ep_square = if mv.is_double_push {
            Some(if piece.color == Color::White { from + 8 } else { from - 8 })
        } else {
            None
        };
        if let Some(ep) = self.ep_square {
            self.hash ^= z.ep_file[file_of(ep) as usize];
        }

        // Clocks and side.
        self.halfmove = if piece.kind == PieceType::Pawn || rollback.captured.is_some() {
            0
        } else {
            prev_half + 1
        };
        if self.side == Color::Black {
            self.fullmove += 1;
        }
        self.side = self.side.other();
        self.hash ^= z.side;

        self.undo_stack.push(rollback);
    }

    pub fn unmake_move(&mut self) {
        let u = self.undo_stack.pop().expect("unmake_move: empty stack");
        let mv = u.flags;
        self.side = self.side.other();
        if self.side == Color::Black {
            self.fullmove -= 1;
        }
        self.castling = u.castling;
        self.ep_square = u.ep_square;
        self.halfmove = u.halfmove;
        self.hash = u.hash;

        let piece = u.moved_piece;
        // Remove the placed piece (promoted) and put back the original.
        let cur = self.mailbox[mv.to as usize].unwrap();
        self.remove_from_restore(mv.to, cur);
        self.put_piece_restore(mv.from, piece);
        if piece.kind == PieceType::King {
            self.king_sq[piece.color.idx()] = mv.from;
        }

        // Castle: move the rook back.
        if mv.is_castle_k || mv.is_castle_q {
            let (rook_from, rook_to) = Self::castle_rook_squares(piece.color, mv.is_castle_k);
            let rook = self.mailbox[rook_to as usize].unwrap();
            self.remove_from_restore(rook_to, rook);
            self.put_piece_restore(rook_from, rook);
        }

        // Restore the captured piece.
        if let Some(cap) = u.captured {
            if mv.is_en_passant {
                let cap_sq = if cap.color == Color::White { mv.to + 8 } else { mv.to - 8 };
                self.put_piece_restore(cap_sq, cap);
            } else {
                self.put_piece_restore(mv.to, cap);
            }
        }

        // The incremental material+PST scores are exactly the ones saved
        // before the move; no PST lookups were done above.
        #[cfg(feature = "advanced-eval")]
        {
            self.mg_scores = u.mg;
            self.eg_scores = u.eg;
        }
    }

    fn remove_from(&mut self, sq: u8, piece: Piece) {
        self.remove_from_impl(sq, piece, true);
    }

    /// `remove_from` without touching the incremental scores.
    fn remove_from_restore(&mut self, sq: u8, piece: Piece) {
        self.remove_from_impl(sq, piece, false);
    }

    fn remove_from_impl(&mut self, sq: u8, piece: Piece, scores: bool) {
        #[cfg(not(feature = "advanced-eval"))]
        let _ = scores;
        let i = Self::slot(piece.color, piece.kind);
        self.pieces[i] &= !bit(sq);
        self.occupied &= !bit(sq);
        self.mailbox[sq as usize] = None;
        #[cfg(feature = "advanced-eval")]
        if scores {
            let (mg, eg) = crate::eval::material_pst(piece.color, piece.kind.idx(), sq);
            self.mg_scores[piece.color.idx()] -= mg;
            self.eg_scores[piece.color.idx()] -= eg;
        }
    }

    // ---------------------------------------------------------- movegen

    /// Generate pseudo-legal moves for the side to move.
    /// If `captures_only` is set, only captures, promotions and (when in
    /// check) evasions are generated — used by quiescence search.
    pub fn generate_moves(&self, captures_only: bool) -> MoveList {
        let in_check = self.is_in_check(self.side);
        self.generate_moves_with_check(captures_only, in_check)
    }

    /// Generate moves, reusing a check test the caller already computed
    /// (avoids a second `is_attacked` scan per search node).
    pub fn generate_moves_with_check(&self, captures_only: bool, in_check: bool) -> MoveList {
        let mut list = MoveList::new();
        self.gen_pseudo(&mut list, captures_only, in_check);
        list
    }

    /// Generate fully legal moves (pseudo-legal filtered by king safety).
    pub fn generate_legal_moves(&mut self) -> MoveList {
        let pseudo = self.generate_moves(false);
        let mut legal = MoveList::new();
        let us = self.side;
        for m in pseudo.iter() {
            // Castling has extra legality rules handled during generation, but
            // re-check king safety here for uniformity.
            self.make_move(m);
            if !self.is_in_check(us) {
                legal.push(*m);
            }
            self.unmake_move();
        }
        legal
    }

    fn gen_pseudo(&self, list: &mut MoveList, captures_only: bool, in_check: bool) {
        let us = self.side;
        let them = us.other();
        let piece_bb = |k: PieceType| self.pieces[Self::slot(us, k)];
        let enemy_king_bb = self.pieces[Self::slot(them, PieceType::King)];
        let enemy_bb = self.pieces[Self::slot(them, PieceType::Pawn)]
            | self.pieces[Self::slot(them, PieceType::Knight)]
            | self.pieces[Self::slot(them, PieceType::Bishop)]
            | self.pieces[Self::slot(them, PieceType::Rook)]
            | self.pieces[Self::slot(them, PieceType::Queen)];
        // Captures never land on the enemy king (that would be illegal;
        // mate is scored by the check test, never by capturing the king).
        let own_bb = self.pieces[Self::slot(us, PieceType::Pawn)]
            | self.pieces[Self::slot(us, PieceType::Knight)]
            | self.pieces[Self::slot(us, PieceType::Bishop)]
            | self.pieces[Self::slot(us, PieceType::Rook)]
            | self.pieces[Self::slot(us, PieceType::Queen)]
            | self.pieces[Self::slot(us, PieceType::King)];
        // In check, quiescence must consider all evasions, not just captures.
        let want_all = !captures_only || in_check;

        // --- pawns
        let pawns_bb = piece_bb(PieceType::Pawn);
        let mut pawns = pawns_bb;
        while pawns != 0 {
            let (sq, rest) = pop_lsb(pawns);
            pawns = rest;
            let dir: i8 = match us { Color::White => 8, Color::Black => -8 };
            let start_rank: u8 = match us { Color::White => 1, Color::Black => 6 };
            let promo_rank: u8 = match us { Color::White => 6, Color::Black => 1 };
            let last_rank: u8 = match us { Color::White => 7, Color::Black => 0 };
            let rank = rank_of(sq);
            // Captures (pawn_attacks covers both diagonals).
            // Excludes the enemy king via `enemy_bb`.
            let mut captures_bb = pawn_attacks(sq, us) & enemy_bb;
            if let Some(ep) = self.ep_square {
                captures_bb |= pawn_attacks(sq, us) & bit(ep);
            }
            let mut captures = captures_bb;
            while captures != 0 {
                let (to, rest_caps) = pop_lsb(captures);
                captures = rest_caps;
                self.push_pawn_move(list, sq, to, rank, promo_rank, enemy_bb);
            }
            if want_all {
                // Single push.
                let one = (sq as i32 + dir as i32) as u8;
                if self.occupied & bit(one) == 0 {
                    if rank == promo_rank {
                        self.push_pawn_move(list, sq, one, rank, promo_rank, enemy_bb);
                    } else if rank != last_rank {
                        let mut m = Move::new(sq, one, None);
                        list.push(m);
                        // Double push.
                        if rank == start_rank {
                            let two = (sq as i32 + 2 * dir as i32) as u8;
                            if self.occupied & bit(two) == 0 {
                                m = Move::new(sq, two, None);
                                m.is_double_push = true;
                                list.push(m);
                            }
                        }
                    }
                }
            } else {
                // Quiescence (quiet position): promotions are tactical too.
                let one = (sq as i32 + dir as i32) as u8;
                if rank == promo_rank && self.occupied & bit(one) == 0 {
                    self.push_pawn_move(list, sq, one, rank, promo_rank, enemy_bb);
                }
            }
        }

        if !want_all {
            return;
        }

        // --- knights
        let mut n = piece_bb(PieceType::Knight);
        while n != 0 {
            let (sq, rest) = pop_lsb(n);
            n = rest;
            let mut targets = knight_attacks(sq) & !own_bb & !enemy_king_bb;
            if captures_only {
                targets &= enemy_bb;
            }
            let mut t = targets;
            while t != 0 {
                let (to, rest) = pop_lsb(t);
                t = rest;
                let mut m = Move::new(sq, to, None);
                if enemy_bb & bit(to) != 0 { m.is_capture = true; }
                list.push(m);
            }
        }

        // --- bishops, rooks, queens
        for k in [PieceType::Bishop, PieceType::Rook, PieceType::Queen] {
            let mut bb = piece_bb(k);
            while bb != 0 {
                let (sq, rest) = pop_lsb(bb);
                bb = rest;
                let attacks = match k {
                    PieceType::Bishop => bishop_attacks(sq, self.occupied),
                    PieceType::Rook => rook_attacks(sq, self.occupied),
                    _ => queen_attacks(sq, self.occupied),
                };
                let mut targets = attacks & !own_bb & !enemy_king_bb;
                if captures_only {
                    targets &= enemy_bb;
                }
                let mut t = targets;
                while t != 0 {
                    let (to, rest) = pop_lsb(t);
                    t = rest;
                    let mut m = Move::new(sq, to, None);
                    if enemy_bb & bit(to) != 0 { m.is_capture = true; }
                    list.push(m);
                }
            }
        }

        // --- king (non-castling)
        let king_bb = piece_bb(PieceType::King);
        if king_bb != 0 {
            let sq = self.king_sq[us.idx()];
            let mut targets = king_attacks(sq) & !own_bb & !enemy_king_bb;
            if captures_only {
                targets &= enemy_bb;
            }
            let mut t = targets;
            while t != 0 {
                let (to, rest) = pop_lsb(t);
                t = rest;
                let mut m = Move::new(sq, to, None);
                if enemy_bb & bit(to) != 0 { m.is_capture = true; }
                list.push(m);
            }
        }

        // --- castling
        self.gen_castling(list, us);
    }

    /// Push one pawn capture/push, expanding promotions on the last rank.
    fn push_pawn_move(&self, list: &mut MoveList, from: u8, to: u8, rank: u8, promo_rank: u8, enemy_bb: Bitboard) {
        let is_ep = self.ep_square == Some(to);
        let is_cap = is_ep || (enemy_bb & bit(to) != 0);
        if rank == promo_rank {
            for promo in [PieceType::Queen, PieceType::Rook, PieceType::Bishop, PieceType::Knight] {
                let mut m = Move::new(from, to, Some(promo));
                m.is_capture = is_cap;
                m.is_en_passant = is_ep;
                list.push(m);
            }
        } else {
            let mut m = Move::new(from, to, None);
            m.is_capture = is_cap;
            m.is_en_passant = is_ep;
            list.push(m);
        }
    }

    fn gen_castling(&self, list: &mut MoveList, us: Color) {
        let (e, rights_k, rights_q, b, f, g, c, d, rook_k, rook_q) = match us {
            Color::White => (4u8, CASTLE_WK, CASTLE_WQ, 1u8, 5u8, 6u8, 2u8, 3u8, 7u8, 0u8),
            Color::Black => (60u8, CASTLE_BK, CASTLE_BQ, 57u8, 61u8, 62u8, 58u8, 59u8, 63u8, 56u8),
        };
        let them = us.other();
        // King-side.
        if self.castling & rights_k != 0
            && self.mailbox[rook_k as usize].map(|p| p.kind == PieceType::Rook && p.color == us).unwrap_or(false)
            && self.occupied & (bit(f) | bit(g)) == 0
            && !self.is_in_check(us)
            && !self.is_square_attacked(e, them)
            && !self.is_square_attacked(f, them)
            && !self.is_square_attacked(g, them)
        {
            let mut m = Move::new(e, g, None);
            m.is_castle_k = true;
            list.push(m);
        }
        // Queen-side.
        if self.castling & rights_q != 0
            && self.mailbox[rook_q as usize].map(|p| p.kind == PieceType::Rook && p.color == us).unwrap_or(false)
            && self.occupied & (bit(d) | bit(c) | bit(b)) == 0
            && !self.is_in_check(us)
            && !self.is_square_attacked(e, them)
            && !self.is_square_attacked(d, them)
            && !self.is_square_attacked(c, them)
        {
            let mut m = Move::new(e, c, None);
            m.is_castle_q = true;
            list.push(m);
        }
    }

    /// Perft node count (legal move generation). `depth=0` counts 1.
    pub fn perft(&mut self, depth: u32) -> u64 {
        if depth == 0 {
            return 1;
        }
        let moves = self.generate_moves(false);
        let us = self.side;
        let mut nodes = 0u64;
        for m in moves.iter() {
            self.make_move(m);
            if !self.is_in_check(us) {
                nodes += self.perft(depth - 1);
            }
            self.unmake_move();
        }
        nodes
    }

    /// Legal move matching a UCI string, or `None`.
    /// Compares only from/to/promotion (move flags are generation-dependent).
    pub fn find_legal(&mut self, uci: &str) -> Option<Move> {
        let target = Move::from_uci(uci)?;
        let legal = self.generate_legal_moves();
        legal.iter().find(|m| m.from == target.from && m.to == target.to && m.promo == target.promo).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fen_roundtrip_startpos() {
        let b = Board::startpos();
        assert_eq!(b.to_fen(), START_FEN);
    }

    #[test]
    fn hash_consistency_after_make_unmake() {
        let mut b = Board::startpos();
        let h0 = b.hash;
        let moves = b.generate_legal_moves();
        assert_eq!(moves.len(), 20);
        for m in moves.iter() {
            b.make_move(m);
            assert_eq!(b.hash, b.compute_hash());
            b.unmake_move();
            assert_eq!(b.hash, h0);
            assert_eq!(b.to_fen(), START_FEN);
        }
    }

    #[test]
    fn perft_startpos() {
        let mut b = Board::startpos();
        assert_eq!(b.perft(0), 1);
        assert_eq!(b.perft(1), 20);
        assert_eq!(b.perft(2), 400);
        assert_eq!(b.perft(3), 8902);
    }

    #[test]
    fn perft_kiwipete() {
        // Well-known test position (CPW position 2).
        let mut b = Board::from_fen(
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R w KQkq - 0 1",
        )
        .unwrap();
        assert_eq!(b.perft(1), 48);
        assert_eq!(b.perft(2), 2039);
        assert_eq!(b.perft(3), 97862);
    }

    #[test]
    fn perft_position4_promotions() {
        // CPW position 4 (promotion/castling stress test).
        let mut b = Board::from_fen(
            "r3k2r/Pppp1ppp/1b3nbN/nP6/BBP1P3/q4N2/Pp1P2PP/R2Q1RK1 w kq - 0 1",
        )
        .unwrap();
        assert_eq!(b.perft(1), 6);
        assert_eq!(b.perft(2), 264);
        assert_eq!(b.perft(3), 9467);
        assert_eq!(b.perft(4), 422333);
    }

    #[test]
    fn perft_position5_promotions() {
        // CPW position 5 (en passant + promotions).
        let mut b = Board::from_fen(
            "rnbq1k1r/pp1Pbppp/2p5/8/2B5/8/PPP1NnPP/RNBQK2R w KQ - 1 8",
        )
        .unwrap();
        assert_eq!(b.perft(1), 44);
        assert_eq!(b.perft(2), 1486);
        assert_eq!(b.perft(3), 62379);
        assert_eq!(b.perft(4), 2103487);
    }

    #[test]
    fn perft_en_passant_kiwipete() {
        // CPW position 3: en passant and pins.
        let mut b = Board::from_fen("8/2p5/3p4/KP5r/1R3p1k/8/4P1P1/8 w - - 0 1").unwrap();
        assert_eq!(b.perft(1), 14);
        assert_eq!(b.perft(2), 191);
        assert_eq!(b.perft(3), 2812);
        assert_eq!(b.perft(4), 43238);
    }

    #[test]
    fn castling_and_promotions() {
        // Castling position: White can castle both ways.
        let mut b = Board::from_fen("r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1").unwrap();
        let legal = b.generate_legal_moves();
        let ucis: Vec<String> = legal.iter().map(|m| m.to_uci()).collect();
        assert!(ucis.iter().any(|u| u == "e1g1"));
        assert!(ucis.iter().any(|u| u == "e1c1"));
        // Promotion position: 4 promo moves.
        let mut b = Board::from_fen("8/P7/8/8/8/8/k6K/8 w - - 0 1").unwrap();
        let legal = b.generate_legal_moves();
        let a7_moves: Vec<&Move> = legal.iter().filter(|m| m.from == 48).collect();
        assert_eq!(a7_moves.len(), 4);
    }

    #[test]
    fn strict_fen_rejects_impossible_positions() {
        assert!(Board::from_fen_strict("8/8/8/8/8/8/8/8 w - - 0 1").is_err());
        assert!(Board::from_fen_strict("k7/8/8/8/8/8/8/K6K w - - 0 1").is_err());
        assert!(Board::from_fen_strict("kK6/8/8/8/8/8/8/8 w - - 0 1").is_err());
        assert!(Board::from_fen_strict("k6K/8/8/8/8/8/8/PPPPPPPP w - - 0 1").is_err());
        assert!(Board::from_fen_strict("k7/8/8/8/8/8/9/K7 w - - 0 1").is_err());
        // Legal positions pass, and the permissive parser still accepts the
        // king-less fixtures used by evaluation unit tests.
        assert!(Board::from_fen_strict("k7/8/8/8/8/8/8/K7 w - - 0 1").is_ok());
        assert!(Board::from_fen("4r3/8/8/8/r3N3/8/8/K7 b - - 0 1").is_ok());
    }

    #[test]
    fn null_move_hash_matches_flipped_position() {
        for fen in [
            "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1",
            "rnbqkbnr/ppp1pppp/8/3pP3/8/8/PPPP1PPP/RNBQKBNR w KQkq d6 0 3",
            "r3k2r/p1ppqpb1/bn2pnp1/3PN3/1p2P3/2N2Q1p/PPPBBPPP/R3K2R b KQkq - 0 1",
        ] {
            let b = Board::from_fen(fen).unwrap();
            let incremental = b.null_move_hash();
            let mut flipped = b.clone();
            flipped.side = flipped.side.other();
            flipped.ep_square = None;
            assert_eq!(incremental, flipped.compute_hash(), "{fen}");
        }
    }

    #[test]
    fn check_detection() {
        // Fool's mate: after 1.f3 e5 2.g4, Black is not in check but White is about to be.
        let mut b = Board::startpos();
        let seq = ["f2f3", "e7e5", "g2g4"];
        for u in seq {
            let m = b.find_legal(u).unwrap();
            b.make_move(&m);
        }
        // Qh4+ — the queen checks on e1.
        let q = b.find_legal("d8h4").unwrap();
        b.make_move(&q);
        assert!(b.is_in_check(Color::White));
    }
}

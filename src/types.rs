//! Small shared types: colours, pieces, squares and moves.
//!
//! Squares are 0..=63 using the usual "a1 = 0" mapping so the bit index in a
//! bitboard directly equals the square number. Ranks are 0..=7 (0 = rank 1)
//! and files 0..=7 (0 = file a).

/// A side to move.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Color {
    White,
    Black,
}

impl Color {
    pub fn other(self) -> Color {
        match self {
            Color::White => Color::Black,
            Color::Black => Color::White,
        }
    }
    /// 0 = White, 1 = Black (bitboard index).
    pub fn idx(self) -> usize {
        match self {
            Color::White => 0,
            Color::Black => 1,
        }
    }
}

/// The six piece types. Order matches the material-score table.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum PieceType {
    Pawn,
    Knight,
    Bishop,
    Rook,
    Queen,
    King,
}

impl PieceType {
    /// 0..=5 in the same order as the enum.
    pub fn idx(self) -> usize {
        match self {
            PieceType::Pawn => 0,
            PieceType::Knight => 1,
            PieceType::Bishop => 2,
            PieceType::Rook => 3,
            PieceType::Queen => 4,
            PieceType::King => 5,
        }
    }
}

/// A piece on the board: what it is and who owns it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Piece {
    pub kind: PieceType,
    pub color: Color,
}

/// A move, encoded from/to squares plus optional promotion piece.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Move {
    pub from: u8,
    pub to: u8,
    /// Promotion piece (`None` for non-promotions).
    pub promo: Option<PieceType>,
    pub is_capture: bool,
    pub is_en_passant: bool,
    pub is_double_push: bool,
    /// King-side castle for White / Black.
    pub is_castle_k: bool,
    /// Queen-side castle for White / Black.
    pub is_castle_q: bool,
}

impl Default for Move {
    fn default() -> Move {
        Move::new(0, 0, None)
    }
}

impl Move {
    pub fn new(from: u8, to: u8, promo: Option<PieceType>) -> Move {
        Move {
            from,
            to,
            promo,
            is_capture: false,
            is_en_passant: false,
            is_double_push: false,
            is_castle_k: false,
            is_castle_q: false,
        }
    }

    /// UCI text form, e.g. `e2e4`, `e7e8q`, `e1g1` (castling).
    pub fn to_uci(&self) -> String {
        let mut s = String::new();
        s.push_str(&square_to_str(self.from));
        s.push_str(&square_to_str(self.to));
        if let Some(p) = self.promo {
            s.push(promo_char(p));
        }
        s
    }

    pub fn from_uci(uci: &str) -> Option<Move> {
        let b = uci.as_bytes();
        if b.len() < 4 || b.len() > 5 {
            return None;
        }
        let from = parse_square(&uci[0..2])?;
        let to = parse_square(&uci[2..4])?;
        let promo = if b.len() == 5 {
            Some(match b[4] as char {
                'n' | 'N' => PieceType::Knight,
                'b' | 'B' => PieceType::Bishop,
                'r' | 'R' => PieceType::Rook,
                _ => PieceType::Queen, // 'q'
            })
        } else {
            None
        };
        Some(Move::new(from, to, promo))
    }
}

fn promo_char(p: PieceType) -> char {
    match p {
        PieceType::Knight => 'n',
        PieceType::Bishop => 'b',
        PieceType::Rook => 'r',
        PieceType::Queen => 'q',
        _ => 'q',
    }
}

/// "e4" -> 28. Returns `None` on malformed input.
pub fn parse_square(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    if b.len() != 2 {
        return None;
    }
    let file = b[0] as char;
    let rank = b[1] as char;
    if !('a'..='h').contains(&file) || !('1'..='8').contains(&rank) {
        return None;
    }
    let f = (file as u8) - b'a';
    let r = (rank as u8) - b'1';
    Some(r * 8 + f)
}

/// 28 -> "e4".
pub fn square_to_str(sq: u8) -> String {
    let f = sq % 8;
    let r = sq / 8;
    let mut s = String::with_capacity(2);
    s.push((b'a' + f) as char);
    s.push((b'1' + r) as char);
    s
}

/// A list of generated moves (fixed capacity, no allocation).
/// Backed by a stack array so `generate_moves` costs no heap allocation.
pub const MAX_MOVES: usize = 256;

#[derive(Clone, Debug)]
pub struct MoveList {
    pub moves: [Move; MAX_MOVES],
    pub len: usize,
}

impl MoveList {
    pub fn new() -> MoveList {
        MoveList { moves: [Move::default(); MAX_MOVES], len: 0 }
    }
    pub fn push(&mut self, m: Move) {
        if self.len < MAX_MOVES {
            self.moves[self.len] = m;
            self.len += 1;
        }
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn iter(&self) -> std::slice::Iter<'_, Move> {
        self.moves[..self.len].iter()
    }
    /// Live moves as a slice, for indexing outside `len`/`iter`.
    pub fn as_slice(&self) -> &[Move] {
        &self.moves[..self.len]
    }
    pub fn as_mut_slice(&mut self) -> &mut [Move] {
        &mut self.moves[..self.len]
    }
}

impl Default for MoveList {
    fn default() -> MoveList {
        MoveList::new()
    }
}

impl std::ops::Index<usize> for MoveList {
    type Output = Move;
    fn index(&self, i: usize) -> &Move {
        &self.moves[i]
    }
}

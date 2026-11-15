//! Ferrum — a chess engine written from scratch in Rust.
//!
//! The engine is UCI-compatible. It exists in two build variants via Cargo features:
//!
//! * **Ferrum Gamma** (default): multithreading, full transposition table,
//!   quiescence search, null-move pruning, late-move reduction, advanced
//!   evaluation. Target ~2000–2400 Elo.
//! * **Ferrum Alpha** (`--no-default-features`): single-threaded, small TT,
//!   no quiescence, no null-move/LMR, simple evaluation.
//!   Target ~1200–1500 Elo.
//!
//! Alpha is Gamma with features removed — same core search, fewer extras.

pub mod bitboard;
pub mod board;
pub mod eval;
pub mod ffi;
pub mod search;
pub mod sliders;
pub mod tt;
pub mod types;
pub mod uci;

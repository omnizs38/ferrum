//! Ferrum — a chess engine written from scratch in Rust.
//!
//! The engine is UCI-compatible. It exists in two build variants via Cargo features:
//!
//! * **Ferrum Gamma** (default): multithreading, full transposition table,
//!   the shared quiescence search, null-move pruning, late-move reduction,
//!   advanced evaluation.
//! * **Ferrum Alpha** (`--no-default-features`): single-threaded, small TT,
//!   the same shared quiescence search, no null-move/LMR, simple evaluation.
//!
//! Alpha is Gamma with features removed — same core search, fewer extras.

pub mod bitboard;
pub mod board;
pub mod eval;
pub mod ffi;
mod params;
pub mod search;
pub mod sliders;
pub mod tt;
pub mod types;
pub mod uci;
